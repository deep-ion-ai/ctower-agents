//! Execução de épico (épico #295): as transições válidas da execução e do
//! nó, as opções de cada pedido de decisão e a forma das áreas de uma
//! sub-issue. Só regras, sem I/O: o `ct-hub-store` guarda, o motor do
//! `cthubd` decide.

use crate::{EpicDecisionOrigin, EpicNodeState, EpicRunStatus};

/// Paralelismo padrão de uma execução (sessões de sub-issue vivas).
pub const EPIC_RUN_PARALLEL_DEFAULT: u32 = 2;
/// Teto do paralelismo de uma execução.
pub const EPIC_RUN_PARALLEL_CEILING: u32 = 8;
/// Áreas por sub-issue no plano.
pub const EPIC_AREAS_MAX: usize = 50;
/// Tamanho máximo de uma área (caminho ou glob).
pub const EPIC_AREA_MAX_CHARS: usize = 200;

impl EpicRunStatus {
    /// Concluída ou cancelada: não muda mais.
    pub fn is_terminal(self) -> bool {
        matches!(self, EpicRunStatus::Concluido | EpicRunStatus::Cancelado)
    }

    /// O motor dirige sessões de sub-issue (ou a de integração) neste
    /// estado.
    pub fn is_running(self) -> bool {
        matches!(
            self,
            EpicRunStatus::Executando | EpicRunStatus::Integrando | EpicRunStatus::AguardandoMerge
        )
    }

    /// Transição permitida de `self` para `to`. Ficar no mesmo estado não
    /// é transição.
    pub fn can_transition(self, to: EpicRunStatus) -> bool {
        use EpicRunStatus::*;
        if self.is_terminal() || self == to {
            return false;
        }
        if to == Cancelado {
            return true;
        }
        match self {
            Planejando => to == AguardandoAprovacao,
            AguardandoAprovacao => matches!(to, Executando | Planejando | Replanejando),
            Executando => matches!(to, Integrando | Replanejando | Pausado | Estacionado),
            Integrando => matches!(to, AguardandoMerge | Pausado | Estacionado),
            AguardandoMerge => matches!(to, Concluido | Integrando | Pausado | Estacionado),
            Pausado => matches!(to, Executando | Integrando | AguardandoMerge | Replanejando),
            // Pausa manual tem precedência sobre o estacionamento.
            Estacionado => matches!(to, Executando | Integrando | AguardandoMerge | Pausado),
            Replanejando => to == AguardandoAprovacao,
            Concluido | Cancelado => false,
        }
    }
}

impl EpicNodeState {
    /// Nó com sessão (ou PR) em andamento: conta no paralelismo e nas
    /// áreas ocupadas.
    pub fn is_live(self) -> bool {
        use EpicNodeState::*;
        matches!(self, EmExecucao | EmPr | Assumida | Estacionada)
    }

    /// Transição permitida de `self` para `to`. `mesclada` é final; um nó
    /// `assumida` ou `estacionada` volta ao estado anterior (guardado no
    /// nó); `bloqueada` volta para onde a resposta da decisão mandar.
    pub fn can_transition(self, to: EpicNodeState) -> bool {
        use EpicNodeState::*;
        if self == to || self == Mesclada {
            return false;
        }
        match self {
            Pendente => matches!(to, Pronta | Bloqueada),
            Pronta => matches!(to, Pendente | EmExecucao | Bloqueada),
            EmExecucao => matches!(
                to,
                Pronta | EmPr | Mesclada | Assumida | Estacionada | Bloqueada
            ),
            EmPr => matches!(
                to,
                EmExecucao | Mesclada | Assumida | Estacionada | Bloqueada
            ),
            // O PR mesclado na `wip` é fato: marca o nó de qualquer estado
            // com sessão ou bloqueado.
            Estacionada => matches!(to, EmExecucao | EmPr | Assumida | Bloqueada | Mesclada),
            Assumida => matches!(to, Pronta | EmExecucao | EmPr | Mesclada | Bloqueada),
            Bloqueada => matches!(
                to,
                Pendente | Pronta | EmExecucao | EmPr | Assumida | Mesclada
            ),
            Mesclada => false,
        }
    }
}

