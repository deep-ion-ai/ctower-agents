//! Servidor HTTP do daemon (spec §8, §9, §11): `/hooks`, `/hooks/permission` e `/mcp` com o token
//! de hook, `/ws`, `/hub/sync`, `/cleanup/plan`, `/cleanup/run` e
//! `/secrets/rotate-env-key` com o token de UI,
//! `/healthz` sem autenticação. Com hub ligado, o daemon também mantém o
//! canal reverso com ele (`hub_channel`, `/sync/channel`). O hub é outro
//! binário (`cthubd`); a base HTTP/WS comum aos dois fica em `ct-http`.

pub mod api;
pub mod checkout;
pub mod cleanup;
pub mod env_config;
pub mod env_secrets;
mod hooks;
pub mod hub_channel;
pub mod hub_config;
pub mod hub_sync;
mod mcp;
mod mcp_gestor;
pub mod permissions;
pub mod update;
pub mod ws;

use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use ct_sessions::{GithubSync, JiraGitlabSync, Notifier, Sessions};
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::sync::broadcast;

pub(crate) use ct_http::{bearer_ok, unauthorized};

/// Versão do protocolo UI ⇄ daemon (`env.info`). 2.0: saíram o papel de hub
/// (`env.info.hub`) e os métodos `hub.*`, exceto `hub.sync.now`. 2.2 (#328):
/// `session.permission.watch|list|respond`, `session.context`, `transcript.tail`,
/// `recovery.archived`, `env.config.get|set`, `env.rollback`,
/// `github.pr.comment`, `review.mark_reviewed { decision_ids }` e
/// `parent_number` no dashboard. 2.3 (#197): `remote_control` em
/// `session.start|start_project|start_gestor|resume`,
/// `session.remote_control.enable`, `Session.rc_session_id`,
/// `EnvInfo.remote_control` e o push `session_remote_control`. 2.4 (épico
/// #353, Jira e GitLab): `tracker`, `forge`, `branch_pattern`, `attribution`
/// e `sync_role` em `Project`, `ref` no `ledger_get_context`, as regras do
/// agente e as sprints do espelho. 2.5 (#457): status `aguardando_epico` do
/// work item (sub-issue à espera do merge do épico). 2.6 (#440): `docs` em
/// `Project` e `EnvInfo.docs` (`root`, `reachable`). 2.7 (#441):
/// `EnvInfo.docs.warning` (`docs_unreachable`) e o alerta `docs_unreachable`
/// da sessão. 2.8 (#444): `docs.dir` ("Abrir pasta de docs" do projeto e do
/// épico). 2.9 (épico #429, console): `shell.open|list|close` (#428), o id do
/// shell em `session.attach|detach|input|resize` e nos frames binários, o
/// push `shell_exited` e `events.list` (#422) — na branch do épico eram 2.5 e
/// 2.6, renumeradas acima das da develop.
pub const PROTOCOL_VERSION: &str = "2.9";

#[derive(Clone)]
pub struct AppState {
    pub sessions: Sessions,
    pub ui_token: Arc<str>,
    pub hook_token: Arc<str>,
    /// Push para as UIs conectadas: `{ "event": ..., "data": ... }`.
    pub events: broadcast::Sender<Value>,
    /// Sync com o GitHub (`sync.now`); `None` = desativado.
    pub github: Option<Arc<GithubSync>>,
    /// Sync dos projetos Jira pelo jira-cli (`sync.now`, `env.info.trackers`);
    /// `None` = desativado.
    pub tracker: Option<Arc<JiraGitlabSync>>,
    /// Nome legível deste ambiente (`env_name`).
    pub env_name: Arc<str>,
    /// Envio deste ambiente ao hub; `None` = não envia.
    pub hub_sender: Option<Arc<hub_sync::HubSender>>,
    /// Atualização do ambiente (`env.check_update`); `None` = desativada.
    pub updates: Option<Arc<update::Updates>>,
    /// Limpeza após a conclusão do work item (`cleanup.*`); `None` =
    /// desativada.
    pub cleanup: Option<Arc<cleanup::Cleanup>>,
    /// Chaves do `config.toml` editáveis pela UI (`env.config.*`); `None` =
    /// desativado.
    pub env_config: Option<Arc<env_config::EnvConfig>>,
    /// Pedidos de permissão esperando a resposta da UI.
    pub permissions: permissions::Shared,
}

impl AppState {
    /// Push para as UIs conectadas (sem UI conectada, nada a fazer).
    pub fn events_send(&self, event: &str, data: Value) {
        let _ = self.events.send(json!({ "event": event, "data": data }));
    }
}

