//! Entrada da busca do GitHub para um projeto central do `hub.db` (issue #60).
//! A busca em si fica no `ct-gh`; a leitura do `hub.db` fica aqui, no hub.

use crate::{Hub, LedgerError, Project, Result, WorkItem};
use ct_gh::sync::{Mirror, SyncInput};

/// Entrada de um projeto central do `hub.db`, que não precisa de clone
/// no hub: ids e espelho são os do hub, `repo_path` vazio.
pub fn sync_input_from_hub(hub: &Hub, project_id: i64, now_ms: i64) -> Result<SyncInput> {
    let p = hub
        .list_projects()?
        .into_iter()
        .find(|p| p.id == project_id)
        .ok_or_else(|| LedgerError::NotFound("project", project_id.to_string()))?;
    let project = Project {
        id: p.id,
        name: p.name,
        gh_owner: p.gh_owner,
        gh_repo: p.gh_repo,
        repo_path: String::new(),
        base_branch: p.base_branch,
        protected_branches: p.protected_branches,
        created_at: p.created_at,
        autonomy: p.autonomy,
        ..Project::default()
    };
    let work_items = hub
        .github_work_items(project_id)?
        .into_iter()
        .map(|w| WorkItem {
            id: w.id,
            project_id: w.project_id,
            gh_number: w.gh_number,
            kind: w.kind,
            status: w.status,
            branch: w.branch,
            worktree_path: None,
            created_at: w.created_at,
            closed_at: w.closed_at,
        })
        .collect();
    let mirror = Mirror {
        issues: hub.list_issues(project_id)?,
        open_prs: hub.open_prs(project_id)?,
        checks: hub.list_pr_checks(project_id)?,
    };
    SyncInput::build(project, work_items, mirror, now_ms, |k| hub.etag(k))
}
