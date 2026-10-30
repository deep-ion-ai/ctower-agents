//! Árvore do projeto (spec §10, tela Projeto): épicos → sub-issues, cada
//! issue com o seu work item, a partir do `issue_cache`.

use crate::{
    DecisionCounts, IssueRow, Ledger, Project, Result, SyncState, WorkItem, WorkItemSummary,
};
use ct_core::DecisionOrigin;
use serde::Serialize;
use std::collections::{BTreeMap, HashMap, HashSet};

#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct TreeNode {
    pub issue: IssueRow,
    pub work_item: Option<WorkItem>,
    /// Mesmo formato do `hub.tree.get` (#46): a tela local mostra PR,
    /// decisões e última atividade quando o hub está fora. Vazio sem work item.
    pub summary: WorkItemSummary,
    pub children: Vec<TreeNode>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct ProjectTree {
    pub project: Project,
    pub sync: Option<SyncState>,
    /// Issues sem pai no cache (épicos e avulsas), por número.
    pub roots: Vec<TreeNode>,
    /// Work items cuja issue ainda não está no cache.
    pub untracked: Vec<WorkItem>,
}

impl Ledger {
    /// Resumo do work item a partir do ledger local. `live_envs` traz só o
    /// `env_id` deste ambiente, quando há sessão viva no work item.
    fn work_item_summary(&self, wi: &WorkItem, live: &HashSet<i64>) -> Result<WorkItemSummary> {
        let decisions = self.list_decisions(&[wi.id], false)?;
        let pending_autonomous = decisions
            .iter()
            .filter(|d| d.origin == DecisionOrigin::Autonoma && d.reviewed_at.is_none())
            .count();
        let last_activity_at: Option<i64> = self.conn.query_row(
            "SELECT max(ts) FROM event WHERE work_item_id = ?1",
            [wi.id],
            |r| r.get(0),
        )?;
        Ok(WorkItemSummary {
            pr: self.prs_for_work_item(wi.id)?.into_iter().next(),
            decisions: DecisionCounts {
                total: decisions.len(),
                pending_autonomous,
            },
            last_activity_at,
            live_envs: if live.contains(&wi.id) {
                vec![self.env_id().to_string()]
            } else {
                Vec::new()
            },
        })
    }

