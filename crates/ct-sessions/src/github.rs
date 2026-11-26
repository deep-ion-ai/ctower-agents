//! Sincronização com o GitHub (spec §7.5): busca sem o lock do ledger,
//! aplica numa transação e avisa a UI das mudanças.
//!
//! O token de cada projeto vem do resolvedor de credencial do ambiente, com
//! a fonte da conexão `github` do projeto (#275, [`Sessions::github_source`]):
//! o mesmo cache, o mesmo descarte no `401` e o mesmo mascaramento das
//! sessões que o Jira e o GitLab.

use crate::resolver::{github_key, Ask, GITHUB_KEY_PREFIX};
use crate::{Result, Sessions, SessionsError};
use ct_core::now_ms;
use ct_core::provider::CredentialSource;
use ct_gh::sync::{fetch, SyncInput};
use ct_gh::{GhClient, GhError, TokenProvider, TokenStatus};
use ct_ledger::{PrRow, Project};
use ct_proc::CREDENTIAL_DEADLINE;
use serde::Serialize;
use serde_json::json;
use std::sync::{Arc, Mutex};
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

    /// Cliente base do GitHub (a API e o `gh` configurados). O token de um
    /// projeto sai de [`Self::client_for`].
    pub fn client(&self) -> &GhClient {
        &self.client
    }

    /// Cliente com o token do projeto (#275): o resolvedor do ambiente com a
    /// fonte da conexão `github` dele.
    pub fn client_for(&self, project: &Project) -> GhClient {
        self.client.with_provider(Arc::new(EnvToken {
            sessions: self.sessions.clone(),
            project: project.clone(),
            cli: self.client.config().cli_cmd(),
            deadline: self.deadline(),
        }))
    }

    /// Prazo da fonte do token: o do `gh auth token` do cliente ou o do
    /// comando de credencial, o maior.
    fn deadline(&self) -> Duration {
        self.client.config().token_timeout.max(CREDENTIAL_DEADLINE)
    }

    /// Situação do token do GitHub do ambiente (`github.status`), sem o
    /// valor: a origem e a conta. Vale a fonte sem projeto: o token do
    /// ambiente ou a conexão `github` padrão, com os secrets globais.
    pub fn token_status(&self) -> TokenStatus {
        let env = self.sessions.github_env_source(None);
        let source = match (&env.source, env.from_env) {
            (_, true) => "token",
            (CredentialSource::Cli, false) => "gh",
            (_, false) => "conexao",
        };
        let cache = self.sessions.env_cache();
        let lookup = |name: &str| {
            let secrets = cache.load().ok().flatten()?;
            let v = secrets
                .resolve(None)
                .get(name)
                .map(|e| e.value.expose().to_string());
            v
        };
        let cli = self.client.config().cli_cmd();
        let token = self
            .sessions
            .credentials()
            .token(Ask {
                key: &format!("{GITHUB_KEY_PREFIX}ambiente"),
                what: "GitHub",
                source: &env.source,
                lookup: &lookup,
                deadline: self.deadline(),
                cli: Some(&cli),
            })
            .map_err(|e| GhError::Token(e.to_string()));
        match token.and_then(|t| self.client.login_of(t.expose())) {
            Ok(login) => TokenStatus {
                source: source.into(),
                login: Some(login),
                error: None,
            },
            Err(e) => TokenStatus {
                source: if matches!(e, GhError::Token(_)) {
                    "none".into()
                } else {
                    source.into()
                },
                login: None,
                error: Some(e.to_string()),
            },
        }
    }

    /// Valida `token` no GitHub e, se valer, grava como token do ambiente
    /// (`github.token`, 0600), que passa a valer no lugar da conexão neste
    /// ambiente. Caminho da tela de configurações do ambiente até a #276.
    pub fn set_env_token(&self, token: &str) -> std::result::Result<TokenStatus, GhError> {
        let token = token.trim();
        if token.is_empty() || token.contains(char::is_whitespace) {
            return Err(GhError::Token("token vazio ou com espaços".into()));
        }
        self.client.login_of(token)?;
        let file = self.sessions.config().dirs.github_token_file();
        ct_core::files::write_private_atomic(&file, token.as_bytes())
            .map_err(|e| GhError::Token(e.to_string()))?;
        // Mesmo arquivo, outro token: a fonte não muda, o cache sai.
        self.sessions
            .credentials()
            .invalidate_prefix(GITHUB_KEY_PREFIX);
        Ok(self.token_status())
    }

    /// Apaga o token do ambiente: volta a valer a conexão `github`.
    pub fn clear_env_token(&self) -> std::result::Result<TokenStatus, GhError> {
        let file = self.sessions.config().dirs.github_token_file();
        match std::fs::remove_file(&file) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(GhError::Token(e.to_string())),
        }
        self.sessions
            .credentials()
            .invalidate_prefix(GITHUB_KEY_PREFIX);
        Ok(self.token_status())
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
            &self.client_for(project),
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
        ct_gh::refs::branch_head_sha(
            &self.client_for(project),
            &project.gh_owner,
            &project.gh_repo,
            branch,
        )
        .map_err(gh_error)
    }

    /// Apaga a branch no GitHub; `false` = já não existia.
    pub fn delete_remote_branch(&self, project: &Project, branch: &str) -> Result<bool> {
        ct_gh::refs::delete_branch(
            &self.client_for(project),
            &project.gh_owner,
            &project.gh_repo,
            branch,
        )
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
        let client = self.client_for(&input.project);
        let result = match fetch(&client, &input) {
            Ok(mut snap) => {
                // O login do token é o "eu" do projeto (#149); sem ele, fica
                // o último conhecido.
                snap.me = client.login().ok();
                let applied = self.sessions.ledger().apply_github(&snap)?;
                for g in &applied.removed {
                    tracing::warn!("github: projeto {project_id}: {g}, removida do espelho");
                }
                for (wi, status) in &applied.status_changes {
                    self.sessions.notify(
                        "work_item_status",
                        json!({ "work_item_id": wi, "status": status }),
                    );
                }
                self.sync_reviews(&client, &input.project)?;
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

impl GithubSync {
    /// Feedback de review dos PRs abertos dos work items (#130): só os que
    /// mudaram (`updated_at`) desde a última leitura, sem o lock do ledger
    /// durante a rede. A falha no GitHub não derruba o sync: fica no log, e
    /// o que faltou é lido no próximo ciclo.
    fn sync_reviews(&self, client: &GhClient, project: &Project) -> Result<()> {
        let targets = self.sessions.ledger().pr_review_targets(project.id)?;
        if targets.is_empty() {
            return Ok(());
        }
        let (got, err) =
            ct_gh::reviews::fetch_reviews(client, &project.gh_owner, &project.gh_repo, &targets);
        self.sessions.ledger().apply_pr_reviews(project.id, &got)?;
        if let Some(e) = err {
            tracing::warn!(
                "github: projeto {}: reviews dos PRs não lidas: {e}",
                project.id
            );
        }
        Ok(())
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

/// Token do GitHub de um projeto pelo resolvedor do ambiente (#275): a
/// chave `github:<projeto>`, a fonte de [`Sessions::github_source`] e os
/// secrets do projeto no cache do ambiente. Cada token novo atualiza o
/// mascaramento das sessões vivas.
struct EnvToken {
    sessions: Sessions,
    project: Project,
    cli: String,
    deadline: Duration,
}

impl TokenProvider for EnvToken {
    fn token(&self) -> std::result::Result<String, GhError> {
        let source = self
            .sessions
            .github_source(&self.project)
            .map_or(CredentialSource::None, |s| s.source);
        let lookup = self.sessions.secret_lookup(&self.project);
        self.sessions
            .credentials()
            .token(Ask {
                key: &github_key(self.project.id),
                what: "GitHub",
                source: &source,
                lookup: &lookup,
                deadline: self.deadline,
                cli: Some(&self.cli),
            })
            .map(|t| t.expose().to_string())
            .map_err(|e| GhError::Token(e.to_string()))
    }

    fn invalidate(&self) {
        self.sessions
            .credentials()
            .invalidate(&github_key(self.project.id));
    }
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

    /// #275: o sync do ambiente usa o token da conexão `github` padrão (a
    /// do pull); trocar a credencial dela vale no ciclo seguinte, sem o
    /// `gh` do ambiente, e o token resolvido entra no mascaramento das
    /// sessões do projeto. O token do ambiente (`github.token`, até a #276)
    /// vale no lugar da conexão.
    #[test]
    fn trocar_a_credencial_da_conexao_github_vale_no_sync_do_ambiente() {
        use ct_core::provider::{Connection, ProviderKind};
        use ct_gh::fake::fake_gh_calls;
        let env = TestEnv::new(FakeClaude::exit(0));
        let gh = FakeGitHub::start("ghp_ficticio_novo_123456");
        let bin = fake_gh(env.tmp.path(), "ghp_ficticio_do_gh");
        let client = GhClient::new(
            GhConfig::new(&gh.url(), bin.to_str().unwrap())
                .with_token_timeout(ct_gh::fake::FAKE_GH_TOKEN_TIMEOUT),
        );
        let (o, r) = ("acme", "acme-web");
        gh.set(&issues_path(o, r, "open", None), json!([]));
        gh.set(
            &issues_path(o, r, "closed", Some(&closed_since(now_ms()))),
            json!([]),
        );
        let p = env
            .sessions
            .ledger()
            .add_project(&NewProject {
                name: "acme-web".into(),
                gh_owner: o.into(),
                gh_repo: r.into(),
                repo_path: "/x".into(),
                base_branch: "develop".into(),
                protected_branches: vec![],
            })
            .unwrap();
        let conexao = |token: &str| Connection {
            id: 1,
            name: "github".into(),
            kind: ProviderKind::Github,
            endpoint: String::new(),
            credential: CredentialSource::Command {
                cmd: format!("printf '{token}\\n'"),
            },
            options: json!({}),
            created_at: 0,
            updated_at: 0,
        };
        let creds = &env.cfg.credentials;
        creds.set_connections(
            vec![conexao("ghp_ficticio_velho_123456")],
            Default::default(),
        );
        let sync = GithubSync::new(env.sessions.clone(), client);
        let r1 = sync.sync_project(p.id).unwrap();
        assert!(!r1.ok, "{r1:?}");
        assert!(r1.error.as_deref().unwrap().contains("401"), "{r1:?}");

        creds.set_connections(
            vec![conexao("ghp_ficticio_novo_123456")],
            Default::default(),
        );
        let r2 = sync.sync_project(p.id).unwrap();
        assert!(r2.ok, "{r2:?}");
        assert_eq!(fake_gh_calls(env.tmp.path()), 0, "o gh não é consultado");
        let mask = env.sessions.connection_mask(&p);
        assert_eq!(
            mask.pty.apply(b"t=ghp_ficticio_novo_123456"),
            b"t=***CONEXAO_TOKEN***"
        );

        // O token do ambiente vale no lugar da conexão até a #276.
        let file = env.sessions.config().dirs.github_token_file();
        std::fs::write(&file, "ghp_ficticio_velho_123456\n").unwrap();
        let r3 = sync.sync_project(p.id).unwrap();
        assert!(!r3.ok, "{r3:?}");
        std::fs::remove_file(&file).unwrap();
        assert!(sync.sync_project(p.id).unwrap().ok);
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

    /// #130: o sync do ambiente lê as threads de review do PR aberto do
    /// work item: a resolvida fica de fora, a não resolvida vem com arquivo
    /// e linha, a de autor externo vem marcada; sem mudança no PR, não relê.
    #[test]
    fn threads_de_review_no_espelho_do_ambiente() {
        let (env, gh, sync) = setup();
        let (o, r) = ("acme", "acme-web");
        let p = env
            .sessions
            .ledger()
            .add_project(&NewProject {
                name: "acme-web".into(),
                gh_owner: o.into(),
                gh_repo: r.into(),
                repo_path: "/srv/acme-web".into(),
                base_branch: "develop".into(),
                protected_branches: vec![],
            })
            .unwrap();
        gh.set(&issues_path(o, r, "open", None), json!([]));
        gh.set(
            &issues_path(o, r, "closed", Some(&closed_since(now_ms()))),
            json!([]),
        );
        let wi = {
            let l = env.sessions.ledger();
            let wi = l.ensure_work_item(p.id, 7, WorkItemKind::Issue).unwrap();
            l.set_work_item_branch(wi.id, "feat/7-x", "/wt").unwrap();
            wi.id
        };
        gh.set(&pulls_path(o, r, "feat/7-x"), json!([pr(30, "feat/7-x")]));
        let thread = |id: &str, resolved: bool, path: &str, line: i64, assoc: &str| {
            json!({
                "id": id, "isResolved": resolved, "isOutdated": false,
                "path": path, "line": line, "originalLine": line,
                "comments": { "totalCount": 2, "nodes": [{
                    "author": { "login": "fulano" }, "authorAssociation": assoc,
                    "body": "renomeie a função",
                    "url": format!("https://github.com/acme/acme-web/pull/30#discussion_{id}"),
                    "createdAt": "2026-10-15T10:00:00Z",
                }]},
            })
        };
        gh.set_graphql(
            json!({ "owner": o, "repo": r, "number": 30 }),
            json!({ "data": { "repository": { "pullRequest": {
                "reviewThreads": {
                    "pageInfo": { "hasNextPage": false, "endCursor": null },
                    "nodes": [
                        thread("T1", true, "src/lib.rs", 3, "MEMBER"),
                        thread("T2", false, "src/main.rs", 21, "COLLABORATOR"),
                        thread("T3", false, "docs/x.md", 4, "FIRST_TIME_CONTRIBUTOR"),
                    ],
                },
                "latestOpinionatedReviews": { "nodes": [{
                    "id": "R1", "state": "CHANGES_REQUESTED",
                    "author": { "login": "beltrano" }, "authorAssociation": "OWNER",
                    "body": "faltou teste",
                    "url": "https://github.com/acme/acme-web/pull/30#pullrequestreview-1",
                    "submittedAt": "2026-10-15T10:30:00Z",
                }]},
            }}}}),
        );

        let res = sync.sync_project(p.id).unwrap();
        assert!(res.ok, "{res:?}");
        let f = env.sessions.ledger().pr_feedback_for_work_item(wi).unwrap();
        assert_eq!(f.len(), 1);
        let threads: Vec<(&str, &str, Option<i64>, bool, i64)> = f[0]
            .threads
            .iter()
            .map(|t| {
                (
                    t.id.as_str(),
                    t.path.as_str(),
                    t.line,
                    t.external,
                    t.replies,
                )
            })
            .collect();
        assert_eq!(
            threads,
            [
                ("T2", "src/main.rs", Some(21), false, 1),
                ("T3", "docs/x.md", Some(4), true, 1)
            ]
        );
        assert_eq!(f[0].changes_requested.len(), 1);
        assert_eq!(f[0].changes_requested[0].author, "beltrano");
        assert_eq!(gh.graphql_calls().len(), 1);

        // Nada mudou no PR: nenhuma consulta GraphQL nova.
        assert!(sync.sync_project(p.id).unwrap().ok);
        assert_eq!(gh.graphql_calls().len(), 1);
    }

    /// Falha no GraphQL não derruba o sync; o PR fica para o próximo ciclo.
    #[test]
    fn falha_nas_reviews_nao_derruba_o_sync() {
        let (env, gh, sync) = setup();
        let wi = work_item_with_branch(&env, 7, "feat/7-x");
        gh.set(
            &pulls_path("deep-ion-ai", "demo", "feat/7-x"),
            json!([pr(30, "feat/7-x")]),
        );
        gh.fail_next("/graphql", 502, &[]);
        assert!(sync.sync_project(env.project_id).unwrap().ok);
        let l = env.sessions.ledger();
        assert_eq!(
            l.pr_feedback_for_work_item(wi).unwrap()[0].read_at_updated,
            None
        );
        assert_eq!(l.pr_review_targets(env.project_id).unwrap().len(), 1);
    }

    /// O espelho do GitHub do ambiente só sincroniza projetos atribuídos.
    #[test]
    fn projeto_desatribuido_fica_fora_do_espelho() {
        let (env, _gh, sync) = setup();
        env.sessions
            .ledger()
            .apply_hub_config(
                &ct_ledger::ConfigResponse {
                    connections: None,
                    connection_names: None,
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
                    connections: None,
                    connection_names: None,
                    config_rev: 1,
                    config_epoch: 1,
                    changed: true,
                    projects: Some(vec![ct_ledger::ConfigProject {
                        docs: Default::default(),
                        session_templates: Default::default(),
                        autonomy: Default::default(),
                        autonomy_runner: false,
                        approval: Default::default(),
                        release: Default::default(),
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