impl EpicDecisionOrigin {
    /// Respostas aceitas para o pedido desta origem (#300).
    pub fn options(self) -> &'static [&'static str] {
        use EpicDecisionOrigin::*;
        match self {
            PlanoErrado => &["aguardar", "replanejar"],
            CiEsgotado => &["assumir", "mais_tentativas", "replanejar"],
            SemProgresso => &["assumir", "reiniciar", "replanejar"],
            ConflitoRebase => &["assumir", "replanejar"],
            IssueAlterada => &["manter", "replanejar"],
        }
    }
}

/// Normaliza uma área (caminho ou glob relativo à raiz do repositório):
/// tira espaços e o `./` do início e troca `\` por `/`. Recusa vazia,
/// absoluta, com `..` ou longa demais.
pub fn normalize_area(area: &str) -> Result<String, String> {
    let mut a = area.trim().replace('\\', "/");
    while let Some(rest) = a.strip_prefix("./") {
        a = rest.to_string();
    }
    if a.is_empty() {
        return Err("área vazia".into());
    }
    if a.starts_with('/') {
        return Err(format!(
            "área absoluta: {a} (use caminho relativo ao repositório)"
        ));
    }
    if a.split('/').any(|s| s == "..") {
        return Err(format!("área com '..': {a}"));
    }
    if a.chars().count() > EPIC_AREA_MAX_CHARS {
        return Err(format!("área com mais de {EPIC_AREA_MAX_CHARS} caracteres"));
    }
    Ok(a)
}

/// O trecho literal de uma área normalizada: até o primeiro caractere de
/// glob (`*`, `?`, `[`, `{`). Inteira se não tiver glob.
fn literal_prefix(area: &str) -> (&str, bool) {
    match area.find(['*', '?', '[', '{']) {
        Some(i) => (&area[..i], true),
        None => (area, false),
    }
}

/// `a` contém `b` como caminho: igual, ou `a` é diretório de `b`.
fn path_contains(a: &str, b: &str) -> bool {
    let a = a.trim_end_matches('/');
    let b = b.trim_end_matches('/');
    a.is_empty() || a == b || b.strip_prefix(a).is_some_and(|r| r.starts_with('/'))
}

/// Duas áreas normalizadas podem tocar o mesmo arquivo. Sem glob, por
/// caminho (igual ou uma dentro da outra). Com glob, pelo trecho literal
/// antes do glob, que pode terminar no meio de um nome: conservador, na
/// dúvida conta como sobreposta.
pub fn area_overlaps(a: &str, b: &str) -> bool {
    let (pa, ga) = literal_prefix(a);
    let (pb, gb) = literal_prefix(b);
    match (ga, gb) {
        (false, false) => path_contains(a, b) || path_contains(b, a),
        (true, false) => b.starts_with(pa) || path_contains(b, pa),
        (false, true) => a.starts_with(pb) || path_contains(a, pb),
        (true, true) => pa.starts_with(pb) || pb.starts_with(pa),
    }
}

/// Algum par das duas listas se sobrepõe ([`area_overlaps`]). Lista vazia
/// (sub-issue sem áreas no plano) não se sobrepõe a nada.
pub fn areas_overlap(a: &[String], b: &[String]) -> bool {
    a.iter().any(|x| b.iter().any(|y| area_overlaps(x, y)))
}

