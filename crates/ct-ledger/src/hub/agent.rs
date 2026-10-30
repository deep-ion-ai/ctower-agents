//! Leituras do hub para agentes (`/agent/v1/*`, issue #217, spec do gestor
//! §6 e §7). Respostas enxutas para caber no contexto do agente: títulos,
//! estados e números, sem o corpo das issues, e toda lista com `limit`
//! (padrão [`RANKING_LIMIT`], teto [`RANKING_MAX`]) e `truncated`.

use super::{
    row_to_event, AssignmentState, HealthCounts, HealthFilter, HealthThresholds, Hub, HubProject,
    Severity, EVENT_COLS, RANKING_MAX,
};
use crate::{IssueRow, Result};
use ct_core::{DecisionOrigin, PrState, Priority, TrackerRef, WorkItemKind, WorkItemStatus};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Atribuição de um projeto a um ambiente, no resumo.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentAssignment {
    pub env: String,
    pub state: AssignmentState,
}

/// Projeto em `GET /agent/v1/overview`, na ordem entre projetos.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentProjectSummary {
    pub owner: String,
    pub repo: String,
    pub name: String,
    pub rank: Option<i64>,
    /// Itens abertos por status (issues sem work item contam como `backlog`).
    pub by_status: BTreeMap<String, usize>,
    /// Itens abertos não concluídos por prioridade (`nao_triado` = sem).
    pub by_priority: BTreeMap<String, usize>,
    pub open_prs: usize,
    /// Decisões autônomas sem revisão.
    pub pending_decisions: usize,
    pub signals: HealthCounts,
    pub assignments: Vec<AgentAssignment>,
    /// Onde estão as issues (`github` ou `jira`, #351).
    #[serde(default = "github_tracker")]
    pub tracker: ct_core::TrackerKind,
    /// Como ler uma issue (`gh issue view <n> -R owner/repo`, ou o jira-cli
    /// com o comando de credencial do cadastro, nunca a senha); `<n>` é o
    /// número.
    #[serde(default)]
    pub issue_view: String,
}

fn github_tracker() -> ct_core::TrackerKind {
    ct_core::TrackerKind::Github
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentEnv {
    pub name: String,
    pub last_seen_at: Option<i64>,
    pub pending: i64,
}

/// `GET /agent/v1/overview`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentOverview {
    pub projects: Vec<AgentProjectSummary>,
    pub envs: Vec<AgentEnv>,
    /// Sinais de saúde de todo o hub, por gravidade.
    pub signals: HealthCounts,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentPr {
    pub number: i64,
    pub state: PrState,
    pub updated_at: i64,
}

/// Issue na árvore de `GET /agent/v1/projects/{owner}/{repo}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentNode {
    pub number: i64,
    /// Referência pronta (`ct_core::item_ref`): `owner/repo#n` ou `PROJ-n`.
    #[serde(rename = "ref", default)]
    pub item_ref: String,
    pub title: Option<String>,
    /// `open` | `closed` no espelho; nulo = work item fora do espelho.
    pub state: Option<String>,
    pub kind: WorkItemKind,
    pub status: WorkItemStatus,
    pub priority: Option<Priority>,
    pub rank: Option<i64>,
    pub labels: Vec<String>,
    pub pr: Option<AgentPr>,
    pub last_activity_at: Option<i64>,
    pub live_envs: Vec<String>,
    pub pending_decisions: usize,
    pub children: Vec<AgentNode>,
}

