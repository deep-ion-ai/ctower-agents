//! Leituras da visão consolidada para os métodos `hub.*` (spec do ledger
//! centralizado §7; formatos alinhados com #46 e #47 do épico #51).

use super::{
    row_to_event, row_to_session, Hub, HubAssignment, HubDecision, HubEnv, HubEvent, HubProject,
    HubSession, HubWorkItem, EVENT_COLS, SESSION_COLS,
};
use crate::sync::project_key;
use crate::{IssueRow, PrRow, Result, SyncState};
use ct_core::{now_ms, DecisionOrigin};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};

/// Onde um projeto (ou work item) está clonado, por ambiente.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct HubLocation {
    pub env_id: String,
    pub env_name: String,
    pub path: Option<String>,
}

/// Item de `hub.projects.list`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct HubProjectView {
    pub project: HubProject,
    /// Atribuições ativas, com o estado (pendente, aplicada, com erro).
    pub assignments: Vec<HubAssignment>,
    /// Onde os ambientes relataram o clone (o relato, não a intenção).
    pub locations: Vec<HubLocation>,
    /// Último sync: o do GitHub pelo hub ou, no projeto Jira, o que o
    /// ambiente de sync relatou (`tracker_status`).
    pub sync: Option<SyncState>,
    /// Projeto Jira: último `tracker_status` aceito, com a prontidão do
    /// ambiente de sync (#348). Nulo no GitHub e até o primeiro relato.
    #[serde(default)]
    pub tracker_status: Option<super::HubTrackerStatus>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct DecisionCounts {
    pub total: usize,
    /// Autônomas ainda não revisadas.
    pub pending_autonomous: usize,
}

/// Resumo do work item na árvore (#46).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct WorkItemSummary {
    /// PR mais recente da branch do work item.
    pub pr: Option<PrRow>,
    pub decisions: DecisionCounts,
    /// `ts` do último evento do work item, sem os de prioridade e ordem.
    pub last_activity_at: Option<i64>,
    /// Ambientes com sessão viva neste work item.
    pub live_envs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct HubTreeNode {
    pub issue: IssueRow,
    pub work_item: Option<HubWorkItem>,
    pub summary: WorkItemSummary,
    pub children: Vec<HubTreeNode>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct HubTreeItem {
    pub work_item: HubWorkItem,
    pub summary: WorkItemSummary,
}

/// Resposta de `hub.tree.get`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct HubTree {
    pub project: HubProject,
    pub sync: Option<SyncState>,
    /// Issues sem pai no espelho (épicos e avulsas), por número.
    pub roots: Vec<HubTreeNode>,
    /// Work items cuja issue ainda não está no espelho.
    pub untracked: Vec<HubTreeItem>,
}

/// Resposta de `hub.workitem.history` (#47).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct HubHistory {
    pub work_item: HubWorkItem,
    pub locations: Vec<HubLocation>,
    pub sessions: Vec<HubSession>,
    /// Do mais recente para o mais antigo (HLC); página de `limit`.
    pub events: Vec<HubEvent>,
    /// HLC para pedir a página seguinte (`before_hlc`), se houver.
    pub next_before_hlc: Option<i64>,
    /// Do work item e, com `include_sub_issues`, também das sub-issues
    /// (`work_item_id` diz de qual).
    pub decisions: Vec<HubDecision>,
    pub prs: Vec<PrRow>,
    /// Work items das sub-issues incluídas em `decisions` e `prs` (vazio sem
    /// `include_sub_issues` ou fora de épico).
    pub sub_work_items: Vec<HubWorkItem>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct HubDashboardWorkItem {
    #[serde(flatten)]
    pub work_item: HubWorkItem,
    pub title: Option<String>,
    /// Autônomas não revisadas; no épico, soma as das sub-issues.
    pub pending_decisions: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct HubDashboardProject {
    pub project: HubProject,
    pub work_items: Vec<HubDashboardWorkItem>,
}

/// Resposta de `hub.dashboard.get`: todos os ambientes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct HubDashboard {
    pub projects: Vec<HubDashboardProject>,
    pub live_sessions: Vec<HubSession>,
    pub interrupted_sessions: Vec<HubSession>,
    /// Saíram com código ≠ 0 nas últimas 24 h.
    pub failed_sessions: Vec<HubSession>,
    pub sync: Vec<SyncState>,
    pub envs: Vec<HubEnv>,
    /// Work item das sessões do gestor (`_ctower/gestor#0`), se já houve
    /// uma: `live_sessions` com ele são do gestor.
    pub gestor_work_item_id: Option<i64>,
}

