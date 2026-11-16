//! Secrets e variáveis entregues pelo hub ao ambiente (issue #184, épico
//! #171). O pull da configuração (`hub_config`) manda a chave pública do
//! ambiente e grava a entrega no cache; aqui ficam o acesso ao cache e a
//! rotação da chave (`ctower secrets rotate-env-key`).
//!
//! - chave: `<config>/env.key` (age X25519, 0600), gerada no primeiro pull;
//! - cache: `<data>/secrets-cache.age` (0600), o arquivo age da última
//!   entrega como veio do hub, com o `config_rev`. Hub fora do ar: vale o
//!   último cache; passado `secrets_max_age_hours`, o spawn avisa (#166);
//! - o "registro" do ambiente é só o `cthubd add-env` no hub (token): a chave
//!   pública vai ao hub no pull, que roda no boot do `ctowerd`.
//!
//! `POST /session/secret` (#165): o valor atual de um secret para a sessão
//! dona do token (`CTOWER_SESSION_TOKEN`), lido por `ctower secret get`.
//!
//! A injeção no spawn e no resume (#166) fica no `ct-sessions`
//! (`Sessions::session_secrets`), que lê o mesmo cache e resolve por projeto
//! com `EnvSecrets::resolve(Some("owner/repo"))` (projeto sobrepõe global).

use crate::hub_config::ConfigPullResult;
use crate::hub_sync::HubSender;
use crate::AppState;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use ct_secrets::{EnvCache, EnvSecrets, SecretsError};
use ct_sessions::release::ReleaseApiError;
use ct_sessions::{Dirs, SecretGetError};
use serde::{Deserialize, Serialize};

/// Chave e cache de secrets do ambiente em `dirs`.
pub fn env_cache(dirs: &Dirs) -> EnvCache {
    EnvCache::new(dirs.env_key_file(), dirs.secrets_cache_file())
}

/// Secrets entregues pelo hub, do cache local (funciona com o hub fora do
/// ar). `None` = nenhuma entrega ainda.
pub fn load(dirs: &Dirs) -> Result<Option<EnvSecrets>, SecretsError> {
    env_cache(dirs).load()
}

/// Resposta de `POST /secrets/rotate-env-key`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RotateEnvKeyResult {
    /// Chave pública nova (`age1...`).
    pub public_key: String,
    /// O cache foi recifrado para a chave nova (continua usável offline).
    pub cache_kept: bool,
    /// Pull feito logo depois, que manda a chave nova ao hub (e recebe a
    /// reentrega). `None` = ambiente não ligado a um hub.
    pub pull: Option<ConfigPullResult>,
}

impl HubSender {
    /// Troca a chave do ambiente (sem pull no meio) e puxa a configuração:
    /// a chave pública nova faz o hub reentregar os secrets. Síncrono.
    pub fn rotate_env_key(&self) -> Result<RotateEnvKeyResult, String> {
        let rotated = {
            let _one = self.pull_lock.lock().unwrap_or_else(|e| e.into_inner());
            self.env_cache.rotate_key().map_err(|e| e.to_string())?
        };
        tracing::info!("secrets: chave do ambiente trocada ({})", rotated.public);
        let pull = self.pull_config();
        Ok(RotateEnvKeyResult {
            public_key: rotated.public.to_string(),
            cache_kept: rotated.cache_kept,
            pull: Some(pull),
        })
    }
}

/// Rotação sem hub ligado: só a chave e o cache locais.
fn rotate_local(cache: &EnvCache) -> Result<RotateEnvKeyResult, String> {
    let r = cache.rotate_key().map_err(|e| e.to_string())?;
    Ok(RotateEnvKeyResult {
        public_key: r.public.to_string(),
        cache_kept: r.cache_kept,
        pull: None,
    })
}