/// `GET /agent/v1/projects/{owner}/{repo}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentProject {
    pub owner: String,
    pub repo: String,
    pub name: String,
    /// Onde estão as issues (`github` ou `jira`, #351).
    #[serde(default = "github_tracker")]
    pub tracker: ct_core::TrackerKind,
    /// Como ler uma issue, como em [`AgentProjectSummary::issue_view`].
    #[serde(default)]
    pub issue_view: String,
    /// Épicos e issues avulsas, com as sub-issues; no fim, os work items
    /// cuja issue não está no espelho.
    pub items: Vec<AgentNode>,
    /// Corte em `limit` itens (contando as sub-issues).
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentItemRef {
    pub number: i64,
    /// Referência pronta (`ct_core::item_ref`): `owner/repo#n` ou `PROJ-n`.
    #[serde(rename = "ref", default)]
    pub item_ref: String,
    pub title: Option<String>,
    pub status: WorkItemStatus,
    pub priority: Option<Priority>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentSession {
    pub id: String,
    pub env: String,
    pub state: ct_core::SessionState,
    pub exit_code: Option<i32>,
    pub started_at: i64,
    pub last_event_at: i64,
    pub ended_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentEvent {
    pub kind: String,
    pub ts: i64,
    pub env: String,
    pub payload: serde_json::Value,
}

/// Decisão em `GET /agent/v1/decisions` e no detalhe do work item.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentDecision {
    pub owner: String,
    pub repo: String,
    pub number: i64,
    /// Referência pronta (`ct_core::item_ref`): `owner/repo#n` ou `PROJ-n`.
    #[serde(rename = "ref", default)]
    pub item_ref: String,
    pub origin: DecisionOrigin,
    pub title: String,
    pub decision: String,
    pub reviewed_at: Option<i64>,
    pub created_at: i64,
}

/// `GET /agent/v1/work-items/{owner}/{repo}/{number}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentWorkItem {
    pub owner: String,
    pub repo: String,
    pub number: i64,
    /// Referência pronta (`ct_core::item_ref`): `owner/repo#n` ou `PROJ-n`.
    #[serde(rename = "ref", default)]
    pub item_ref: String,
    pub title: Option<String>,
    pub state: Option<String>,
    pub kind: WorkItemKind,
    pub status: WorkItemStatus,
    pub priority: Option<Priority>,
    pub rank: Option<i64>,
    pub branch: Option<String>,
    pub labels: Vec<String>,
    pub parent: Option<AgentItemRef>,
    pub children: Vec<AgentItemRef>,
    pub sessions: Vec<AgentSession>,
    pub prs: Vec<AgentPr>,
    /// Do mais recente para o mais antigo, até `events`.
    pub events: Vec<AgentEvent>,
    pub decisions: Vec<AgentDecision>,
    /// Propostas do item (todas, da mais nova para a mais velha; até 20).
    pub proposals: Vec<super::HubProposal>,
}

