//! Regras do agente por projeto e por épico no hub (issue #357, spec do
//! Jira e GitLab §10). O usuário edita (ou importa um `CLAUDE.md`) na UI do
//! hub; o agente só propõe o texto completo novo (`rules_propose`, pelo
//! outbox), que fica `pendente` até o usuário aprovar ou rejeitar. Cada
//! mudança grava uma versão nova (só inclusão), com a origem, e incrementa o
//! `config_rev`: o próximo pull dos ambientes traz as vigentes no
//! `ConfigProject.rules`.
//!
//! Proposta feita sobre uma versão que já não é a vigente continua
//! aprovável: aprovar grava o texto completo proposto, e a UI avisa antes
//! que a base mudou (`base_changed`), mostrando a diferença contra o texto
//! vigente. Só o usuário decide; o hub nunca descarta sozinho.

use super::{Hub, HubProject};
use crate::sync::project_key;
use crate::{
    check_rules_size, ConfigRules, EpicRuleText, LedgerError, Result, RuleText,
    RulesProposalChange, RULES_MAX_BYTES, RULES_REASON_MAX, RULES_WARN_BYTES,
};
use ct_core::{ProposalState, RuleSource};
use rusqlite::{params, OptionalExtension, Row};
use serde::{Deserialize, Serialize};

/// Uma versão das regras de um escopo.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct HubRuleVersion {
    pub version: i64,
    pub content: String,
    /// Tamanho em bytes (UTF-8), o que conta para o limite.
    pub bytes: i64,
    pub source: RuleSource,
    /// Proposta que originou a versão (`source = proposta`).
    pub proposal_id: Option<i64>,
    /// Ambiente que propôs (versão de proposta); nulo = o usuário na UI.
    pub author_env_id: Option<String>,
    pub author_env_name: Option<String>,
    /// Sessão que propôs, se houver.
    pub session_id: Option<String>,
    /// Justificativa da proposta ou nota do usuário.
    pub note: Option<String>,
    pub created_at: i64,
}

/// `hub.rules.get`: as regras vigentes de um escopo.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct HubRules {
    pub owner: String,
    pub repo: String,
    /// 0 = regras do projeto.
    pub epic_number: i64,
    /// Nome do escopo para pessoas: o nome do projeto ou a referência do
    /// épico (`owner/repo#n`, `PROJ-n`).
    pub label: String,
    /// `None` = escopo ainda sem regras.
    pub current: Option<HubRuleVersion>,
    /// Propostas pendentes do escopo.
    pub pending: i64,
    pub max_bytes: i64,
    pub warn_bytes: i64,
}

/// `hub.rules.history`: da versão mais nova para a mais velha.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct HubRulesHistory {
    pub versions: Vec<HubRuleVersion>,
    pub truncated: bool,
}

/// Proposta de regras do agente.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct HubRulesProposal {
    pub id: i64,
    pub uid: String,
    pub owner: String,
    pub repo: String,
    pub epic_number: i64,
    /// Como em [`HubRules::label`].
    pub label: String,
    /// Versão vigente quando o agente propôs (0 = sem regras).
    pub base_version: i64,
    /// Versão vigente agora (0 = sem regras).
    pub current_version: i64,
    /// Texto vigente agora, para a diferença (só nas pendentes).
    pub current_content: Option<String>,
    /// A vigente já não é a `base_version`: aprovar substitui também o que
    /// mudou depois que o agente leu as regras (só nas pendentes).
    pub base_changed: bool,
    /// Texto completo proposto.
    pub content: String,
    pub reason: String,
    pub state: ProposalState,
    pub env_id: String,
    pub env_name: String,
    pub session_id: Option<String>,
    pub created_at: i64,
    pub decided_at: Option<i64>,
    pub decision_note: Option<String>,
    /// Versão gravada pela aprovação.
    pub applied_version: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct HubRulesProposals {
    /// Da mais nova para a mais velha.
    pub proposals: Vec<HubRulesProposal>,
    pub truncated: bool,
}