/// Escopo de `ledger_list_decisions` respondido pelo hub.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionScope {
    WorkItem,
    Epic,
    Project,
}

const FAILED_WINDOW_MS: i64 = 24 * 3600 * 1000;

impl Hub {
    fn project_id(&self, owner: &str, repo: &str) -> Result<Option<i64>> {
        Ok(self
            .conn
            .query_row(
                "SELECT id FROM project WHERE key = ?1 AND deleted_at IS NULL AND kind = 'github'",
                [project_key(owner, repo)],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// Nome de cada ambiente que já enviou ao hub, por `env_id`.
    pub fn env_names(&self) -> Result<HashMap<String, String>> {
        Ok(self
            .list_envs()?
            .into_iter()
            .map(|e| (e.env_id, e.name))
            .collect())
    }

    pub fn project_views(&self) -> Result<Vec<HubProjectView>> {
        let names = self.env_names()?;
        let mut out = Vec::new();
        for project in self.list_projects()? {
            let locations = self
                .project_locations(project.id)?
                .into_iter()
                .map(|(env_id, path)| HubLocation {
                    env_name: names
                        .get(&env_id)
                        .cloned()
                        .unwrap_or_else(|| env_id.clone()),
                    env_id,
                    path: Some(path),
                })
                .collect();
            out.push(HubProjectView {
                sync: self.sync_state(project.id)?,
                tracker_status: self.tracker_status(project.id)?,
                assignments: self.project_assignments(project.id)?,
                project,
                locations,
            });
        }
        Ok(out)
    }

    /// O work item e os das sub-issues (pelo espelho do GitHub do hub).
    pub fn epic_work_items(&self, id: i64) -> Result<Vec<HubWorkItem>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {} FROM work_item w WHERE w.id = ?1
                OR (w.project_id = (SELECT project_id FROM work_item WHERE id = ?1)
                    AND w.gh_number IN (
                        SELECT gh_number FROM issue_cache
                        WHERE project_id = (SELECT project_id FROM work_item WHERE id = ?1)
                          AND parent_number = (SELECT gh_number FROM work_item WHERE id = ?1)))
             ORDER BY gh_number",
            super::WORK_ITEM_COLS
                .split(", ")
                .map(|c| format!("w.{c}"))
                .collect::<Vec<_>>()
                .join(", ")
        ))?;
        let rows = stmt.query_map([id], super::row_to_work_item)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Decisões do escopo, a partir do work item `owner/repo#number`.
    /// `None` se o work item ainda não chegou ao hub.
    pub fn scoped_decisions(
        &self,
        owner: &str,
        repo: &str,
        number: i64,
        scope: DecisionScope,
    ) -> Result<Option<Vec<HubDecision>>> {
        let Some(wi) = self.find_work_item(owner, repo, number)? else {
            return Ok(None);
        };
        let ids: Vec<i64> = match scope {
            DecisionScope::WorkItem => vec![wi.id],
            DecisionScope::Epic => self.epic_work_items(wi.id)?.iter().map(|w| w.id).collect(),
            DecisionScope::Project => self
                .list_work_items(wi.project_id)?
                .iter()
                .map(|w| w.id)
                .collect(),
        };
        self.list_decisions(&ids, false).map(Some)
    }

    fn pending_autonomous(&self, ids: &[i64]) -> Result<usize> {
        Ok(self
            .list_decisions(ids, true)?
            .iter()
            .filter(|d| d.origin == DecisionOrigin::Autonoma)
            .count())
    }

    fn summary(&self, wi: &HubWorkItem) -> Result<WorkItemSummary> {
        let decisions = self.list_decisions(&[wi.id], false)?;
        let pending_autonomous = decisions
            .iter()
            .filter(|d| d.origin == DecisionOrigin::Autonoma && d.reviewed_at.is_none())
            .count();
        // Triar não é trabalhar no item: os eventos de prioridade e ordem
        // (#215) não contam como atividade.
        let last_activity_at: Option<i64> = self.conn.query_row(
            "SELECT max(ts) FROM event WHERE work_item_id = ?1
               AND type NOT IN ('priority_set', 'rank_moved')",
            [wi.id],
            |r| r.get(0),
        )?;
        let mut live_envs: Vec<String> = {
            let mut stmt = self.conn.prepare(
                "SELECT DISTINCT coalesce(e.name, s.env_id) FROM session s
                 LEFT JOIN env e ON e.env_id = s.env_id
                 WHERE s.work_item_id = ?1 AND s.state IN ('starting', 'running', 'idle')",
            )?;
            let rows = stmt.query_map([wi.id], |r| r.get(0))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        live_envs.sort();
        Ok(WorkItemSummary {
            pr: self.prs_for_work_item(wi.id)?.into_iter().next(),
            decisions: DecisionCounts {
                total: decisions.len(),
                pending_autonomous,
            },
            last_activity_at,
            live_envs,
        })
    }

    /// Árvore do projeto: épicos → sub-issues, cada uma com o work item
    /// consolidado e o resumo. `None` se o projeto não está no hub.
    pub fn tree(&self, owner: &str, repo: &str) -> Result<Option<HubTree>> {
        let Some(pid) = self.project_id(owner, repo)? else {
            return Ok(None);
        };
        let project = self
            .list_projects()?
            .into_iter()
            .find(|p| p.id == pid)
            .expect("projeto existe");
        let issues = self.list_issues(pid)?;
        let mut work_items: HashMap<i64, HubWorkItem> = self
            .list_work_items(pid)?
            .into_iter()
            .map(|w| (w.gh_number, w))
            .collect();
        let known: HashSet<i64> = issues.iter().map(|i| i.gh_number).collect();
        let mut children: BTreeMap<i64, Vec<IssueRow>> = BTreeMap::new();
        let mut roots = Vec::new();
        for i in issues {
            match i.parent_number.filter(|p| known.contains(p)) {
                Some(p) => children.entry(p).or_default().push(i),
                None => roots.push(i),
            }
        }
        fn build(
            hub: &Hub,
            i: IssueRow,
            children: &mut BTreeMap<i64, Vec<IssueRow>>,
            work_items: &mut HashMap<i64, HubWorkItem>,
        ) -> Result<HubTreeNode> {
            let kids = children.remove(&i.gh_number).unwrap_or_default();
            let work_item = work_items.remove(&i.gh_number);
            let summary = match &work_item {
                Some(w) => hub.summary(w)?,
                None => WorkItemSummary::default(),
            };
            Ok(HubTreeNode {
                work_item,
                summary,
                children: kids
                    .into_iter()
                    .map(|c| build(hub, c, children, work_items))
                    .collect::<Result<_>>()?,
                issue: i,
            })
        }
        let roots = roots
            .into_iter()
            .map(|i| build(self, i, &mut children, &mut work_items))
            .collect::<Result<_>>()?;
        let mut rest: Vec<HubWorkItem> = work_items.into_values().collect();
        rest.sort_by_key(|w| w.gh_number);
        let untracked = rest
            .into_iter()
            .map(|w| {
                Ok(HubTreeItem {
                    summary: self.summary(&w)?,
                    work_item: w,
                })
            })
            .collect::<Result<_>>()?;
        Ok(Some(HubTree {
            sync: self.sync_state(pid)?,
            project,
            roots,
            untracked,
        }))
    }

    /// Histórico consolidado do work item. `None` se não está no hub.
    pub fn history(
        &self,
        owner: &str,
        repo: &str,
        number: i64,
        limit: u32,
        before_hlc: Option<i64>,
        include_sub_issues: bool,
    ) -> Result<Option<HubHistory>> {
        let Some(work_item) = self.find_work_item(owner, repo, number)? else {
            return Ok(None);
        };
        let names = self.env_names()?;
        let locations = self
            .work_item_locations(work_item.id)?
            .into_iter()
            .map(|(env_id, path)| HubLocation {
                env_name: names
                    .get(&env_id)
                    .cloned()
                    .unwrap_or_else(|| env_id.clone()),
                env_id,
                path,
            })
            .collect();
        let limit = limit.clamp(1, 1000);
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {EVENT_COLS} FROM event WHERE work_item_id = ?1 AND hlc < ?2
             ORDER BY hlc DESC, env_id DESC LIMIT ?3"
        ))?;
        let events: Vec<HubEvent> = stmt
            .query_map(
                params![work_item.id, before_hlc.unwrap_or(i64::MAX), limit + 1],
                row_to_event,
            )?
            .collect::<rusqlite::Result<_>>()?;
        let (events, next_before_hlc) = if events.len() > limit as usize {
            let page: Vec<HubEvent> = events.into_iter().take(limit as usize).collect();
            let next = page.last().map(|e| e.hlc);
            (page, next)
        } else {
            (events, None)
        };
        // Eventos e sessões ficam só do work item; decisões e PRs do épico
        // podem trazer os das sub-issues (Revisão, #328).
        let sub_work_items: Vec<HubWorkItem> = if include_sub_issues {
            self.epic_work_items(work_item.id)?
                .into_iter()
                .filter(|w| w.id != work_item.id)
                .collect()
        } else {
            Vec::new()
        };
        let ids: Vec<i64> = std::iter::once(work_item.id)
            .chain(sub_work_items.iter().map(|w| w.id))
            .collect();
        let mut prs = Vec::new();
        for id in &ids {
            prs.extend(self.prs_for_work_item(*id)?);
        }
        Ok(Some(HubHistory {
            locations,
            sessions: self.list_sessions(work_item.id)?,
            events,
            next_before_hlc,
            decisions: self.list_decisions(&ids, false)?,
            prs,
            work_item,
            sub_work_items,
        }))
    }

