//! Propostas do gestor (issue #218, spec do gestor §8). O agente cria
//! (`POST /agent/v1/proposals`), o usuário aprova ou rejeita na UI do hub
//! (`hub.proposals.decide`), e quem aplica é o hub, na mesma transação, pelo
//! mesmo código das mudanças manuais (`set_priority`, `move_work_item`,
//! `set_status_with`). Só o hub marca `obsoleta`: quando o valor atual do
//! alvo muda, quando chega outra proposta para o mesmo alvo e depois de
//! [`PROPOSAL_TTL_MS`] pendente.
//!
//! Cada decisão do usuário vira também uma decisão `orientada` no work item
//! do alvo (autor = o hub), para o gestor e a revisão do épico a acharem.

use super::{Hub, HubWorkItem, Placement};
use crate::sync::project_key;
use crate::{LedgerError, Result};
use ct_core::{Priority, ProposalKind, ProposalState, WorkItemStatus};
use rusqlite::{params, OptionalExtension, Row};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// Pendentes no hub inteiro (spec §8.2).
pub const PROPOSAL_PENDING_MAX: i64 = 100;
/// Pendente há mais que isso vira `obsoleta`.
pub const PROPOSAL_TTL_MS: i64 = 14 * 24 * 3600 * 1000;
const JUSTIFICATION_MAX: usize = 2000;

/// Status que uma proposta pode pedir. Os automáticos (`em_execucao`,
/// `aguardando_voce`, `em_revisao`) seriam desfeitos pelo próximo gatilho.
const PROPOSABLE_STATUS: &[WorkItemStatus] = &[
    WorkItemStatus::Backlog,
    WorkItemStatus::Bloqueado,
    WorkItemStatus::Pausado,
    WorkItemStatus::Concluido,
];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct HubProposal {
    pub id: i64,
    pub uid: String,
    pub kind: ProposalKind,
    pub owner: String,
    pub repo: String,
    pub number: i64,
    /// Referência pronta do alvo (`ct_core::item_ref`): `owner/repo#n` no
    /// GitHub, `PROJ-n` no Jira.
    #[serde(rename = "ref", default)]
    #[cfg_attr(feature = "ts", ts(rename = "ref"))]
    pub item_ref: String,
    /// Título da issue no espelho, se houver.
    pub title: Option<String>,
    /// Valor do alvo lido pelo hub ao criar: `{ "priority": .. }` ou
    /// `{ "status": .. }`.
    #[cfg_attr(feature = "ts", ts(type = "unknown"))]
    pub current: Value,
    /// `{ "priority": "P1" | null, "before"?: n, "after"?: n }` ou
    /// `{ "status": .. }`.
    #[cfg_attr(feature = "ts", ts(type = "unknown"))]
    pub proposed: Value,
    pub justification: String,
    pub state: ProposalState,
    pub env_id: String,
    /// Nome do ambiente que propôs.
    pub env_name: String,
    pub session_id: Option<String>,
    pub created_at: i64,
    pub decided_at: Option<i64>,
    /// Nota do usuário ou motivo da obsolescência.
    pub decision_note: Option<String>,
    /// Evento que aplicou a mudança (aprovada com algo a mudar).
    pub applied_event_uid: Option<String>,
}

/// O que o agente pede em `POST /agent/v1/proposals`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NewProposal {
    pub kind: ProposalKind,
    pub owner: String,
    pub repo: String,
    pub number: i64,
    pub proposed: Value,
    pub justification: String,
}

/// Filtros de `hub.proposals.list` e `GET /agent/v1/proposals`.
#[derive(Debug, Clone, Default)]
pub struct ProposalFilter {
    /// `None` = todos os estados.
    pub state: Option<ProposalState>,
    pub project: Option<(String, String)>,
    pub number: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct HubProposals {
    /// Da mais nova para a mais velha.
    pub proposals: Vec<HubProposal>,
    pub truncated: bool,
}

/// Resultado de cada id em `hub.proposals.decide` em lote.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct ProposalDecided {
    pub id: i64,
    /// A proposta depois da decisão (pode ter virado `obsoleta`).
    pub proposal: Option<HubProposal>,
    pub error: Option<String>,
}

const COLS: &str = "pr.id, pr.uid, pr.kind, p.gh_owner, p.gh_repo, pr.gh_number, i.title, \
                    pr.current_value, pr.proposed_value, pr.justification, pr.state, pr.env_id, \
                    coalesce(e.name, pr.env_id), pr.session_id, pr.created_at, pr.decided_at, \
                    pr.decision_note, pr.applied_event_uid, p.tracker_kind, p.tracker_config";
const FROM: &str = "FROM proposal pr
     JOIN project p ON p.id = pr.project_id
     LEFT JOIN issue_cache i ON i.project_id = pr.project_id AND i.gh_number = pr.gh_number
     LEFT JOIN env e ON e.env_id = pr.env_id";