/// `POST /secrets/rotate-env-key` (token de UI): `ctower secrets
/// rotate-env-key`.
pub(crate) async fn rotate_env_key_http(
    State(st): State<AppState>,
    headers: HeaderMap,
) -> Response {
    if !crate::bearer_ok(&headers, &st.ui_token) {
        return crate::unauthorized();
    }
    let r = match st.hub_sender.clone() {
        Some(s) => tokio::task::spawn_blocking(move || s.rotate_env_key()).await,
        None => {
            let cache = env_cache(&st.sessions.config().dirs);
            tokio::task::spawn_blocking(move || rotate_local(&cache)).await
        }
    };
    match r {
        Ok(Ok(r)) => axum::Json(r).into_response(),
        Ok(Err(e)) => (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// Corpo de `POST /session/secret`.
#[derive(Debug, Deserialize)]
pub struct SecretGetRequest {
    pub name: String,
}

/// `POST /session/secret` com `Authorization: Bearer <token da sessão>`
/// (#165). Responde o valor em texto puro (`200`), sem quebra de linha. A
/// sessão vem só do token; `X-Ctower-Session` é ignorado. Erros, sem valor:
/// `401` token ausente ou inválido, `400` nome inválido, `404` nome que não
/// vale para o projeto da sessão (inexistente, de outro projeto ou
/// reservado), `503` sem cache ou cache ilegível.
pub(crate) async fn session_secret_http(
    State(st): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let Some(token) = ct_http::bearer_token(&headers) else {
        return (StatusCode::UNAUTHORIZED, "token de sessão ausente").into_response();
    };
    let Ok(req) = serde_json::from_slice::<SecretGetRequest>(&body) else {
        return (
            StatusCode::BAD_REQUEST,
            "corpo esperado: {\"name\": \"NOME\"}",
        )
            .into_response();
    };
    let sessions = st.sessions.clone();
    let r = tokio::task::spawn_blocking(move || sessions.session_secret(&token, &req.name)).await;
    let (code, msg) = match r {
        Ok(Ok(value)) => {
            let mut resp = value.into_response();
            resp.headers_mut().insert(
                axum::http::header::CACHE_CONTROL,
                axum::http::HeaderValue::from_static("no-store"),
            );
            return resp;
        }
        Ok(Err(e)) => (
            match e {
                SecretGetError::Unauthorized => StatusCode::UNAUTHORIZED,
                SecretGetError::Invalid(_) => StatusCode::BAD_REQUEST,
                SecretGetError::NotFound(_) => StatusCode::NOT_FOUND,
                SecretGetError::Unavailable(_) => StatusCode::SERVICE_UNAVAILABLE,
            },
            e.to_string(),
        ),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    (code, msg).into_response()
}

/// Status HTTP de uma recusa das rotas da release.
fn release_error(e: ReleaseApiError) -> Response {
    let code = match e {
        ReleaseApiError::Unauthorized => StatusCode::UNAUTHORIZED,
        ReleaseApiError::Forbidden(_) => StatusCode::FORBIDDEN,
        ReleaseApiError::Invalid(_) => StatusCode::BAD_REQUEST,
        ReleaseApiError::Unavailable(_) => StatusCode::SERVICE_UNAVAILABLE,
    };
    (code, e.to_string()).into_response()
}

/// `POST /session/release/context` com `Authorization: Bearer <token da
/// sessão>` (#204): o repositório, a configuração de release do projeto e a
/// raiz da API do GitHub, em JSON. Só a sessão de release: `403` nas outras,
/// `401` token ausente ou inválido.
pub(crate) async fn release_context_http(
    State(st): State<AppState>,
    headers: HeaderMap,
) -> Response {
    let Some(token) = ct_http::bearer_token(&headers) else {
        return (StatusCode::UNAUTHORIZED, "token de sessão ausente").into_response();
    };
    let sessions = st.sessions.clone();
    let r = tokio::task::spawn_blocking(move || sessions.release_context(&token)).await;
    match r {
        Ok(Ok(ctx)) => {
            let api_url = st
                .github
                .as_ref()
                .map(|g| g.client().config().api_url.clone())
                .unwrap_or_else(|| DEFAULT_GITHUB_API.to_string());
            let mut v = serde_json::to_value(&ctx).expect("ReleaseContext serializa");
            v["api_url"] = serde_json::Value::String(api_url);
            axum::Json(v).into_response()
        }
        Ok(Err(e)) => release_error(e),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// Raiz da API do GitHub sem a integração configurada no ambiente.
const DEFAULT_GITHUB_API: &str = "https://api.github.com";

/// `POST /session/release/event` com o token da sessão (#204): grava o
/// desfecho de uma etapa (`{step, result, ...}`) como evento `release_step`
/// no ledger. Responde `{ "id": N }`.
pub(crate) async fn release_event_http(
    State(st): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let Some(token) = ct_http::bearer_token(&headers) else {
        return (StatusCode::UNAUTHORIZED, "token de sessão ausente").into_response();
    };
    let Ok(payload) = serde_json::from_slice::<serde_json::Value>(&body) else {
        return (StatusCode::BAD_REQUEST, "corpo esperado: JSON da etapa").into_response();
    };
    let sessions = st.sessions.clone();
    let r = tokio::task::spawn_blocking(move || sessions.release_event(&token, &payload)).await;
    match r {
        Ok(Ok(id)) => axum::Json(serde_json::json!({ "id": id })).into_response(),
        Ok(Err(e)) => release_error(e),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hub_sync::HubTarget;
    use crate::test_util::{serve_hub_state, DeadHub, Srv, UI};
    use ct_ledger::{Hub, SharedHub};
    use ct_secrets::{Identity, Kind, Scope, SecretValue};
    use ct_sessions::test_support::FakeClaude;
    use std::sync::Arc;
    use std::time::Duration;

    const DEMO: &str = "deep-ion-ai/demo";

    fn sender(srv: &Srv, url: &str, token: &str) -> Arc<HubSender> {
        Arc::new(HubSender::new(
            srv.env.sessions.clone(),
            HubTarget {
                url: url.to_string(),
                token: token.to_string(),
            },
            Duration::from_secs(5),
        ))
    }

    async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
        tokio::task::spawn_blocking(f).await.unwrap()
    }

    /// `(escopo, nome, valor)` do cache do ambiente.
    fn cache(srv: &Srv) -> Vec<(String, String, String)> {
        let s = load(&srv.env.sessions.config().dirs)
            .unwrap()
            .expect("sem cache");
        s.entries()
            .iter()
            .map(|e| {
                let scope = match &e.scope {
                    Scope::Global => "global".to_string(),
                    Scope::Project(k) => k.clone(),
                };
                (scope, e.name.clone(), e.value.expose().to_string())
            })
            .collect()
    }

    fn g() -> (String, String, String) {
        ("global".into(), "G".into(), "g".into())
    }

    /// Ponta a ponta com um `cthubd` de verdade: o ambiente recebe só os
    /// globais e os do projeto atribuído; o cache só abre com a chave dele;
    /// desatribuir tira os do projeto; hub fora mantém o cache; a rotação
    /// mantém o cache legível e o hub reentrega; revogar corta a entrega.
    #[tokio::test(flavor = "multi_thread")]
    async fn entrega_ponta_a_ponta() {
        let d = tempfile::tempdir().unwrap();
        let hub_dirs = cthubd::config::Dirs {
            config: d.path().join("config"),
            data: d.path().join("data"),
        };
        hub_dirs.ensure().unwrap();
        let hub = SharedHub::new(Hub::open_in_memory().unwrap());
        let tok = {
            let h = hub.lock();
            for repo in ["demo", "outro"] {
                h.add_project(&ct_ledger::NewHubProject {
                    owner: "deep-ion-ai".into(),
                    repo: repo.into(),
                    name: repo.into(),
                    base_branch: "develop".into(),
                    protected_branches: vec![],
                })
                .unwrap();
            }
            h.add_legacy_env_token("a").unwrap()
        };
        let secrets = cthubd::secrets::HubSecrets::open(&hub_dirs, hub.clone()).unwrap();
        secrets
            .set(Scope::Global, "G", Kind::Variable, SecretValue::new("g"))
            .unwrap();
        secrets
            .set(
                Scope::parse_project(DEMO).unwrap(),
                "D",
                Kind::Secret,
                SecretValue::new("d-secreto"),
            )
            .unwrap();
        secrets
            .set(
                Scope::parse_project("deep-ion-ai/outro").unwrap(),
                "O",
                Kind::Secret,
                SecretValue::new("o-secreto"),
            )
            .unwrap();
        let mut st = cthubd::HubState::new(hub.clone(), "ui-do-hub", "hub", "hub-id");
        st.secrets = Some(Arc::new(secrets));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        serve_hub_state(st, listener).await;

        let a = Srv::new(FakeClaude::exit(0));
        let dirs = a.env.sessions.config().dirs.clone();
        let env_a = a.env.sessions.ledger().env_id().to_string();
        let up = sender(&a, &url, &tok);
        assert!(load(&dirs).unwrap().is_none());
        // Primeiro pull: chave gerada e enviada; nada atribuído = só globais.
        let s = Arc::clone(&up);
        let r = blocking(move || s.pull_config()).await;
        assert!(r.ok, "{r:?}");
        assert_eq!(r.secrets, Some(1));
        assert_eq!(cache(&a), [g()]);
        let mode = |p: &std::path::Path| {
            use std::os::unix::fs::PermissionsExt;
            std::fs::metadata(p).unwrap().permissions().mode() & 0o777
        };
        assert_eq!(mode(&dirs.env_key_file()), 0o600);
        assert_eq!(mode(&dirs.secrets_cache_file()), 0o600);
        assert!(hub.lock().env_pubkey(&env_a).unwrap().is_some());

        hub.lock()
            .add_assignment(
                "deep-ion-ai",
                "demo",
                &env_a,
                Some(&a.env.repo.display().to_string()),
            )
            .unwrap();
        let s = Arc::clone(&up);
        let r = blocking(move || s.pull_config()).await;
        assert_eq!(r.secrets, Some(2), "{r:?}");
        assert_eq!(
            cache(&a),
            [g(), (DEMO.into(), "D".into(), "d-secreto".into())]
        );
        let bruto = std::fs::read(dirs.secrets_cache_file()).unwrap();
        assert!(!String::from_utf8_lossy(&bruto).contains("d-secreto"));
        assert!(!String::from_utf8_lossy(&bruto).contains("o-secreto"));
        // Só a chave do ambiente abre o cache.
        assert!(EnvSecrets::open(&bruto, &Identity::generate()).is_err());
        // Resolução por projeto (o que a injeção no spawn usa).
        let l = load(&dirs).unwrap().unwrap();
        let r = l.resolve(Some("Deep-Ion-AI/Demo"));
        assert_eq!(r.keys().collect::<Vec<_>>(), ["D", "G"]);
        assert!(!l.resolve(Some("deep-ion-ai/outro")).contains_key("O"));

        // Em dia: o hub não reentrega e o cache fica.
        let s = Arc::clone(&up);
        let r = blocking(move || s.pull_config()).await;
        assert_eq!((r.changed, r.secrets), (false, None));
        assert_eq!(cache(&a).len(), 2);

        // Desatribuir tira do cache os secrets do projeto.
        hub.lock()
            .remove_assignment("deep-ion-ai", "demo", &env_a)
            .unwrap();
        let s = Arc::clone(&up);
        let r = blocking(move || s.pull_config()).await;
        assert_eq!(r.secrets, Some(1));
        assert_eq!(cache(&a), [g()]);

        // Hub fora: o pull falha e o cache continua utilizável.
        let dead = DeadHub::new();
        let down = sender(&a, &dead.url, &tok);
        let s = Arc::clone(&down);
        let r = blocking(move || s.pull_config()).await;
        assert!(!r.ok && r.secrets.is_none());
        assert_eq!(cache(&a), [g()]);

        // Rotação offline mantém o cache; com o hub no ar, ele reentrega.
        let antiga = ct_secrets::read_identity(&dirs.env_key_file()).unwrap();
        let s = Arc::clone(&down);
        let r = blocking(move || s.rotate_env_key()).await.unwrap();
        assert!(r.cache_kept && !r.pull.unwrap().ok);
        assert_eq!(cache(&a), [g()]);
        let bruto = std::fs::read(dirs.secrets_cache_file()).unwrap();
        assert!(EnvSecrets::open(&bruto, &antiga).is_err());
        let s = Arc::clone(&up);
        let r = blocking(move || s.pull_config()).await;
        assert_eq!((r.changed, r.secrets), (true, Some(1)), "{r:?}");
        let s = Arc::clone(&up);
        let r = blocking(move || s.rotate_env_key()).await.unwrap();
        assert_eq!(r.pull.as_ref().unwrap().secrets, Some(1), "{r:?}");
        assert_eq!(
            hub.lock().env_pubkey(&env_a).unwrap().unwrap().pubkey,
            r.public_key
        );
        assert_eq!(cache(&a), [g()]);

        // Revogado: 401, nada entregue, o cache fica para uso offline.
        assert!(hub.lock().revoke_env_token("a").unwrap());
        let s = Arc::clone(&up);
        let r = blocking(move || s.pull_config()).await;
        assert!(!r.ok && r.secrets.is_none(), "{r:?}");
        assert!(hub.lock().env_pubkey(&env_a).unwrap().is_none());
        assert_eq!(cache(&a), [g()]);
    }

    /// Perda da máquina do hub (#170), ponta a ponta: secrets globais e de
    /// projeto atribuído com a recuperação configurada; o ambiente recebe;
    /// o config_dir do hub some inteiro; `secrets restore` do snapshot com a
    /// chave de recuperação; os valores voltam, o hub ganha chave nova, a
    /// recuperação fica desconfigurada, a lista a rotacionar aparece, o
    /// config_rev sobe e o ambiente recebe tudo de novo.
    #[tokio::test(flavor = "multi_thread")]
    async fn restauracao_com_a_recuperacao_reentrega_aos_ambientes() {
        use cthubd::secrets::restore::{apply_restore, open_snapshot, OpenedWith};
        let d = tempfile::tempdir().unwrap();
        let hub_dirs = cthubd::config::Dirs {
            config: d.path().join("config"),
            data: d.path().join("data"),
        };
        hub_dirs.ensure().unwrap();
        let snaps = d.path().join("OneDrive/cthub");
        let hub = SharedHub::new(Hub::open(&hub_dirs.hub_db()).unwrap());
        let tok = {
            let h = hub.lock();
            h.add_project(&ct_ledger::NewHubProject {
                owner: "deep-ion-ai".into(),
                repo: "demo".into(),
                name: "demo".into(),
                base_branch: "develop".into(),
                protected_branches: vec![],
            })
            .unwrap();
            h.add_legacy_env_token("a").unwrap()
        };
        let secrets = cthubd::secrets::HubSecrets::open(&hub_dirs, hub.clone())
            .unwrap()
            .with_snapshot_dir(snaps.clone());
        let g_rec = secrets.recovery_generate().unwrap();
        secrets.recovery_verify(&g_rec.private_key).unwrap();
        secrets
            .set(Scope::Global, "G", Kind::Variable, SecretValue::new("g"))
            .unwrap();
        secrets
            .set(
                Scope::Global,
                "API_TOKEN",
                Kind::Secret,
                SecretValue::new("token-global-123"),
            )
            .unwrap();
        secrets
            .set(
                Scope::parse_project(DEMO).unwrap(),
                "D",
                Kind::Secret,
                SecretValue::new("d-secreto"),
            )
            .unwrap();
        let antiga = ct_secrets::read_identity(&hub_dirs.secrets_key_file()).unwrap();
        let mut st = cthubd::HubState::new(hub.clone(), "ui-do-hub", "hub", "hub-id");
        st.secrets = Some(Arc::new(secrets));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        serve_hub_state(st, listener).await;

        let a = Srv::new(FakeClaude::exit(0));
        let env_a = a.env.sessions.ledger().env_id().to_string();
        let s = sender(&a, &url, &tok);
        assert!(blocking(move || s.pull_config()).await.ok);
        hub.lock()
            .add_assignment(
                "deep-ion-ai",
                "demo",
                &env_a,
                Some(&a.env.repo.display().to_string()),
            )
            .unwrap();
        let s = sender(&a, &url, &tok);
        let r = blocking(move || s.pull_config()).await;
        assert_eq!(r.secrets, Some(3), "{r:?}");
        let esperado = vec![
            (
                "global".to_string(),
                "API_TOKEN".to_string(),
                "token-global-123".to_string(),
            ),
            g(),
            (DEMO.into(), "D".into(), "d-secreto".into()),
        ];
        assert_eq!(cache(&a), esperado);
        let rev_antes = hub.lock().config_rev().unwrap();
        // O snapshot mais recente na pasta sincronizada é o store atual.
        let store = std::fs::read(hub_dirs.secrets_file()).unwrap();
        let snap = std::fs::read_dir(&snaps)
            .unwrap()
            .map(|e| e.unwrap().path())
            .find(|p| std::fs::read(p).unwrap() == store)
            .unwrap();

        // A máquina do hub se perde: o config_dir some inteiro (o hub.db
        // volta à parte, com `cthubd restore`; aqui ele fica).
        std::fs::remove_dir_all(&hub_dirs.config).unwrap();
        let rec = ct_secrets::Zeroizing::new(g_rec.private_key.clone());
        let opened = open_snapshot(&hub_dirs, &snap, || Ok(rec.clone())).unwrap();
        assert_eq!(opened.opened_with(), OpenedWith::RecoveryKey);
        let rep = apply_restore(&hub_dirs, opened, &snaps).unwrap();
        assert!(rep.config_rev > rev_antes);
        assert_eq!(rep.entries, 3);
        let nova = ct_secrets::read_identity(&hub_dirs.secrets_key_file()).unwrap();
        assert_ne!(nova.to_public(), antiga.to_public());
        let names: Vec<_> = rep.rotate_pending.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, ["API_TOKEN", "D"]);

        // O serviço sobe de novo com o store restaurado.
        let hub2 = SharedHub::new(Hub::open(&hub_dirs.hub_db()).unwrap());
        let secrets2 = cthubd::secrets::HubSecrets::open(&hub_dirs, hub2.clone()).unwrap();
        assert!(!secrets2.recovery_status().configured);
        assert_eq!(
            secrets2.resolve(Some(DEMO))["D"].value.expose(),
            "d-secreto"
        );
        assert_eq!(
            secrets2.resolve(None)["API_TOKEN"].value.expose(),
            "token-global-123"
        );
        assert_eq!(
            secrets2.list().unwrap().rotate_pending.unwrap().items.len(),
            2
        );
        let mut st2 = cthubd::HubState::new(hub2.clone(), "ui-do-hub", "hub", "hub-id");
        st2.secrets = Some(Arc::new(secrets2));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url2 = format!("http://{}", listener.local_addr().unwrap());
        serve_hub_state(st2, listener).await;

        // O ambiente recebe a reentrega com os mesmos valores.
        let s = sender(&a, &url2, &tok);
        let r = blocking(move || s.pull_config()).await;
        assert!(r.ok && r.changed, "{r:?}");
        assert_eq!(r.secrets, Some(3), "{r:?}");
        assert_eq!(cache(&a), esperado);
        let s = sender(&a, &url2, &tok);
        let r = blocking(move || s.pull_config()).await;
        assert_eq!((r.changed, r.secrets), (false, None));
    }

    /// `POST /session/secret` direto no router: (status, corpo em texto).
    async fn get_secret(
        srv: &Srv,
        token: Option<&str>,
        header_session: Option<&str>,
        body: &str,
    ) -> (StatusCode, String) {
        use axum::body::Body;
        use http_body_util::BodyExt;
        use tower::ServiceExt;
        let mut req =
            axum::http::Request::post("/session/secret").header("content-type", "application/json");
        if let Some(t) = token {
            req = req.header("authorization", format!("Bearer {t}"));
        }
        if let Some(s) = header_session {
            req = req.header("x-ctower-session", s);
        }
        let resp = crate::router(srv.state.clone())
            .oneshot(req.body(Body::from(body.to_string())).unwrap())
            .await
            .unwrap();
        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        (status, String::from_utf8_lossy(&bytes).to_string())
    }

    /// Token da sessão, do registro do `claude` falso.
    fn token_of(srv: &Srv, session_id: &str) -> String {
        srv.env
            .wait_fake_output(session_id)
            .lines()
            .find_map(|l| l.strip_prefix("CTOWER_SESSION_TOKEN="))
            .expect("sessão sem token")
            .to_string()
    }

    fn name(n: &str) -> String {
        serde_json::json!({ "name": n }).to_string()
    }

    /// A rota identifica a sessão só pelo token: trocar `X-Ctower-Session`
    /// não dá acesso ao escopo de outra sessão.
    #[tokio::test(flavor = "multi_thread")]
    async fn rota_do_valor_atual_pelo_token_da_sessao() {
        let srv = Srv::new(FakeClaude::sleep(30));
        let projeto = srv.env.sessions.start(srv.env.request(41, "Rota")).unwrap();
        let gestor = srv
            .env
            .sessions
            .start_gestor(
                &ct_sessions::GestorScope::Portfolio,
                "",
                None,
                &ct_sessions::AgentOptions::default(),
            )
            .unwrap();
        let (tp, tg) = (token_of(&srv, &projeto.id), token_of(&srv, &gestor.id));
        // Sem cache ainda: 503, sem valor.
        let (s, b) = get_secret(&srv, Some(&tp), None, &name("API_JWT")).await;
        assert_eq!(s, StatusCode::SERVICE_UNAVAILABLE, "{b}");
        srv.env.write_secrets_cache(&[
            (None, "API_JWT", "jwt-global-aaaaaaaa"),
            (Some(DEMO), "API_JWT", "jwt-do-projeto-bbbbbbbb"),
            (Some("acme/acme-web"), "SO_OUTRO", "valor-de-outro-projeto"),
        ]);
        let (s, b) = get_secret(&srv, Some(&tp), None, &name("API_JWT")).await;
        assert_eq!((s, b.as_str()), (StatusCode::OK, "jwt-do-projeto-bbbbbbbb"));
        // O gestor, mesmo com o cabeçalho da sessão de projeto, fica com o
        // escopo dele (só globais).
        let (s, b) = get_secret(&srv, Some(&tg), Some(&projeto.id), &name("API_JWT")).await;
        assert_eq!((s, b.as_str()), (StatusCode::OK, "jwt-global-aaaaaaaa"));
        // Token ausente, de UI, de hook ou inventado: 401.
        for t in [None, Some(UI), Some(crate::test_util::HOOK), Some("x")] {
            let (s, b) = get_secret(&srv, t, Some(&projeto.id), &name("API_JWT")).await;
            assert_eq!(s, StatusCode::UNAUTHORIZED, "{t:?}");
            assert!(!b.contains("jwt-"), "{b}");
        }
        // Outro projeto, inexistente e reservado: 404, sem valor.
        for n in ["SO_OUTRO", "NAO_EXISTE", "CTOWER_TOKEN"] {
            let (s, b) = get_secret(&srv, Some(&tp), None, &name(n)).await;
            assert_eq!(s, StatusCode::NOT_FOUND, "{n}: {b}");
            assert!(!b.contains("valor") && !b.contains("jwt-"), "{b}");
        }
        let (s, _) = get_secret(&srv, Some(&tp), None, "nada").await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        srv.env.sessions.kill(&projeto.id).unwrap();
        srv.env.sessions.kill(&gestor.id).unwrap();
    }

    /// `POST` direto no router com o token da sessão: (status, corpo).
    async fn post_release(
        srv: &Srv,
        path: &str,
        token: Option<&str>,
        body: &str,
    ) -> (StatusCode, String) {
        use axum::body::Body;
        use http_body_util::BodyExt;
        use tower::ServiceExt;
        let mut req = axum::http::Request::post(path).header("content-type", "application/json");
        if let Some(t) = token {
            req = req.header("authorization", format!("Bearer {t}"));
        }
        let resp = crate::router(srv.state.clone())
            .oneshot(req.body(Body::from(body.to_string())).unwrap())
            .await
            .unwrap();
        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        (status, String::from_utf8_lossy(&bytes).to_string())
    }

    /// Rotas do `ctower release` (#204): só a sessão de release fala com
    /// elas; a de issue do mesmo projeto recebe `403` e não lê o token
    /// exclusivo pelo `/session/secret`.
    #[tokio::test(flavor = "multi_thread")]
    async fn rotas_da_release_so_para_a_sessao_de_release() {
        let srv = Srv::new(FakeClaude::sleep(30));
        srv.env.assign_config(1, |p| {
            p.release = ct_core::ReleaseConfig {
                enabled: true,
                ..Default::default()
            };
        });
        srv.env
            .write_secrets_cache(&[(Some(DEMO), "CT_RELEASE_GH_TOKEN", "tok-release-12345678")]);
        let rel = srv
            .env
            .sessions
            .start_release(
                srv.env.project_id,
                "Release",
                None,
                &ct_sessions::AgentOptions::default(),
            )
            .unwrap();
        let issue = srv
            .env
            .sessions
            .start(srv.env.request(42, "Issue"))
            .unwrap();
        let (tr, ti) = (token_of(&srv, &rel.id), token_of(&srv, &issue.id));

        let (s, b) = post_release(&srv, "/session/release/context", Some(&tr), "").await;
        assert_eq!(s, StatusCode::OK, "{b}");
        let v: serde_json::Value = serde_json::from_str(&b).unwrap();
        assert_eq!(v["repo"], "demo");
        assert_eq!(v["release"]["token_secret"], "CT_RELEASE_GH_TOKEN");
        assert_eq!(v["api_url"], "https://api.github.com");
        assert!(!b.contains("tok-release"), "{b}");

        let ev = r#"{"step":"promote","result":"refused","message":"nada a lançar"}"#;
        let (s, b) = post_release(&srv, "/session/release/event", Some(&tr), ev).await;
        assert_eq!(s, StatusCode::OK, "{b}");
        let (s, _) = post_release(&srv, "/session/release/event", Some(&tr), "{}").await;
        assert_eq!(s, StatusCode::BAD_REQUEST);

        for path in ["/session/release/context", "/session/release/event"] {
            let (s, _) = post_release(&srv, path, Some(&ti), ev).await;
            assert_eq!(s, StatusCode::FORBIDDEN, "{path}");
            let (s, _) = post_release(&srv, path, None, ev).await;
            assert_eq!(s, StatusCode::UNAUTHORIZED, "{path}");
        }
        let (s, b) = get_secret(&srv, Some(&ti), None, &name("CT_RELEASE_GH_TOKEN")).await;
        assert_eq!(s, StatusCode::NOT_FOUND, "{b}");
        let (s, b) = get_secret(&srv, Some(&tr), None, &name("CT_RELEASE_GH_TOKEN")).await;
        assert_eq!((s, b.as_str()), (StatusCode::OK, "tok-release-12345678"));
        srv.env.sessions.kill(&rel.id).unwrap();
        srv.env.sessions.kill(&issue.id).unwrap();
    }

    /// `/ws` do hub com o token de UI: uma chamada e a resposta.
    async fn hub_ws_call(url: &str, method: &str, params: serde_json::Value) -> serde_json::Value {
        use futures_util::{SinkExt, StreamExt};
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;
        use tokio_tungstenite::tungstenite::Message;
        let mut req = format!("{}/ws", url.replacen("http://", "ws://", 1))
            .into_client_request()
            .unwrap();
        req.headers_mut()
            .insert("authorization", "Bearer ui-do-hub".parse().unwrap());
        let (mut ws, _) = tokio_tungstenite::connect_async(req).await.unwrap();
        let msg = serde_json::json!({ "id": 7, "method": method, "params": params });
        ws.send(Message::Text(msg.to_string().into()))
            .await
            .unwrap();
        loop {
            let Some(Ok(Message::Text(t))) = ws.next().await else {
                panic!("hub fechou o /ws");
            };
            let v: serde_json::Value = serde_json::from_str(&t).unwrap();
            if v["id"] == 7 {
                return v;
            }
        }
    }

    /// Critério da #165, ponta a ponta com o `cthubd` de verdade: com a
    /// sessão aberta, trocar o secret no hub faz o valor novo chegar ao
    /// `POST /session/secret` em poucos segundos (o hub avisa pelo canal e
    /// o pull roda na hora, sem esperar o intervalo de 1 h do teste), e o
    /// valor novo sai mascarado no PTY, junto com o antigo.
    #[tokio::test(flavor = "multi_thread")]
    async fn secret_renovado_no_hub_chega_a_sessao_aberta() {
        let d = tempfile::tempdir().unwrap();
        let hub_dirs = cthubd::config::Dirs {
            config: d.path().join("config"),
            data: d.path().join("data"),
        };
        hub_dirs.ensure().unwrap();
        let hub = SharedHub::new(Hub::open_in_memory().unwrap());
        hub.lock()
            .add_project(&ct_ledger::NewHubProject {
                owner: "deep-ion-ai".into(),
                repo: "demo".into(),
                name: "demo".into(),
                base_branch: "develop".into(),
                protected_branches: vec![],
            })
            .unwrap();
        let secrets = cthubd::secrets::HubSecrets::open(&hub_dirs, hub.clone()).unwrap();
        secrets
            .set(
                Scope::parse_project(DEMO).unwrap(),
                "API_JWT",
                Kind::Secret,
                SecretValue::new("jwt-antigo-aaaaaaaa"),
            )
            .unwrap();
        let mut st = cthubd::HubState::new(hub.clone(), "ui-do-hub", "hub", "hub-id");
        st.secrets = Some(Arc::new(secrets));
        let channels = Arc::clone(&st.channels);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        serve_hub_state(st, listener).await;

        let mut a = Srv::new(FakeClaude::script(
            "while read l; do echo \"saida=$l\"; done",
        ));
        let env_a = a.env.sessions.ledger().env_id().to_string();
        let tok = hub
            .lock()
            .issue_env_token_with_env_id("a", &env_a)
            .unwrap()
            .token;
        hub.lock()
            .add_assignment(
                "deep-ion-ai",
                "demo",
                &env_a,
                Some(&a.env.repo.display().to_string()),
            )
            .unwrap();
        let up = Arc::new(
            HubSender::new(
                a.env.sessions.clone(),
                HubTarget {
                    url: url.clone(),
                    token: tok,
                },
                Duration::from_secs(5),
            )
            .with_config(crate::hub_config::ConfigOptions {
                pull_every: Duration::from_secs(3600),
                ..Default::default()
            }),
        );
        a.state.hub_sender = Some(Arc::clone(&up));
        // Primeiro pull feito à mão: cache com o valor antigo.
        let s = Arc::clone(&up);
        assert_eq!(blocking(move || s.pull_config()).await.secrets, Some(1));
        let sess = a.env.sessions.start(a.env.request(42, "JWT")).unwrap();
        let token = token_of(&a, &sess.id);
        let (s, b) = get_secret(&a, Some(&token), None, &name("API_JWT")).await;
        assert_eq!((s, b.as_str()), (StatusCode::OK, "jwt-antigo-aaaaaaaa"));

        // Laço do pull (o do boot já passou: o próximo seria em 1 h) e canal.
        crate::hub_config::spawn_puller(Arc::clone(&up));
        let canal = crate::hub_channel::spawn(
            a.state.clone(),
            crate::hub_channel::ChannelOptions {
                base_delay: Duration::from_millis(20),
                max_backoff: Duration::from_millis(200),
                idle_timeout: Duration::from_secs(5),
            },
        )
        .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while !channels.is_online(&env_a) {
            assert!(std::time::Instant::now() < deadline, "canal não abriu");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        // Deixa o pull do boot do laço terminar antes de trocar o valor.
        tokio::time::sleep(Duration::from_millis(300)).await;

        let r = hub_ws_call(
            &url,
            "hub.secrets.set",
            serde_json::json!({ "scope": { "project": DEMO }, "name": "API_JWT",
                                "kind": "secret", "value": "jwt-renovado-bbbbbbbb" }),
        )
        .await;
        assert!(r["result"]["rev"].is_i64(), "{r}");
        let t0 = std::time::Instant::now();
        loop {
            let (s, b) = get_secret(&a, Some(&token), None, &name("API_JWT")).await;
            assert_eq!(s, StatusCode::OK, "{b}");
            if b == "jwt-renovado-bbbbbbbb" {
                break;
            }
            assert!(
                t0.elapsed() < Duration::from_secs(5),
                "valor novo não chegou"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        // O valor novo e o antigo saem mascarados na saída do PTY.
        let pty = a.env.sessions.pty(&sess.id).unwrap();
        pty.write(b"jwt-renovado-bbbbbbbb jwt-antigo-aaaaaaaa\n")
            .unwrap();
        let t0 = std::time::Instant::now();
        let out = loop {
            let out = String::from_utf8_lossy(&pty.snapshot()).to_string();
            if out.contains("saida=***API_JWT*** ***API_JWT***") {
                break out;
            }
            assert!(t0.elapsed() < Duration::from_secs(5), "{out}");
            tokio::time::sleep(Duration::from_millis(20)).await;
        };
        assert!(
            !out.contains("jwt-renovado") && !out.contains("jwt-antigo"),
            "{out}"
        );
        canal.abort();
        a.env.sessions.kill(&sess.id).unwrap();
    }

    /// `POST /secrets/rotate-env-key`: token de UI; sem hub, só local.
    #[tokio::test(flavor = "multi_thread")]
    async fn rota_de_rotacao() {
        let srv = Srv::new(FakeClaude::exit(0));
        let dirs = srv.env.sessions.config().dirs.clone();
        let antes = env_cache(&dirs).public_key().unwrap();
        let (s, _) = srv
            .post(
                "/secrets/rotate-env-key",
                Some("x"),
                &[],
                serde_json::json!({}),
            )
            .await;
        assert_eq!(s, StatusCode::UNAUTHORIZED);
        let (s, v) = srv
            .post(
                "/secrets/rotate-env-key",
                Some(UI),
                &[],
                serde_json::json!({}),
            )
            .await;
        assert_eq!(s, StatusCode::OK, "{v}");
        assert!(v["pull"].is_null());
        assert_eq!(v["cache_kept"], false);
        let depois = env_cache(&dirs).public_key().unwrap();
        assert_ne!(antes, depois);
        assert_eq!(v["public_key"], depois.to_string());
    }
}