    pub fn project_tree(&self, project_id: i64) -> Result<ProjectTree> {
        let project = self.get_project(project_id)?;
        let issues = self.list_issues(project_id)?;
        let mut work_items: HashMap<i64, WorkItem> = self
            .list_work_items(project_id)?
            .into_iter()
            .map(|w| (w.gh_number, w))
            .collect();

        let known: std::collections::HashSet<i64> = issues.iter().map(|i| i.gh_number).collect();
        let mut children: BTreeMap<i64, Vec<IssueRow>> = BTreeMap::new();
        let mut roots = Vec::new();
        for i in issues {
            match i.parent_number.filter(|p| known.contains(p)) {
                Some(p) => children.entry(p).or_default().push(i),
                None => roots.push(i),
            }
        }

        let live: HashSet<i64> = self
            .live_sessions()?
            .iter()
            .map(|s| s.work_item_id)
            .collect();

        fn build(
            l: &Ledger,
            live: &HashSet<i64>,
            i: IssueRow,
            children: &mut BTreeMap<i64, Vec<IssueRow>>,
            work_items: &mut HashMap<i64, WorkItem>,
        ) -> Result<TreeNode> {
            let kids = children.remove(&i.gh_number).unwrap_or_default();
            let work_item = work_items.remove(&i.gh_number);
            let summary = match &work_item {
                Some(w) => l.work_item_summary(w, live)?,
                None => WorkItemSummary::default(),
            };
            Ok(TreeNode {
                work_item,
                summary,
                children: kids
                    .into_iter()
                    .map(|c| build(l, live, c, children, work_items))
                    .collect::<Result<_>>()?,
                issue: i,
            })
        }
        let roots = roots
            .into_iter()
            .map(|i| build(self, &live, i, &mut children, &mut work_items))
            .collect::<Result<_>>()?;
        let mut untracked: Vec<WorkItem> = work_items.into_values().collect();
        untracked.sort_by_key(|w| w.id);
        Ok(ProjectTree {
            sync: self.sync_state(project_id)?,
            project,
            roots,
            untracked,
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::test_support::ledger_with_project;
    use crate::{GithubSnapshot, IssueRow, NewDecision, NewSession, PrRow, WorkItemSummary};
    use ct_core::{DecisionOrigin, PrState, SessionState, Tool, WorkItemKind};

    fn issue(n: i64, parent: Option<i64>) -> IssueRow {
        IssueRow {
            gh_number: n,
            title: format!("Issue {n}"),
            state: "open".into(),
            labels: vec![],
            parent_number: parent,
            has_sub_issues: false,
            url: String::new(),
            updated_at: 0,
            sprints: Vec::new(),
        }
    }

    #[test]
    fn monta_epico_filhas_e_avulsos() {
        let (l, p) = ledger_with_project();
        let epic_wi = l.ensure_work_item(p.id, 1, WorkItemKind::Epic).unwrap();
        let filha_wi = l.ensure_work_item(p.id, 3, WorkItemKind::Issue).unwrap();
        let sem_cache = l.ensure_work_item(p.id, 50, WorkItemKind::Issue).unwrap();
        l.apply_github(&GithubSnapshot {
            project_id: p.id,
            issues: vec![
                issue(1, None),
                issue(2, Some(1)),
                issue(3, Some(1)),
                issue(4, None),
                // Pai fora do cache: vira raiz.
                issue(5, Some(99)),
            ],
            prs: vec![],
            etags: vec![],
            synced_at: 10,
        })
        .unwrap();

        let t = l.project_tree(p.id).unwrap();
        assert_eq!(t.project.id, p.id);
        assert_eq!(t.sync.unwrap().last_ok_at, Some(10));
        let nums: Vec<i64> = t.roots.iter().map(|n| n.issue.gh_number).collect();
        assert_eq!(nums, vec![1, 4, 5]);
        let epico = &t.roots[0];
        assert_eq!(epico.work_item.as_ref().unwrap().id, epic_wi.id);
        let filhas: Vec<i64> = epico.children.iter().map(|n| n.issue.gh_number).collect();
        assert_eq!(filhas, vec![2, 3]);
        assert!(epico.children[0].work_item.is_none());
        assert_eq!(
            epico.children[1].work_item.as_ref().unwrap().id,
            filha_wi.id
        );
        assert_eq!(t.untracked, vec![sem_cache]);
    }

    #[test]
    fn resumo_do_work_item_igual_ao_do_hub() {
        let (l, p) = ledger_with_project();
        let wi = l.ensure_work_item(p.id, 1, WorkItemKind::Issue).unwrap();
        let s = l
            .create_session(&NewSession {
                work_item_id: wi.id,
                tool: Tool::Claude,
                cwd: "/w".into(),
                launch_cmd: vec![],
                model: None,
                mode: ct_core::SessionMode::Native,
            })
            .unwrap();
        for origin in [DecisionOrigin::Autonoma, DecisionOrigin::Orientada] {
            l.record_decision(&NewDecision {
                work_item_id: wi.id,
                session_id: Some(s.id.clone()),
                origin,
                title: "t".into(),
                context: "c".into(),
                decision: "d".into(),
                consequences: None,
            })
            .unwrap();
        }
        let pr = |n| PrRow {
            number: n,
            work_item_id: wi.id,
            head: "feat/1-x".into(),
            base: "develop".into(),
            state: PrState::Open,
            url: String::new(),
            updated_at: 1,
            head_sha: None,
        };
        l.apply_github(&GithubSnapshot {
            project_id: p.id,
            issues: vec![issue(1, None), issue(2, None)],
            prs: vec![pr(7), pr(9)],
            etags: vec![],
            synced_at: 10,
        })
        .unwrap();

        let t = l.project_tree(p.id).unwrap();
        let sm = &t.roots[0].summary;
        assert_eq!(sm.pr.as_ref().map(|p| p.number), Some(9));
        assert_eq!(
            (sm.decisions.total, sm.decisions.pending_autonomous),
            (2, 1)
        );
        let last = l
            .list_events(wi.id, 1000)
            .unwrap()
            .iter()
            .map(|e| e.ts)
            .max();
        assert_eq!(sm.last_activity_at, last);
        assert_eq!(sm.live_envs, vec![l.env_id().to_string()]);
        // Issue sem work item: resumo vazio.
        assert_eq!(t.roots[1].summary, WorkItemSummary::default());

        l.transition_session(&s.id, SessionState::Exited, Some(0))
            .unwrap();
        let t = l.project_tree(p.id).unwrap();
        assert!(t.roots[0].summary.live_envs.is_empty());
    }
}
