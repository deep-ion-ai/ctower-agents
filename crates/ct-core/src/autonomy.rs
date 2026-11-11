//! Modo autônomo do projeto (épico #178, issue #181): a branch de integração
//! (`wip/autonoma`) onde as sessões autônomas mesclam, alinhada com a base
//! do projeto, e o PR rascunho `wip/autonoma → <base>` que a pessoa revisa.
//!
//! Só regras puras: o hub guarda o [`AutonomyConfig`] no cadastro do projeto
//! e o entrega aos ambientes pelo `config_rev`; o ambiente autônomo cria a
//! branch, alinha e abre o PR, e relata o resultado num [`AutonomyReport`].

use crate::branch::valid_branch_name;
use serde::{Deserialize, Serialize};

/// Nome padrão da branch de integração.
pub const DEFAULT_AUTONOMY_BRANCH: &str = "wip/autonoma";

/// Como o ambiente traz a base para a branch de integração antes de iniciar
/// uma issue.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum AlignStrategy {
    /// `git merge` da base na branch (commit de merge, push sem força).
    #[default]
    Merge,
    /// `git rebase` da branch sobre a base (push com `--force-with-lease`).
    Rebase,
}

impl AlignStrategy {
    pub fn as_str(self) -> &'static str {
        match self {
            AlignStrategy::Merge => "merge",
            AlignStrategy::Rebase => "rebase",
        }
    }
}

/// Modo autônomo no cadastro do projeto (`HubProject.autonomy`). Ausente num
/// hub ou ambiente anterior à #181: desligado.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct AutonomyConfig {
    /// Desligado, o projeto se comporta como antes.
    pub enabled: bool,
    /// `env_id` do ambiente onde as sessões autônomas rodam (um ambiente
    /// marcado como autônomo e atribuído ao projeto).
    pub env_id: Option<String>,
    /// Branch de integração; vazio = [`DEFAULT_AUTONOMY_BRANCH`].
    pub branch: String,
    /// Base da branch e do PR; vazio = a base do projeto.
    pub base_branch: String,
    /// Merge ou rebase no alinhamento.
    pub strategy: AlignStrategy,
}

impl AutonomyConfig {
    /// JSON da coluna `autonomy_config`.
    pub fn config_json(&self) -> String {
        serde_json::to_string(self).expect("AutonomyConfig serializa")
    }

    /// Nada configurado (o padrão): não vai no pull da configuração.
    pub fn is_default(&self) -> bool {
        self == &AutonomyConfig::default()
    }

    /// Ligado e com ambiente.
    pub fn is_on(&self) -> bool {
        self.enabled && self.env_id.is_some()
    }

    /// Branch de integração em vigor.
    pub fn branch(&self) -> &str {
        match self.branch.trim() {
            "" => DEFAULT_AUTONOMY_BRANCH,
            b => b,
        }
    }

    /// Base em vigor: a do modo ou a do projeto.
    pub fn base<'a>(&'a self, project_base: &'a str) -> &'a str {
        match self.base_branch.trim() {
            "" => project_base,
            b => b,
        }
    }

    /// Aparado; branch igual ao padrão e base igual à do projeto viram vazio
    /// (seguem o padrão se ele mudar); ambiente vazio vira `None`.
    pub fn normalized(&self, project_base: &str) -> AutonomyConfig {
        let branch = self.branch.trim();
        let base = self.base_branch.trim();
        AutonomyConfig {
            enabled: self.enabled,
            env_id: self
                .env_id
                .as_deref()
                .map(str::trim)
                .filter(|e| !e.is_empty())
                .map(String::from),
            branch: match branch {
                DEFAULT_AUTONOMY_BRANCH => String::new(),
                b => b.to_string(),
            },
            base_branch: match base {
                b if b == project_base => String::new(),
                b => b.to_string(),
            },
            strategy: self.strategy,
        }
    }

    /// Confere a forma (sem rede nem banco): ligado exige ambiente; a branch
    /// é um nome válido, diferente da base e fora das protegidas (o agente
    /// mescla nela).
    pub fn validate(&self, project_base: &str, protected: &[String]) -> Result<(), String> {
        let branch = self.branch();
        let base = self.base(project_base);
        if self.enabled && self.env_id.is_none() {
            return Err("modo autônomo: escolha o ambiente onde as sessões autônomas rodam".into());
        }
        if !valid_branch_name(branch) {
            return Err(format!(
                "modo autônomo: nome de branch inválido: {branch:?}"
            ));
        }
        if !valid_branch_name(base) {
            return Err(format!("modo autônomo: branch base inválida: {base:?}"));
        }
        if branch == base {
            return Err(format!(
                "modo autônomo: a branch {branch} não pode ser a própria base"
            ));
        }
        if protected.iter().any(|p| p == branch) {
            return Err(format!(
                "modo autônomo: {branch} é uma branch protegida do projeto; use outra (o agente \
                 mescla nela)"
            ));
        }
        Ok(())
    }
}