fn row(r: &Row) -> rusqlite::Result<HubProposal> {
    let (owner, repo, number): (String, String, i64) = (r.get(3)?, r.get(4)?, r.get(5)?);
    let tracker = crate::tracker_col(r, 18)?;
    Ok(HubProposal {
        id: r.get(0)?,
        uid: r.get(1)?,
        kind: crate::parse_col(r, 2)?,
        item_ref: ct_core::item_ref(&ct_core::TrackerRef::new(&owner, &repo, &tracker), number),
        owner,
        repo,
        number,
        title: r.get(6)?,
        current: crate::json_col(r, 7)?,
        proposed: crate::json_col(r, 8)?,
        justification: r.get(9)?,
        state: crate::parse_col(r, 10)?,
        env_id: r.get(11)?,
        env_name: r.get(12)?,
        session_id: r.get(13)?,
        created_at: r.get(14)?,
        decided_at: r.get(15)?,
        decision_note: r.get(16)?,
        applied_event_uid: r.get(17)?,
    })
}

fn invalid(msg: impl Into<String>) -> LedgerError {
    LedgerError::Conflict(msg.into())
}

/// Confere e normaliza `proposed` (spec §8.1).
fn normalize(kind: ProposalKind, proposed: &Value) -> Result<Value> {
    let obj = proposed
        .as_object()
        .ok_or_else(|| invalid("proposed deve ser um objeto"))?;
    match kind {
        ProposalKind::Priority => {
            let priority = match obj.get("priority") {
                Some(Value::Null) => None,
                Some(Value::String(p)) => Some(
                    p.parse::<Priority>()
                        .map_err(|_| invalid("priority deve ser P0, P1, P2, P3 ou null"))?,
                ),
                _ => return Err(invalid("proposed.priority é obrigatório (P0–P3 ou null)")),
            };
            let anchor = |k: &str| match obj.get(k) {
                None | Some(Value::Null) => Ok(None),
                Some(v) => v
                    .as_i64()
                    .map(Some)
                    .ok_or_else(|| invalid(format!("proposed.{k} deve ser o número de um item"))),
            };
            let (before, after) = (anchor("before")?, anchor("after")?);
            if before.is_some() && after.is_some() {
                return Err(invalid("use before ou after, não os dois"));
            }
            if priority.is_none() && (before.is_some() || after.is_some()) {
                return Err(invalid("item não triado não tem posição"));
            }
            let mut v = json!({ "priority": priority });
            if let Some(b) = before {
                v["before"] = json!(b);
            }
            if let Some(a) = after {
                v["after"] = json!(a);
            }
            Ok(v)
        }
        ProposalKind::Status => {
            let status = obj
                .get("status")
                .and_then(Value::as_str)
                .and_then(|s| s.parse::<WorkItemStatus>().ok())
                .filter(|s| PROPOSABLE_STATUS.contains(s))
                .ok_or_else(|| {
                    invalid("proposed.status deve ser backlog, bloqueado, pausado ou concluido")
                })?;
            Ok(json!({ "status": status }))
        }
    }
}

impl Hub {
    /// Valor atual do alvo para `kind`, ou `None` se o alvo não serve
    /// (sem work item e sem issue aberta no espelho).
    fn proposal_current(&self, kind: ProposalKind, pid: i64, number: i64) -> Result<Option<Value>> {
        let wi: Option<HubWorkItem> = self
            .conn
            .query_row(
                &format!(
                    "SELECT {} FROM work_item WHERE project_id = ?1 AND gh_number = ?2",
                    super::WORK_ITEM_COLS
                ),
                params![pid, number],
                super::row_to_work_item,
            )
            .optional()?;
        if wi.is_none()
            && !self
                .get_issue(pid, number)?
                .is_some_and(|i| i.state == "open")
        {
            return Ok(None);
        }
        Ok(Some(match kind {
            ProposalKind::Priority => json!({ "priority": wi.as_ref().and_then(|w| w.priority) }),
            ProposalKind::Status => json!({
                "status": wi.map(|w| w.status).unwrap_or(WorkItemStatus::Backlog)
            }),
        }))
    }

    pub fn proposal(&self, id: i64) -> Result<Option<HubProposal>> {
        Ok(self
            .conn
            .query_row(&format!("SELECT {COLS} {FROM} WHERE pr.id = ?1"), [id], row)
            .optional()?)
    }