    fn sessions_where(
        &self,
        filter: &str,
        args: &[&dyn rusqlite::ToSql],
    ) -> Result<Vec<HubSession>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {SESSION_COLS} FROM session WHERE {filter} ORDER BY started_at"
        ))?;
        let rows = stmt.query_map(args, row_to_session)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn dashboard(&self) -> Result<HubDashboard> {
        let mut projects = Vec::new();
        for project in self.list_projects()? {
            let titles: HashMap<i64, String> = self
                .list_issues(project.id)?
                .into_iter()
                .map(|i| (i.gh_number, i.title))
                .collect();
            let mut work_items = Vec::new();
            for wi in self.list_work_items(project.id)? {
                let ids: Vec<i64> = self.epic_work_items(wi.id)?.iter().map(|w| w.id).collect();
                work_items.push(HubDashboardWorkItem {
                    title: titles.get(&wi.gh_number).cloned(),
                    pending_decisions: self.pending_autonomous(&ids)?,
                    work_item: wi,
                });
            }
            projects.push(HubDashboardProject {
                project,
                work_items,
            });
        }
        Ok(HubDashboard {
            projects,
            live_sessions: self.sessions_where("state IN ('starting', 'running', 'idle')", &[])?,
            interrupted_sessions: self.sessions_where("state = 'interrupted'", &[])?,
            failed_sessions: self.sessions_where(
                "state = 'exited' AND exit_code IS NOT NULL AND exit_code <> 0 AND ended_at >= ?1",
                &[&(now_ms() - FAILED_WINDOW_MS)],
            )?,
            sync: self.sync_states()?,
            envs: self.list_envs()?,
            gestor_work_item_id: self
                .conn
                .query_row(
                    "SELECT w.id FROM work_item w JOIN project p ON p.id = w.project_id
                     WHERE p.kind = 'gestor' AND w.gh_number = 0",
                    [],
                    |r| r.get(0),
                )
                .optional()?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{GithubSnapshot, Ledger, NewDecision, NewProject, NewSession};
    use ct_core::{PrState, Tool, WorkItemKind};

    fn ledger(path: &str) -> Ledger {
        let l = Ledger::open_in_memory().unwrap();
        l.add_project(&NewProject {
            name: "demo".into(),
            gh_owner: "deep-ion-ai".into(),
            gh_repo: "demo".into(),
            repo_path: path.into(),
            base_branch: "develop".into(),
            protected_branches: vec![],
        })
        .unwrap();
        l
    }

    fn push(hub: &Hub, l: &Ledger, name: &str) {
        let b = l.outbox_pending(10_000).unwrap();
        hub.ingest(l.env_id(), name, &b).unwrap();
        l.outbox_ack(i64::MAX).unwrap();
    }

    fn issue(n: i64, parent: Option<i64>, subs: bool) -> IssueRow {
        IssueRow {
            gh_number: n,
            title: format!("Issue {n}"),
            state: "open".into(),
            labels: vec![],
            parent_number: parent,
            has_sub_issues: subs,
            url: "u".into(),
            updated_at: 1,
            ..Default::default()
        }
    }

    fn decide(l: &Ledger, wi: i64, origin: DecisionOrigin) -> crate::Decision {
        l.record_decision(&NewDecision {
            work_item_id: wi,
            session_id: None,
            origin,
            title: "t".into(),
            context: "c".into(),
            decision: "d".into(),
            consequences: None,
        })
        .unwrap()
    }

    /// Épico 10 com a filha 11 em `a` e a filha 12 em `b`.
    fn cenario() -> (Hub, Ledger, Ledger) {
        let hub = Hub::open_in_memory().unwrap();
        hub.add_project(&crate::NewHubProject {
            owner: "deep-ion-ai".into(),
            repo: "demo".into(),
            name: "demo".into(),
            base_branch: "develop".into(),
            protected_branches: vec![],
        })
        .unwrap();
        let a = ledger("/a/demo");
        let b = ledger("/b/demo");
        let e = a.ensure_work_item(1, 10, WorkItemKind::Epic).unwrap();
        let f11 = a.ensure_work_item(1, 11, WorkItemKind::Issue).unwrap();
        a.set_work_item_branch(f11.id, "feat/11", "/a/wt/11")
            .unwrap();
        decide(&a, e.id, DecisionOrigin::Orientada);
        decide(&a, f11.id, DecisionOrigin::Autonoma);
        let f12 = b.ensure_work_item(1, 12, WorkItemKind::Issue).unwrap();
        decide(&b, f12.id, DecisionOrigin::Autonoma);
        b.create_session(&NewSession {
            work_item_id: f12.id,
            tool: Tool::Claude,
            cwd: "/b/wt/12".into(),
            launch_cmd: vec![],
            model: None,
            mode: ct_core::SessionMode::Native,
        })
        .unwrap();
        a.ensure_work_item(1, 99, WorkItemKind::Issue).unwrap();
        push(&hub, &a, "a");
        push(&hub, &b, "b");
        let pid = hub.find_project("deep-ion-ai", "demo").unwrap().unwrap().id;
        let w11 = hub
            .find_work_item("deep-ion-ai", "demo", 11)
            .unwrap()
            .unwrap()
            .id;
        hub.apply_github(
            &GithubSnapshot {
                project_id: pid,
                issues: vec![
                    issue(10, None, true),
                    issue(11, Some(10), false),
                    issue(12, Some(10), false),
                ],
                prs: vec![PrRow {
                    number: 50,
                    work_item_id: w11,
                    head: "feat/11".into(),
                    base: "develop".into(),
                    state: PrState::Open,
                    url: "u".into(),
                    updated_at: 1,
                    head_sha: None,
                }],
                etags: vec![],
                synced_at: 1,
            },
            "hub",
        )
        .unwrap();
        (hub, a, b)
    }

    #[test]
    fn arvore_consolidada_com_resumo() {
        let (hub, _a, _b) = cenario();
        let t = hub.tree("Deep-Ion-AI", "demo").unwrap().unwrap();
        assert_eq!(t.roots.len(), 1);
        let epico = &t.roots[0];
        assert_eq!(epico.issue.gh_number, 10);
        assert_eq!(
            epico.summary.decisions,
            DecisionCounts {
                total: 1,
                pending_autonomous: 0
            }
        );
        let filhas: Vec<_> = epico.children.iter().map(|c| c.issue.gh_number).collect();
        assert_eq!(filhas, vec![11, 12]);
        let f11 = &epico.children[0].summary;
        assert_eq!(f11.pr.as_ref().unwrap().number, 50);
        assert_eq!(f11.decisions.pending_autonomous, 1);
        assert!(f11.last_activity_at.is_some());
        let f12 = &epico.children[1].summary;
        assert_eq!(f12.live_envs, vec!["b"]);
        assert_eq!(t.untracked.len(), 1);
        assert_eq!(t.untracked[0].work_item.gh_number, 99);
        assert!(hub.tree("x", "y").unwrap().is_none());
    }

    #[test]
    fn historico_paginado_por_hlc() {
        let (hub, _a, _b) = cenario();
        let h = hub
            .history("deep-ion-ai", "demo", 11, 2, None, false)
            .unwrap()
            .unwrap();
        assert_eq!(h.events.len(), 2);
        assert!(h.events[0].hlc > h.events[1].hlc);
        assert_eq!(h.locations[0].env_name, "a");
        assert_eq!(h.locations[0].path.as_deref(), Some("/a/wt/11"));
        assert_eq!(h.prs.len(), 1);
        assert_eq!(h.decisions.len(), 1);
        let mut todos = h.events.clone();
        let mut next = h.next_before_hlc;
        while let Some(b) = next {
            let p = hub
                .history("deep-ion-ai", "demo", 11, 2, Some(b), false)
                .unwrap()
                .unwrap();
            todos.extend(p.events);
            next = p.next_before_hlc;
        }
        let total: i64 = hub
            .conn
            .query_row(
                "SELECT count(*) FROM event WHERE work_item_id = ?1",
                [h.work_item.id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(todos.len() as i64, total);
        assert!(todos.windows(2).all(|w| w[0].hlc >= w[1].hlc));
        assert!(hub
            .history("deep-ion-ai", "demo", 777, 10, None, false)
            .unwrap()
            .is_none());
    }

    #[test]
    fn historico_do_epico_com_sub_issues() {
        let (hub, _a, _b) = cenario();
        let so = hub
            .history("deep-ion-ai", "demo", 10, 50, None, false)
            .unwrap()
            .unwrap();
        assert!(so.sub_work_items.is_empty());
        let h = hub
            .history("deep-ion-ai", "demo", 10, 50, None, true)
            .unwrap()
            .unwrap();
        let subs: Vec<i64> = h.sub_work_items.iter().map(|w| w.gh_number).collect();
        assert_eq!(subs, vec![11, 12]);
        let f11 = h.sub_work_items[0].id;
        assert!(h.decisions.len() > so.decisions.len());
        assert!(h.decisions.iter().any(|d| d.work_item_id == f11));
        assert!(h.prs.iter().any(|p| p.number == 50));
        // Eventos e sessões continuam só os do épico.
        assert_eq!(h.events, so.events);
        assert_eq!(h.sessions, so.sessions);
    }

    #[test]
    fn dashboard_e_decisoes_por_escopo() {
        let (hub, _a, b) = cenario();
        let d = hub.dashboard().unwrap();
        assert_eq!(d.projects.len(), 1);
        let epico = d.projects[0]
            .work_items
            .iter()
            .find(|w| w.work_item.gh_number == 10)
            .unwrap();
        // Soma as autônomas pendentes das duas filhas, de ambientes diferentes.
        assert_eq!(epico.pending_decisions, 2);
        assert_eq!(epico.title.as_deref(), Some("Issue 10"));
        assert_eq!(d.live_sessions.len(), 1);
        assert_eq!(d.live_sessions[0].env_id, b.env_id());
        // O autor "hub" dos eventos do GitHub não é ambiente: só a e b.
        assert_eq!(d.envs.len(), 2);

        let ep = hub
            .scoped_decisions("deep-ion-ai", "demo", 11, DecisionScope::Epic)
            .unwrap()
            .unwrap();
        assert_eq!(ep.len(), 1, "a filha 11 não é épico: só as dela");
        let ep = hub
            .scoped_decisions("deep-ion-ai", "demo", 10, DecisionScope::Epic)
            .unwrap()
            .unwrap();
        assert_eq!(ep.len(), 3);
        let pj = hub
            .scoped_decisions("deep-ion-ai", "demo", 99, DecisionScope::Project)
            .unwrap()
            .unwrap();
        assert_eq!(pj.len(), 3);
        assert!(hub
            .scoped_decisions("deep-ion-ai", "demo", 5, DecisionScope::Epic)
            .unwrap()
            .is_none());

        let v = hub.project_views().unwrap();
        let mut nomes: Vec<_> = v[0].locations.iter().map(|l| l.env_name.clone()).collect();
        nomes.sort();
        assert_eq!(nomes, vec!["a", "b"]);
        assert!(v[0].sync.is_some());
    }
}
