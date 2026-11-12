//! Modelo e esforço do agente por work item (issue #182). Só do hub, como a
//! prioridade: a pessoa define na tela da issue (`hub.workitem.set_model`)
//! ou aprova a proposta `model` do gestor. A sessão da issue inicia com
//! `--model` e `--effort` daqui, e o agendador do piloto automático (#183)
//! só inicia issues com modelo definido ([`Hub::issue_agent`]).

use super::priority::cause;
use super::{Hub, HubWorkItem};
use crate::{LedgerError, Result};
use ct_core::{CLAUDE_EFFORTS, CLAUDE_MODELS};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use serde_json::json;

/// Modelo e esforço escolhidos para a sessão de uma issue.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct IssueAgent {
    /// Alias de `ct_core::CLAUDE_MODELS`; `None` = não definido.
    pub model: Option<String>,
    /// Nível de `ct_core::CLAUDE_EFFORTS`; `None` = padrão do Claude Code.
    pub effort: Option<String>,
}

impl IssueAgent {
    /// Confere os valores contra as listas aceitas pelo Claude Code.
    /// Esforço sem modelo não vale: o modelo é o que torna a issue pronta.
    pub fn validate(&self) -> std::result::Result<(), String> {
        if let Some(m) = &self.model {
            if !CLAUDE_MODELS.contains(&m.as_str()) {
                return Err(format!(
                    "model inválido: {m} (aceitos: {})",
                    CLAUDE_MODELS.join(", ")
                ));
            }
        }
        if let Some(e) = &self.effort {
            if !CLAUDE_EFFORTS.contains(&e.as_str()) {
                return Err(format!(
                    "effort inválido: {e} (aceitos: {})",
                    CLAUDE_EFFORTS.join(", ")
                ));
            }
            if self.model.is_none() {
                return Err("effort sem model: defina o modelo junto".into());
            }
        }
        Ok(())
    }

    /// `{ "model": .., "effort": .. }`, a forma do `current`/`proposed` da
    /// proposta `model`.
    pub fn to_json(&self) -> serde_json::Value {
        json!({ "model": self.model, "effort": self.effort })
    }

    /// "opus, esforço high" / "opus" / "sem modelo", para decisões e notas.
    pub fn label(&self) -> String {
        match (&self.model, &self.effort) {
            (None, _) => "sem modelo".into(),
            (Some(m), None) => m.clone(),
            (Some(m), Some(e)) => format!("{m}, esforço {e}"),
        }
    }
}

impl From<&HubWorkItem> for IssueAgent {
    fn from(w: &HubWorkItem) -> Self {
        IssueAgent {
            model: w.model.clone(),
            effort: w.effort.clone(),
        }
    }
}

impl Hub {
    /// `hub.workitem.set_model`: define (ou limpa, com `model = None`) o
    /// modelo e o esforço do work item, criando-o como a prioridade faz.
    /// Grava o evento `model_set` e torna obsoletas as propostas `model`
    /// pendentes do item. Mesmo valor = nada a gravar.
    #[allow(clippy::too_many_arguments)]
    pub fn set_model(
        &self,
        owner: &str,
        repo: &str,
        gh_number: i64,
        agent: &IssueAgent,
        reason: Option<&str>,
        proposal_id: Option<i64>,
        env_id: &str,
    ) -> Result<HubWorkItem> {
        agent.validate().map_err(LedgerError::Conflict)?;
        let Some(pid) = self.find_project(owner, repo)?.map(|p| p.id) else {
            return Err(LedgerError::NotFound(
                "projeto",
                format!("{owner}/{repo} não está no hub"),
            ));
        };
        let tx = self.tx()?;
        let wi = match self.find_work_item(owner, repo, gh_number)? {
            Some(w) => w,
            None => self.work_item(self.manual_work_item(pid, owner, repo, gh_number)?)?,
        };
        let from = IssueAgent::from(&wi);
        if &from != agent {
            self.conn.execute(
                "UPDATE work_item SET model = ?2, effort = ?3 WHERE id = ?1",
                params![wi.id, agent.model, agent.effort],
            )?;
            let mut payload = json!({ "from": from.to_json(), "to": agent.to_json() });
            cause(&mut payload, proposal_id);
            if let Some(r) = reason.map(str::trim).filter(|r| !r.is_empty()) {
                payload["reason"] = json!(r);
            }
            self.hub_event(env_id, pid, Some(wi.id), "model_set", &payload)?;
            self.obsolete_changed(wi.id)?;
        }
        tx.commit()?;
        self.work_item(wi.id)
    }

