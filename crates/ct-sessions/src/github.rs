//! Sincronização com o GitHub (spec §7.5): busca sem o lock do ledger,
//! aplica numa transação e avisa a UI das mudanças.

use crate::{Result, Sessions, SessionsError};
use ct_core::now_ms;
use ct_gh::sync::{fetch, SyncInput};
use ct_gh::{GhClient, GhError};
use ct_ledger::{PrRow, Project};
use serde::Serialize;
use serde_json::json;
use std::sync::Mutex;
use std::time::Duration;

fn gh_error(e: GhError) -> SessionsError {
    SessionsError::Invalid(format!("GitHub: {e}"))
}

/// Teto do backoff entre ciclos com falha (spec §7.5).
pub const MAX_BACKOFF: Duration = Duration::from_secs(15 * 60);

#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct ProjectSync {
    pub project_id: i64,
    pub ok: bool,
    pub error: Option<String>,
    pub issues: usize,
    pub prs: usize,
    pub status_changes: usize,
}

pub struct GithubSync {
    sessions: Sessions,
    client: GhClient,
    /// Uma sincronização por vez (laço periódico × `sync.now`).
    running: Mutex<()>,
}

impl GithubSync {
    pub fn new(sessions: Sessions, client: GhClient) -> Self {
        GithubSync {
            sessions,
            client,
            running: Mutex::new(()),
        }
    }

    /// Cliente do GitHub (token, status da conta).
    pub fn client(&self) -> &GhClient {
        &self.client
    }

    /// Head do PR (#286): o do espelho, gravado pelo sync com ETag. O PR
    /// que entrou no espelho antes da coluna `head_sha` (já concluído, o
    /// sync não o consulta mais) é buscado uma vez na API e gravado.
    /// `None` = o PR não existe ou o token não tem acesso.
    pub fn pr_head_sha(&self, project: &Project, pr: &PrRow) -> Result<Option<String>> {
        if let Some(sha) = &pr.head_sha {
            return Ok(Some(sha.clone()));
        }
        let sha = ct_gh::refs::pull_head_sha(
            &self.client,
            &project.gh_owner,
            &project.gh_repo,
            pr.number,
        )
        .map_err(gh_error)?;
        if let Some(s) = &sha {
            self.sessions
                .ledger()
                .set_pr_head_sha(project.id, pr.number, s)?;
        }
        Ok(sha)
    }

    /// Ponta da branch no GitHub agora; `None` = ela não existe.
    pub fn remote_branch_sha(&self, project: &Project, branch: &str) -> Result<Option<String>> {
        ct_gh::refs::branch_head_sha(&self.client, &project.gh_owner, &project.gh_repo, branch)
            .map_err(gh_error)
    }

    /// Apaga a branch no GitHub; `false` = já não existia.
    pub fn delete_remote_branch(&self, project: &Project, branch: &str) -> Result<bool> {
        ct_gh::refs::delete_branch(&self.client, &project.gh_owner, &project.gh_repo, branch)
            .map_err(gh_error)
    }

    /// Todos os projetos. A falha de um não impede os outros: fica em
    /// `sync_state.last_error` e no resultado.
    pub fn sync_all(&self) -> Result<Vec<ProjectSync>> {
        let _one = self.running.lock().unwrap_or_else(|e| e.into_inner());
        let ids: Vec<i64> = self
            .sessions
            .ledger()
            .list_projects()?
            .iter()
            // O projeto Jira é do `JiraGitlabSync` (spec do Jira e GitLab §5.2).
            .filter(|p| p.tracker.kind() == ct_core::TrackerKind::Github)
            .map(|p| p.id)
            .collect();
        ids.into_iter().map(|id| self.sync_one(id)).collect()
    }

    pub fn sync_project(&self, project_id: i64) -> Result<ProjectSync> {
        let _one = self.running.lock().unwrap_or_else(|e| e.into_inner());
        self.sync_one(project_id)
    }

    fn sync_one(&self, project_id: i64) -> Result<ProjectSync> {
        // O espelho do GitHub do ambiente é só dos projetos atribuídos (spec
        // do hub como plano de controle §4.2).
        if !self.sessions.ledger().project_active(project_id)? {
            return Ok(ProjectSync {
                project_id,
                ok: false,
                error: Some("projeto não atribuído a este ambiente".into()),
                issues: 0,
                prs: 0,
                status_changes: 0,
            });
        }
        if self
            .sessions
            .ledger()
            .get_project(project_id)?
            .tracker
            .kind()
            != ct_core::TrackerKind::Github
        {
            return Ok(ProjectSync {
                project_id,
                ok: false,
                error: Some("projeto Jira: as issues vêm do sync do jira-cli".into()),
                issues: 0,
                prs: 0,
                status_changes: 0,
            });
        }
        // Lock curto para ler; a rede roda sem ele.
        let input = SyncInput::load(&self.sessions.ledger(), project_id, now_ms())?;
        let result = match fetch(&self.client, &input) {
            Ok(snap) => {
                let applied = self.sessions.ledger().apply_github(&snap)?;
                for (wi, status) in &applied.status_changes {
                    self.sessions.notify(
                        "work_item_status",
                        json!({ "work_item_id": wi, "status": status }),
                    );
                }
                ProjectSync {
                    project_id,
                    ok: true,
                    error: None,
                    issues: applied.issues,
                    prs: applied.prs,
                    status_changes: applied.status_changes.len(),
                }
            }
            Err(e) => {
                let msg = e.to_string();
                self.sessions.ledger().record_sync_error(project_id, &msg)?;
                ProjectSync {
                    project_id,
                    ok: false,
                    error: Some(msg),
                    issues: 0,
                    prs: 0,
                    status_changes: 0,
                }
            }
        };
        let last_ok_at = self
            .sessions
            .ledger()
            .sync_state(project_id)?
            .and_then(|s| s.last_ok_at);
        self.sessions.notify(
            "sync_status",
            json!({ "project_id": project_id, "ok": result.ok,
                    "last_ok_at": last_ok_at, "error": result.error }),
        );
        Ok(result)
    }
}

