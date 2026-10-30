//! Mascaramento de valores na saída (issue #167): cada ocorrência de um
//! valor vira `***NOME***`. O crate não conhece secrets: quem chama passa
//! os pares (nome, bytes do valor) e decide quais valores entram (no
//! `ctowerd`, só os secrets injetados com pelo menos
//! `ct_secrets::MIN_MASK_LEN` bytes).
//!
//! - Opera em bytes: um valor pode cruzar fronteiras de UTF-8 e de leitura.
//! - Valores que começam na mesma posição: vence o mais longo.
//! - Em fluxo ([`Masker`]), segura no máximo (maior valor − 1) bytes e só
//!   enquanto o fim do que chegou for prefixo de algum valor; o resto sai
//!   na hora. O resultado é o mesmo de aplicar [`MaskSet::apply`] na saída
//!   inteira.
//! - Valores e caudas pendentes ficam em [`Zeroizing`]: a memória é zerada
//!   ao liberar.

use std::sync::Arc;
use zeroize::Zeroizing;

struct Rule {
    value: Zeroizing<Vec<u8>>,
    replacement: Vec<u8>,
}

/// Conjunto de valores a mascarar. Clonar compartilha o conjunto.
#[derive(Clone, Default)]
pub struct MaskSet {
    /// Ordenadas do valor mais longo para o mais curto.
    rules: Arc<Vec<Rule>>,
}

impl MaskSet {
    /// Valores vazios são ignorados; valores repetidos ficam com o primeiro
    /// nome.
    pub fn new(pairs: impl IntoIterator<Item = (String, Zeroizing<Vec<u8>>)>) -> Self {
        let mut rules: Vec<Rule> = Vec::new();
        for (name, value) in pairs {
            if value.is_empty() || rules.iter().any(|r| *r.value == *value) {
                continue;
            }
            rules.push(Rule {
                replacement: format!("***{name}***").into_bytes(),
                value,
            });
        }
        // Estável: entre valores do mesmo tamanho, mantém a ordem de chegada.
        rules.sort_by_key(|r| std::cmp::Reverse(r.value.len()));
        MaskSet {
            rules: Arc::new(rules),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// Mascara um bloco completo (ex.: a cópia de um arquivo).
    pub fn apply(&self, data: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(data.len());
        self.scan(data, true, &mut out);
        out
    }

    /// Filtro em fluxo com este conjunto.
    pub fn masker(&self) -> Masker {
        Masker {
            set: self.clone(),
            pending: Zeroizing::new(Vec::new()),
        }
    }

    fn max_len(&self) -> usize {
        self.rules.first().map_or(0, |r| r.value.len())
    }

    /// Escreve `data` mascarado em `out`. Sem `eof`, para no primeiro ponto
    /// em que o resto de `data` é prefixo próprio de um valor (que pode se
    /// completar no próximo bloco) e devolve onde parou; com `eof`, vai até
    /// o fim. Devolve `data.len()` quando não sobra nada.
    fn scan(&self, data: &[u8], eof: bool, out: &mut Vec<u8>) -> usize {
        if self.rules.is_empty() {
            out.extend_from_slice(data);
            return data.len();
        }
        let mut first = [false; 256];
        for r in self.rules.iter() {
            first[usize::from(r.value[0])] = true;
        }
        let mut i = 0;
        let mut plain = 0;
        'pos: while i < data.len() {
            if !first[usize::from(data[i])] {
                i += 1;
                continue;
            }
            let rest = &data[i..];
            // Do mais longo para o mais curto: um prefixo de um valor longo
            // ainda em aberto vale mais que um valor curto já completo.
            for r in self.rules.iter() {
                if rest.len() >= r.value.len() {
                    if rest.starts_with(&r.value) {
                        out.extend_from_slice(&data[plain..i]);
                        out.extend_from_slice(&r.replacement);
                        i += r.value.len();
                        plain = i;
                        continue 'pos;
                    }
                } else if !eof && r.value.starts_with(rest) {
                    out.extend_from_slice(&data[plain..i]);
                    return i;
                }
            }
            i += 1;
        }
        out.extend_from_slice(&data[plain..]);
        data.len()
    }
}

/// Filtro em fluxo: recebe os blocos na ordem de leitura e devolve o que já
/// pode sair mascarado.
pub struct Masker {
    set: MaskSet,
    pending: Zeroizing<Vec<u8>>,
}

impl Masker {
    /// Processa um bloco. A cauda que ainda pode completar um valor fica
    /// pendente (no máximo maior valor − 1 bytes).
    pub fn feed(&mut self, chunk: &[u8]) -> Vec<u8> {
        if self.set.is_empty() {
            return chunk.to_vec();
        }
        let mut data = Zeroizing::new(std::mem::take(&mut *self.pending));
        data.extend_from_slice(chunk);
        let mut out = Vec::with_capacity(data.len());
        let stop = self.set.scan(&data, false, &mut out);
        self.pending.extend_from_slice(&data[stop..]);
        debug_assert!(self.pending.len() < self.set.max_len().max(1));
        out
    }