    /// Modelo e esforço da issue `owner/repo#n` para iniciar a sessão:
    /// `None` = sem work item no hub. O agendador (#183) só inicia a issue
    /// com `model` definido; o início pelo hub (`hub.env.call` com
    /// `session.start`) completa os parâmetros com isto.
    pub fn issue_agent(
        &self,
        owner: &str,
        repo: &str,
        gh_number: i64,
    ) -> Result<Option<IssueAgent>> {
        Ok(self
            .find_work_item(owner, repo, gh_number)?
            .map(|w| IssueAgent::from(&w)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{GithubSnapshot, IssueRow, NewHubProject};

    const HUB: &str = "hub-id";

    fn hub() -> Hub {
        let hub = Hub::open_in_memory().unwrap();
        hub.add_project(&NewHubProject {
            owner: "acme".into(),
            repo: "acme-web".into(),
            name: "acme-web".into(),
            base_branch: "develop".into(),
            protected_branches: vec![],
        })
        .unwrap();
        let pid = hub.find_project("acme", "acme-web").unwrap().unwrap().id;
        hub.apply_github(
            &GithubSnapshot {
                project_id: pid,
                issues: vec![IssueRow {
                    gh_number: 7,
                    title: "Tela de login".into(),
                    state: "open".into(),
                    url: "u".into(),
                    updated_at: 1,
                    ..Default::default()
                }],
                prs: vec![],
                etags: vec![],
                synced_at: 1,
                gone: Vec::new(),
                me: None,
            },
            HUB,
        )
        .unwrap();
        hub
    }

    fn agent(model: Option<&str>, effort: Option<&str>) -> IssueAgent {
        IssueAgent {
            model: model.map(str::to_string),
            effort: effort.map(str::to_string),
        }
    }

    #[test]
    fn define_limpa_e_registra_o_evento() {
        let hub = hub();
        assert_eq!(hub.issue_agent("acme", "acme-web", 7).unwrap(), None);
        let w = hub
            .set_model(
                "acme",
                "acme-web",
                7,
                &agent(Some("opus"), Some("high")),
                Some("toca o login"),
                None,
                HUB,
            )
            .unwrap();
        assert_eq!(
            (w.model.as_deref(), w.effort.as_deref()),
            (Some("opus"), Some("high"))
        );
        assert_eq!(
            hub.issue_agent("ACME", "acme-web", 7).unwrap(),
            Some(agent(Some("opus"), Some("high")))
        );
        let ev = hub.list_events(w.id, 10).unwrap();
        let set = ev.iter().find(|e| e.kind == "model_set").unwrap();
        assert_eq!(
            set.payload["to"],
            json!({ "model": "opus", "effort": "high" })
        );
        assert_eq!(
            set.payload["from"],
            json!({ "model": null, "effort": null })
        );
        assert_eq!(set.payload["cause"], "manual");
        assert_eq!(set.payload["reason"], "toca o login");

        // Mesmo valor: nada a gravar.
        hub.set_model(
            "acme",
            "acme-web",
            7,
            &agent(Some("opus"), Some("high")),
            None,
            None,
            HUB,
        )
        .unwrap();
        let n = |hub: &Hub| {
            hub.list_events(w.id, 50)
                .unwrap()
                .iter()
                .filter(|e| e.kind == "model_set")
                .count()
        };
        assert_eq!(n(&hub), 1);

        // Limpar.
        let w = hub
            .set_model("acme", "acme-web", 7, &agent(None, None), None, None, HUB)
            .unwrap();
        assert_eq!((w.model, w.effort), (None, None));
        assert_eq!(n(&hub), 2);
    }

    #[test]
    fn recusa_valores_fora_da_lista() {
        let hub = hub();
        for a in [
            agent(Some("gpt-4"), None),
            agent(Some("opus"), Some("turbo")),
            agent(None, Some("high")),
        ] {
            assert!(
                matches!(
                    hub.set_model("acme", "acme-web", 7, &a, None, None, HUB),
                    Err(LedgerError::Conflict(_))
                ),
                "{a:?}"
            );
        }
        assert!(matches!(
            hub.set_model("acme", "x", 7, &agent(Some("opus"), None), None, None, HUB),
            Err(LedgerError::NotFound("projeto", _))
        ));
        assert!(hub.find_work_item("acme", "acme-web", 7).unwrap().is_none());
    }
}
