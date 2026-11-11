//! Rótulo de aprovação por projeto (#124): o rótulo (label) que marca a
//! issue como aprovada para trabalho e a opção de exigi-lo para iniciar
//! sessão. Fica no cadastro do projeto no hub e chega aos ambientes pelo
//! `config_rev`; quem recusa o `session.start` é o ambiente.
//!
//! Regras:
//! - Rótulo vazio desliga: tudo conta como aprovado.
//! - O item conta pelos próprios rótulos: o épico pelo dele, a sub-issue
//!   pelo dela (não herda do épico).
//! - Comparação sem diferenciar maiúsculas e sem espaços nas pontas (o
//!   GitHub trata rótulos assim).
//! - Sessão de planejamento (sem issue) e sessão do gestor não passam por
//!   aqui.

use serde::{Deserialize, Serialize};

/// Rótulo padrão de um projeto novo (e de um hub anterior à #124).
pub const DEFAULT_APPROVAL_LABEL: &str = "aprovada";

/// Tamanho máximo do rótulo, em caracteres (o GitHub aceita até 50).
pub const MAX_APPROVAL_LABEL_CHARS: usize = 100;

/// Rótulo de aprovação do projeto (`HubProject.approval`). Ausente num hub
/// anterior: o rótulo padrão, sem exigir.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct ApprovalConfig {
    /// Rótulo que aprova a issue; vazio = desligado (tudo aprovado).
    pub label: String,
    /// Exigir o rótulo para iniciar sessão numa issue: sem ele, o ambiente
    /// recusa o `session.start` (a UI oferece "iniciar mesmo assim").
    pub required: bool,
}

impl Default for ApprovalConfig {
    fn default() -> Self {
        Self {
            label: DEFAULT_APPROVAL_LABEL.into(),
            required: false,
        }
    }
}

impl ApprovalConfig {
    /// JSON da coluna `approval_config`.
    pub fn config_json(&self) -> String {
        serde_json::to_string(self).expect("ApprovalConfig serializa")
    }

    /// Igual ao padrão (o JSON do pull não leva o campo).
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// Rótulo sem espaços nas pontas.
    pub fn normalized(&self) -> ApprovalConfig {
        ApprovalConfig {
            label: self.label.trim().to_string(),
            required: self.required,
        }
    }

    /// Rótulo de uma linha, com no máximo [`MAX_APPROVAL_LABEL_CHARS`].
    pub fn validate(&self) -> Result<(), String> {
        let label = self.label.trim();
        if label.chars().count() > MAX_APPROVAL_LABEL_CHARS {
            return Err(format!(
                "approval.label: no máximo {MAX_APPROVAL_LABEL_CHARS} caracteres"
            ));
        }
        if label.chars().any(char::is_control) {
            return Err("approval.label: sem quebras de linha nem caracteres de controle".into());
        }
        Ok(())
    }

    /// O rótulo, se ligado (não vazio).
    pub fn active_label(&self) -> Option<&str> {
        Some(self.label.trim()).filter(|l| !l.is_empty())
    }

    /// A exigência vale: opção ligada e rótulo não vazio.
    pub fn enforced(&self) -> bool {
        self.required && self.active_label().is_some()
    }

    /// O item com estes rótulos está aprovado (rótulo vazio = sempre).
    /// Não olha `required`: é a regra da fila do piloto automático (#183),
    /// que só pega issue aprovada mesmo com a exigência desligada.
    pub fn approves<S: AsRef<str>>(&self, labels: &[S]) -> bool {
        match self.active_label() {
            None => true,
            Some(want) => labels
                .iter()
                .any(|l| l.as_ref().trim().eq_ignore_ascii_case(want)),
        }
    }

    /// Iniciar sessão no item com estes rótulos é recusado: exigência
    /// ligada e o item sem o rótulo.
    pub fn blocks_start<S: AsRef<str>>(&self, labels: &[S]) -> bool {
        self.enforced() && !self.approves(labels)
    }

    /// Motivo da recusa, para a mensagem de erro e a tela.
    pub fn refusal(&self, item: &str) -> String {
        format!(
            "{APPROVAL_REFUSAL_PREFIX}: {item} não tem o rótulo \"{}\", que o projeto exige \
             para iniciar sessão",
            self.label.trim()
        )
    }
}

/// Início fixo da mensagem de recusa (a UI o reconhece, além do código
/// `APPROVAL_REQUIRED` do JSON-RPC).
pub const APPROVAL_REFUSAL_PREFIX: &str = "aguardando aprovação";

#[cfg(test)]
mod tests {
    use super::*;

    fn exige(label: &str) -> ApprovalConfig {
        ApprovalConfig {
            label: label.into(),
            required: true,
        }
    }

    #[test]
    fn padrao_e_json_ausente() {
        let a: ApprovalConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(a, ApprovalConfig::default());
        assert_eq!(a.label, "aprovada");
        assert!(!a.required && !a.enforced());
        assert!(a.is_default());
        let a: ApprovalConfig = serde_json::from_str(r#"{"required":true}"#).unwrap();
        assert_eq!(a.label, "aprovada");
        let back: ApprovalConfig = serde_json::from_str(&a.config_json()).unwrap();
        assert_eq!(back, a);
    }

    #[test]
    fn aprova_pelo_rotulo_sem_diferenciar_maiusculas() {
        let a = exige(" Aprovada ");
        assert!(a.approves(&["bug", "aprovada"]));
        assert!(a.approves(&["APROVADA "]));
        assert!(!a.approves(&["bug"]));
        assert!(!a.approves::<&str>(&[]));
        assert!(a.blocks_start(&["bug"]));
        assert!(!a.blocks_start(&["aprovada"]));
    }

    #[test]
    fn rotulo_vazio_desliga() {
        let a = exige("  ");
        assert!(!a.enforced());
        assert!(a.approves::<&str>(&[]));
        assert!(!a.blocks_start::<&str>(&[]));
        assert_eq!(a.normalized().label, "");
    }

    #[test]
    fn sem_exigir_nao_bloqueia_mas_a_fila_confere() {
        let a = ApprovalConfig {
            label: "aprovada".into(),
            required: false,
        };
        assert!(!a.blocks_start(&["bug"]));
        assert!(!a.approves(&["bug"]));
    }

    #[test]
    fn valida_o_rotulo() {
        assert!(exige("aprovada").validate().is_ok());
        assert!(exige(&"x".repeat(MAX_APPROVAL_LABEL_CHARS + 1))
            .validate()
            .unwrap_err()
            .contains("approval.label"));
        assert!(exige("a\nb").validate().is_err());
    }

    #[test]
    fn motivo_da_recusa() {
        let m = exige("aprovada").refusal("ACME-123");
        assert!(m.starts_with(APPROVAL_REFUSAL_PREFIX), "{m}");
        assert!(m.contains("ACME-123") && m.contains("\"aprovada\""), "{m}");
    }
}