    /// Fim do fluxo (ou prazo da cauda vencido): descarrega o pendente, com
    /// o que já der para mascarar nele.
    pub fn finish(&mut self) -> Vec<u8> {
        let data = Zeroizing::new(std::mem::take(&mut *self.pending));
        let mut out = Vec::with_capacity(data.len());
        self.set.scan(&data, true, &mut out);
        out
    }

    /// Há bytes segurados à espera do próximo bloco.
    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(pairs: &[(&str, &str)]) -> MaskSet {
        MaskSet::new(
            pairs
                .iter()
                .map(|(n, v)| (n.to_string(), Zeroizing::new(v.as_bytes().to_vec()))),
        )
    }

    /// Passa `chunks` pelo filtro em fluxo e descarrega no fim.
    fn stream(s: &MaskSet, chunks: &[&[u8]]) -> Vec<u8> {
        let mut m = s.masker();
        let mut out = Vec::new();
        for c in chunks {
            out.extend(m.feed(c));
        }
        out.extend(m.finish());
        out
    }

    fn txt(b: Vec<u8>) -> String {
        String::from_utf8(b).unwrap()
    }

    #[test]
    fn valor_inteiro_num_bloco() {
        let s = set(&[("API_TOKEN", "sk-abc123456")]);
        let mut m = s.masker();
        assert_eq!(
            txt(m.feed(b"TOKEN=sk-abc123456\n")),
            "TOKEN=***API_TOKEN***\n"
        );
        assert!(!m.has_pending());
    }

    #[test]
    fn valor_partido_entre_dois_blocos() {
        let s = set(&[("API_TOKEN", "sk-abc123456")]);
        let mut m = s.masker();
        // O que não pode ser começo do valor sai na hora.
        assert_eq!(txt(m.feed(b"x=sk-ab")), "x=");
        assert!(m.has_pending());
        assert_eq!(txt(m.feed(b"c123456 fim")), "***API_TOKEN*** fim");
        assert!(!m.has_pending());
    }

    #[test]
    fn valor_partido_em_tres_ou_mais_blocos() {
        let s = set(&[("API_TOKEN", "sk-abc123456")]);
        let out = stream(&s, &[b"a s", b"k-", b"abc1", b"2345", b"6 b"]);
        assert_eq!(txt(out), "a ***API_TOKEN*** b");
        // Um byte por bloco.
        let all = b"[sk-abc123456]";
        let chunks: Vec<&[u8]> = all.chunks(1).collect();
        assert_eq!(txt(stream(&s, &chunks)), "[***API_TOKEN***]");
    }

    #[test]
    fn valor_repetido_no_mesmo_bloco_e_entre_blocos() {
        let s = set(&[("T", "segredo-123")]);
        assert_eq!(
            txt(s.apply(b"segredo-123segredo-123 e segredo-123")),
            "***T******T*** e ***T***"
        );
        let out = stream(&s, &[b"segredo-123 segre", b"do-123 segredo-12", b"3"]);
        assert_eq!(txt(out), "***T*** ***T*** ***T***");
    }