/// Filtros de `hub.rules.proposals.list`.
#[derive(Debug, Clone, Default)]
pub struct RulesProposalFilter {
    /// `None` = todos os estados.
    pub state: Option<ProposalState>,
    pub project: Option<(String, String)>,
    /// `Some(0)` = só as do projeto; `None` = todos os escopos.
    pub epic_number: Option<i64>,
}

const VERSION_COLS: &str = "r.version, r.content, r.source, r.proposal_id, r.author_env_id, \
                            e.name, pr.session_id, r.note, r.created_at";
const VERSION_FROM: &str = "FROM agent_rules r
     LEFT JOIN env e ON e.env_id = r.author_env_id
     LEFT JOIN agent_rules_proposal pr ON pr.id = r.proposal_id";

fn version_row(r: &Row) -> rusqlite::Result<HubRuleVersion> {
    let content: String = r.get(1)?;
    Ok(HubRuleVersion {
        version: r.get(0)?,
        bytes: content.len() as i64,
        content,
        source: crate::parse_col(r, 2)?,
        proposal_id: r.get(3)?,
        author_env_id: r.get(4)?,
        author_env_name: r.get(5)?,
        session_id: r.get(6)?,
        note: r.get(7)?,
        created_at: r.get(8)?,
    })
}

const PROPOSAL_COLS: &str = "pr.id, pr.uid, p.gh_owner, p.gh_repo, pr.epic_number, \
     pr.base_version, pr.content, pr.reason, pr.state, pr.env_id, coalesce(e.name, pr.env_id), \
     pr.session_id, pr.created_at, pr.decided_at, pr.decision_note, pr.applied_version, \
     p.tracker_kind, p.tracker_config, p.name, pr.project_id";
const PROPOSAL_FROM: &str = "FROM agent_rules_proposal pr
     JOIN project p ON p.id = pr.project_id
     LEFT JOIN env e ON e.env_id = pr.env_id";

/// Linha da proposta sem a versão vigente (preenchida depois) e o id do
/// projeto.
fn proposal_row(r: &Row) -> rusqlite::Result<(HubRulesProposal, i64)> {
    let (owner, repo): (String, String) = (r.get(2)?, r.get(3)?);
    let epic_number: i64 = r.get(4)?;
    let tracker = crate::tracker_col(r, 16)?;
    let name: String = r.get(18)?;
    let label = scope_label(&name, &owner, &repo, &tracker, epic_number);
    Ok((
        HubRulesProposal {
            id: r.get(0)?,
            uid: r.get(1)?,
            owner,
            repo,
            epic_number,
            label,
            base_version: r.get(5)?,
            current_version: 0,
            current_content: None,
            base_changed: false,
            content: r.get(6)?,
            reason: r.get(7)?,
            state: crate::parse_col(r, 8)?,
            env_id: r.get(9)?,
            env_name: r.get(10)?,
            session_id: r.get(11)?,
            created_at: r.get(12)?,
            decided_at: r.get(13)?,
            decision_note: r.get(14)?,
            applied_version: r.get(15)?,
        },
        r.get(19)?,
    ))
}

fn scope_label(
    name: &str,
    owner: &str,
    repo: &str,
    tracker: &ct_core::TrackerConfig,
    epic_number: i64,
) -> String {
    if epic_number == 0 {
        name.to_string()
    } else {
        ct_core::item_ref(&ct_core::TrackerRef::new(owner, repo, tracker), epic_number)
    }
}

fn invalid(msg: impl Into<String>) -> LedgerError {
    LedgerError::Conflict(msg.into())
}

impl Hub {
    /// Projeto cadastrado e escopo válido (0 = projeto, > 0 = épico).
    fn rules_project(&self, owner: &str, repo: &str, epic_number: i64) -> Result<HubProject> {
        if epic_number < 0 {
            return Err(invalid(
                "epic_number deve ser 0 (projeto) ou o número do épico",
            ));
        }
        self.find_project(owner, repo)?.ok_or_else(|| {
            LedgerError::NotFound("projeto", format!("{owner}/{repo} não está no hub"))
        })
    }

