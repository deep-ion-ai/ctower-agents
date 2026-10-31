//! Sync com o GitHub de todos os projetos GitHub do `hub.db`, com o token do
//! próprio hub (`github.token` ou `gh auth token`). Busca sem o lock do
//! `hub.db` e aplica numa transação; os gatilhos de PR viram eventos do hub
//! com `uid` determinístico, deduplicados com os que os ambientes enviam.

use ct_core::now_ms;
use ct_gh::sync::{fetch, SyncInput};
use ct_gh::GhClient;
use ct_ledger::SharedHub;
use serde::Serialize;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::broadcast;

/// Teto do backoff entre ciclos com falha.
pub const MAX_BACKOFF: Duration = Duration::from_secs(15 * 60);

/// Resultado do sync de um projeto.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct ProjectSync {
    pub project_id: i64,
    pub ok: bool,
    pub error: Option<String>,
    pub issues: usize,
    pub prs: usize,
    pub status_changes: usize,
    /// O projeto mudou de `rev` (excluído, recadastrado ou editado)
    /// durante a busca: nada foi aplicado. Fica fora do resultado.
    #[serde(skip)]
    pub skipped: bool,
}

pub struct HubGithub {
    hub: SharedHub,
    client: GhClient,
    /// Identidade do hub nos eventos gerados aqui.
    hub_id: String,
    events: broadcast::Sender<Value>,
    /// Uma sincronização por vez.
    running: Mutex<()>,
}

impl HubGithub {
    pub fn new(
        hub: SharedHub,
        client: GhClient,
        hub_id: &str,
        events: broadcast::Sender<Value>,
    ) -> Self {
        HubGithub {
            hub,
            client,
            hub_id: hub_id.to_string(),
            events,
            running: Mutex::new(()),
        }
    }

    /// Cliente do GitHub (token, status da conta).
    pub fn client(&self) -> &GhClient {
        &self.client
    }

    /// Valida com o token do hub que `owner/repo` existe e é acessível.
    /// Devolve `owner` e `repo` com a caixa do GitHub. Síncrono.
    pub fn validate_repo(&self, owner: &str, repo: &str) -> Result<(String, String), String> {
        let v = self
            .client
            .get_json(&format!("/repos/{owner}/{repo}"))
            .map_err(|e| format!("não foi possível validar {owner}/{repo} no GitHub: {e}"))?
            .ok_or_else(|| {
                format!(
                    "repositório {owner}/{repo} não encontrado ou inacessível com o token do \
                     GitHub do hub"
                )
            })?;
        let canon_owner = v
            .pointer("/owner/login")
            .and_then(Value::as_str)
            .unwrap_or(owner);
        let canon_repo = v.get("name").and_then(Value::as_str).unwrap_or(repo);
        Ok((canon_owner.to_string(), canon_repo.to_string()))
    }

    /// Valida que a branch existe em `owner/repo`. Síncrono.
    pub fn validate_branch(&self, owner: &str, repo: &str, branch: &str) -> Result<(), String> {
        self.client
            .get_json(&format!(
                "/repos/{owner}/{repo}/branches/{}",
                encode_branch(branch)
            ))
            .map_err(|e| format!("não foi possível validar a branch {branch} no GitHub: {e}"))?
            .map(|_| ())
            .ok_or_else(|| format!("a branch {branch} não existe em {owner}/{repo}"))
    }