    fn set_obsolete(&self, id: i64, note: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE proposal SET state = 'obsoleta', decided_at = ?2, decision_note = ?3
             WHERE id = ?1 AND state = 'pendente'",
            params![id, ct_core::now_ms(), note],
        )?;
        Ok(())
    }

    /// Quantas propostas estão pendentes no hub.
    pub fn pending_proposals(&self) -> Result<i64> {
        Ok(self.conn.query_row(
            "SELECT count(*) FROM proposal WHERE state = 'pendente'",
            [],
            |r| r.get(0),
        )?)
    }

    /// Pendentes há mais de [`PROPOSAL_TTL_MS`] viram `obsoleta`.
    /// Devolve quantas.
    pub fn expire_proposals(&self, now: i64) -> Result<usize> {
        Ok(self.conn.execute(
            "UPDATE proposal SET state = 'obsoleta', decided_at = ?1,
                    decision_note = 'expirou: pendente há mais de 14 dias'
             WHERE state = 'pendente' AND created_at < ?2",
            params![now, now - PROPOSAL_TTL_MS],
        )?)
    }

    /// Pendentes do work item cujo valor atual mudou viram `obsoleta`
    /// (chamado em toda mudança de prioridade e de status do alvo).
    pub(crate) fn obsolete_changed(&self, work_item_id: i64) -> Result<()> {
        let pending: Vec<(i64, ProposalKind, i64, i64, String)> = {
            let mut stmt = self.conn.prepare(
                "SELECT pr.id, pr.kind, pr.project_id, pr.gh_number, pr.current_value
                 FROM proposal pr JOIN work_item w
                   ON w.project_id = pr.project_id AND w.gh_number = pr.gh_number
                 WHERE w.id = ?1 AND pr.state = 'pendente'",
            )?;
            let rows = stmt.query_map([work_item_id], |r| {
                Ok((
                    r.get(0)?,
                    crate::parse_col(r, 1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                ))
            })?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        for (id, kind, pid, number, stored) in pending {
            let stored: Value =
                serde_json::from_str(&stored).map_err(|e| LedgerError::Corrupt(e.to_string()))?;
            let now = self.proposal_current(kind, pid, number)?;
            if now.as_ref() != Some(&stored) {
                self.set_obsolete(id, &changed_note(now.as_ref()))?;
            }
        }
        Ok(())
    }

    /// `POST /agent/v1/proposals`: cria uma proposta pendente. `env_id` é o
    /// do token de agente; `session_id`, a sessão do gestor.
    pub fn create_proposal(
        &self,
        np: &NewProposal,
        env_id: &str,
        session_id: Option<&str>,
        now: i64,
    ) -> Result<HubProposal> {
        let justification = np.justification.trim();
        if justification.is_empty() || justification.chars().count() > JUSTIFICATION_MAX {
            return Err(invalid(format!(
                "justification é obrigatória, com até {JUSTIFICATION_MAX} caracteres"
            )));
        }
        let proposed = normalize(np.kind, &np.proposed)?;
        let Some(project) = self.find_project(&np.owner, &np.repo)? else {
            return Err(LedgerError::NotFound(
                "projeto",
                format!("{}/{} não está no hub", np.owner, np.repo),
            ));
        };
        let target = project.item_ref(np.number);
        let tx = self.tx()?;
        self.expire_proposals(now)?;
        let Some(current) = self.proposal_current(np.kind, project.id, np.number)? else {
            return Err(LedgerError::NotFound(
                "issue",
                format!("{target} não é work item do hub nem issue aberta no espelho"),
            ));
        };
        let positioned = proposed.get("before").is_some() || proposed.get("after").is_some();
        let same = match np.kind {
            ProposalKind::Priority => current["priority"] == proposed["priority"] && !positioned,
            ProposalKind::Status => current == proposed,
        };
        if same {
            return Err(invalid(format!("nada a mudar: {target} já está assim")));
        }
        if self.pending_proposals()? >= PROPOSAL_PENDING_MAX {
            return Err(invalid(
                "muitas propostas pendentes: peça ao usuário para revisar as que existem",
            ));
        }
        self.conn.execute(
            "INSERT INTO proposal (uid, kind, project_id, gh_number, current_value,
                                   proposed_value, justification, env_id, session_id, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                uuid::Uuid::new_v4().to_string(),
                np.kind.as_str(),
                project.id,
                np.number,
                current.to_string(),
                proposed.to_string(),
                justification,
                env_id,
                session_id,
                now
            ],
        )?;
        let id = self.conn.last_insert_rowid();
        // A anterior do mesmo tipo e alvo sai do caminho.
        self.conn.execute(
            "UPDATE proposal SET state = 'obsoleta', decided_at = ?4, decision_note = ?5
             WHERE project_id = ?1 AND gh_number = ?2 AND kind = ?3 AND state = 'pendente'
               AND id <> ?6",
            params![
                project.id,
                np.number,
                np.kind.as_str(),
                now,
                format!("substituída pela proposta #{id}"),
                id
            ],
        )?;
        let wi = self
            .find_work_item(&np.owner, &np.repo, np.number)?
            .map(|w| w.id);
        self.hub_event(
            env_id,
            project.id,
            wi,
            "proposal_created",
            &json!({ "proposal_id": id, "kind": np.kind, "proposed": proposed }),
        )?;
        tx.commit()?;
        Ok(self.proposal(id)?.expect("acabou de ser criada"))
    }

    /// `hub.proposals.list` e `GET /agent/v1/proposals`.
    pub fn list_proposals(&self, f: &ProposalFilter, limit: u32, now: i64) -> Result<HubProposals> {
        self.expire_proposals(now)?;
        let limit = limit.clamp(1, super::RANKING_MAX) as usize;
        let key = f.project.as_ref().map(|(o, r)| project_key(o, r));
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {COLS} {FROM}
             WHERE p.deleted_at IS NULL AND (?1 IS NULL OR pr.state = ?1)
               AND (?2 IS NULL OR p.key = ?2) AND (?3 IS NULL OR pr.gh_number = ?3)
             ORDER BY pr.created_at DESC, pr.id DESC LIMIT ?4"
        ))?;
        let rows = stmt.query_map(
            params![
                f.state.map(ProposalState::as_str),
                key,
                f.number,
                limit as i64 + 1
            ],
            row,
        )?;
        let mut proposals: Vec<HubProposal> = rows.collect::<rusqlite::Result<_>>()?;
        let truncated = proposals.len() > limit;
        proposals.truncate(limit);
        Ok(HubProposals {
            proposals,
            truncated,
        })
    }

    /// `hub.proposals.decide` para uma proposta, numa transação: relê o
    /// valor atual (mudou = `obsoleta`, sem aplicar), aplica se aprovada,
    /// grava o evento `proposal_decided` e a decisão `orientada`. Só a UI
    /// do hub chega aqui.
    pub fn decide_proposal(
        &self,
        id: i64,
        approve: bool,
        note: Option<&str>,
        hub_id: &str,
        now: i64,
    ) -> Result<HubProposal> {
        let note = note.map(str::trim).filter(|n| !n.is_empty());
        let tx = self.tx()?;
        self.expire_proposals(now)?;
        let p = self
            .proposal(id)?
            .ok_or_else(|| LedgerError::NotFound("proposta", format!("#{id}")))?;
        if p.state != ProposalState::Pendente {
            return Err(invalid(format!("a proposta #{id} já está {}", p.state)));
        }
        let pid: i64 =
            self.conn
                .query_row("SELECT project_id FROM proposal WHERE id = ?1", [id], |r| {
                    r.get(0)
                })?;
        let current = self.proposal_current(p.kind, pid, p.number)?;
        if current.as_ref() != Some(&p.current) {
            self.set_obsolete(id, &changed_note(current.as_ref()))?;
            tx.commit()?;
            return Ok(self.proposal(id)?.expect("existe"));
        }
        let state = if approve {
            ProposalState::Aprovada
        } else {
            ProposalState::Rejeitada
        };
        self.conn.execute(
            "UPDATE proposal SET state = ?2, decided_at = ?3, decision_note = ?4 WHERE id = ?1",
            params![id, state.as_str(), now, note],
        )?;
        let (owner, repo, n) = (p.owner.as_str(), p.repo.as_str(), p.number);
        let applied = if approve {
            self.apply_proposal(&p, note, hub_id)?
        } else {
            None
        };
        if let Some(uid) = &applied {
            self.conn.execute(
                "UPDATE proposal SET applied_event_uid = ?2 WHERE id = ?1",
                params![id, uid],
            )?;
        }
        // O work item do alvo recebe o evento e a decisão (rejeitar uma
        // proposta de issue sem work item cria o work item, como a UI).
        let wi = match self.find_work_item(owner, repo, n)? {
            Some(w) => w.id,
            None => self.manual_work_item(pid, owner, repo, n)?,
        };
        let mut payload = json!({ "proposal_id": id, "state": state });
        if let Some(n) = note {
            payload["note"] = json!(n);
        }
        self.hub_event(hub_id, pid, Some(wi), "proposal_decided", &payload)?;
        self.proposal_decision(&p, state, note, wi, hub_id, now)?;
        tx.commit()?;
        Ok(self.proposal(id)?.expect("existe"))
    }

    /// Aplica a proposta aprovada. Devolve o `uid` do evento que mudou o
    /// alvo, se algo mudou.
    fn apply_proposal(
        &self,
        p: &HubProposal,
        note: Option<&str>,
        hub_id: &str,
    ) -> Result<Option<String>> {
        let (owner, repo, n) = (p.owner.as_str(), p.repo.as_str(), p.number);
        let reason = Some(note.unwrap_or(p.justification.as_str()));
        match p.kind {
            ProposalKind::Priority => {
                let priority = p.proposed["priority"]
                    .as_str()
                    .map(|s| s.parse::<Priority>())
                    .transpose()?;
                self.set_priority(owner, repo, n, priority, reason, Some(p.id), hub_id)?;
                let to = match (p.proposed["before"].as_i64(), p.proposed["after"].as_i64()) {
                    (Some(b), _) => Some(Placement::Before(b)),
                    (_, Some(a)) => Some(Placement::After(a)),
                    _ => None,
                };
                if let (Some(to), Some(_)) = (to, priority) {
                    // Âncora que mudou de classe (ou saiu do ranking): o
                    // item vai para o fim da classe. Item fora do ranking
                    // (concluído) fica sem posição.
                    match self.move_work_item(owner, repo, n, to, Some(p.id), hub_id) {
                        Ok(_) => {}
                        Err(LedgerError::Conflict(_)) => match self.move_work_item(
                            owner,
                            repo,
                            n,
                            Placement::End,
                            Some(p.id),
                            hub_id,
                        ) {
                            Ok(_) | Err(LedgerError::Conflict(_)) => {}
                            Err(e) => return Err(e),
                        },
                        Err(e) => return Err(e),
                    }
                }
            }
            ProposalKind::Status => {
                let status: WorkItemStatus =
                    p.proposed["status"].as_str().unwrap_or_default().parse()?;
                self.set_status_with(owner, repo, n, status, reason, Some(p.id), hub_id)?;
            }
        }
        Ok(self
            .conn
            .query_row(
                "SELECT uid FROM event
                 WHERE type IN ('priority_set', 'rank_moved', 'status_set_manual')
                   AND json_extract(payload, '$.proposal_id') = ?1
                 ORDER BY hlc DESC LIMIT 1",
                [p.id],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// Decisão `orientada` com o que o usuário decidiu sobre a proposta.
    fn proposal_decision(
        &self,
        p: &HubProposal,
        state: ProposalState,
        note: Option<&str>,
        wi: i64,
        hub_id: &str,
        now: i64,
    ) -> Result<()> {
        let what = match p.kind {
            ProposalKind::Priority => format!(
                "prioridade {}",
                p.proposed["priority"].as_str().unwrap_or("não triado")
            ),
            ProposalKind::Status => {
                format!("status {}", p.proposed["status"].as_str().unwrap_or("?"))
            }
        };
        let verb = if state == ProposalState::Aprovada {
            "aprovada"
        } else {
            "rejeitada"
        };
        let target = &p.item_ref;
        let mut decision = format!(
            "O usuário {verb} na tela Hub a proposta #{} ({what} para {target}).",
            p.id
        );
        if let Some(n) = note {
            decision.push_str(&format!(" Nota: {n}"));
        }
        self.conn.execute(
            "INSERT INTO decision (uid, env_id, work_item_id, session_id, origin, title, context,
                                   decision, consequences, created_at)
             VALUES (?1, ?2, ?3, ?4, 'orientada', ?5, ?6, ?7, ?8, ?9)",
            params![
                uuid::Uuid::new_v4().to_string(),
                hub_id,
                wi,
                p.session_id,
                format!("Proposta #{} {verb}: {what} para {target}", p.id),
                format!("Proposta do gestor ({}): {}", p.env_name, p.justification),
                decision,
                (state == ProposalState::Aprovada).then_some("Aplicada pelo hub."),
                now
            ],
        )?;
        Ok(())
    }
}

fn changed_note(now: Option<&Value>) -> String {
    match now {
        None => "o alvo saiu do hub ou a issue foi fechada".into(),
        Some(v) => {
            let (k, x) = v
                .as_object()
                .and_then(|o| o.iter().next())
                .map(|(k, x)| (k.as_str(), x.as_str().unwrap_or("não triado").to_string()))
                .unwrap_or(("valor", v.to_string()));
            let k = if k == "priority" { "prioridade" } else { k };
            format!("valor atual mudou: {k} {x}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{GithubSnapshot, IssueRow, NewHubProject};
    use ct_core::DecisionOrigin;

    const HUB: &str = "hub-id";
    const ENV: &str = "env-gestor";

    fn issue(n: i64, state: &str) -> IssueRow {
        IssueRow {
            gh_number: n,
            title: format!("Issue {n}"),
            state: state.into(),
            labels: vec![],
            parent_number: None,
            has_sub_issues: false,
            url: "u".into(),
            updated_at: 1,
            ..Default::default()
        }
    }

    fn hub() -> Hub {
        let hub = Hub::open_in_memory().unwrap();
        hub.add_project(&NewHubProject {
            owner: "o".into(),
            repo: "a".into(),
            name: "a".into(),
            base_branch: "develop".into(),
            protected_branches: vec![],
        })
        .unwrap();
        let pid = hub.find_project("o", "a").unwrap().unwrap().id;
        hub.apply_github(
            &GithubSnapshot {
                project_id: pid,
                issues: vec![
                    issue(1, "open"),
                    issue(2, "open"),
                    issue(3, "open"),
                    issue(9, "closed"),
                ],
                prs: vec![],
                etags: vec![],
                synced_at: 1,
            },
            HUB,
        )
        .unwrap();
        hub.conn
            .execute(
                "INSERT INTO env (env_id, name, created_at, last_seen_at) VALUES (?1, 'wsl', 0, 0)",
                [ENV],
            )
            .unwrap();
        hub
    }

    fn propose(hub: &Hub, kind: ProposalKind, n: i64, proposed: Value) -> Result<HubProposal> {
        hub.create_proposal(
            &NewProposal {
                kind,
                owner: "o".into(),
                repo: "a".into(),
                number: n,
                proposed,
                justification: "porque sim".into(),
            },
            ENV,
            Some("sessao-1"),
            ct_core::now_ms(),
        )
    }

    fn prio(hub: &Hub, n: i64, p: &str) -> HubProposal {
        propose(hub, ProposalKind::Priority, n, json!({ "priority": p })).unwrap()
    }

    fn decide(hub: &Hub, id: i64, approve: bool) -> Result<HubProposal> {
        hub.decide_proposal(id, approve, Some("ok"), HUB, ct_core::now_ms())
    }

    fn decisions(hub: &Hub, n: i64) -> Vec<crate::HubDecision> {
        let wi = hub.find_work_item("o", "a", n).unwrap().unwrap();
        hub.list_decisions(&[wi.id], false).unwrap()
    }

    #[test]
    fn criar_le_o_valor_atual_e_recusa_o_que_nao_muda() {
        let hub = hub();
        let p = prio(&hub, 1, "P1");
        assert_eq!(p.state, ProposalState::Pendente);
        assert_eq!(p.current, json!({ "priority": null }));
        assert_eq!(p.proposed, json!({ "priority": "P1" }));
        assert_eq!(
            (p.env_name.as_str(), p.session_id.as_deref()),
            ("wsl", Some("sessao-1"))
        );
        assert_eq!(p.title.as_deref(), Some("Issue 1"));
        // Criar não aplica nada nem cria work item.
        assert!(hub.find_work_item("o", "a", 1).unwrap().is_none());
        let s = propose(
            &hub,
            ProposalKind::Status,
            2,
            json!({ "status": "pausado" }),
        )
        .unwrap();
        assert_eq!(s.current, json!({ "status": "backlog" }));

        for (kind, n, v) in [
            (ProposalKind::Status, 2, json!({ "status": "backlog" })), // nada a mudar
            (ProposalKind::Status, 2, json!({ "status": "em_execucao" })), // automático
            (ProposalKind::Priority, 2, json!({ "priority": null })),  // já não triado
            (ProposalKind::Priority, 2, json!({ "priority": "P9" })),
            (
                ProposalKind::Priority,
                2,
                json!({ "priority": "P1", "before": 1, "after": 3 }),
            ),
            (
                ProposalKind::Priority,
                2,
                json!({ "priority": null, "before": 1 }),
            ),
            (ProposalKind::Priority, 2, json!({})),
        ] {
            assert!(
                matches!(
                    propose(&hub, kind, n, v.clone()),
                    Err(LedgerError::Conflict(_))
                ),
                "{v}"
            );
        }
        // Alvo inválido: issue fechada sem work item, issue inexistente, projeto.
        for n in [9, 99] {
            assert!(matches!(
                propose(&hub, ProposalKind::Priority, n, json!({ "priority": "P1" })),
                Err(LedgerError::NotFound("issue", _))
            ));
        }
        let mut np = NewProposal {
            kind: ProposalKind::Priority,
            owner: "o".into(),
            repo: "x".into(),
            number: 1,
            proposed: json!({ "priority": "P1" }),
            justification: "j".into(),
        };
        assert!(matches!(
            hub.create_proposal(&np, ENV, None, 0),
            Err(LedgerError::NotFound("projeto", _))
        ));
        np.repo = "a".into();
        np.justification = "  ".into();
        assert!(matches!(
            hub.create_proposal(&np, ENV, None, 0),
            Err(LedgerError::Conflict(_))
        ));
        // Evento proposal_created no projeto, sem work item.
        let n: i64 = hub
            .conn
            .query_row(
                "SELECT count(*) FROM event WHERE type = 'proposal_created' AND env_id = ?1
                   AND work_item_id IS NULL",
                [ENV],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 2);
    }

    #[test]
    fn aprovar_aplica_e_registra() {
        let hub = hub();
        let p = prio(&hub, 1, "P1");
        let d = decide(&hub, p.id, true).unwrap();
        assert_eq!(d.state, ProposalState::Aprovada);
        assert_eq!(d.decision_note.as_deref(), Some("ok"));
        let w = hub.find_work_item("o", "a", 1).unwrap().unwrap();
        assert_eq!((w.priority, w.rank), (Some(Priority::P1), Some(1)));
        let ev = hub.list_events(w.id, 10).unwrap();
        let set = ev.iter().find(|e| e.kind == "priority_set").unwrap();
        assert_eq!(set.payload["cause"], "proposal");
        assert_eq!(set.payload["proposal_id"], p.id);
        assert_eq!(set.env_id, HUB);
        assert_eq!(d.applied_event_uid.as_deref(), Some(set.uid.as_str()));
        assert!(ev.iter().any(|e| e.kind == "proposal_decided"
            && e.payload["state"] == "aprovada"
            && e.payload["note"] == "ok"));
        let dec = decisions(&hub, 1);
        assert_eq!(dec.len(), 1);
        assert_eq!(dec[0].origin, DecisionOrigin::Orientada);
        assert_eq!(dec[0].env_id, HUB);
        assert!(dec[0].title.contains("aprovada"), "{}", dec[0].title);
        assert!(dec[0].context.contains("porque sim"));
        // Decidida uma vez só.
        assert!(matches!(
            decide(&hub, p.id, false),
            Err(LedgerError::Conflict(_))
        ));
        assert!(hub
            .conn
            .execute(
                "UPDATE proposal SET state = 'pendente' WHERE id = ?1",
                [p.id]
            )
            .is_err());
        assert!(hub
            .conn
            .execute(
                "UPDATE proposal SET justification = 'x' WHERE id = ?1",
                [p.id]
            )
            .is_err());

        // Status aprovado: status_set_manual com cause proposal.
        let s = propose(
            &hub,
            ProposalKind::Status,
            1,
            json!({ "status": "bloqueado" }),
        )
        .unwrap();
        decide(&hub, s.id, true).unwrap();
        let w = hub.find_work_item("o", "a", 1).unwrap().unwrap();
        assert_eq!(w.status, WorkItemStatus::Bloqueado);
        let ev = hub.list_events(w.id, 20).unwrap();
        assert!(ev.iter().any(|e| e.kind == "status_set_manual"
            && e.payload["cause"] == "proposal"
            && e.payload["reason"] == "ok"));
    }

    #[test]
    fn aprovar_com_posicao_e_ancora_que_mudou_de_classe() {
        let hub = hub();
        for n in [1, 2] {
            hub.set_priority("o", "a", n, Some(Priority::P1), None, None, HUB)
                .unwrap();
        }
        let p = propose(
            &hub,
            ProposalKind::Priority,
            3,
            json!({ "priority": "P1", "before": 1 }),
        )
        .unwrap();
        decide(&hub, p.id, true).unwrap();
        let w = hub.find_work_item("o", "a", 3).unwrap().unwrap();
        assert_eq!(w.rank, Some(1));
        // Mover dentro da mesma classe também é proposta válida.
        let m = propose(
            &hub,
            ProposalKind::Priority,
            3,
            json!({ "priority": "P1", "after": 2 }),
        )
        .unwrap();
        // A âncora muda de classe antes da aprovação: fica no fim.
        hub.set_priority("o", "a", 2, Some(Priority::P2), None, None, HUB)
            .unwrap();
        let d = decide(&hub, m.id, true).unwrap();
        assert_eq!(d.state, ProposalState::Aprovada);
        let w = hub.find_work_item("o", "a", 3).unwrap().unwrap();
        assert_eq!((w.priority, w.rank), (Some(Priority::P1), Some(2)));
    }

    #[test]
    fn rejeitar_nao_aplica() {
        let hub = hub();
        let p = prio(&hub, 1, "P0");
        let d = decide(&hub, p.id, false).unwrap();
        assert_eq!(d.state, ProposalState::Rejeitada);
        assert!(d.applied_event_uid.is_none());
        let w = hub.find_work_item("o", "a", 1).unwrap().unwrap();
        assert_eq!(w.priority, None);
        let ev = hub.list_events(w.id, 10).unwrap();
        assert!(!ev.iter().any(|e| e.kind == "priority_set"));
        assert!(ev
            .iter()
            .any(|e| e.kind == "proposal_decided" && e.payload["state"] == "rejeitada"));
        let dec = decisions(&hub, 1);
        assert!(dec[0].title.contains("rejeitada"));
        assert_eq!(dec[0].consequences, None);
    }

    #[test]
    fn obsoleta_quando_o_valor_muda_substituida_ou_expirada() {
        let hub = hub();
        // Valor atual mudou pela UI: obsoleta na hora, e não pode ser aprovada.
        let p = prio(&hub, 1, "P1");
        hub.set_priority("o", "a", 1, Some(Priority::P2), None, None, HUB)
            .unwrap();
        let p = hub.proposal(p.id).unwrap().unwrap();
        assert_eq!(p.state, ProposalState::Obsoleta);
        assert_eq!(
            p.decision_note.as_deref(),
            Some("valor atual mudou: prioridade P2")
        );
        assert!(matches!(
            decide(&hub, p.id, true),
            Err(LedgerError::Conflict(_))
        ));

        // Status mudou por um gatilho (recompute_status).
        let s = propose(
            &hub,
            ProposalKind::Status,
            1,
            json!({ "status": "pausado" }),
        )
        .unwrap();
        hub.set_status_manual("o", "a", 1, WorkItemStatus::Bloqueado, None, HUB)
            .unwrap();
        assert_eq!(
            hub.proposal(s.id).unwrap().unwrap().state,
            ProposalState::Obsoleta
        );

        // Mudou sem passar pelo gancho (ex.: banco editado): decidir relê e
        // marca obsoleta sem aplicar.
        let q = prio(&hub, 2, "P1");
        hub.conn
            .execute(
                "INSERT INTO work_item (project_id, gh_number, created_at, priority)
                 VALUES ((SELECT id FROM project WHERE kind = 'github'), 2, 0, 'P3')",
                [],
            )
            .unwrap();
        let d = decide(&hub, q.id, true).unwrap();
        assert_eq!(d.state, ProposalState::Obsoleta);
        assert_eq!(
            hub.find_work_item("o", "a", 2).unwrap().unwrap().priority,
            Some(Priority::P3)
        );

        // Substituída por outra do mesmo tipo e alvo; a de outro tipo fica.
        let a = prio(&hub, 3, "P1");
        let st = propose(
            &hub,
            ProposalKind::Status,
            3,
            json!({ "status": "pausado" }),
        )
        .unwrap();
        let b = prio(&hub, 3, "P2");
        let a = hub.proposal(a.id).unwrap().unwrap();
        assert_eq!(a.state, ProposalState::Obsoleta);
        assert_eq!(
            a.decision_note.unwrap(),
            format!("substituída pela proposta #{}", b.id)
        );
        assert_eq!(
            hub.proposal(st.id).unwrap().unwrap().state,
            ProposalState::Pendente
        );

        // Expirada depois de 14 dias.
        let n = hub
            .expire_proposals(ct_core::now_ms() + PROPOSAL_TTL_MS + 1)
            .unwrap();
        assert_eq!(n, 2);
        assert_eq!(
            hub.proposal(b.id).unwrap().unwrap().state,
            ProposalState::Obsoleta
        );
    }

    #[test]
    fn aprovacao_em_lote_listagem_e_limite() {
        let hub = hub();
        let ids: Vec<i64> = [1, 2, 3].iter().map(|&n| prio(&hub, n, "P1").id).collect();
        // Uma delas fica obsoleta antes do lote; as outras seguem.
        hub.set_priority("o", "a", 2, Some(Priority::P3), None, None, HUB)
            .unwrap();
        let out: Vec<_> = ids.iter().map(|&id| decide(&hub, id, true)).collect();
        assert_eq!(out[0].as_ref().unwrap().state, ProposalState::Aprovada);
        assert!(out[1].is_err(), "obsoleta não é aprovada");
        assert_eq!(out[2].as_ref().unwrap().state, ProposalState::Aprovada);
        let w3 = hub.find_work_item("o", "a", 3).unwrap().unwrap();
        assert_eq!((w3.priority, w3.rank), (Some(Priority::P1), Some(2)));

        let all = hub
            .list_proposals(&ProposalFilter::default(), 50, ct_core::now_ms())
            .unwrap();
        assert_eq!(all.proposals.len(), 3);
        assert_eq!(
            all.proposals[0].id, ids[2],
            "da mais nova para a mais velha"
        );
        let f = ProposalFilter {
            state: Some(ProposalState::Aprovada),
            project: Some(("O".into(), "A".into())),
            number: Some(1),
        };
        let one = hub.list_proposals(&f, 50, ct_core::now_ms()).unwrap();
        assert_eq!(one.proposals.len(), 1);
        let cut = hub
            .list_proposals(&ProposalFilter::default(), 1, ct_core::now_ms())
            .unwrap();
        assert!(cut.truncated);

        // Limite de pendentes no hub inteiro.
        hub.conn
            .execute_batch(&format!(
                "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < {})
                 INSERT INTO proposal (uid, kind, project_id, gh_number, current_value,
                                       proposed_value, justification, env_id, created_at)
                 SELECT 'x' || i, 'status', (SELECT id FROM project WHERE kind = 'github'), 1000 + i, '{{}}', '{{}}',
                        'j', '{ENV}', {} FROM n;",
                PROPOSAL_PENDING_MAX,
                ct_core::now_ms()
            ))
            .unwrap();
        assert!(matches!(
            propose(&hub, ProposalKind::Status, 1, json!({ "status": "pausado" })),
            Err(LedgerError::Conflict(m)) if m.contains("muitas propostas")
        ));
    }

    #[test]
    fn excluir_projeto_apaga_as_propostas() {
        let hub = hub();
        prio(&hub, 1, "P1");
        hub.delete_project("o", "a").unwrap();
        let n: i64 = hub
            .conn
            .query_row("SELECT count(*) FROM proposal", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0);
    }
}