/// Filtros de `GET /agent/v1/decisions`.
#[derive(Debug, Clone, Default)]
pub struct AgentDecisionFilter {
    pub project: Option<(String, String)>,
    /// Com `project`: o work item de partida do `scope`.
    pub number: Option<i64>,
    pub scope: Option<super::DecisionScope>,
    pub origin: Option<DecisionOrigin>,
    pub unreviewed: bool,
    /// `created_at` mínimo (ms).
    pub since: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentDecisions {
    /// Da mais nova para a mais velha.
    pub decisions: Vec<AgentDecision>,
    pub truncated: bool,
}

fn clamp(limit: u32) -> usize {
    limit.clamp(1, RANKING_MAX) as usize
}

impl Hub {
    /// `GET /agent/v1/overview`.
    pub fn agent_overview(
        &self,
        t: &HealthThresholds,
        gh_interval: u64,
        now: i64,
    ) -> Result<AgentOverview> {
        let health = self.health(t, gh_interval, &HealthFilter::default(), now)?;
        let mut projects = Vec::new();
        for p in self.list_projects()? {
            let rows: Vec<(String, Option<String>)> = {
                let mut stmt = self.conn.prepare(
                    "SELECT coalesce(w.status, 'backlog'), w.priority FROM issue_cache i
                     LEFT JOIN work_item w ON w.project_id = i.project_id AND w.gh_number = i.gh_number
                     WHERE i.project_id = ?1 AND i.state = 'open'
                     UNION ALL
                     SELECT w.status, w.priority FROM work_item w
                     WHERE w.project_id = ?1 AND w.gh_number > 0
                       AND NOT EXISTS (SELECT 1 FROM issue_cache i
                                       WHERE i.project_id = w.project_id AND i.gh_number = w.gh_number)",
                )?;
                let rows = stmt.query_map([p.id], |r| Ok((r.get(0)?, r.get(1)?)))?;
                rows.collect::<rusqlite::Result<_>>()?
            };
            let mut by_status = BTreeMap::new();
            let mut by_priority = BTreeMap::new();
            for (status, priority) in rows {
                if status != WorkItemStatus::Concluido.as_str() {
                    *by_priority
                        .entry(priority.unwrap_or_else(|| "nao_triado".into()))
                        .or_insert(0) += 1;
                }
                *by_status.entry(status).or_insert(0) += 1;
            }
            let open_prs: i64 = self.conn.query_row(
                "SELECT count(*) FROM pr_cache WHERE project_id = ?1 AND state = 'open'",
                [p.id],
                |r| r.get(0),
            )?;
            let pending: i64 = self.conn.query_row(
                "SELECT count(*) FROM decision d JOIN work_item w ON w.id = d.work_item_id
                 WHERE w.project_id = ?1 AND d.origin = 'autonoma' AND d.reviewed_at IS NULL",
                [p.id],
                |r| r.get(0),
            )?;
            let mut signals = HealthCounts::default();
            for s in health.signals.iter().filter(|s| {
                s.owner.as_deref() == Some(p.gh_owner.as_str())
                    && s.repo.as_deref() == Some(p.gh_repo.as_str())
            }) {
                match s.severity {
                    Severity::Critico => signals.critico += 1,
                    Severity::Aviso => signals.aviso += 1,
                    Severity::Info => signals.info += 1,
                }
            }
            let assignments = self
                .project_assignments(p.id)?
                .into_iter()
                .map(|a| AgentAssignment {
                    env: a.env_name,
                    state: a.state,
                })
                .collect();
            let tracker = p.tracker.kind();
            let issue_view = ct_core::issue_view_cmd(&p.gh_owner, &p.gh_repo, &p.tracker);
            projects.push(AgentProjectSummary {
                tracker,
                issue_view,
                owner: p.gh_owner,
                repo: p.gh_repo,
                name: p.name,
                rank: p.rank,
                by_status,
                by_priority,
                open_prs: open_prs as usize,
                pending_decisions: pending as usize,
                signals,
                assignments,
            });
        }
        let envs = self
            .list_envs()?
            .into_iter()
            .map(|e| AgentEnv {
                name: e.name,
                last_seen_at: e.last_seen_at,
                pending: e.pending,
            })
            .collect();
        Ok(AgentOverview {
            projects,
            envs,
            signals: health.counts,
        })
    }