    /// Todos os projetos do hub. A falha de um (no GitHub ou no `hub.db`)
    /// não impede os outros: fica em `sync_state.last_error` e no resultado.
    /// Síncrono: chame fora do runtime.
    pub fn sync_all(&self) -> ct_ledger::Result<Vec<ProjectSync>> {
        let _one = self.running.lock().unwrap_or_else(|e| e.into_inner());
        // Propostas pendentes há mais de 14 dias viram obsoletas (spec do
        // gestor §8.3), no mesmo ciclo periódico.
        if self.hub.lock().expire_proposals(ct_core::now_ms())? > 0 {
            let _ = self
                .events
                .send(json!({ "event": "hub.proposals.changed", "data": {} }));
        }
        // Projeto Jira não é sincronizado pelo hub: o espelho vem do
        // ambiente de sync (spec do Jira e GitLab §5.2).
        let ids: Vec<i64> = self
            .hub
            .lock()
            .list_projects()?
            .iter()
            .filter(|p| p.tracker.kind() == ct_core::TrackerKind::Github)
            .map(|p| p.id)
            .collect();
        let mut out = Vec::new();
        for project_id in ids {
            let r = self.sync_one(project_id).unwrap_or_else(|e| ProjectSync {
                project_id,
                ok: false,
                error: Some(e.to_string()),
                ..ProjectSync::default()
            });
            if r.skipped {
                continue;
            }
            // Sem UI conectada não há receptor: tudo bem.
            let _ = self.events.send(json!({
                "event": "hub_sync_status",
                "data": { "project_id": project_id, "ok": r.ok, "error": r.error },
            }));
            out.push(r);
        }
        Ok(out)
    }

    /// Um projeto: lê com lock curto, busca sem ele e aplica só se o
    /// projeto não mudou de `rev` nesse meio-tempo (excluído, recadastrado
    /// ou editado); senão descarta o resultado (`skipped`), e o próximo
    /// ciclo busca de novo.
    fn sync_one(&self, project_id: i64) -> ct_ledger::Result<ProjectSync> {
        let skipped = ProjectSync {
            project_id,
            ok: true,
            skipped: true,
            ..ProjectSync::default()
        };
        let (rev, input) = {
            let h = self.hub.lock();
            let Some(rev) = h.live_project_rev(project_id)? else {
                return Ok(skipped);
            };
            (rev, SyncInput::load_hub(&h, project_id, now_ms())?)
        };
        let fetched = fetch(&self.client, &input);
        let h = self.hub.lock();
        if h.live_project_rev(project_id)? != Some(rev) {
            tracing::info!(
                "github: projeto {project_id} mudou durante o sync; fica para o próximo ciclo"
            );
            return Ok(skipped);
        }
        Ok(match fetched {
            Ok(snap) => {
                let applied = h.apply_github(&snap, &self.hub_id)?;
                for g in &applied.removed {
                    tracing::warn!("github: projeto {project_id}: {g}, removida do espelho");
                }
                ProjectSync {
                    project_id,
                    ok: true,
                    issues: applied.issues,
                    prs: applied.prs,
                    status_changes: applied.status_changes.len(),
                    ..ProjectSync::default()
                }
            }
            Err(e) => {
                let msg = e.to_string();
                h.record_sync_error(project_id, &msg)?;
                ProjectSync {
                    project_id,
                    ok: false,
                    error: Some(msg),
                    ..ProjectSync::default()
                }
            }
        })
    }
}

/// Nome da branch no caminho da API: `/` fica (o GitHub aceita), o resto
/// fora de `[A-Za-z0-9-._]` vai em `%XX`.
pub fn encode_branch(b: &str) -> String {
    b.bytes()
        .map(|c| match c {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'/' => {
                (c as char).to_string()
            }
            c => format!("%{c:02X}"),
        })
        .collect()
}

/// Espera até o próximo ciclo: `interval` dobrando a cada falha seguida,
/// até `MAX_BACKOFF` (ou `interval`, se for maior).
pub fn next_delay(interval: Duration, failures: u32) -> Duration {
    let cap = MAX_BACKOFF.max(interval);
    interval
        .checked_mul(1u32 << failures.min(16))
        .unwrap_or(cap)
        .min(cap)
}