/// Espera até o próximo ciclo: o intervalo, dobrando a cada ciclo com
/// falha, até `MAX_BACKOFF` (ou o próprio intervalo, se for maior).
pub fn next_delay(interval: Duration, failures: u32) -> Duration {
    let cap = MAX_BACKOFF.max(interval);
    interval
        .checked_mul(1u32 << failures.min(16))
        .unwrap_or(cap)
        .min(cap)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{FakeClaude, TestEnv};
    use ct_core::{WorkItemKind, WorkItemStatus};
    use ct_gh::fake::{fake_gh, FakeGitHub};
    use ct_gh::sync::{closed_since, issues_path, pulls_path};
    use ct_gh::GhConfig;
    use ct_ledger::NewProject;
    use serde_json::Value;
    use std::sync::Arc;
    use std::time::Instant;

    fn setup() -> (TestEnv, FakeGitHub, Arc<GithubSync>) {
        let env = TestEnv::new(FakeClaude::exit(0));
        let gh = FakeGitHub::start("tok");
        let bin = fake_gh(env.tmp.path(), "tok");
        let client = GhClient::new(
            GhConfig::new(&gh.url(), bin.to_str().unwrap())
                .with_token_timeout(ct_gh::fake::FAKE_GH_TOKEN_TIMEOUT),
        );
        let (o, r) = ("deep-ion-ai", "demo");
        gh.set(&issues_path(o, r, "open", None), json!([]));
        gh.set(
            &issues_path(o, r, "closed", Some(&closed_since(now_ms()))),
            json!([]),
        );
        let sync = Arc::new(GithubSync::new(env.sessions.clone(), client));
        (env, gh, sync)
    }

    fn pr(n: i64, head: &str) -> Value {
        json!({ "number": n, "state": "open", "merged_at": null,
                "head": { "ref": head }, "base": { "ref": "develop" },
                "html_url": format!("https://github.com/x/pull/{n}"),
                "updated_at": "2026-10-15T11:00:00Z" })
    }

    fn work_item_with_branch(env: &TestEnv, n: i64, branch: &str) -> i64 {
        let l = env.sessions.ledger();
        let wi = l
            .ensure_work_item(env.project_id, n, WorkItemKind::Issue)
            .unwrap();
        l.set_work_item_branch(wi.id, branch, "/wt").unwrap();
        wi.id
    }

    #[test]
    fn falha_de_um_projeto_nao_impede_os_outros() {
        let (env, gh, sync) = setup();
        let wi = work_item_with_branch(&env, 7, "feat/7-x");
        gh.set(
            &pulls_path("deep-ion-ai", "demo", "feat/7-x"),
            json!([pr(30, "feat/7-x")]),
        );
        let outro = env
            .sessions
            .ledger()
            .add_project(&NewProject {
                name: "zz-outro".into(),
                gh_owner: "o".into(),
                gh_repo: "sem-rotas".into(),
                repo_path: "/x".into(),
                base_branch: "develop".into(),
                protected_branches: vec![],
            })
            .unwrap();

        let r = sync.sync_all().unwrap();
        assert_eq!(r.len(), 2);
        assert!(r[0].ok && r[0].status_changes == 1, "{r:?}");
        assert!(!r[1].ok && r[1].error.as_deref().unwrap().contains("404"));
        let states = env.sessions.ledger().sync_states().unwrap();
        assert!(states[0].last_ok_at.is_some());
        assert_eq!(states[1].project_id, outro.id);
        assert!(states[1].last_ok_at.is_none() && states[1].last_error.is_some());
        assert_eq!(
            env.sessions.ledger().get_work_item(wi).unwrap().status,
            WorkItemStatus::EmRevisao
        );

        let notices = env.notices();
        assert!(notices.iter().any(|(n, d)| n == "work_item_status"
            && d["work_item_id"] == wi
            && d["status"] == "em_revisao"));
        let status: Vec<&Value> = notices
            .iter()
            .filter(|(n, _)| n == "sync_status")
            .map(|(_, d)| d)
            .collect();
        assert_eq!(status.len(), 2);
        assert_eq!(status[0]["ok"], true);
        assert!(status[0]["last_ok_at"].is_i64());
        assert_eq!(status[1]["ok"], false);

        // Falhas seguidas do projeto que falha (#90): contam a cada rodada,
        // com o início da sequência; o que deu certo fica em zero.
        assert_eq!((states[0].failures, states[0].failing_since), (0, None));
        assert_eq!(states[1].failures, 1);
        let desde = states[1].failing_since.unwrap();
        sync.sync_all().unwrap();
        let st = env.sessions.ledger().sync_state(outro.id).unwrap().unwrap();
        assert_eq!((st.failures, st.failing_since), (2, Some(desde)));
    }

    /// O espelho do GitHub do ambiente só sincroniza projetos atribuídos.
    #[test]
    fn projeto_desatribuido_fica_fora_do_espelho() {
        let (env, _gh, sync) = setup();
        env.sessions
            .ledger()
            .apply_hub_config(
                &ct_ledger::ConfigResponse {
                    config_rev: 1,
                    config_epoch: 1,
                    changed: true,
                    projects: Some(vec![]),
                    removed: Some(vec![]),
                },
                |_| String::new(),
            )
            .unwrap();
        assert!(sync.sync_all().unwrap().is_empty());
        let r = sync.sync_project(env.project_id).unwrap();
        assert!(!r.ok && r.error.as_deref().unwrap().contains("não atribuído"));
        assert!(env.sessions.ledger().sync_states().unwrap().is_empty());
    }

    /// O projeto Jira é do `JiraGitlabSync`: o laço do GitHub não o busca
    /// nem grava o `sync_state` dele (#349).
    #[test]
    fn laco_do_github_nao_toca_projeto_jira() {
        let (env, gh, sync) = setup();
        env.sessions
            .ledger()
            .apply_hub_config(
                &ct_ledger::ConfigResponse {
                    config_rev: 1,
                    config_epoch: 1,
                    changed: true,
                    projects: Some(vec![ct_ledger::ConfigProject {
                        docs: Default::default(),
                        owner: "deep-ion-ai".into(),
                        repo: "demo".into(),
                        name: "demo".into(),
                        base_branch: "develop".into(),
                        protected_branches: vec![],
                        repo_path: String::new(),
                        rev: 1,
                        tracker: ct_core::TrackerConfig::Jira(ct_core::JiraConfig {
                            project_key: "PROJ".into(),
                            ..Default::default()
                        }),
                        forge: ct_core::ForgeConfig::Github,
                        branch_pattern: None,
                        attribution: true,
                        sync: true,
                        rules: Default::default(),
                    }]),
                    removed: Some(vec![]),
                },
                |_| "/x".into(),
            )
            .unwrap();
        gh.clear_requests();
        assert!(sync.sync_all().unwrap().is_empty());
        let r = sync.sync_project(env.project_id).unwrap();
        assert!(!r.ok && r.error.as_deref().unwrap().contains("projeto Jira"));
        assert!(gh.requests().is_empty(), "{:?}", gh.requests());
        assert!(env.sessions.ledger().sync_states().unwrap().is_empty());
    }

    #[test]
    fn rede_lenta_nao_segura_o_ledger() {
        let (env, gh, sync) = setup();
        gh.set_delay(Duration::from_millis(800));
        let s = Arc::clone(&sync);
        let t = std::thread::spawn(move || s.sync_all().unwrap());
        std::thread::sleep(Duration::from_millis(300));
        let start = Instant::now();
        drop(env.sessions.ledger().list_projects().unwrap());
        assert!(
            start.elapsed() < Duration::from_millis(200),
            "ledger travado por {:?}",
            start.elapsed()
        );
        assert!(t.join().unwrap()[0].ok);
    }

    #[test]
    fn syncs_concorrentes_aplicam_uma_vez() {
        let (env, gh, sync) = setup();
        let wi = work_item_with_branch(&env, 7, "feat/7-x");
        gh.set(
            &pulls_path("deep-ion-ai", "demo", "feat/7-x"),
            json!([pr(30, "feat/7-x")]),
        );
        let threads: Vec<_> = (0..2)
            .map(|_| {
                let s = Arc::clone(&sync);
                std::thread::spawn(move || s.sync_all().unwrap())
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        let changed = env
            .sessions
            .ledger()
            .list_events(wi, 50)
            .unwrap()
            .iter()
            .filter(|e| e.kind == "status_changed")
            .count();
        assert_eq!(changed, 1);
    }

    #[test]
    fn backoff_dobra_ate_o_teto() {
        let i = Duration::from_secs(180);
        assert_eq!(next_delay(i, 0), i);
        assert_eq!(next_delay(i, 1), Duration::from_secs(360));
        assert_eq!(next_delay(i, 2), Duration::from_secs(720));
        assert_eq!(next_delay(i, 3), MAX_BACKOFF);
        assert_eq!(next_delay(i, 40), MAX_BACKOFF);
        // Intervalo maior que o teto: não encurta.
        let h = Duration::from_secs(3600);
        assert_eq!(next_delay(h, 5), h);
    }
}
