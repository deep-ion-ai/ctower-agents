//! Regras do agente por projeto e por épico (issue #357, spec do Jira e
//! GitLab §10), do lado do ambiente.
//!
//! O hub guarda as versões e entrega só as vigentes no `ConfigProject.rules`
//! de `GET /sync/config`; o ambiente as copia para `agent_rules_cache`
//! (substituída a cada pull com mudança). O `launch` lê o cache a cada início
//! e retomada e as põe no `--append-system-prompt` (§10.1). A proposta do
//! agente (`rules_propose`) sobe pelo outbox e fica pendente no hub: nada
//! muda aqui até o usuário aprovar e o próximo pull trazer a versão nova.

use crate::sync::new_uid;
use crate::{Ledger, LedgerError, Project, Result, SyncChange, WorkItem};
use ct_core::WorkItemKind;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

/// Limite de cada escopo (projeto ou épico), em bytes de UTF-8 (§10.2).
pub const RULES_MAX_BYTES: usize = 16 * 1024;
/// A partir daqui a UI avisa que o limite está perto.
pub const RULES_WARN_BYTES: usize = 12 * 1024;

/// Recusa conteúdo acima de [`RULES_MAX_BYTES`], com a mensagem para o
/// usuário ou o agente.
pub fn check_rules_size(content: &str) -> std::result::Result<(), String> {
    let n = content.len();
    if n > RULES_MAX_BYTES {
        return Err(format!(
            "as regras passam do limite de 16 KiB por escopo: {n} bytes (máximo {RULES_MAX_BYTES}); \
             encurte o texto"
        ));
    }
    Ok(())
}

/// Texto vigente de um escopo.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuleText {
    pub version: i64,
    pub content: String,
}

/// Regras vigentes de um épico.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EpicRuleText {
    pub number: i64,
    pub version: i64,
    pub content: String,
}

/// Regras vigentes do projeto, como vêm em `ConfigProject.rules`. Escopo
/// sem regras (ou com a versão vigente vazia) fica de fora.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ConfigRules {
    #[serde(default)]
    pub project: Option<RuleText>,
    #[serde(default)]
    pub epics: Vec<EpicRuleText>,
}

impl ConfigRules {
    pub fn is_empty(&self) -> bool {
        self.project.is_none() && self.epics.is_empty()
    }
}

/// Regras que valem para uma sessão: as do projeto e as do épico (o
/// próprio, numa sessão de épico, ou o pai, numa sub-issue).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct SessionRules {
    pub project: Option<RuleText>,
    /// Épico da sessão, com ou sem regras (`None` = issue sem épico, sessão
    /// do projeto ou do gestor).
    pub epic_number: Option<i64>,
    pub epic: Option<RuleText>,
}

impl SessionRules {
    /// Texto para o `--append-system-prompt`: projeto e depois épico, cada
    /// um com o cabeçalho do escopo e da versão. `None` = nada a injetar.
    pub fn system_prompt(&self, project: &Project) -> Option<String> {
        let mut parts = Vec::new();
        if let Some(r) = &self.project {
            parts.push(format!(
                "Regras do projeto {} (versão {}, Control Tower):\n{}",
                project.name,
                r.version,
                r.content.trim_end()
            ));
        }
        if let (Some(n), Some(r)) = (self.epic_number, &self.epic) {
            parts.push(format!(
                "Regras do épico {} (versão {}, Control Tower):\n{}",
                project.item_ref(n),
                r.version,
                r.content.trim_end()
            ));
        }
        (!parts.is_empty()).then(|| parts.join("\n\n"))
    }
}

/// Proposta de regras do agente, como vai no outbox (§10.3): o texto
/// **completo** novo do escopo, sobre a versão `base_version` que a sessão
/// conhecia (0 = escopo ainda sem regras).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RulesProposalChange {
    pub uid: String,
    pub owner: String,
    pub repo: String,
    /// 0 = regras do projeto.
    pub epic_number: i64,
    pub base_version: i64,
    pub content: String,
    pub reason: String,
    #[serde(default)]
    pub session_id: Option<String>,
}

/// Tamanho máximo da justificativa da proposta.
pub const RULES_REASON_MAX: usize = 2000;