/// Sync logo após o boot e depois a cada `interval`, com backoff enquanto
/// algum projeto falhar.
pub fn spawn_periodic(sync: Arc<HubGithub>, interval: Duration) {
    tokio::spawn(async move {
        let mut failures = 0u32;
        loop {
            let s = Arc::clone(&sync);
            let ok = match tokio::task::spawn_blocking(move || s.sync_all()).await {
                Ok(Ok(r)) => {
                    let changes: usize = r.iter().map(|p| p.status_changes).sum();
                    if changes > 0 {
                        tracing::info!("github: {changes} work item(s) mudaram de status");
                    }
                    for p in r.iter().filter(|p| !p.ok) {
                        tracing::warn!(
                            "github: projeto {} falhou: {}",
                            p.project_id,
                            p.error.as_deref().unwrap_or("")
                        );
                    }
                    r.iter().all(|p| p.ok)
                }
                Ok(Err(e)) => {
                    tracing::warn!("github: sync falhou: {e}");
                    false
                }
                Err(e) => {
                    tracing::warn!("github: sync falhou: {e}");
                    false
                }
            };
            failures = if ok { 0 } else { failures.saturating_add(1) };
            tokio::time::sleep(next_delay(interval, failures)).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use ct_core::{WorkItemKind, WorkItemStatus};
    use ct_gh::fake::{fake_gh, FakeGitHub};
    use ct_gh::sync::{closed_since, issues_path, pulls_path, sub_issues_path};
    use ct_gh::GhConfig;
    use ct_ledger::{Hub, Ledger, NewHubProject, NewProject};

    /// Hub com `owner/repo` cadastrado.
    fn hub_com(owner: &str, repo: &str) -> SharedHub {
        let hub = Hub::open_in_memory().unwrap();
        hub.add_project(&NewHubProject {
            owner: owner.into(),
            repo: repo.into(),
            name: repo.into(),
            base_branch: "develop".into(),
            protected_branches: vec![],
        })
        .unwrap();
        SharedHub::new(hub)
    }

    fn pr(n: i64, head: &str) -> Value {
        json!({ "number": n, "state": "open", "merged_at": null,
                "head": { "ref": head }, "base": { "ref": "develop" },
                "html_url": format!("https://github.com/x/pull/{n}"),
                "updated_at": "2026-10-15T11:00:00Z" })
    }

    /// O hub sincroniza um projeto que não tem clone em lugar nenhum desta
    /// máquina: só o que um ambiente enviou.
    #[test]
    fn hub_sincroniza_projeto_sem_clone() {
        let tmp = tempfile::tempdir().unwrap();
        let gh = FakeGitHub::start("tok");
        let bin = fake_gh(tmp.path(), "tok");
        let client = GhClient::new(
            GhConfig::new(&gh.url(), bin.to_str().unwrap())
                .with_token_timeout(ct_gh::fake::FAKE_GH_TOKEN_TIMEOUT),
        );
        let (o, r) = ("central", "so-no-hub");
        gh.set(&issues_path(o, r, "open", None), json!([]));
        gh.set(
            &issues_path(o, r, "closed", Some(&closed_since(now_ms()))),
            json!([]),
        );
        gh.set(&pulls_path(o, r, "feat/5-x"), json!([pr(40, "feat/5-x")]));

        // Um ambiente tem o projeto e envia ao hub.
        let outro = Ledger::open_in_memory().unwrap();
        let p = outro
            .add_project(&NewProject {
                name: "central".into(),
                gh_owner: o.into(),
                gh_repo: r.into(),
                repo_path: "/longe".into(),
                base_branch: "develop".into(),
                protected_branches: vec![],
            })
            .unwrap();
        let wi = outro
            .ensure_work_item(p.id, 5, WorkItemKind::Issue)
            .unwrap();
        outro
            .set_work_item_branch(wi.id, "feat/5-x", "/longe/wt")
            .unwrap();
        let hub = hub_com(o, r);
        hub.lock()
            .ingest(
                outro.env_id(),
                "outro",
                &outro.outbox_pending(1000).unwrap(),
            )
            .unwrap();

        let (events, mut rx) = broadcast::channel(16);
        let sync = HubGithub::new(hub.clone(), client, "hub-id", events);
        let res = sync.sync_all().unwrap();
        assert_eq!(res.len(), 1);
        assert!(res[0].ok && res[0].status_changes == 1, "{res:?}");
        let h = hub.lock();
        let w = h.find_work_item(o, r, 5).unwrap().unwrap();
        assert_eq!(w.status, WorkItemStatus::EmRevisao);
        assert_eq!(h.prs_for_work_item(w.id).unwrap()[0].number, 40);
        let ev = rx.try_recv().unwrap();
        assert_eq!(ev["event"], "hub_sync_status");
        assert_eq!(ev["data"]["ok"], true);
    }

    /// #89: o hub busca pelo mesmo `fetch`. O épico 1 do espelho do hub é
    /// apagado com a filha 2: a rodada não falha, os dois saem do espelho do
    /// hub e o work item da filha conclui com o motivo.
    #[test]
    fn epico_apagado_sai_do_espelho_do_hub() {
        let tmp = tempfile::tempdir().unwrap();
        let gh = FakeGitHub::start("tok");
        let bin = fake_gh(tmp.path(), "tok");
        let client = GhClient::new(
            GhConfig::new(&gh.url(), bin.to_str().unwrap())
                .with_token_timeout(ct_gh::fake::FAKE_GH_TOKEN_TIMEOUT),
        );
        let (o, r) = ("o", "r");
        let issue = |n: i64, parent: Option<i64>, subs: i64| {
            json!({ "number": n, "title": format!("Issue {n}"), "state": "open",
                    "labels": [], "html_url": format!("https://github.com/o/r/issues/{n}"),
                    "updated_at": "2026-10-14T10:00:00Z",
                    "parent_issue_url": parent.map(|p| format!("https://api.github.com/repos/o/r/issues/{p}")),
                    "sub_issues_summary": { "total": subs } })
        };
        let open = issues_path(o, r, "open", None);
        let subs = sub_issues_path(o, r, 1);
        gh.set(
            &open,
            json!([issue(1, None, 1), issue(2, Some(1), 0), issue(3, None, 0)]),
        );
        gh.set(
            &issues_path(o, r, "closed", Some(&closed_since(now_ms()))),
            json!([]),
        );
        gh.set(&subs, json!([issue(2, Some(1), 0)]));

        // Um ambiente tem o work item da filha 2 e envia ao hub.
        let outro = Ledger::open_in_memory().unwrap();
        let p = outro
            .add_project(&NewProject {
                name: "r".into(),
                gh_owner: o.into(),
                gh_repo: r.into(),
                repo_path: "/longe".into(),
                base_branch: "develop".into(),
                protected_branches: vec![],
            })
            .unwrap();
        outro
            .ensure_work_item(p.id, 2, WorkItemKind::Issue)
            .unwrap();
        let hub = hub_com(o, r);
        hub.lock()
            .ingest(outro.env_id(), "outro", &outro.outbox_pending(100).unwrap())
            .unwrap();
        let (events, _) = broadcast::channel(16);
        let sync = HubGithub::new(hub.clone(), client, "hub-id", events);
        assert!(sync.sync_all().unwrap()[0].ok);
        let pid = hub.lock().list_projects().unwrap()[0].id;
        let nums = || -> Vec<i64> {
            let h = hub.lock();
            h.list_issues(pid)
                .unwrap()
                .iter()
                .map(|i| i.gh_number)
                .collect()
        };
        assert_eq!(nums(), vec![1, 2, 3]);

        gh.set(&open, json!([issue(3, None, 0)]));
        gh.fail_next(&subs, 410, &[]);
        let res = sync.sync_all().unwrap();
        assert!(res[0].ok && res[0].status_changes == 1, "{res:?}");
        assert_eq!(nums(), vec![3]);
        let h = hub.lock();
        assert_eq!(h.sync_states().unwrap()[0].last_error, None);
        let w = h.find_work_item(o, r, 2).unwrap().unwrap();
        assert_eq!(w.status, WorkItemStatus::Concluido);
        let ev = &h.list_events(w.id, 1).unwrap()[0];
        assert_eq!(ev.payload["cause"], "issue_gone");
        assert_eq!(
            ev.payload["reason"],
            "issue apagada ou transferida no GitHub"
        );
    }

    #[test]
    fn falha_fica_no_sync_state() {
        let tmp = tempfile::tempdir().unwrap();
        let gh = FakeGitHub::start("tok");
        // `gh` falso com outro token: a API recusa.
        let bin = fake_gh(tmp.path(), "errado");
        let client = GhClient::new(
            GhConfig::new(&gh.url(), bin.to_str().unwrap())
                .with_token_timeout(ct_gh::fake::FAKE_GH_TOKEN_TIMEOUT),
        );
        let outro = Ledger::open_in_memory().unwrap();
        let p = outro
            .add_project(&NewProject {
                name: "demo".into(),
                gh_owner: "o".into(),
                gh_repo: "r".into(),
                repo_path: "/x".into(),
                base_branch: "develop".into(),
                protected_branches: vec![],
            })
            .unwrap();
        outro
            .ensure_work_item(p.id, 1, WorkItemKind::Issue)
            .unwrap();
        let hub = hub_com("o", "r");
        hub.lock()
            .ingest(outro.env_id(), "a", &outro.outbox_pending(100).unwrap())
            .unwrap();
        let (events, _) = broadcast::channel(16);
        let res = HubGithub::new(hub.clone(), client, "hub-id", events)
            .sync_all()
            .unwrap();
        assert!(!res[0].ok && res[0].error.is_some(), "{res:?}");
        let st = hub.lock().sync_states().unwrap();
        assert!(st[0].last_error.is_some());
    }

    /// Projeto Jira + GitLab: o hub não busca nada (o espelho vem do
    /// ambiente de sync) e não grava `sync_state` de erro.
    #[test]
    fn sync_do_hub_pula_projeto_jira() {
        let tmp = tempfile::tempdir().unwrap();
        let gh = FakeGitHub::start("tok");
        // Qualquer chamada falharia: o token do `gh` falso não confere.
        let bin = fake_gh(tmp.path(), "errado");
        let client = GhClient::new(GhConfig::new(&gh.url(), bin.to_str().unwrap()));
        let hub = SharedHub::new(Hub::open_in_memory().unwrap());
        hub.lock()
            .add_project_with(
                &NewHubProject {
                    owner: "grupo/projeto".into(),
                    repo: "app".into(),
                    name: "app".into(),
                    base_branch: "develop".into(),
                    protected_branches: vec![],
                },
                &ct_ledger::ProjectProvider {
                    tracker: ct_core::TrackerConfig::Jira(ct_core::JiraConfig {
                        url: "https://jira.example.com".into(),
                        project_key: "PROJ".into(),
                        credential_cmd: "pass show jira".into(),
                        active_sprint_only: true,
                        ..Default::default()
                    }),
                    forge: ct_core::ForgeConfig::Gitlab(ct_core::GitlabConfig {
                        host: "gitlab.example.com".into(),
                        path: "grupo/projeto/app".into(),
                        clone_url: "https://gitlab.example.com/grupo/projeto/app.git".into(),
                        credential_cmd: "pass show gitlab".into(),
                    }),
                    ..Default::default()
                },
            )
            .unwrap();
        let (events, _) = broadcast::channel(16);
        let res = HubGithub::new(hub.clone(), client, "hub-id", events)
            .sync_all()
            .unwrap();
        assert!(res.is_empty(), "{res:?}");
        assert!(hub.lock().sync_states().unwrap().is_empty());
    }

    #[test]
    fn valida_repositorio_e_branch_com_o_token_do_hub() {
        let tmp = tempfile::tempdir().unwrap();
        let gh = FakeGitHub::start("tok");
        let bin = fake_gh(tmp.path(), "tok");
        let client = GhClient::new(
            GhConfig::new(&gh.url(), bin.to_str().unwrap())
                .with_token_timeout(ct_gh::fake::FAKE_GH_TOKEN_TIMEOUT),
        );
        gh.set(
            "/repos/deep-ion-ai/demo",
            json!({ "name": "Demo", "owner": { "login": "Deep-Ion-AI" } }),
        );
        gh.set("/repos/Deep-Ion-AI/Demo/branches/feat/a%20b", json!({}));
        let (events, _) = broadcast::channel(4);
        let g = HubGithub::new(
            SharedHub::new(Hub::open_in_memory().unwrap()),
            client,
            "hub-id",
            events,
        );
        assert_eq!(
            g.validate_repo("deep-ion-ai", "demo").unwrap(),
            ("Deep-Ion-AI".to_string(), "Demo".to_string())
        );
        let e = g.validate_repo("x", "y").unwrap_err();
        assert!(e.contains("x/y") && e.contains("token"), "{e}");
        g.validate_branch("Deep-Ion-AI", "Demo", "feat/a b")
            .unwrap();
        let e = g
            .validate_branch("Deep-Ion-AI", "Demo", "develop")
            .unwrap_err();
        assert!(e.contains("develop"), "{e}");
        gh.fail_next("/repos/o/r", 500, &[]);
        let e = g.validate_repo("o", "r").unwrap_err();
        assert!(e.contains("não foi possível validar"), "{e}");
    }

    /// Hub com os projetos `o/a` e `o/b`, cada um com o work item #5 na
    /// branch `feat/5`, enviados por um ambiente.
    fn dois_projetos() -> SharedHub {
        let hub = hub_com("o", "a");
        hub.lock().add_project(&novo_hub("o", "b")).unwrap();
        let l = Ledger::open_in_memory().unwrap();
        for r in ["a", "b"] {
            let p = l
                .add_project(&NewProject {
                    name: r.into(),
                    gh_owner: "o".into(),
                    gh_repo: r.into(),
                    repo_path: format!("/x/{r}"),
                    base_branch: "develop".into(),
                    protected_branches: vec![],
                })
                .unwrap();
            let wi = l.ensure_work_item(p.id, 5, WorkItemKind::Issue).unwrap();
            l.set_work_item_branch(wi.id, "feat/5", "/x/wt").unwrap();
        }
        hub.lock()
            .ingest(l.env_id(), "env", &l.outbox_pending(1000).unwrap())
            .unwrap();
        hub
    }

    fn novo_hub(owner: &str, repo: &str) -> NewHubProject {
        NewHubProject {
            owner: owner.into(),
            repo: repo.into(),
            name: repo.into(),
            base_branch: "develop".into(),
            protected_branches: vec![],
        }
    }

    fn publica(gh: &FakeGitHub, r: &str) {
        gh.set(&issues_path("o", r, "open", None), json!([]));
        gh.set(
            &issues_path("o", r, "closed", Some(&closed_since(now_ms()))),
            json!([]),
        );
        gh.set(&pulls_path("o", r, "feat/5"), json!([pr(40, "feat/5")]));
    }

    /// Roda `sync_all` em outra thread e, assim que a busca de `o/a`
    /// começa, aplica `mexer` no hub.
    fn sync_com_mudanca_no_meio(
        hub: &SharedHub,
        gh: &FakeGitHub,
        client: GhClient,
        mexer: impl FnOnce(&Hub),
    ) -> Vec<ProjectSync> {
        gh.set_delay(Duration::from_millis(150));
        let (events, _) = broadcast::channel(16);
        let sync = Arc::new(HubGithub::new(hub.clone(), client, "hub-id", events));
        let s = Arc::clone(&sync);
        let t = std::thread::spawn(move || s.sync_all());
        let start = std::time::Instant::now();
        while gh.requests().is_empty() {
            assert!(start.elapsed() < Duration::from_secs(10));
            std::thread::sleep(Duration::from_millis(5));
        }
        mexer(&hub.lock());
        t.join().unwrap().expect("um projeto não aborta o ciclo")
    }

    /// Excluir e recadastrar durante a busca: o resultado antigo (com
    /// work item já purgado) é descartado e o outro projeto segue.
    #[test]
    fn projeto_recadastrado_durante_a_busca_e_descartado() {
        let tmp = tempfile::tempdir().unwrap();
        let gh = FakeGitHub::start("tok");
        let bin = fake_gh(tmp.path(), "tok");
        let client = GhClient::new(
            GhConfig::new(&gh.url(), bin.to_str().unwrap())
                .with_token_timeout(ct_gh::fake::FAKE_GH_TOKEN_TIMEOUT),
        );
        publica(&gh, "a");
        publica(&gh, "b");
        let hub = dois_projetos();
        let a = hub.lock().find_project("o", "a").unwrap().unwrap().id;
        let res = sync_com_mudanca_no_meio(&hub, &gh, client, |h| {
            h.delete_project("o", "a").unwrap();
            h.add_project(&novo_hub("o", "a")).unwrap();
        });
        assert_eq!(res.len(), 1, "{res:?}");
        assert!(res[0].ok && res[0].project_id != a, "{res:?}");
        let h = hub.lock();
        assert!(h.sync_state(a).unwrap().is_none());
        let n: i64 = h
            .conn()
            .query_row(
                "SELECT count(*) FROM pr_cache WHERE project_id = ?1",
                [a],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 0);
        let b = h.find_work_item("o", "b", 5).unwrap().unwrap();
        assert_eq!(b.status, WorkItemStatus::EmRevisao);
    }

    /// Falha na busca de um projeto excluído no meio: a lápide não ganha
    /// `sync_state` e o outro projeto segue.
    #[test]
    fn falha_de_projeto_excluido_durante_a_busca_nao_recria_sync_state() {
        let tmp = tempfile::tempdir().unwrap();
        let gh = FakeGitHub::start("tok");
        let bin = fake_gh(tmp.path(), "tok");
        let client = GhClient::new(
            GhConfig::new(&gh.url(), bin.to_str().unwrap())
                .with_token_timeout(ct_gh::fake::FAKE_GH_TOKEN_TIMEOUT),
        );
        // `o/a` sem rotas: a busca falha com 404.
        publica(&gh, "b");
        let hub = dois_projetos();
        let a = hub.lock().find_project("o", "a").unwrap().unwrap().id;
        let res = sync_com_mudanca_no_meio(&hub, &gh, client, |h| {
            h.delete_project("o", "a").unwrap();
        });
        assert_eq!(res.len(), 1, "{res:?}");
        assert!(res[0].ok, "{res:?}");
        let h = hub.lock();
        assert!(h.sync_state(a).unwrap().is_none());
        // E a lápide segue sem sync_state mesmo chamando direto.
        h.record_sync_error(a, "x").unwrap();
        assert!(h.sync_state(a).unwrap().is_none());
        assert!(h.sync_states().unwrap().iter().all(|s| s.project_id != a));
    }

    #[test]
    fn backoff_dobra_ate_o_teto() {
        let i = Duration::from_secs(180);
        assert_eq!(next_delay(i, 0), i);
        assert_eq!(next_delay(i, 1), i * 2);
        assert_eq!(next_delay(i, 20), MAX_BACKOFF);
        // Intervalo maior que o teto: nunca encurta.
        let longo = Duration::from_secs(3600);
        assert_eq!(next_delay(longo, 3), longo);
    }
}