/// Situação da branch de integração no último relato do ambiente autônomo.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum AutonomyState {
    /// Branch criada (ou existente), alinhada e com o PR rascunho aberto.
    Ok,
    /// A base não entra na branch sem conflito: nenhuma issue autônoma
    /// começa até alguém resolver.
    Conflict,
    /// Outra falha (git, rede, GitHub): tenta de novo no próximo pull ou
    /// início.
    Error,
}

/// Relato do ambiente autônomo sobre a branch de integração (sobe ao hub na
/// fotografia do projeto, `ProjectChange.autonomy`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct AutonomyReport {
    pub state: AutonomyState,
    pub branch: String,
    pub base: String,
    /// Aviso para a tela (conflito) ou o erro.
    #[serde(default)]
    pub message: Option<String>,
    /// Arquivos em conflito, no conflito.
    #[serde(default)]
    pub conflicts: Vec<String>,
    /// PR rascunho `branch → base`, quando conhecido.
    #[serde(default)]
    pub pr_url: Option<String>,
    /// Quando o relato foi feito (ms).
    pub at: i64,
}

/// Aviso do projeto com a branch de integração em conflito com a base.
pub fn conflict_warning(branch: &str) -> String {
    format!("{branch} em conflito com a base")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn protected() -> Vec<String> {
        vec!["develop".into(), "main".into()]
    }

    #[test]
    fn padroes_e_normalizacao() {
        let c = AutonomyConfig::default();
        assert!(c.is_default());
        assert!(!c.is_on());
        assert_eq!(c.branch(), "wip/autonoma");
        assert_eq!(c.base("develop"), "develop");
        let n = AutonomyConfig {
            enabled: true,
            env_id: Some("  env-auto ".into()),
            branch: " wip/autonoma ".into(),
            base_branch: "develop".into(),
            strategy: AlignStrategy::Rebase,
        }
        .normalized("develop");
        assert_eq!(n.env_id.as_deref(), Some("env-auto"));
        assert_eq!(n.branch, "");
        assert_eq!(n.base_branch, "");
        assert!(n.is_on());
        let outra = AutonomyConfig {
            branch: "wip/acme".into(),
            base_branch: "main".into(),
            env_id: Some(" ".into()),
            ..Default::default()
        }
        .normalized("develop");
        assert_eq!(outra.branch(), "wip/acme");
        assert_eq!(outra.base("develop"), "main");
        assert_eq!(outra.env_id, None);
    }

    #[test]
    fn validacao() {
        let ok = AutonomyConfig {
            enabled: true,
            env_id: Some("env-auto".into()),
            ..Default::default()
        };
        assert!(ok.validate("develop", &protected()).is_ok());
        let sem_env = AutonomyConfig {
            env_id: None,
            ..ok.clone()
        };
        assert!(sem_env.validate("develop", &protected()).is_err());
        // Desligado sem ambiente: ok (só guarda as escolhas).
        assert!(AutonomyConfig::default()
            .validate("develop", &protected())
            .is_ok());
        for (branch, base) in [
            ("develop", ""),
            ("a..b", ""),
            ("main", "develop"),
            ("x", "a b"),
        ] {
            let c = AutonomyConfig {
                branch: branch.into(),
                base_branch: base.into(),
                ..ok.clone()
            };
            assert!(
                c.validate("develop", &protected()).is_err(),
                "{branch} {base}"
            );
        }
    }

    #[test]
    fn serde_tolera_campos_ausentes() {
        let c: AutonomyConfig = serde_json::from_str(r#"{"enabled":true}"#).unwrap();
        assert!(c.enabled);
        assert_eq!(c.strategy, AlignStrategy::Merge);
        let r: AutonomyReport = serde_json::from_str(
            r#"{"state":"conflict","branch":"wip/autonoma","base":"develop","at":1}"#,
        )
        .unwrap();
        assert_eq!(r.state, AutonomyState::Conflict);
        assert!(r.conflicts.is_empty());
        assert_eq!(
            conflict_warning("wip/autonoma"),
            "wip/autonoma em conflito com a base"
        );
    }
}
