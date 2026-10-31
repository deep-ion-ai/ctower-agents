//! Busca de um projeto no GitHub (spec §7.5), sem tocar no ledger: o
//! resultado é um `GithubSnapshot` que `Ledger::apply_github` aplica numa
//! transação. Assim o lock do ledger nunca é segurado durante a rede.

use crate::{time, Fetched, GhClient, GhError};
use ct_core::branch::issue_number_from_branch;
use ct_core::{PrState, WorkItemKind, WorkItemStatus};
use ct_ledger::{GithubSnapshot, Hub, IssueRow, Ledger, PrRow, Project, WorkItem};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// Janela das issues fechadas (spec §7.5).
const CLOSED_WINDOW_MS: i64 = 30 * 86_400_000;

/// O que a busca precisa do ledger, lido de uma vez (lock curto).
#[derive(Debug, Clone)]
pub struct SyncInput {
    pub project: Project,
    /// Work items com branch e ainda não concluídos (os PRs a observar).
    /// Quem aguarda o épico fica de fora: o PR dele já foi mesclado na
    /// branch do épico, e o que falta vem do PR do épico (#454).
    pub work_items: Vec<WorkItem>,
    /// Épicos abertos já conhecidos no cache.
    pub open_epics: Vec<i64>,
    /// `sub-issue → épico` do cache: o PR para a branch de um épico só vale
    /// para uma sub-issue dele (#454).
    pub parents: HashMap<i64, i64>,
    /// ETags gravados das consultas desta rodada.
    pub etags: HashMap<String, String>,
    pub now_ms: i64,
}

impl SyncInput {
    pub fn load(l: &Ledger, project_id: i64, now_ms: i64) -> ct_ledger::Result<Self> {
        let project = l.get_project(project_id)?;
        let work_items: Vec<WorkItem> = l
            .list_work_items(project_id)?
            .into_iter()
            .filter(|w| w.branch.is_some() && w.status != WorkItemStatus::Concluido)
            .collect();
        let issues = l.list_issues(project_id)?;
        Self::build(project, work_items, &issues, now_ms, |k| l.etag(k))
    }