impl Ledger {
    /// Substitui o cache das regras do projeto pelas vigentes vindas do hub
    /// (na transação do pull).
    pub fn replace_agent_rules(&self, project_id: i64, rules: &ConfigRules) -> Result<()> {
        self.atomic(|| {
            self.conn.execute(
                "DELETE FROM agent_rules_cache WHERE project_id = ?1",
                [project_id],
            )?;
            let put = |epic: i64, version: i64, content: &str| -> Result<()> {
                self.conn.execute(
                    "INSERT INTO agent_rules_cache (project_id, epic_number, version, content)
                     VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT (project_id, epic_number) DO UPDATE SET
                        version = excluded.version, content = excluded.content",
                    params![project_id, epic, version, content],
                )?;
                Ok(())
            };
            if let Some(p) = &rules.project {
                put(0, p.version, &p.content)?;
            }
            for e in rules.epics.iter().filter(|e| e.number > 0) {
                put(e.number, e.version, &e.content)?;
            }
            Ok(())
        })
    }

    /// Regras vigentes de um escopo no cache (0 = projeto).
    pub fn agent_rules(&self, project_id: i64, epic_number: i64) -> Result<Option<RuleText>> {
        Ok(self
            .conn
            .query_row(
                "SELECT version, content FROM agent_rules_cache
                 WHERE project_id = ?1 AND epic_number = ?2",
                params![project_id, epic_number],
                |r| {
                    Ok(RuleText {
                        version: r.get(0)?,
                        content: r.get(1)?,
                    })
                },
            )
            .optional()?)
    }

    /// Épico cujas regras valem para o work item: ele mesmo, se for épico;
    /// o pai no espelho, se for sub-issue; nenhum na sessão do projeto (#0).
    pub fn rules_epic(&self, wi: &WorkItem) -> Result<Option<i64>> {
        if wi.gh_number <= 0 {
            return Ok(None);
        }
        let issue = self.get_issue(wi.project_id, wi.gh_number)?;
        if wi.kind == WorkItemKind::Epic || issue.as_ref().is_some_and(|i| i.has_sub_issues) {
            return Ok(Some(wi.gh_number));
        }
        Ok(issue.and_then(|i| i.parent_number).filter(|n| *n > 0))
    }

    /// Regras vigentes da sessão do work item (§10.1). O gestor não recebe
    /// regras de projeto.
    pub fn session_rules(&self, project: &Project, wi: &WorkItem) -> Result<SessionRules> {
        if project.kind == ct_core::ProjectKind::Gestor {
            return Ok(SessionRules::default());
        }
        let epic_number = self.rules_epic(wi)?;
        Ok(SessionRules {
            project: self.agent_rules(project.id, 0)?,
            epic: match epic_number {
                Some(n) => self.agent_rules(project.id, n)?,
                None => None,
            },
            epic_number,
        })
    }