    /// `GET /agent/v1/projects/{owner}/{repo}`. `None` se o projeto não
    /// está no hub. Sem `include_closed`, issues fechadas e itens
    /// concluídos ficam de fora (com as sub-issues deles).
    pub fn agent_project(
        &self,
        owner: &str,
        repo: &str,
        include_closed: bool,
        limit: u32,
    ) -> Result<Option<AgentProject>> {
        let Some(tree) = self.tree(owner, repo)? else {
            return Ok(None);
        };
        let mut left = clamp(limit);
        let mut truncated = false;
        fn keep(include_closed: bool, state: Option<&str>, status: WorkItemStatus) -> bool {
            include_closed || (state != Some("closed") && status != WorkItemStatus::Concluido)
        }
        fn node(
            t: &TrackerRef,
            i: &IssueRow,
            w: Option<&super::HubWorkItem>,
            s: &super::WorkItemSummary,
        ) -> AgentNode {
            AgentNode {
                number: i.gh_number,
                item_ref: ct_core::item_ref(t, i.gh_number),
                title: Some(i.title.clone()),
                state: Some(i.state.clone()),
                kind: w.map(|w| w.kind).unwrap_or(if i.has_sub_issues {
                    WorkItemKind::Epic
                } else {
                    WorkItemKind::Issue
                }),
                status: w.map(|w| w.status).unwrap_or(WorkItemStatus::Backlog),
                priority: w.and_then(|w| w.priority),
                rank: w.and_then(|w| w.rank),
                labels: i.labels.clone(),
                pr: s.pr.as_ref().map(|p| AgentPr {
                    number: p.number,
                    state: p.state,
                    updated_at: p.updated_at,
                }),
                last_activity_at: s.last_activity_at,
                live_envs: s.live_envs.clone(),
                pending_decisions: s.decisions.pending_autonomous,
                children: Vec::new(),
            }
        }
        fn walk(
            t: &TrackerRef,
            n: &super::HubTreeNode,
            include_closed: bool,
            left: &mut usize,
            truncated: &mut bool,
        ) -> Option<AgentNode> {
            let status = n
                .work_item
                .as_ref()
                .map(|w| w.status)
                .unwrap_or(WorkItemStatus::Backlog);
            if !keep(include_closed, Some(&n.issue.state), status) {
                return None;
            }
            if *left == 0 {
                *truncated = true;
                return None;
            }
            *left -= 1;
            let mut out = node(t, &n.issue, n.work_item.as_ref(), &n.summary);
            out.children = n
                .children
                .iter()
                .filter_map(|c| walk(t, c, include_closed, left, truncated))
                .collect();
            Some(out)
        }
        let t = tree.project.tracker_ref();
        let mut items: Vec<AgentNode> = tree
            .roots
            .iter()
            .filter_map(|n| walk(&t, n, include_closed, &mut left, &mut truncated))
            .collect();
        for u in &tree.untracked {
            let w = &u.work_item;
            if w.gh_number <= 0 || !keep(include_closed, None, w.status) {
                continue;
            }
            if left == 0 {
                truncated = true;
                break;
            }
            left -= 1;
            items.push(AgentNode {
                number: w.gh_number,
                item_ref: ct_core::item_ref(&t, w.gh_number),
                title: None,
                state: None,
                kind: w.kind,
                status: w.status,
                priority: w.priority,
                rank: w.rank,
                labels: vec![],
                pr: u.summary.pr.as_ref().map(|p| AgentPr {
                    number: p.number,
                    state: p.state,
                    updated_at: p.updated_at,
                }),
                last_activity_at: u.summary.last_activity_at,
                live_envs: u.summary.live_envs.clone(),
                pending_decisions: u.summary.decisions.pending_autonomous,
                children: vec![],
            });
        }
        Ok(Some(AgentProject {
            tracker: tree.project.tracker.kind(),
            issue_view: ct_core::issue_view_cmd(
                &tree.project.gh_owner,
                &tree.project.gh_repo,
                &tree.project.tracker,
            ),
            owner: tree.project.gh_owner.clone(),
            repo: tree.project.gh_repo.clone(),
            name: tree.project.name.clone(),
            items,
            truncated,
        }))
    }

    fn agent_item_ref(&self, project: &HubProject, number: i64) -> Result<AgentItemRef> {
        let pid = project.id;
        let issue = self.get_issue(pid, number)?;
        let wi = self.conn.query_row(
            "SELECT status, priority FROM work_item WHERE project_id = ?1 AND gh_number = ?2",
            params![pid, number],
            |r| Ok((crate::parse_col(r, 0)?, crate::opt_parse_col(r, 1)?)),
        );
        let (status, priority) = match wi {
            Ok(v) => v,
            Err(rusqlite::Error::QueryReturnedNoRows) => (WorkItemStatus::Backlog, None),
            Err(e) => return Err(e.into()),
        };
        Ok(AgentItemRef {
            number,
            item_ref: project.item_ref(number),
            title: issue.map(|i| i.title),
            status,
            priority,
        })
    }