/// Normaliza as áreas de um nó (sem repetidas, na ordem dada).
pub fn normalize_areas(areas: &[String]) -> Result<Vec<String>, String> {
    if areas.len() > EPIC_AREAS_MAX {
        return Err(format!("no máximo {EPIC_AREAS_MAX} áreas por sub-issue"));
    }
    let mut out: Vec<String> = Vec::with_capacity(areas.len());
    for a in areas {
        let n = normalize_area(a)?;
        if !out.contains(&n) {
            out.push(n);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn execucao_recusa_transicoes_invalidas() {
        use EpicRunStatus::*;
        assert!(Planejando.can_transition(AguardandoAprovacao));
        assert!(!Planejando.can_transition(Executando));
        assert!(AguardandoAprovacao.can_transition(Executando));
        assert!(Executando.can_transition(Pausado));
        assert!(Estacionado.can_transition(Pausado));
        assert!(!Pausado.can_transition(Estacionado));
        assert!(!Executando.can_transition(Concluido));
        assert!(AguardandoMerge.can_transition(Concluido));
        assert!(!Replanejando.can_transition(Executando));
        assert!(!Executando.can_transition(Executando));
        for s in EpicRunStatus::ALL {
            assert!(!Concluido.can_transition(*s));
            assert!(!Cancelado.can_transition(*s));
            if !s.is_terminal() {
                assert!(s.can_transition(Cancelado), "{s} -> cancelado");
            }
        }
    }

    #[test]
    fn no_recusa_transicoes_invalidas() {
        use EpicNodeState::*;
        assert!(Pendente.can_transition(Pronta));
        assert!(!Pendente.can_transition(EmExecucao));
        assert!(EmPr.can_transition(Mesclada));
        assert!(EmPr.can_transition(Assumida));
        assert!(Assumida.can_transition(EmPr));
        assert!(!Estacionada.can_transition(Pronta));
        for s in EpicNodeState::ALL {
            assert!(!Mesclada.can_transition(*s));
        }
        for s in [EmExecucao, EmPr, Assumida, Estacionada, Bloqueada] {
            assert!(s.can_transition(Mesclada), "{s} -> mesclada");
        }
        assert!(!Pendente.can_transition(Mesclada) && !Pronta.can_transition(Mesclada));
        assert!(EmPr.is_live() && !Pronta.is_live() && !Mesclada.is_live());
    }

    #[test]
    fn opcoes_por_origem() {
        assert_eq!(
            EpicDecisionOrigin::PlanoErrado.options(),
            &["aguardar", "replanejar"]
        );
        for o in EpicDecisionOrigin::ALL {
            assert!(o.options().contains(&"replanejar"));
        }
    }

    #[test]
    fn sobreposicao_de_areas() {
        let o = area_overlaps;
        assert!(o("crates/a", "crates/a"));
        assert!(o("crates/a/", "crates/a/src/lib.rs"));
        assert!(o("crates/a/src/lib.rs", "crates/a"));
        assert!(!o("crates/a", "crates/ab"));
        assert!(!o("crates/a/", "crates/b/"));
        assert!(o("crates/a/**", "crates/a/src/x.rs"));
        assert!(!o("crates/a/**", "crates/b/x.rs"));
        assert!(o("ui/src/*.ts", "ui/src/main.ts"));
        assert!(o("ui/src/*.ts", "ui/src/*.tsx"));
        assert!(!o("ui/src/*.ts", "crates/**"));
        // Prefixo no meio do nome: conservador.
        assert!(o("crates/a*", "crates/ab/x"));
        assert!(o("**/*.rs", "crates/a"));
        assert!(!areas_overlap(&[], &["crates/a".into()]));
        assert!(areas_overlap(
            &["docs/".into(), "crates/a/".into()],
            &["crates/a/src".into()]
        ));
    }

    #[test]
    fn areas_normalizadas() {
        assert_eq!(
            normalize_area(" ./crates/ct-core/ ").unwrap(),
            "crates/ct-core/"
        );
        assert_eq!(normalize_area("ui\\src\\*.ts").unwrap(), "ui/src/*.ts");
        assert!(normalize_area("").is_err());
        assert!(normalize_area("/etc").is_err());
        assert!(normalize_area("a/../b").is_err());
        assert!(normalize_area(&"x".repeat(EPIC_AREA_MAX_CHARS + 1)).is_err());
        assert_eq!(
            normalize_areas(&["a/".into(), "./a/".into(), "b".into()]).unwrap(),
            vec!["a/", "b"]
        );
    }
}