    #[test]
    fn prefixo_que_nao_completa_sai_intacto() {
        let s = set(&[("T", "segredo-123")]);
        let mut m = s.masker();
        assert_eq!(txt(m.feed(b"ok segre")), "ok ");
        // O bloco seguinte desmente o prefixo: tudo sai como chegou.
        assert_eq!(txt(m.feed(b"gado")), "segregado");
        assert!(!m.has_pending());
        // Prefixo no fim do fluxo: o `finish` libera.
        let mut m = s.masker();
        assert_eq!(txt(m.feed(b"fim segredo-1")), "fim ");
        assert_eq!(txt(m.finish()), "segredo-1");
        // Prefixo que recomeça dentro dele mesmo.
        assert_eq!(txt(stream(&s, &[b"segsegr", b"edo-123"])), "seg***T***");
    }

    #[test]
    fn sem_valores_passa_direto_e_vazio_e_ignorado() {
        let s = MaskSet::default();
        assert!(s.is_empty());
        let mut m = s.masker();
        assert_eq!(m.feed(b"abc"), b"abc");
        assert!(!m.has_pending());
        let s = set(&[("VAZIO", "")]);
        assert!(s.is_empty());
    }

    #[test]
    fn dois_secrets_e_sobreposicao_prefere_o_mais_longo() {
        let s = set(&[("CURTO", "abcdefgh"), ("LONGO", "abcdefgh-ijkl")]);
        assert_eq!(
            txt(s.apply(b"abcdefgh-ijkl abcdefgh abcdefgh-ij")),
            "***LONGO*** ***CURTO*** ***CURTO***-ij"
        );
        // Em fluxo, o curto completo espera a chance do longo.
        let mut m = s.masker();
        assert_eq!(txt(m.feed(b"abcdefgh")), "");
        assert_eq!(txt(m.feed(b"-ijkl")), "***LONGO***");
        assert_eq!(txt(stream(&s, &[b"abcdefgh", b"-x"])), "***CURTO***-x");
        assert_eq!(txt(stream(&s, &[b"abcdefgh"])), "***CURTO***");

        let s = set(&[("A", "primeiro-valor"), ("B", "segundo-valor")]);
        assert_eq!(
            txt(stream(&s, &[b"primeiro-va", b"lor/segundo", b"-valor"])),
            "***A***/***B***"
        );
    }

    #[test]
    fn bytes_fora_de_utf8_e_valor_cruzando_caractere() {
        let s = set(&[("U", "ação-secreta")]);
        let all = "x ação-secreta y".as_bytes();
        // Corta no meio do `ç` (2 bytes).
        let (a, b) = all.split_at(4);
        assert_eq!(txt(stream(&s, &[a, b])), "x ***U*** y");
        let s = set(&[("BIN", "\u{1}\u{2}\u{3}\u{4}\u{5}\u{6}\u{7}\u{8}")]);
        assert_eq!(s.apply(b"\xff\x01\x02\x03\x04\x05\x06\x07\x08\xfe"), {
            let mut v = vec![0xff];
            v.extend(b"***BIN***");
            v.push(0xfe);
            v
        });
    }

    /// Em fluxo, qualquer partição dá o mesmo resultado que o bloco inteiro.
    #[test]
    fn qualquer_particao_equivale_ao_bloco_inteiro() {
        let s = set(&[("A", "abcabcab"), ("B", "abcabcabcabd"), ("C", "bcabcabx")]);
        let all: &[u8] = b"abcabcabcabdxabcabcabcabcabxabcabcab-bcabcabx abcab";
        let want = s.apply(all);
        assert!(!want.windows(8).any(|w| w == b"abcabcab"));
        for i in 0..=all.len() {
            for j in i..=all.len() {
                let got = stream(&s, &[&all[..i], &all[i..j], &all[j..]]);
                assert_eq!(got, want, "partição {i}/{j}");
            }
        }
    }
}