    /// `GET /agent/v1/work-items/{owner}/{repo}/{number}`. Issue do
    /// espelho sem work item vem como `backlog`, sem sessões. `None` se nem
    /// o projeto nem a issue estão no hub.
    pub fn agent_work_item(
        &self,
        owner: &str,
        repo: &str,
        number: i64,
        events: u32,
    ) -> Result<Option<AgentWorkItem>> {
        let Some(project) = self.find_project(owner, repo)? else {
            return Ok(None);
        };
        let issue = self.get_issue(project.id, number)?;
        let wi = self.find_work_item(owner, repo, number)?;
        if issue.is_none() && wi.is_none() {
            return Ok(None);
        }
        let names = self.env_names()?;
        let env = |id: &str| names.get(id).cloned().unwrap_or_else(|| id.to_string());
        let parent = match issue.as_ref().and_then(|i| i.parent_number) {
            Some(p) => Some(self.agent_item_ref(&project, p)?),
            None => None,
        };
        let children = self
            .list_issues(project.id)?
            .into_iter()
            .filter(|i| i.parent_number == Some(number))
            .map(|i| self.agent_item_ref(&project, i.gh_number))
            .collect::<Result<_>>()?;
        let (sessions, prs, evs, decisions) = match &wi {
            None => (vec![], vec![], vec![], vec![]),
            Some(w) => {
                let sessions = self
                    .list_sessions(w.id)?
                    .into_iter()
                    .map(|s| AgentSession {
                        env: env(&s.env_id),
                        id: s.id,
                        state: s.state,
                        exit_code: s.exit_code,
                        started_at: s.started_at,
                        last_event_at: s.last_event_at,
                        ended_at: s.ended_at,
                    })
                    .collect();
                let prs = self
                    .prs_for_work_item(w.id)?
                    .into_iter()
                    .map(|p| AgentPr {
                        number: p.number,
                        state: p.state,
                        updated_at: p.updated_at,
                    })
                    .collect();
                let mut stmt = self.conn.prepare(&format!(
                    "SELECT {EVENT_COLS} FROM event WHERE work_item_id = ?1
                     ORDER BY hlc DESC, env_id DESC LIMIT ?2"
                ))?;
                let evs = stmt
                    .query_map(params![w.id, events.min(RANKING_MAX)], row_to_event)?
                    .map(|e| {
                        e.map(|e| AgentEvent {
                            env: env(&e.env_id),
                            kind: e.kind,
                            ts: e.ts,
                            payload: e.payload,
                        })
                    })
                    .collect::<rusqlite::Result<_>>()?;
                let decisions = self
                    .list_decisions(&[w.id], false)?
                    .into_iter()
                    .map(|d| AgentDecision {
                        owner: project.gh_owner.clone(),
                        repo: project.gh_repo.clone(),
                        number,
                        item_ref: project.item_ref(number),
                        origin: d.origin,
                        title: d.title,
                        decision: d.decision,
                        reviewed_at: d.reviewed_at,
                        created_at: d.created_at,
                    })
                    .collect();
                (sessions, prs, evs, decisions)
            }
        };
        Ok(Some(AgentWorkItem {
            owner: project.gh_owner.clone(),
            repo: project.gh_repo.clone(),
            number,
            item_ref: project.item_ref(number),
            title: issue.as_ref().map(|i| i.title.clone()),
            state: issue.as_ref().map(|i| i.state.clone()),
            kind: wi.as_ref().map(|w| w.kind).unwrap_or(
                if issue.as_ref().is_some_and(|i| i.has_sub_issues) {
                    WorkItemKind::Epic
                } else {
                    WorkItemKind::Issue
                },
            ),
            status: wi
                .as_ref()
                .map(|w| w.status)
                .unwrap_or(WorkItemStatus::Backlog),
            priority: wi.as_ref().and_then(|w| w.priority),
            rank: wi.as_ref().and_then(|w| w.rank),
            branch: wi.as_ref().and_then(|w| w.branch.clone()),
            labels: issue.map(|i| i.labels).unwrap_or_default(),
            parent,
            children,
            sessions,
            prs,
            events: evs,
            decisions,
            proposals: self
                .list_proposals(
                    &super::ProposalFilter {
                        state: None,
                        project: Some((owner.to_string(), repo.to_string())),
                        number: Some(number),
                    },
                    20,
                    ct_core::now_ms(),
                )?
                .proposals,
        }))
    }

