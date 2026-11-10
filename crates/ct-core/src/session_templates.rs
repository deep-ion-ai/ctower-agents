//! Templates do prompt inicial por projeto (#166): um override opcional por
//! tipo de sessão (issue, épico, sessão do projeto), no cadastro do projeto
//! no hub e entregue aos ambientes pelo `config_rev`. Vazio = o template
//! global do rastreador do projeto (o `ui.json` do app). Quem resolve e
//! renderiza é a UI (`ui/src/project/templates.ts`).

use serde::{Deserialize, Serialize};

/// Tamanho máximo de cada template, em bytes.
pub const MAX_SESSION_TEMPLATE_BYTES: usize = 64 * 1024;

/// Overrides do projeto (`HubProject.session_templates`). Ausente num hub
/// anterior: nenhum override.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct SessionTemplates {
    /// Sessões de issue (avulsa, sub-issue ou sub-tarefa).
    pub issue: String,
    /// Sessões de épico.
    pub epic: String,
    /// Sessão do projeto, sem issue.
    pub project: String,
}

impl SessionTemplates {
    /// JSON da coluna `session_templates`.
    pub fn config_json(&self) -> String {
        serde_json::to_string(self).expect("SessionTemplates serializa")
    }

    /// Template só com espaços vira vazio (herda o global); o resto fica
    /// como veio.
    pub fn normalized(&self) -> SessionTemplates {
        let clean = |s: &str| {
            if s.trim().is_empty() {
                String::new()
            } else {
                s.to_string()
            }
        };
        SessionTemplates {
            issue: clean(&self.issue),
            epic: clean(&self.epic),
            project: clean(&self.project),
        }
    }

    /// Nenhum override.
    pub fn is_empty(&self) -> bool {
        self.issue.is_empty() && self.epic.is_empty() && self.project.is_empty()
    }

    /// Cada template cabe em [`MAX_SESSION_TEMPLATE_BYTES`].
    pub fn validate(&self) -> Result<(), String> {
        for (label, t) in [
            ("issue", &self.issue),
            ("epic", &self.epic),
            ("project", &self.project),
        ] {
            if t.len() > MAX_SESSION_TEMPLATE_BYTES {
                return Err(format!(
                    "session_templates.{label}: no máximo {} KiB ({} bytes informados)",
                    MAX_SESSION_TEMPLATE_BYTES / 1024,
                    t.len()
                ));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn so_espacos_vira_vazio_e_o_resto_fica_intacto() {
        let t = SessionTemplates {
            issue: "  \n ".into(),
            epic: "  Épico {{key}}\n".into(),
            project: String::new(),
        }
        .normalized();
        assert_eq!(t.issue, "");
        assert_eq!(t.epic, "  Épico {{key}}\n");
        assert!(!t.is_empty());
        assert!(SessionTemplates::default().is_empty());
    }

    #[test]
    fn json_ausente_ou_parcial_herda() {
        let t: SessionTemplates = serde_json::from_str("{}").unwrap();
        assert!(t.is_empty());
        let t: SessionTemplates = serde_json::from_str(r#"{"issue":"x"}"#).unwrap();
        assert_eq!(t.issue, "x");
        assert_eq!(t.epic, "");
        let back: SessionTemplates = serde_json::from_str(&t.config_json()).unwrap();
        assert_eq!(back, t);
    }

    #[test]
    fn recusa_template_grande_demais() {
        let t = SessionTemplates {
            project: "x".repeat(MAX_SESSION_TEMPLATE_BYTES + 1),
            ..Default::default()
        };
        assert!(t
            .validate()
            .unwrap_err()
            .contains("session_templates.project"));
        assert!(SessionTemplates::default().validate().is_ok());
    }
}