    /// Entrada de um projeto central do `hub.db`, que não precisa de clone
    /// no hub (issue #60): ids e espelho são os do hub, `repo_path` vazio.
    pub fn load_hub(hub: &Hub, project_id: i64, now_ms: i64) -> ct_ledger::Result<Self> {
        let p = hub
            .list_projects()?
            .into_iter()
            .find(|p| p.id == project_id)
            .ok_or_else(|| ct_ledger::LedgerError::NotFound("project", project_id.to_string()))?;
        let project = Project {
            id: p.id,
            name: p.name,
            gh_owner: p.gh_owner,
            gh_repo: p.gh_repo,
            repo_path: String::new(),
            base_branch: p.base_branch,
            protected_branches: p.protected_branches,
            created_at: p.created_at,
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
        let issues = hub.list_issues(project_id)?;
        Self::build(project, work_items, &issues, now_ms, |k| hub.etag(k))
    }

    fn build(
        project: Project,
        mut work_items: Vec<WorkItem>,
        issues: &[IssueRow],
        now_ms: i64,
        etag: impl Fn(&str) -> ct_ledger::Result<Option<String>>,
    ) -> ct_ledger::Result<Self> {
        work_items.retain(|w| w.status != WorkItemStatus::AguardandoEpico);
        let open_epics: Vec<i64> = issues
            .iter()
            .filter(|i| i.has_sub_issues && i.state == "open")
            .map(|i| i.gh_number)
            .collect();
        let (o, r) = (&project.gh_owner, &project.gh_repo);
        let mut paths = vec![
            issues_path(o, r, "open", None),
            issues_path(o, r, "closed", Some(&closed_since(now_ms))),
        ];
        paths.extend(open_epics.iter().map(|n| sub_issues_path(o, r, *n)));
        paths.extend(
            work_items
                .iter()
                .filter_map(|w| w.branch.as_deref())
                .map(|b| pulls_path(o, r, b)),
        );
        // PRs para a branch de cada épico em andamento: os das sub-issues.
        paths.extend(epic_branches(&work_items).map(|(_, b)| pulls_base_path(o, r, b)));
        let mut etags = HashMap::new();
        for p in paths {
            if let Some(e) = etag(&p)? {
                etags.insert(p, e);
            }
        }
        let parents = issues
            .iter()
            .filter_map(|i| Some((i.gh_number, i.parent_number?)))
            .collect();
        Ok(SyncInput {
            project,
            work_items,
            open_epics,
            parents,
            etags,
            now_ms,
        })
    }
}

/// `since` das issues fechadas: início do dia UTC de 30 dias atrás (estável
/// ao longo do dia, para o ETag servir entre ciclos).
pub fn closed_since(now_ms: i64) -> String {
    time::day_start_utc(now_ms - CLOSED_WINDOW_MS)
}

pub fn issues_path(owner: &str, repo: &str, state: &str, since: Option<&str>) -> String {
    let mut p = format!("/repos/{owner}/{repo}/issues?state={state}&per_page=100");
    if let Some(s) = since {
        p.push_str("&since=");
        p.push_str(&encode(s));
    }
    p
}

pub fn sub_issues_path(owner: &str, repo: &str, number: i64) -> String {
    format!("/repos/{owner}/{repo}/issues/{number}/sub_issues?per_page=100")
}

pub fn pulls_path(owner: &str, repo: &str, branch: &str) -> String {
    format!(
        "/repos/{owner}/{repo}/pulls?state=all&head={}&per_page=100",
        encode(&format!("{owner}:{branch}"))
    )
}

/// PRs (todos os estados) cuja base é `branch`: os das sub-issues para a
/// branch do épico (#454).
pub fn pulls_base_path(owner: &str, repo: &str, branch: &str) -> String {
    format!(
        "/repos/{owner}/{repo}/pulls?state=all&base={}&per_page=100",
        encode(branch)
    )
}

/// Épicos com branch entre os work items observados: `(work item, branch)`.
fn epic_branches(work_items: &[WorkItem]) -> impl Iterator<Item = (&WorkItem, &str)> {
    work_items
        .iter()
        .filter(|w| w.kind == WorkItemKind::Epic)
        .filter_map(|w| Some((w, w.branch.as_deref()?)))
}

/// Sub-issue de um PR para a branch do épico `epic`: o `n` do head
/// `<tipo>/<n>-<slug>`, se `n` é sub-issue do épico (#454). Head sem número,
/// ou de issue que não é filha do épico, fica de fora.
pub fn sub_issue_of_head(head: &str, epic: i64, parents: &HashMap<i64, i64>) -> Option<i64> {
    let n = issue_number_from_branch(head)?;
    (parents.get(&n) == Some(&epic)).then_some(n)
}

/// Percent-encoding de um valor de query (tudo fora de `A-Za-z0-9-._~`).
fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// Issue da API → linha do cache; `None` para PRs (que também vêm em
/// `/issues`) e objetos incompletos.
pub fn issue_row(v: &Value) -> Option<IssueRow> {
    if v.get("pull_request").is_some() {
        return None;
    }
    Some(IssueRow {
        gh_number: v.get("number")?.as_i64()?,
        title: v.get("title")?.as_str()?.to_string(),
        state: v.get("state")?.as_str()?.to_string(),
        labels: v
            .get("labels")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|l| l.get("name").and_then(Value::as_str).map(String::from))
                    .collect()
            })
            .unwrap_or_default(),
        parent_number: v
            .get("parent_issue_url")
            .and_then(Value::as_str)
            .and_then(|u| u.rsplit('/').next()?.parse().ok()),
        has_sub_issues: v
            .pointer("/sub_issues_summary/total")
            .and_then(Value::as_i64)
            .is_some_and(|t| t > 0),
        url: v.get("html_url")?.as_str()?.to_string(),
        updated_at: v
            .get("updated_at")
            .and_then(Value::as_str)
            .and_then(time::parse_rfc3339)
            .unwrap_or(0),
        ..Default::default()
    })
}