    /// `GET /agent/v1/decisions`. Com `project` e `number`, o `scope` (padrão
    /// `work_item`) parte desse work item; só com `project`, o projeto
    /// inteiro; sem nada, todos os projetos. `None` se o projeto ou o work
    /// item pedido não está no hub.
    pub fn agent_decisions(
        &self,
        f: &AgentDecisionFilter,
        limit: u32,
    ) -> Result<Option<AgentDecisions>> {
        let ids: Option<Vec<i64>> = match (&f.project, f.number) {
            (Some((o, r)), Some(n)) => {
                let scope = f.scope.unwrap_or(super::DecisionScope::WorkItem);
                let Some(wi) = self.find_work_item(o, r, n)? else {
                    return Ok(None);
                };
                Some(match scope {
                    super::DecisionScope::WorkItem => vec![wi.id],
                    super::DecisionScope::Epic => {
                        self.epic_work_items(wi.id)?.iter().map(|w| w.id).collect()
                    }
                    super::DecisionScope::Project => self
                        .list_work_items(wi.project_id)?
                        .iter()
                        .map(|w| w.id)
                        .collect(),
                })
            }
            (Some((o, r)), None) => {
                let Some(p) = self.find_project(o, r)? else {
                    return Ok(None);
                };
                Some(self.list_work_items(p.id)?.iter().map(|w| w.id).collect())
            }
            (None, _) => None,
        };
        let id_filter = match &ids {
            Some(ids) if ids.is_empty() => "AND 0".to_string(),
            Some(ids) => format!(
                "AND d.work_item_id IN ({})",
                ids.iter()
                    .map(|i| i.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            None => String::new(),
        };
        let limit = clamp(limit);
        let mut stmt = self.conn.prepare(&format!(
            "SELECT p.gh_owner, p.gh_repo, w.gh_number, d.origin, d.title, d.decision,
                    d.reviewed_at, d.created_at, p.tracker_kind, p.tracker_config
             FROM decision d
             JOIN work_item w ON w.id = d.work_item_id
             JOIN project p ON p.id = w.project_id AND p.deleted_at IS NULL
             WHERE (?1 IS NULL OR d.origin = ?1) AND (?2 = 0 OR d.reviewed_at IS NULL)
               AND (?3 IS NULL OR d.created_at >= ?3) {id_filter}
             ORDER BY d.created_at DESC, d.uid DESC
             LIMIT ?4"
        ))?;
        let rows = stmt.query_map(
            params![
                f.origin.map(DecisionOrigin::as_str),
                f.unreviewed,
                f.since,
                limit as i64 + 1
            ],
            |r| {
                let (owner, repo, number): (String, String, i64) =
                    (r.get(0)?, r.get(1)?, r.get(2)?);
                let tracker = crate::tracker_col(r, 8)?;
                Ok(AgentDecision {
                    item_ref: ct_core::item_ref(&TrackerRef::new(&owner, &repo, &tracker), number),
                    owner,
                    repo,
                    number,
                    origin: crate::parse_col(r, 3)?,
                    title: r.get(4)?,
                    decision: r.get(5)?,
                    reviewed_at: r.get(6)?,
                    created_at: r.get(7)?,
                })
            },
        )?;
        let mut decisions: Vec<AgentDecision> = rows.collect::<rusqlite::Result<_>>()?;
        let truncated = decisions.len() > limit;
        decisions.truncate(limit);
        Ok(Some(AgentDecisions {
            decisions,
            truncated,
        }))
    }
}