    fn rules_label(p: &HubProject, epic_number: i64) -> String {
        scope_label(&p.name, &p.gh_owner, &p.gh_repo, &p.tracker, epic_number)
    }

    /// Versão vigente do escopo.
    fn current_rules(&self, project_id: i64, epic_number: i64) -> Result<Option<HubRuleVersion>> {
        Ok(self
            .conn
            .query_row(
                &format!(
                    "SELECT {VERSION_COLS} {VERSION_FROM}
                     WHERE r.project_id = ?1 AND r.epic_number = ?2
                     ORDER BY r.version DESC LIMIT 1"
                ),
                params![project_id, epic_number],
                version_row,
            )
            .optional()?)
    }

    /// `hub.rules.get`.
    pub fn rules(&self, owner: &str, repo: &str, epic_number: i64) -> Result<HubRules> {
        let p = self.rules_project(owner, repo, epic_number)?;
        let pending: i64 = self.conn.query_row(
            "SELECT count(*) FROM agent_rules_proposal
             WHERE project_id = ?1 AND epic_number = ?2 AND state = 'pendente'",
            params![p.id, epic_number],
            |r| r.get(0),
        )?;
        Ok(HubRules {
            label: Self::rules_label(&p, epic_number),
            current: self.current_rules(p.id, epic_number)?,
            owner: p.gh_owner,
            repo: p.gh_repo,
            epic_number,
            pending,
            max_bytes: RULES_MAX_BYTES as i64,
            warn_bytes: RULES_WARN_BYTES as i64,
        })
    }

