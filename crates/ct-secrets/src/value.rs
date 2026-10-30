//! Valor de uma entrada, que nunca aparece em log nem em mensagem de erro.

use serde::{Deserialize, Serialize};
use std::fmt;
use zeroize::Zeroize;

/// Valor de um secret ou variável. Sem `Display`; o `Debug` imprime `***`.
/// Para ler o conteúdo, chame [`SecretValue::expose`] explicitamente. A
/// memória é zerada no `drop`.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SecretValue(String);

impl SecretValue {
    pub fn new(value: impl Into<String>) -> Self {
        SecretValue(value.into())
    }

    /// O valor em claro. Não use em log nem em mensagem de erro.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SecretValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("***")
    }
}

impl Drop for SecretValue {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_nao_mostra_o_valor() {
        let v = SecretValue::new("hunter2");
        assert_eq!(format!("{v:?}"), "***");
        assert_eq!(format!("{v:#?}"), "***");
        assert_eq!(v.expose(), "hunter2");
        assert_eq!(serde_json::to_string(&v).unwrap(), "\"hunter2\"");
    }
}
