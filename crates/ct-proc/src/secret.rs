//! Senha ou token lido do comando de credencial (spec §6.1, §8.1): só na
//! memória, até o fim do ciclo de sync.

use std::fmt;
use zeroize::Zeroize;

/// Valor que nunca aparece em log, evento nem mensagem de erro: `Debug` e
/// `Display` imprimem `***`, e a memória é zerada no `drop`. Para usar o
/// conteúdo (só no ambiente do processo filho), chame [`Secret::expose`].
pub struct Secret(String);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Secret(value.into())
    }

    /// O valor em claro. Não use em log nem em mensagem de erro.
    pub fn expose(&self) -> &str {
        &self.0
    }

    /// `text` com o valor trocado por `***` (saída do jira-cli que ecoa a
    /// senha numa mensagem de erro).
    pub fn redact(&self, text: &str) -> String {
        if self.0.is_empty() {
            return text.to_string();
        }
        text.replace(&self.0, "***")
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("***")
    }
}

impl fmt::Display for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("***")
    }
}

impl Drop for Secret {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_e_display_nao_mostram_o_valor() {
        let s = Secret::new("hunter2");
        assert_eq!(format!("{s:?}"), "***");
        assert_eq!(format!("{s:#?}"), "***");
        assert_eq!(format!("{s}"), "***");
        assert_eq!(s.expose(), "hunter2");
        assert_eq!(s.redact("senha hunter2 recusada"), "senha *** recusada");
        assert_eq!(Secret::new("").redact("x"), "x");
    }
}