    /// Grava a versão seguinte do escopo e incrementa o `config_rev`, na
    /// transação de quem chama. Devolve o número da versão.
    #[allow(clippy::too_many_arguments)]
    fn insert_rules_version(
        &self,
        project_id: i64,
        epic_number: i64,
        content: &str,
        source: RuleSource,
        proposal_id: Option<i64>,
        author_env_id: Option<&str>,
        note: Option<&str>,
        now: i64,
    ) -> Result<i64> {
        let version: i64 = self.conn.query_row(
            "SELECT coalesce(max(version), 0) + 1 FROM agent_rules
             WHERE project_id = ?1 AND epic_number = ?2",
            params![project_id, epic_number],
            |r| r.get(0),
        )?;
        self.conn.execute(
            "INSERT INTO agent_rules (project_id, epic_number, version, content, source,
                                      proposal_id, author_env_id, note, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                project_id,
                epic_number,
                version,
                content,
                source.as_str(),
                proposal_id,
                author_env_id,
                note,
                now
            ],
        )?;
        self.bump_config_rev()?;
        Ok(version)
    }

    /// `hub.rules.set` (`source = usuario`) e `hub.rules.import`
    /// (`source = importacao`): o usuário grava o texto completo do escopo.
    /// `base_version` = a versão que a UI mostrava: se a vigente mudou desde
    /// então, recusa (o usuário recarrega e decide de novo).
    #[allow(clippy::too_many_arguments)]
    pub fn set_rules(
        &self,
        owner: &str,
        repo: &str,
        epic_number: i64,
        content: &str,
        source: RuleSource,
        base_version: Option<i64>,
        note: Option<&str>,
        now: i64,
    ) -> Result<HubRuleVersion> {
        if source == RuleSource::Proposta {
            return Err(invalid(
                "versão de proposta só sai da aprovação da proposta",
            ));
        }
        check_rules_size(content).map_err(LedgerError::Conflict)?;
        let p = self.rules_project(owner, repo, epic_number)?;
        let label = Self::rules_label(&p, epic_number);
        let tx = self.tx()?;
        let cur = self.current_rules(p.id, epic_number)?;
        let cur_version = cur.as_ref().map_or(0, |c| c.version);
        if let Some(b) = base_version.filter(|b| *b != cur_version) {
            return Err(invalid(format!(
                "as regras de {label} mudaram desde a versão {b} (vigente: {cur_version}); \
                 recarregue antes de gravar"
            )));
        }
        if cur.as_ref().map_or("", |c| c.content.as_str()) == content {
            return Err(invalid(format!(
                "nada a mudar: o texto é igual às regras vigentes de {label}"
            )));
        }
        let note = note.map(str::trim).filter(|n| !n.is_empty());
        let v =
            self.insert_rules_version(p.id, epic_number, content, source, None, None, note, now)?;
        tx.commit()?;
        Ok(self
            .rules_version(p.id, epic_number, v)?
            .expect("acabou de ser gravada"))
    }

    fn rules_version(
        &self,
        project_id: i64,
        epic_number: i64,
        version: i64,
    ) -> Result<Option<HubRuleVersion>> {
        Ok(self
            .conn
            .query_row(
                &format!(
                    "SELECT {VERSION_COLS} {VERSION_FROM}
                     WHERE r.project_id = ?1 AND r.epic_number = ?2 AND r.version = ?3"
                ),
                params![project_id, epic_number, version],
                version_row,
            )
            .optional()?)
    }

    /// `hub.rules.history`.
    pub fn rules_history(
        &self,
        owner: &str,
        repo: &str,
        epic_number: i64,
        limit: u32,
    ) -> Result<HubRulesHistory> {
        let p = self.rules_project(owner, repo, epic_number)?;
        let limit = limit.clamp(1, 500) as usize;
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {VERSION_COLS} {VERSION_FROM}
             WHERE r.project_id = ?1 AND r.epic_number = ?2
             ORDER BY r.version DESC LIMIT ?3"
        ))?;
        let mut versions: Vec<HubRuleVersion> = stmt
            .query_map(params![p.id, epic_number, limit as i64 + 1], version_row)?
            .collect::<rusqlite::Result<_>>()?;
        let truncated = versions.len() > limit;
        versions.truncate(limit);
        Ok(HubRulesHistory {
            versions,
            truncated,
        })
    }

    /// Completa a proposta com a versão vigente do escopo.
    fn with_current(&self, (mut p, pid): (HubRulesProposal, i64)) -> Result<HubRulesProposal> {
        let cur = self.current_rules(pid, p.epic_number)?;
        p.current_version = cur.as_ref().map_or(0, |c| c.version);
        if p.state == ProposalState::Pendente {
            p.base_changed = p.current_version != p.base_version;
            p.current_content = Some(cur.map(|c| c.content).unwrap_or_default());
        }
        Ok(p)
    }

    pub fn rules_proposal(&self, id: i64) -> Result<Option<HubRulesProposal>> {
        let row = self
            .conn
            .query_row(
                &format!("SELECT {PROPOSAL_COLS} {PROPOSAL_FROM} WHERE pr.id = ?1"),
                [id],
                proposal_row,
            )
            .optional()?;
        row.map(|r| self.with_current(r)).transpose()
    }

    /// `hub.rules.proposals.list`.
    pub fn list_rules_proposals(
        &self,
        f: &RulesProposalFilter,
        limit: u32,
    ) -> Result<HubRulesProposals> {
        let limit = limit.clamp(1, 500) as usize;
        let key = f.project.as_ref().map(|(o, r)| project_key(o, r));
        let rows: Vec<(HubRulesProposal, i64)> = {
            let mut stmt = self.conn.prepare(&format!(
                "SELECT {PROPOSAL_COLS} {PROPOSAL_FROM}
                 WHERE p.deleted_at IS NULL AND (?1 IS NULL OR pr.state = ?1)
                   AND (?2 IS NULL OR p.key = ?2) AND (?3 IS NULL OR pr.epic_number = ?3)
                 ORDER BY pr.created_at DESC, pr.id DESC LIMIT ?4"
            ))?;
            let rows = stmt.query_map(
                params![
                    f.state.map(ProposalState::as_str),
                    key,
                    f.epic_number,
                    limit as i64 + 1
                ],
                proposal_row,
            )?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        let truncated = rows.len() > limit;
        let proposals = rows
            .into_iter()
            .take(limit)
            .map(|r| self.with_current(r))
            .collect::<Result<_>>()?;
        Ok(HubRulesProposals {
            proposals,
            truncated,
        })
    }

    /// Proposta vinda do outbox de `env`: fica `pendente`. Reenvio do mesmo
    /// `uid` não duplica. Uma que o ambiente devia ter recusado (acima do
    /// limite, sem justificativa, escopo inválido) entra já `obsoleta`, com
    /// o motivo, para não sumir sem rastro.
    pub(super) fn ingest_rules_proposal(&self, env: &str, r: &RulesProposalChange) -> Result<()> {
        let pid = self.ensure_project(&r.owner, &r.repo)?;
        let reason = r.reason.trim();
        let problem = if let Err(e) = check_rules_size(&r.content) {
            Some(e)
        } else if reason.is_empty() || reason.chars().count() > RULES_REASON_MAX {
            Some(format!(
                "justificativa vazia ou com mais de {RULES_REASON_MAX} caracteres"
            ))
        } else if r.epic_number < 0 {
            Some("escopo inválido".to_string())
        } else {
            None
        };
        let now = ct_core::now_ms();
        let (state, decided_at) = match &problem {
            Some(_) => (ProposalState::Obsoleta, Some(now)),
            None => (ProposalState::Pendente, None),
        };
        self.conn.execute(
            "INSERT INTO agent_rules_proposal (uid, project_id, epic_number, base_version, content,
                                               reason, state, env_id, session_id, created_at,
                                               decided_at, decision_note)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
             ON CONFLICT (uid) DO NOTHING",
            params![
                r.uid,
                pid,
                r.epic_number.max(0),
                r.base_version,
                r.content,
                reason,
                state.as_str(),
                env,
                r.session_id,
                now,
                decided_at,
                problem.map(|p| format!("recusada pelo hub: {p}"))
            ],
        )?;
        Ok(())
    }

    /// `hub.rules.proposals.decide`: aprovar grava o texto completo proposto
    /// como versão nova (`source = proposta`), mesmo que a vigente já não
    /// seja a base (a UI avisou); rejeitar só marca. Só a UI do hub chega
    /// aqui.
    pub fn decide_rules_proposal(
        &self,
        id: i64,
        approve: bool,
        note: Option<&str>,
        now: i64,
    ) -> Result<HubRulesProposal> {
        let note = note.map(str::trim).filter(|n| !n.is_empty());
        let tx = self.tx()?;
        let p = self
            .rules_proposal(id)?
            .ok_or_else(|| LedgerError::NotFound("proposta de regras", format!("#{id}")))?;
        if p.state != ProposalState::Pendente {
            return Err(invalid(format!("a proposta #{id} já está {}", p.state)));
        }
        let pid: i64 = self.conn.query_row(
            "SELECT project_id FROM agent_rules_proposal WHERE id = ?1",
            [id],
            |r| r.get(0),
        )?;
        let (state, applied) = if approve {
            let same = p.current_content.as_deref() == Some(p.content.as_str());
            let v = if same {
                // O texto já é o vigente (o usuário editou igual): nada a gravar.
                p.current_version
            } else {
                self.insert_rules_version(
                    pid,
                    p.epic_number,
                    &p.content,
                    RuleSource::Proposta,
                    Some(id),
                    Some(&p.env_id),
                    Some(note.unwrap_or(p.reason.as_str())),
                    now,
                )?
            };
            (ProposalState::Aprovada, Some(v))
        } else {
            (ProposalState::Rejeitada, None)
        };
        self.conn.execute(
            "UPDATE agent_rules_proposal SET state = ?2, decided_at = ?3, decision_note = ?4,
                    applied_version = ?5
             WHERE id = ?1",
            params![id, state.as_str(), now, note, applied],
        )?;
        tx.commit()?;
        Ok(self.rules_proposal(id)?.expect("existe"))
    }

    /// Regras vigentes do projeto para o `ConfigProject.rules`: a última
    /// versão de cada escopo, sem os vazios.
    pub(crate) fn config_rules(&self, project_id: i64) -> Result<ConfigRules> {
        let mut stmt = self.conn.prepare(
            "SELECT r.epic_number, r.version, r.content FROM agent_rules r
             WHERE r.project_id = ?1 AND r.version = (
                 SELECT max(version) FROM agent_rules
                 WHERE project_id = r.project_id AND epic_number = r.epic_number)
             ORDER BY r.epic_number",
        )?;
        let rows = stmt.query_map([project_id], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?;
        let mut out = ConfigRules::default();
        for row in rows {
            let (epic, version, content) = row?;
            if content.trim().is_empty() {
                continue;
            }
            if epic == 0 {
                out.project = Some(RuleText { version, content });
            } else {
                out.epics.push(EpicRuleText {
                    number: epic,
                    version,
                    content,
                });
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ConfigResponse, Ledger, NewHubProject, NewProject, OutboxEntry, SyncChange};

    fn hub_with_project() -> Hub {
        let hub = Hub::open_in_memory().unwrap();
        hub.add_project(&NewHubProject {
            owner: "deep-ion-ai".into(),
            repo: "demo".into(),
            name: "demo".into(),
            base_branch: "develop".into(),
            protected_branches: vec!["develop".into()],
        })
        .unwrap();
        hub
    }

    fn proposal(uid: &str, epic: i64, base: i64, content: &str) -> OutboxEntry {
        OutboxEntry {
            seq: 0,
            created_at: 0,
            change: SyncChange::RulesProposal(RulesProposalChange {
                uid: uid.into(),
                owner: "deep-ion-ai".into(),
                repo: "demo".into(),
                epic_number: epic,
                base_version: base,
                content: content.into(),
                reason: "aprendi na sessão".into(),
                session_id: Some("s-1".into()),
            }),
        }
    }

    fn push(hub: &Hub, entries: &[OutboxEntry]) -> crate::IngestResult {
        let entries: Vec<OutboxEntry> = entries
            .iter()
            .enumerate()
            .map(|(i, e)| OutboxEntry {
                seq: i as i64 + 1,
                ..e.clone()
            })
            .collect();
        hub.ingest("env-a", "env-a", &entries).unwrap()
    }

    #[test]
    fn edicao_e_importacao_gravam_versoes_com_a_origem_e_mudam_o_config_rev() {
        let hub = hub_with_project();
        let rev0 = hub.config_rev().unwrap();
        let r = hub.rules("deep-ion-ai", "demo", 0).unwrap();
        assert_eq!((r.current, r.label.as_str()), (None, "demo"));
        let v1 = hub
            .set_rules(
                "deep-ion-ai",
                "demo",
                0,
                "# CLAUDE.md\nregra",
                RuleSource::Importacao,
                Some(0),
                None,
                1,
            )
            .unwrap();
        assert_eq!(
            (v1.version, v1.source, v1.bytes),
            (1, RuleSource::Importacao, 17)
        );
        assert!(hub.config_rev().unwrap() > rev0);
        // Base desatualizada: recusa.
        let err = hub
            .set_rules(
                "deep-ion-ai",
                "demo",
                0,
                "outra",
                RuleSource::Usuario,
                Some(0),
                None,
                2,
            )
            .unwrap_err();
        assert!(
            err.to_string().contains("mudaram desde a versão 0"),
            "{err}"
        );
        // Igual: nada a mudar.
        assert!(hub
            .set_rules(
                "deep-ion-ai",
                "demo",
                0,
                "# CLAUDE.md\nregra",
                RuleSource::Usuario,
                None,
                None,
                2
            )
            .is_err());
        // Acima do limite: recusa com o limite na mensagem.
        let big = "x".repeat(RULES_MAX_BYTES + 1);
        let err = hub
            .set_rules(
                "deep-ion-ai",
                "demo",
                0,
                &big,
                RuleSource::Usuario,
                None,
                None,
                2,
            )
            .unwrap_err();
        assert!(err.to_string().contains("16 KiB"), "{err}");
        // Exatamente no limite: aceita.
        let max = "y".repeat(RULES_MAX_BYTES);
        hub.set_rules(
            "deep-ion-ai",
            "demo",
            0,
            &max,
            RuleSource::Usuario,
            Some(1),
            Some("cheio"),
            3,
        )
        .unwrap();
        let h = hub.rules_history("deep-ion-ai", "demo", 0, 10).unwrap();
        let got: Vec<_> = h
            .versions
            .iter()
            .map(|v| (v.version, v.source, v.note.clone()))
            .collect();
        assert_eq!(
            got,
            [
                (2, RuleSource::Usuario, Some("cheio".to_string())),
                (1, RuleSource::Importacao, None)
            ]
        );
        // A história é só inclusão.
        assert!(hub
            .conn
            .execute("UPDATE agent_rules SET content = 'x'", [])
            .is_err());
        assert!(hub.conn.execute("DELETE FROM agent_rules", []).is_err());
    }

    #[test]
    fn proposta_do_agente_fica_pendente_e_so_vale_depois_de_aprovada() {
        let hub = hub_with_project();
        hub.set_rules(
            "deep-ion-ai",
            "demo",
            0,
            "v1",
            RuleSource::Usuario,
            None,
            None,
            1,
        )
        .unwrap();
        let r = push(&hub, &[proposal("u-1", 0, 1, "v1\nnova regra")]);
        assert_eq!(r.rules_proposals, 1);
        // Reenvio do mesmo uid não duplica.
        push(&hub, &[proposal("u-1", 0, 1, "v1\nnova regra")]);
        let pending = hub
            .list_rules_proposals(
                &RulesProposalFilter {
                    state: Some(ProposalState::Pendente),
                    ..Default::default()
                },
                50,
            )
            .unwrap()
            .proposals;
        assert_eq!(pending.len(), 1);
        let p = &pending[0];
        assert_eq!(
            (
                p.current_version,
                p.base_changed,
                p.current_content.as_deref()
            ),
            (1, false, Some("v1"))
        );
        // Pendente: as regras vigentes (e as entregues) não mudaram.
        assert_eq!(
            hub.rules("deep-ion-ai", "demo", 0)
                .unwrap()
                .current
                .unwrap()
                .version,
            1
        );
        assert_eq!(hub.rules("deep-ion-ai", "demo", 0).unwrap().pending, 1);
        let pid = hub.find_project("deep-ion-ai", "demo").unwrap().unwrap().id;
        assert_eq!(
            hub.config_rules(pid).unwrap().project.unwrap().content,
            "v1"
        );

        let rev = hub.config_rev().unwrap();
        let d = hub.decide_rules_proposal(p.id, true, None, 5).unwrap();
        assert_eq!(
            (d.state, d.applied_version),
            (ProposalState::Aprovada, Some(2))
        );
        assert!(hub.config_rev().unwrap() > rev);
        let cur = hub
            .rules("deep-ion-ai", "demo", 0)
            .unwrap()
            .current
            .unwrap();
        assert_eq!(
            (
                cur.version,
                cur.source,
                cur.proposal_id,
                cur.author_env_id.as_deref()
            ),
            (2, RuleSource::Proposta, Some(p.id), Some("env-a"))
        );
        assert_eq!(cur.session_id.as_deref(), Some("s-1"));
        assert_eq!(cur.note.as_deref(), Some("aprendi na sessão"));
        assert!(hub.decide_rules_proposal(p.id, false, None, 6).is_err());
    }

    #[test]
    fn proposta_sobre_versao_antiga_avisa_e_aprovar_aplica_o_texto_completo() {
        let hub = hub_with_project();
        hub.set_rules(
            "deep-ion-ai",
            "demo",
            7,
            "épico v1",
            RuleSource::Usuario,
            None,
            None,
            1,
        )
        .unwrap();
        push(&hub, &[proposal("u-2", 7, 1, "épico do agente")]);
        hub.set_rules(
            "deep-ion-ai",
            "demo",
            7,
            "épico v2",
            RuleSource::Usuario,
            None,
            None,
            2,
        )
        .unwrap();
        let p = &hub
            .list_rules_proposals(&RulesProposalFilter::default(), 10)
            .unwrap()
            .proposals[0];
        assert_eq!(p.label, "deep-ion-ai/demo#7");
        assert!(p.base_changed);
        assert_eq!(p.current_content.as_deref(), Some("épico v2"));
        let d = hub
            .decide_rules_proposal(p.id, true, Some("ok"), 3)
            .unwrap();
        assert_eq!(d.applied_version, Some(3));
        assert_eq!(
            hub.rules("deep-ion-ai", "demo", 7)
                .unwrap()
                .current
                .unwrap()
                .content,
            "épico do agente"
        );
        // Rejeitada não grava versão.
        push(&hub, &[proposal("u-3", 7, 3, "outra")]);
        let id = hub
            .list_rules_proposals(
                &RulesProposalFilter {
                    state: Some(ProposalState::Pendente),
                    ..Default::default()
                },
                10,
            )
            .unwrap()
            .proposals[0]
            .id;
        let d = hub.decide_rules_proposal(id, false, None, 4).unwrap();
        assert_eq!(
            (d.state, d.applied_version),
            (ProposalState::Rejeitada, None)
        );
        assert_eq!(
            hub.rules_history("deep-ion-ai", "demo", 7, 10)
                .unwrap()
                .versions
                .len(),
            3
        );
    }

    #[test]
    fn proposta_invalida_entra_obsoleta_com_o_motivo() {
        let hub = hub_with_project();
        push(
            &hub,
            &[proposal("u-4", 0, 0, &"z".repeat(RULES_MAX_BYTES + 1))],
        );
        let p = &hub
            .list_rules_proposals(&RulesProposalFilter::default(), 10)
            .unwrap()
            .proposals[0];
        assert_eq!(p.state, ProposalState::Obsoleta);
        assert!(p.decision_note.as_deref().unwrap().contains("16 KiB"));
    }

    #[test]
    fn config_entrega_as_vigentes_e_o_ambiente_guarda_no_cache() {
        let hub = hub_with_project();
        hub.issue_env_token_with_env_id("env-a", "env-a").unwrap();
        hub.add_assignment("deep-ion-ai", "demo", "env-a", None)
            .unwrap();
        hub.set_rules(
            "deep-ion-ai",
            "demo",
            0,
            "projeto",
            RuleSource::Usuario,
            None,
            None,
            1,
        )
        .unwrap();
        hub.set_rules(
            "deep-ion-ai",
            "demo",
            9,
            "épico 9",
            RuleSource::Usuario,
            None,
            None,
            1,
        )
        .unwrap();
        // Épico esvaziado sai da entrega.
        hub.set_rules(
            "deep-ion-ai",
            "demo",
            4,
            "x",
            RuleSource::Usuario,
            None,
            None,
            1,
        )
        .unwrap();
        hub.set_rules(
            "deep-ion-ai",
            "demo",
            4,
            "",
            RuleSource::Usuario,
            None,
            None,
            2,
        )
        .unwrap();
        let cfg: ConfigResponse = hub.env_config("env-a", None, None).unwrap();
        let rules = &cfg.projects.as_ref().unwrap()[0].rules;
        assert_eq!(rules.project.as_ref().unwrap().content, "projeto");
        assert_eq!(
            rules
                .epics
                .iter()
                .map(|e| (e.number, e.version))
                .collect::<Vec<_>>(),
            [(9, 1)]
        );
        // O JSON do `/sync/config` passa por serde e chega ao cache do ambiente.
        let cfg: ConfigResponse =
            serde_json::from_value(serde_json::to_value(&cfg).unwrap()).unwrap();
        let l = Ledger::open_in_memory().unwrap();
        l.add_project(&NewProject {
            name: "demo".into(),
            gh_owner: "deep-ion-ai".into(),
            gh_repo: "demo".into(),
            repo_path: "/r".into(),
            base_branch: "develop".into(),
            protected_branches: vec![],
        })
        .unwrap();
        l.apply_hub_config(&cfg, |_| "/r".into()).unwrap();
        let pid = l.list_projects().unwrap()[0].id;
        assert_eq!(l.agent_rules(pid, 9).unwrap().unwrap().content, "épico 9");
        assert_eq!(l.agent_rules(pid, 0).unwrap().unwrap().version, 1);
        assert_eq!(l.agent_rules(pid, 4).unwrap(), None);
    }
}