/// PR da API → linha do cache, ligado ao work item da branch.
pub fn pr_row(v: &Value, work_item_id: i64) -> Option<PrRow> {
    let state = if v.get("merged_at").is_some_and(|m| !m.is_null()) {
        PrState::Merged
    } else if v.get("state")?.as_str()? == "open" {
        PrState::Open
    } else {
        PrState::Closed
    };
    Some(PrRow {
        number: v.get("number")?.as_i64()?,
        work_item_id,
        head: v.pointer("/head/ref")?.as_str()?.to_string(),
        base: v.pointer("/base/ref")?.as_str()?.to_string(),
        state,
        url: v.get("html_url")?.as_str()?.to_string(),
        updated_at: v
            .get("updated_at")
            .and_then(Value::as_str)
            .and_then(time::parse_rfc3339)
            .unwrap_or(0),
        head_sha: v
            .pointer("/head/sha")
            .and_then(Value::as_str)
            .map(String::from),
    })
}

/// Busca o projeto: issues abertas e fechadas na janela, filhas dos épicos
/// abertos, PRs das branches dos work items e PRs das sub-issues para a
/// branch de cada épico. Consultas em `304` não entram no snapshot; os ETags
/// novos sim.
pub fn fetch(api: &GhClient, input: &SyncInput) -> Result<GithubSnapshot, GhError> {
    let (o, r) = (&input.project.gh_owner, &input.project.gh_repo);
    let mut etags = Vec::new();
    let mut get = |path: String| -> Result<Option<Vec<Value>>, GhError> {
        match api.get_all(&path, input.etags.get(&path).map(String::as_str))? {
            Fetched::NotModified => Ok(None),
            Fetched::Modified { body, etag } => {
                if let Some(e) = etag {
                    etags.push((path, e));
                }
                Ok(Some(body))
            }
        }
    };

    // Última ocorrência vence; as filhas de `sub_issues` vêm depois das listagens.
    let mut issues: BTreeMap<i64, IssueRow> = BTreeMap::new();
    let since = closed_since(input.now_ms);
    for path in [
        issues_path(o, r, "open", None),
        issues_path(o, r, "closed", Some(&since)),
    ] {
        for v in get(path)?.unwrap_or_default() {
            if let Some(row) = issue_row(&v) {
                issues.insert(row.gh_number, row);
            }
        }
    }

    let epics: BTreeSet<i64> = input
        .open_epics
        .iter()
        .copied()
        .chain(
            issues
                .values()
                .filter(|i| i.has_sub_issues && i.state == "open")
                .map(|i| i.gh_number),
        )
        .collect();
    for epic in epics {
        for v in get(sub_issues_path(o, r, epic))?.unwrap_or_default() {
            if let Some(mut row) = issue_row(&v) {
                row.parent_number = Some(epic);
                issues.insert(row.gh_number, row);
            }
        }
    }

    let epic_wis: Vec<(&WorkItem, &str)> = epic_branches(&input.work_items).collect();
    let mut prs = Vec::new();
    for wi in &input.work_items {
        let Some(branch) = wi.branch.as_deref() else {
            continue;
        };
        for v in get(pulls_path(o, r, branch))?.unwrap_or_default() {
            // PR para a branch de um épico vem da consulta pela base, abaixo.
            if let Some(row) = pr_row(&v, wi.id).filter(|p| !epic_wis.iter().any(|e| e.1 == p.base))
            {
                prs.push(row);
            }
        }
    }
    // PRs das sub-issues para a branch do épico (#454). O `work_item_id` é o
    // do épico: quem aplica cria (ou acha) o work item da sub-issue pelo head
    // e troca o id. As filhas desta rodada valem junto com as do cache.
    let mut parents = input.parents.clone();
    parents.extend(
        issues
            .values()
            .filter_map(|i| Some((i.gh_number, i.parent_number?))),
    );
    for (epic, branch) in &epic_wis {
        for v in get(pulls_base_path(o, r, branch))?.unwrap_or_default() {
            if let Some(row) = pr_row(&v, epic.id)
                .filter(|p| sub_issue_of_head(&p.head, epic.gh_number, &parents).is_some())
            {
                prs.push(row);
            }
        }
    }
    // Do mais antigo ao mais novo: o PR mais recente da branch decide por último.
    prs.sort_by_key(|p| p.number);

    Ok(GithubSnapshot {
        project_id: input.project.id,
        issues: issues.into_values().collect(),
        prs,
        etags,
        synced_at: input.now_ms,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::{fake_gh, FakeGitHub};
    use crate::GhConfig;
    use ct_core::WorkItemKind;
    use ct_ledger::NewProject;
    use serde_json::json;

    const NOW: &str = "2026-10-15T12:00:00Z";

    fn now() -> i64 {
        time::parse_rfc3339(NOW).unwrap()
    }

    fn gh_issue(n: i64, state: &str, parent: Option<i64>, subs: i64) -> Value {
        json!({
            "number": n, "title": format!("Issue {n}"), "state": state,
            "labels": [{ "name": "enhancement" }],
            "html_url": format!("https://github.com/o/r/issues/{n}"),
            "updated_at": "2026-10-14T10:00:00Z",
            "parent_issue_url": parent.map(|p| format!("https://api.github.com/repos/o/r/issues/{p}")),
            "sub_issues_summary": { "total": subs, "completed": 0, "percent_completed": 0 },
        })
    }

    fn gh_pr(n: i64, state: &str, merged: bool, head: &str) -> Value {
        gh_pr_base(n, state, merged, head, "develop")
    }

    fn gh_pr_base(n: i64, state: &str, merged: bool, head: &str, base: &str) -> Value {
        json!({
            "number": n, "state": state,
            "merged_at": if merged { json!("2026-10-15T11:00:00Z") } else { Value::Null },
            "head": { "ref": head, "sha": format!("sha-{n}") }, "base": { "ref": base },
            "html_url": format!("https://github.com/o/r/pull/{n}"),
            "updated_at": "2026-10-15T11:00:00Z",
        })
    }

    struct Env {
        _d: tempfile::TempDir,
        gh: FakeGitHub,
        api: GhClient,
        l: Ledger,
        project: i64,
    }

    impl Env {
        fn new() -> Self {
            let d = tempfile::tempdir().unwrap();
            let gh = FakeGitHub::start("tok");
            let bin = fake_gh(d.path(), "tok");
            let api = GhClient::new(
                GhConfig::new(&gh.url(), bin.to_str().unwrap())
                    .with_token_timeout(crate::fake::FAKE_GH_TOKEN_TIMEOUT),
            );
            let l = Ledger::open_in_memory().unwrap();
            let project = l
                .add_project(&NewProject {
                    name: "r".into(),
                    gh_owner: "o".into(),
                    gh_repo: "r".into(),
                    repo_path: "/r".into(),
                    base_branch: "develop".into(),
                    protected_branches: vec!["develop".into()],
                })
                .unwrap()
                .id;
            // Por padrão tudo vazio.
            gh.set(&issues_path("o", "r", "open", None), json!([]));
            gh.set(
                &issues_path("o", "r", "closed", Some(&closed_since(now()))),
                json!([]),
            );
            Env {
                _d: d,
                gh,
                api,
                l,
                project,
            }
        }

        fn sync(&self) -> ct_ledger::GithubApplied {
            let input = SyncInput::load(&self.l, self.project, now()).unwrap();
            let snap = fetch(&self.api, &input).unwrap();
            self.l.apply_github(&snap).unwrap()
        }
    }

    #[test]
    fn caminhos_e_janela() {
        assert_eq!(closed_since(now()), "2026-09-15T00:00:00Z");
        assert_eq!(
            issues_path("o", "r", "closed", Some("2026-09-15T00:00:00Z")),
            "/repos/o/r/issues?state=closed&per_page=100&since=2026-09-15T00%3A00%3A00Z"
        );
        assert_eq!(
            pulls_path("o", "r", "feat/2-x"),
            "/repos/o/r/pulls?state=all&head=o%3Afeat%2F2-x&per_page=100"
        );
        assert_eq!(
            pulls_base_path("o", "r", "wip/1-x"),
            "/repos/o/r/pulls?state=all&base=wip%2F1-x&per_page=100"
        );
    }

    #[test]
    fn head_para_sub_issue_do_epico() {
        let parents = HashMap::from([(2, 1), (3, 9)]);
        assert_eq!(sub_issue_of_head("feat/2-x", 1, &parents), Some(2));
        assert_eq!(sub_issue_of_head("fix/2", 1, &parents), Some(2));
        // Filha de outro épico, fora do cache e head sem número.
        assert_eq!(sub_issue_of_head("feat/3-y", 1, &parents), None);
        assert_eq!(sub_issue_of_head("feat/4-z", 1, &parents), None);
        assert_eq!(sub_issue_of_head("fix/teste-instavel", 1, &parents), None);
    }

    /// Sem épico com branch em andamento, nenhuma consulta pela base: um
    /// épico sem branch e uma issue com branch só geram as de sempre.
    #[test]
    fn sem_epico_com_branch_nao_ha_consulta_pela_base() {
        let e = Env::new();
        e.l.ensure_work_item(e.project, 1, WorkItemKind::Epic)
            .unwrap();
        let wi =
            e.l.ensure_work_item(e.project, 2, WorkItemKind::Issue)
                .unwrap();
        e.l.set_work_item_branch(wi.id, "feat/2-x", "/wt").unwrap();
        e.gh.set(&pulls_path("o", "r", "feat/2-x"), json!([]));
        e.sync();
        let paths: Vec<String> = e.gh.requests().into_iter().map(|(p, _)| p).collect();
        assert_eq!(paths.len(), 3, "{paths:?}");
        assert!(paths.iter().all(|p| !p.contains("base=")), "{paths:?}");
    }

    /// #454: os PRs para a branch do épico, com ETag, viram os das
    /// sub-issues; o merge do épico conclui quem o aguarda.
    #[test]
    fn prs_das_sub_issues_pela_branch_do_epico() {
        let e = Env::new();
        let epic =
            e.l.ensure_work_item(e.project, 1, WorkItemKind::Epic)
                .unwrap();
        e.l.set_work_item_branch(epic.id, "wip/1-e", "/wt/wip__1-e")
            .unwrap();
        e.gh.set(
            &issues_path("o", "r", "open", None),
            json!([
                gh_issue(1, "open", None, 3),
                gh_issue(2, "open", Some(1), 0),
                gh_issue(3, "open", Some(1), 0),
                gh_issue(5, "open", None, 0)
            ]),
        );
        e.gh.set(
            &sub_issues_path("o", "r", 1),
            json!([
                gh_issue(2, "open", Some(1), 0),
                gh_issue(3, "open", Some(1), 0)
            ]),
        );
        let base = pulls_base_path("o", "r", "wip/1-e");
        let head = pulls_path("o", "r", "wip/1-e");
        e.gh.set(&head, json!([]));
        e.gh.set(
            &base,
            json!([
                gh_pr_base(44, "closed", true, "feat/2-a", "wip/1-e"),
                gh_pr_base(43, "open", false, "feat/3-b", "wip/1-e"),
                gh_pr_base(42, "closed", true, "fix/teste-instavel", "wip/1-e"),
                gh_pr_base(41, "closed", true, "feat/5-c", "wip/1-e")
            ]),
        );
        let r = e.sync();
        assert_eq!(r.prs, 2);
        let w2 = e.l.find_work_item(e.project, 2).unwrap().unwrap();
        let w3 = e.l.find_work_item(e.project, 3).unwrap().unwrap();
        assert_eq!(
            r.status_changes,
            vec![(w2.id, WorkItemStatus::AguardandoEpico)]
        );
        assert_eq!(
            (w2.branch.as_deref(), w3.branch.as_deref(), w3.status),
            (Some("feat/2-a"), Some("feat/3-b"), WorkItemStatus::Backlog)
        );
        assert!(e.l.find_work_item(e.project, 5).unwrap().is_none());
        assert_eq!(
            e.l.get_work_item(epic.id).unwrap().status,
            WorkItemStatus::Backlog
        );

        // Ciclo seguinte: a base vai com ETag; a 3 (PR aberto) é consultada
        // pelo head, e a 2, que aguarda o épico, não. O PR da 3 achado pelo
        // head fica de fora: o que é para a branch do épico vem pela base.
        e.gh.set(
            &pulls_path("o", "r", "feat/3-b"),
            json!([gh_pr_base(43, "open", false, "feat/3-b", "wip/1-e")]),
        );
        e.gh.clear_requests();
        let r = e.sync();
        assert!(r.status_changes.is_empty() && r.prs == 0, "{r:?}");
        let reqs = e.gh.requests();
        assert!(reqs.iter().any(|(p, inm)| *p == base && inm.is_some()));
        assert!(reqs.iter().all(|(p, _)| !p.contains("feat%2F2-a")));
        assert_eq!(
            e.l.find_work_item(e.project, 3).unwrap().unwrap().status,
            WorkItemStatus::Backlog
        );

        // O PR do épico entra na base: o épico e a 2 concluem; a 3 não.
        e.gh.set(&head, json!([gh_pr(60, "closed", true, "wip/1-e")]));
        let r = e.sync();
        assert_eq!(
            r.status_changes,
            vec![
                (epic.id, WorkItemStatus::Concluido),
                (w2.id, WorkItemStatus::Concluido)
            ]
        );
        assert_eq!(
            e.l.list_events(w2.id, 1).unwrap()[0].uid,
            "gh:pr:o/r#60:merged:sub:2"
        );
        // Épico concluído: a branch dele deixa de ser consultada pela base.
        e.gh.clear_requests();
        e.sync();
        assert!(e.gh.requests().iter().all(|(p, _)| *p != base));
    }

    #[test]
    fn issues_subissues_e_descarte_de_prs() {
        let e = Env::new();
        let mut pr_na_listagem = gh_issue(9, "open", None, 0);
        pr_na_listagem["pull_request"] = json!({ "url": "x" });
        e.gh.set(
            &issues_path("o", "r", "open", None),
            json!([
                gh_issue(1, "open", None, 2),
                gh_issue(2, "open", Some(1), 0),
                pr_na_listagem
            ]),
        );
        e.gh.set(
            &issues_path("o", "r", "closed", Some(&closed_since(now()))),
            json!([gh_issue(3, "closed", None, 0)]),
        );
        // A filha 4 fechou há muito tempo: só aparece em sub_issues.
        e.gh.set(
            &sub_issues_path("o", "r", 1),
            json!([
                gh_issue(2, "open", Some(1), 0),
                gh_issue(4, "closed", None, 0)
            ]),
        );
        let r = e.sync();
        assert_eq!(r.issues, 4);
        let issues = e.l.list_issues(e.project).unwrap();
        let resumo: Vec<_> = issues
            .iter()
            .map(|i| {
                (
                    i.gh_number,
                    i.state.as_str(),
                    i.parent_number,
                    i.has_sub_issues,
                )
            })
            .collect();
        assert_eq!(
            resumo,
            vec![
                (1, "open", None, true),
                (2, "open", Some(1), false),
                (3, "closed", None, false),
                (4, "closed", Some(1), false),
            ]
        );
        assert_eq!(issues[0].labels, vec!["enhancement"]);
        assert_eq!(
            issues[0].updated_at,
            time::parse_rfc3339("2026-10-14T10:00:00Z").unwrap()
        );

        // Segunda rodada: tudo em 304, nada a aplicar.
        e.gh.clear_requests();
        let input = SyncInput::load(&e.l, e.project, now()).unwrap();
        let snap = fetch(&e.api, &input).unwrap();
        assert!(snap.issues.is_empty() && snap.prs.is_empty() && snap.etags.is_empty());
        let reqs = e.gh.requests();
        assert_eq!(reqs.len(), 3);
        assert!(reqs.iter().all(|(_, inm)| inm.is_some()));
    }

    #[test]
    fn pr_aberto_e_depois_mesclado() {
        let e = Env::new();
        let wi =
            e.l.ensure_work_item(e.project, 2, WorkItemKind::Issue)
                .unwrap();
        e.l.set_work_item_branch(wi.id, "feat/2-x", "/wt/feat__2-x")
            .unwrap();
        let pulls = pulls_path("o", "r", "feat/2-x");
        e.gh.set(&pulls, json!([]));
        assert!(e.sync().status_changes.is_empty());

        e.gh.set(&pulls, json!([gh_pr(30, "open", false, "feat/2-x")]));
        assert_eq!(
            e.sync().status_changes,
            vec![(wi.id, WorkItemStatus::EmRevisao)]
        );
        // Ciclo seguinte sem mudança no GitHub: 304, nada muda.
        assert!(e.sync().status_changes.is_empty());

        e.gh.set(&pulls, json!([gh_pr(30, "closed", true, "feat/2-x")]));
        assert_eq!(
            e.sync().status_changes,
            vec![(wi.id, WorkItemStatus::Concluido)]
        );
        let prs = e.l.prs_for_work_item(wi.id).unwrap();
        assert_eq!((prs[0].number, prs[0].state), (30, PrState::Merged));
        // O head do PR vem da mesma listagem (#286).
        assert_eq!(prs[0].head_sha.as_deref(), Some("sha-30"));
        // Concluído: a branch deixa de ser consultada.
        e.gh.clear_requests();
        e.sync();
        assert!(e.gh.requests().iter().all(|(p, _)| !p.contains("/pulls")));
    }

    #[test]
    fn pr_reaberto_em_novo_numero_e_fechado_sem_merge() {
        let e = Env::new();
        let wi =
            e.l.ensure_work_item(e.project, 2, WorkItemKind::Issue)
                .unwrap();
        e.l.set_work_item_branch(wi.id, "feat/2-x", "/wt").unwrap();
        let pulls = pulls_path("o", "r", "feat/2-x");
        // A API lista do mais novo para o mais antigo.
        e.gh.set(
            &pulls,
            json!([
                gh_pr(31, "open", false, "feat/2-x"),
                gh_pr(30, "closed", false, "feat/2-x")
            ]),
        );
        assert_eq!(
            e.sync().status_changes,
            vec![(wi.id, WorkItemStatus::EmRevisao)]
        );
        assert_eq!(e.l.prs_for_work_item(wi.id).unwrap().len(), 2);
    }

    #[test]
    fn erro_numa_consulta_nao_gera_snapshot() {
        let e = Env::new();
        e.gh.set(
            &issues_path("o", "r", "open", None),
            json!([gh_issue(1, "open", None, 1)]),
        );
        // sub_issues do épico não publicado → 404.
        let input = SyncInput::load(&e.l, e.project, now()).unwrap();
        let err = fetch(&e.api, &input).unwrap_err();
        assert!(matches!(err, GhError::Http { status: 404, .. }), "{err}");
        assert!(e.l.list_issues(e.project).unwrap().is_empty());
        assert_eq!(
            e.l.etag(&issues_path("o", "r", "open", None)).unwrap(),
            None
        );
    }
}
