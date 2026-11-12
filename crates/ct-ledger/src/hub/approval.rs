//! Rótulo de aprovação no hub (#124): a consulta, sem iniciar sessão, de
//! se um item do espelho está aprovado. É a porta do agendador do piloto
//! automático (#183), que nunca usa o "iniciar mesmo assim": só inicia
//! item com `approved`. A regra é a de `ct_core::ApprovalConfig`; quem
//! recusa o `session.start` é o ambiente.

use super::Hub;
use crate::{LedgerError, Result};
use serde::Serialize;

/// Situação de aprovação de um item do espelho do hub.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct ApprovalStatus {
    /// O item tem o rótulo (ou o rótulo do projeto está vazio). Item fora
    /// do espelho, com rótulo ligado, não está aprovado.
    pub approved: bool,
    /// Iniciar sessão no item é recusado pelo ambiente: exigência ligada e
    /// `approved` falso.
    pub blocks_start: bool,
    /// Rótulo do projeto; `None` = desligado.
    pub label: Option<String>,
    /// "Exigir para iniciar sessão" do projeto.
    pub required: bool,
    /// O item está no espelho (os rótulos são conhecidos).
    pub in_mirror: bool,
}

impl Hub {
    /// Aprovação do item `number` de `owner/repo` pelos rótulos do espelho
    /// do hub. O item conta pelos próprios rótulos: o épico pelo dele, a
    /// sub-issue pelo dela. Projeto fora do hub = `NotFound`.
    pub fn approval_status(&self, owner: &str, repo: &str, number: i64) -> Result<ApprovalStatus> {
        let Some(p) = self.find_project(owner, repo)? else {
            return Err(LedgerError::NotFound(
                "projeto",
                format!("{owner}/{repo} não está no hub"),
            ));
        };
        let issue = self.get_issue(p.id, number)?;
        let a = &p.approval;
        let approved = match &issue {
            Some(i) => a.approves(&i.labels),
            None => a.active_label().is_none(),
        };
        Ok(ApprovalStatus {
            approved,
            blocks_start: a.enforced() && !approved,
            label: a.active_label().map(String::from),
            required: a.required,
            in_mirror: issue.is_some(),
        })
    }

    /// Atalho de [`Hub::approval_status`] para o agendador (#183): o item
    /// pode entrar na fila (tem o rótulo, ou o projeto não usa rótulo).
    pub fn is_approved(&self, owner: &str, repo: &str, number: i64) -> Result<bool> {
        Ok(self.approval_status(owner, repo, number)?.approved)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hub::{HubProjectPatch, NewHubProject};
    use crate::IssueRow;

    fn issue(n: i64, labels: &[&str], parent: Option<i64>) -> IssueRow {
        IssueRow {
            gh_number: n,
            title: format!("ACME-{n}"),
            state: "open".into(),
            labels: labels.iter().map(|s| s.to_string()).collect(),
            parent_number: parent,
            has_sub_issues: parent.is_none() && n == 10,
            url: String::new(),
            updated_at: 0,
            sprints: vec![],
            meta: Default::default(),
            assignees: vec![],
        }
    }

    fn hub_acme() -> Hub {
        let hub = Hub::open_in_memory().unwrap();
        hub.add_project(&NewHubProject {
            owner: "acme".into(),
            repo: "acme-web".into(),
            name: "acme-web".into(),
            base_branch: "develop".into(),
            protected_branches: vec!["develop".into()],
        })
        .unwrap();
        let pid = hub.find_project("acme", "acme-web").unwrap().unwrap().id;
        for i in [
            issue(10, &["aprovada"], None),
            issue(11, &[], Some(10)),
            issue(123, &["bug", "Aprovada"], None),
            issue(124, &["bug"], None),
        ] {
            hub.gh().upsert_issue(pid, &i, 1).unwrap();
        }
        hub
    }

    fn exigir(hub: &Hub, label: &str, required: bool) {
        hub.update_project(
            "acme",
            "acme-web",
            &HubProjectPatch {
                approval: Some(ct_core::ApprovalConfig {
                    label: label.into(),
                    required,
                }),
                ..Default::default()
            },
        )
        .unwrap();
    }

    #[test]
    fn aprovacao_pelo_espelho_do_hub() {
        let hub = hub_acme();
        // Padrão: rótulo `aprovada`, sem exigir. A fila já confere o rótulo.
        let s = hub.approval_status("acme", "acme-web", 124).unwrap();
        assert!(!s.approved && !s.blocks_start && !s.required, "{s:?}");
        assert_eq!(s.label.as_deref(), Some("aprovada"));
        assert!(hub.is_approved("acme", "acme-web", 123).unwrap());

        exigir(&hub, "aprovada", true);
        let s = hub.approval_status("acme", "acme-web", 124).unwrap();
        assert!(!s.approved && s.blocks_start && s.in_mirror, "{s:?}");
        // O épico conta pelo próprio rótulo; a sub-issue precisa do dela.
        assert!(hub.is_approved("acme", "acme-web", 10).unwrap());
        assert!(!hub.is_approved("acme", "acme-web", 11).unwrap());
        // Fora do espelho: rótulos desconhecidos, não aprovado.
        let s = hub.approval_status("acme", "acme-web", 999).unwrap();
        assert!(!s.in_mirror && !s.approved && s.blocks_start, "{s:?}");

        // Rótulo vazio desliga: tudo aprovado.
        exigir(&hub, "  ", true);
        let s = hub.approval_status("acme", "acme-web", 999).unwrap();
        assert!(s.approved && !s.blocks_start && s.label.is_none(), "{s:?}");

        assert!(matches!(
            hub.approval_status("acme", "outro", 1),
            Err(LedgerError::NotFound(..))
        ));
    }

    #[test]
    fn rotulo_no_cadastro_na_edicao_e_no_pull() {
        let hub = hub_acme();
        let p = hub.find_project("acme", "acme-web").unwrap().unwrap();
        assert_eq!(p.approval, ct_core::ApprovalConfig::default());
        hub.ingest("env-a", "a", &[]).unwrap();
        hub.add_assignment("acme", "acme-web", "env-a", None)
            .unwrap();
        let ep = Some(hub.config_epoch().unwrap());
        let rev = hub.config_rev().unwrap();
        exigir(&hub, " pronta ", true);
        let p = hub.find_project("acme", "acme-web").unwrap().unwrap();
        assert_eq!(p.approval.label, "pronta");
        assert!(p.approval.required);
        assert!(hub.config_rev().unwrap() > rev);
        let c = hub.env_config("env-a", Some(rev), ep).unwrap();
        assert!(c.changed);
        let a = &c.projects.unwrap()[0].approval;
        assert_eq!((a.label.as_str(), a.required), ("pronta", true));

        // Outra edição não mexe no rótulo.
        let p = hub
            .update_project(
                "acme",
                "acme-web",
                &HubProjectPatch {
                    name: Some("ACME Web".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(p.approval.label, "pronta");

        // Rótulo inválido é recusado.
        let err = hub
            .update_project(
                "acme",
                "acme-web",
                &HubProjectPatch {
                    approval: Some(ct_core::ApprovalConfig {
                        label: "a\nb".into(),
                        required: true,
                    }),
                    ..Default::default()
                },
            )
            .unwrap_err();
        assert!(err.to_string().contains("approval.label"), "{err}");
    }
}