    /// `rules_propose`: põe no outbox a proposta do texto completo do
    /// escopo (0 = projeto). Só projeto vindo do hub: é o hub que guarda as
    /// regras e decide.
    pub fn enqueue_rules_proposal(
        &self,
        project_id: i64,
        epic_number: i64,
        content: &str,
        reason: &str,
        session_id: Option<&str>,
    ) -> Result<RulesProposalChange> {
        let p = self.get_project(project_id)?;
        if !p.from_hub() {
            return Err(LedgerError::Conflict(
                "o projeto não vem do hub: as regras do agente só existem em projetos do hub"
                    .into(),
            ));
        }
        check_rules_size(content).map_err(LedgerError::Conflict)?;
        let reason = reason.trim();
        if reason.is_empty() || reason.chars().count() > RULES_REASON_MAX {
            return Err(LedgerError::Conflict(format!(
                "reason é obrigatória, com até {RULES_REASON_MAX} caracteres"
            )));
        }
        let current = self.agent_rules(project_id, epic_number)?;
        if current.as_ref().map(|r| r.content.as_str()).unwrap_or("") == content {
            return Err(LedgerError::Conflict(
                "nada a mudar: o texto proposto é igual às regras vigentes".into(),
            ));
        }
        let change = RulesProposalChange {
            uid: new_uid(),
            owner: p.gh_owner,
            repo: p.gh_repo,
            epic_number,
            base_version: current.map_or(0, |r| r.version),
            content: content.to_string(),
            reason: reason.to_string(),
            session_id: session_id.map(str::to_string),
        };
        self.enqueue(&SyncChange::RulesProposal(change.clone()))?;
        Ok(change)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::ledger_with_project;
    use crate::{ConfigProject, ConfigResponse, GithubSnapshot, IssueRow};

    fn rt(version: i64, content: &str) -> RuleText {
        RuleText {
            version,
            content: content.into(),
        }
    }

    fn issue(n: i64, parent: Option<i64>, subs: bool) -> IssueRow {
        IssueRow {
            gh_number: n,
            title: format!("issue {n}"),
            state: "open".into(),
            labels: vec![],
            parent_number: parent,
            has_sub_issues: subs,
            url: String::new(),
            updated_at: 0,
            ..Default::default()
        }
    }

    fn pull(l: &Ledger, rev: i64, rules: ConfigRules) {
        l.apply_hub_config(
            &ConfigResponse {
                config_rev: rev,
                config_epoch: 1,
                changed: true,
                projects: Some(vec![ConfigProject {
                    docs: Default::default(),
                    owner: "deep-ion-ai".into(),
                    repo: "demo".into(),
                    name: "demo".into(),
                    base_branch: "develop".into(),
                    protected_branches: vec![],
                    repo_path: String::new(),
                    rev,
                    tracker: Default::default(),
                    forge: Default::default(),
                    branch_pattern: None,
                    attribution: true,
                    sync: false,
                    rules,
                }]),
                removed: Some(vec![]),
            },
            |_| "/home/u/projetos/demo".into(),
        )
        .unwrap();
    }

    #[test]
    fn pull_substitui_o_cache_e_sessao_ve_projeto_e_epico() {
        let (l, p) = ledger_with_project();
        pull(
            &l,
            1,
            ConfigRules {
                project: Some(rt(2, "projeto v2")),
                epics: vec![EpicRuleText {
                    number: 10,
                    version: 1,
                    content: "épico v1".into(),
                }],
            },
        );
        l.apply_github(&GithubSnapshot {
            project_id: p.id,
            issues: vec![
                issue(10, None, true),
                issue(11, Some(10), false),
                issue(12, None, false),
            ],
            ..Default::default()
        })
        .unwrap();
        let p = l.get_project(p.id).unwrap();
        let sub = l.ensure_work_item(p.id, 11, WorkItemKind::Issue).unwrap();
        let r = l.session_rules(&p, &sub).unwrap();
        assert_eq!(r.project, Some(rt(2, "projeto v2")));
        assert_eq!(
            (r.epic_number, r.epic.clone()),
            (Some(10), Some(rt(1, "épico v1")))
        );
        assert_eq!(
            r.system_prompt(&p).unwrap(),
            "Regras do projeto demo (versão 2, Control Tower):\nprojeto v2\n\n\
             Regras do épico deep-ion-ai/demo#10 (versão 1, Control Tower):\népico v1"
        );
        // O próprio épico recebe as dele; a issue sem épico, só as do projeto.
        let epic = l.ensure_work_item(p.id, 10, WorkItemKind::Epic).unwrap();
        assert_eq!(l.session_rules(&p, &epic).unwrap().epic_number, Some(10));
        let solta = l.ensure_work_item(p.id, 12, WorkItemKind::Issue).unwrap();
        let r = l.session_rules(&p, &solta).unwrap();
        assert_eq!((r.epic_number, r.epic), (None, None));
        assert!(r.project.is_some());

        // O pull seguinte substitui tudo: o épico perdeu as regras.
        pull(
            &l,
            2,
            ConfigRules {
                project: Some(rt(3, "projeto v3")),
                epics: vec![],
            },
        );
        let r = l.session_rules(&p, &sub).unwrap();
        assert_eq!((r.project, r.epic), (Some(rt(3, "projeto v3")), None));
        assert!(r.epic_number.is_some());
    }

    #[test]
    fn proposta_vai_ao_outbox_com_a_versao_base_e_respeita_o_limite() {
        let (l, p) = ledger_with_project();
        // Projeto local (fora do hub): recusa.
        assert!(matches!(
            l.enqueue_rules_proposal(p.id, 0, "x", "porque", None),
            Err(LedgerError::Conflict(_))
        ));
        pull(
            &l,
            1,
            ConfigRules {
                project: Some(rt(4, "atual")),
                epics: vec![],
            },
        );
        let big = "a".repeat(RULES_MAX_BYTES + 1);
        let err = l
            .enqueue_rules_proposal(p.id, 0, &big, "porque", None)
            .unwrap_err();
        assert!(err.to_string().contains("16 KiB"), "{err}");
        assert!(l
            .enqueue_rules_proposal(p.id, 0, "atual", "porque", None)
            .is_err());
        assert!(l
            .enqueue_rules_proposal(p.id, 0, "novo", "  ", None)
            .is_err());
        let c = l
            .enqueue_rules_proposal(p.id, 0, "novo", "aprendi", Some("s-1"))
            .unwrap();
        assert_eq!((c.base_version, c.epic_number), (4, 0));
        let e = l
            .enqueue_rules_proposal(p.id, 7, "do épico", "x", None)
            .unwrap();
        assert_eq!(e.base_version, 0);
        let out: Vec<_> = l
            .outbox_pending(100)
            .unwrap()
            .into_iter()
            .filter_map(|e| match e.change {
                SyncChange::RulesProposal(r) => Some(r),
                _ => None,
            })
            .collect();
        assert_eq!(out, vec![c, e]);
    }
}