/// Notificador de `ct-sessions` que publica no canal de push das UIs.
pub fn notifier(events: broadcast::Sender<Value>) -> Notifier {
    Arc::new(move |event: &str, data: Value| {
        // Sem UI conectada não há receptor: tudo bem.
        let _ = events.send(json!({ "event": event, "data": data }));
    })
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/hooks", post(hooks::handle))
        .route("/hooks/permission", post(permissions::handle))
        .route("/mcp", post(mcp::handle).get(mcp::method_not_allowed))
        .route("/ws", get(ws::upgrade))
        .route("/status", get(status))
        .route("/hub/sync", post(hub_sync::sync_now_http))
        .route("/cleanup/plan", post(cleanup::plan_http))
        .route("/cleanup/run", post(cleanup::run_http))
        .route(
            "/secrets/rotate-env-key",
            post(env_secrets::rotate_env_key_http),
        )
        .with_state(state)
}

async fn healthz() -> impl IntoResponse {
    axum::Json(json!({
        "ok": true,
        "version": ct_core::VERSION,
        "commit": ct_core::BUILD_COMMIT,
        "protocol": PROTOCOL_VERSION,
    }))
}

/// Resumo para `ctower status`/`ctower restart` (token de UI).
async fn status(
    axum::extract::State(st): axum::extract::State<AppState>,
    headers: HeaderMap,
) -> Response {
    if !bearer_ok(&headers, &st.ui_token) {
        return unauthorized();
    }
    let sessions = st.sessions.clone();
    let r = tokio::task::spawn_blocking(move || {
        let l = sessions.ledger();
        let live = l.live_sessions()?;
        let interrupted = ct_sessions::recoverable_sessions(&l)?.len();
        Ok::<_, ct_sessions::SessionsError>(json!({
            "version": env!("CARGO_PKG_VERSION"),
            "live_sessions": live,
            "interrupted": interrupted,
        }))
    })
    .await;
    match r {
        Ok(Ok(v)) => axum::Json(v).into_response(),
        Ok(Err(e)) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

#[cfg(test)]
pub(crate) mod test_util {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use ct_sessions::test_support::{FakeClaude, TestEnv};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    pub const UI: &str = "ui-secreto";
    pub const HOOK: &str = "hook-secreto";

    /// `cthubd` (o hub de verdade) servindo `hub` em `listener`.
    pub async fn serve_hub(hub: ct_ledger::SharedHub, listener: tokio::net::TcpListener) {
        serve_hub_state(
            cthubd::HubState::new(hub, "ui-do-hub", "hub", "hub-id"),
            listener,
        )
        .await;
    }

    /// Como `serve_hub`, com o estado montado pelo teste (ex.: com o store
    /// de secrets).
    pub async fn serve_hub_state(state: cthubd::HubState, listener: tokio::net::TcpListener) {
        let app = cthubd::router(state);
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
    }

    /// Hub fora do ar sem corrida de porta: o teste segura o listener, que
    /// aceita e derruba cada conexão (o cliente vê "hub inacessível").
    /// `revive` devolve o mesmo listener para um hub de verdade subir nele.
    pub struct DeadHub {
        pub url: String,
        stop: Arc<std::sync::atomic::AtomicBool>,
        handle: Option<std::thread::JoinHandle<std::net::TcpListener>>,
    }

    impl DeadHub {
        pub fn new() -> Self {
            use std::sync::atomic::Ordering;
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            l.set_nonblocking(true).unwrap();
            let url = format!("http://{}", l.local_addr().unwrap());
            let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let flag = Arc::clone(&stop);
            let handle = std::thread::spawn(move || {
                while !flag.load(Ordering::SeqCst) {
                    match l.accept() {
                        Ok((c, _)) => drop(c),
                        Err(_) => std::thread::sleep(std::time::Duration::from_millis(5)),
                    }
                }
                l
            });
            DeadHub {
                url,
                stop,
                handle: Some(handle),
            }
        }

        pub fn revive(mut self) -> tokio::net::TcpListener {
            self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
            let l = self.handle.take().unwrap().join().unwrap();
            tokio::net::TcpListener::from_std(l).unwrap()
        }
    }

    impl Drop for DeadHub {
        fn drop(&mut self) {
            self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
        }
    }

    pub struct Srv {
        pub env: TestEnv,
        pub state: AppState,
    }

    impl Srv {
        pub fn new(fake: FakeClaude) -> Self {
            let (events, _) = broadcast::channel(256);
            let env = TestEnv::with_notifier(fake, notifier(events.clone()));
            let state = AppState {
                sessions: env.sessions.clone(),
                ui_token: UI.into(),
                hook_token: HOOK.into(),
                events,
                github: None,
                tracker: None,
                env_name: "teste".into(),
                hub_sender: None,
                updates: None,
                cleanup: None,
                env_config: None,
                permissions: Default::default(),
            };
            Srv { env, state }
        }

        /// Como `new`, ligado a um hub (`hub_url` e `hub.token`), que pode
        /// iniciar sessões. Nenhum laço de envio ou pull roda: o teste não
        /// fala com o hub, e os projetos do `TestEnv` são locais legados
        /// (valem até o primeiro pull).
        pub fn linked(fake: FakeClaude) -> Self {
            let mut srv = Self::new(fake);
            srv.link();
            srv
        }

        pub fn link(&mut self) {
            self.state.hub_sender = Some(Arc::new(hub_sync::HubSender::new(
                self.env.sessions.clone(),
                hub_sync::HubTarget {
                    url: "http://127.0.0.1:9".into(),
                    token: "sync".into(),
                },
                std::time::Duration::from_secs(1),
            )));
        }

        /// Como `new`, com o sync apontando para um GitHub falso (token "tok")
        /// que já responde vazio às listagens de issues do projeto "demo".
        pub fn with_github(fake: FakeClaude) -> (Self, ct_gh::fake::FakeGitHub) {
            use ct_gh::sync::{closed_since, issues_path};
            let mut srv = Self::new(fake);
            let gh = ct_gh::fake::FakeGitHub::start("tok");
            let bin = ct_gh::fake::fake_gh(srv.env.tmp.path(), "tok");
            let client = ct_gh::GhClient::new(
                ct_gh::GhConfig::new(&gh.url(), bin.to_str().unwrap())
                    .with_token_timeout(ct_gh::fake::FAKE_GH_TOKEN_TIMEOUT)
                    .with_token_file(srv.env.tmp.path().join("github.token")),
            );
            gh.set(&issues_path("deep-ion-ai", "demo", "open", None), json!([]));
            gh.set(
                &issues_path(
                    "deep-ion-ai",
                    "demo",
                    "closed",
                    Some(&closed_since(ct_core::now_ms())),
                ),
                json!([]),
            );
            srv.state.github = Some(Arc::new(GithubSync::new(srv.env.sessions.clone(), client)));
            (srv, gh)
        }

        pub async fn post(
            &self,
            path: &str,
            token: Option<&str>,
            extra: &[(&str, &str)],
            body: Value,
        ) -> (StatusCode, Value) {
            let mut req = Request::post(path).header("content-type", "application/json");
            if let Some(t) = token {
                req = req.header("authorization", format!("Bearer {t}"));
            }
            for (k, v) in extra {
                req = req.header(*k, *v);
            }
            let resp = router(self.state.clone())
                .oneshot(req.body(Body::from(body.to_string())).unwrap())
                .await
                .unwrap();
            let status = resp.status();
            let bytes = resp.into_body().collect().await.unwrap().to_bytes();
            let v = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
            (status, v)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_util::*;
    use super::*;
    use ct_sessions::test_support::FakeClaude;

    #[tokio::test]
    async fn status_exige_token_de_ui() {
        use axum::body::Body;
        use axum::http::Request;
        use tower::ServiceExt;
        let srv = Srv::new(FakeClaude::sleep(30));
        let s = srv.env.sessions.start(srv.env.request(1, "A")).unwrap();
        let req = |t: &str| {
            Request::get("/status")
                .header("authorization", format!("Bearer {t}"))
                .body(Body::empty())
                .unwrap()
        };
        let r = router(srv.state.clone()).oneshot(req(HOOK)).await.unwrap();
        assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
        let r = router(srv.state.clone()).oneshot(req(UI)).await.unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        use http_body_util::BodyExt;
        let v: Value =
            serde_json::from_slice(&r.into_body().collect().await.unwrap().to_bytes()).unwrap();
        assert_eq!(v["live_sessions"][0]["id"], s.id.as_str());
        srv.env.sessions.kill(&s.id).unwrap();
    }

    #[tokio::test]
    async fn healthz_sem_token() {
        use axum::body::Body;
        use axum::http::Request;
        use tower::ServiceExt;
        let srv = Srv::new(FakeClaude::exit(0));
        let r = router(srv.state.clone())
            .oneshot(Request::get("/healthz").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
    }
}
