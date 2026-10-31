//! UI web servida pelo hub (#199; spec da PWA §3, §4 e §8): o bundle
//! embutido (`/`, `/assets/*`, fallback SPA), o login (`/web/login`,
//! `/web/logout`, `/web/me`, #200) e o `/web/ws`, o mesmo JSON-RPC do `/ws`
//! com a lista de permissões [`WEB_METHODS`] aplicada antes do despacho.
//!
//! Só existe com `[web] enabled = true`; desligado, essas rotas respondem
//! `404`. A sessão web (cookie `ct_web`, validade, bloqueio, proxy) fica em
//! [`crate::web_auth`]. Com proxy configurado, toda rota web (inclusive o
//! bundle) exige a identidade dele; o `/web/ws` exige também o cookie.

use crate::web_auth::{Denied, LoginError, WebAuth};
use crate::HubState;
use axum::extract::ws::WebSocketUpgrade;
use axum::extract::{ConnectInfo, FromRequestParts, State};
use axum::http::request::Parts;
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::sync::Arc;

/// Bundle de `ui/dist-web` ou a página de aviso (veja o `build.rs`).
#[derive(rust_embed::Embed)]
#[folder = "$CTHUB_WEB_DIR"]
struct Bundle;

/// O binário foi compilado com o bundle web (e não com a página de aviso).
pub fn bundle_present() -> bool {
    env!("CTHUB_WEB_BUNDLE") == "1"
}

/// Métodos do `/ws` atendidos pelo `/web/ws` (spec da PWA §4.1). Método
/// novo de leitura entra quando a UI web o usa; método que escreve só com
/// decisão registrada.
pub const WEB_METHODS: &[&str] = &[
    "hub.info",
    "hub.status",
    "hub.projects.list",
    "hub.assignments.list",
    "hub.quarantine.list",
    "hub.tree.get",
    "hub.workitem.history",
    // "Ver issue" (#434): só leitura, de passagem, sem gravar.
    "hub.workitem.body",
    "hub.workitem.set_status",
    "hub.workitem.set_priority",
    "hub.ranking.get",
    "hub.health.get",
    "hub.proposals.list",
    "hub.proposals.decide",
    "hub.dashboard.get",
    "hub.envs.list",
    // Regras do agente (épico #353): só a leitura, que a UI web mostra no
    // projeto e no épico; editar, importar e decidir propostas ficam fora.
    "hub.rules.get",
    "hub.rules.history",
    "hub.rules.proposals.list",
    // Revogar só reduz acesso (spec §3.1 e §4.2).
    "hub.web.sessions.list",
    "hub.web.sessions.revoke",
    // Web Push (#203, spec §4.2): inscrever e escolher os eventos do próprio
    // dispositivo; listar, testar e remover não abrem nada além disso.
    "hub.push.vapid_key",
    "hub.push.subscribe",
    "hub.push.list",
    "hub.push.update",
    "hub.push.remove",
    "hub.push.test",
    // Fase 2 (#343, spec §2.2 e §5.4): ações do ambiente pelo canal reverso,
    // só com os métodos de `ct_http::sync::CHANNEL_METHODS` (sem terminal,
    // permissões, atualização, limpeza nem token do GitHub).
    "hub.env.call",
    // Atualização do hub (#421): só o estado; verificar, disparar e voltar
    // ficam no app desktop, como as demais ações de manutenção.
    "hub.update.get",
    // Tela Logs (#65): só leitura do log do hub, já mascarado; o modo seguir
    // vale só para a própria conexão.
    "hub.logs.tail",
    "hub.logs.follow",
    "hub.logs.unfollow",
];

/// Sensíveis da decisão do épico #205: nunca na web, mesmo com sessão válida.
pub const WEB_SENSITIVE: &[&str] = &[
    "hub.projects.delete",
    "hub.secrets.set",
    "hub.secrets.delete",
    "hub.recovery.generate",
    "hub.recovery.verify",
    "github.set_token",
    "github.clear_token",
];

/// Os demais métodos do `/ws` fora da web (escolhas da spec, §13 item 2).
pub const WEB_REFUSED: &[&str] = &[
    "hub.projects.add",
    "hub.projects.update",
    "hub.projects.impact",
    "hub.projects.move",
    "hub.assignments.add",
    "hub.assignments.remove",
    "hub.quarantine.discard",
    "hub.workitem.move",
    "hub.rules.set",
    "hub.rules.import",
    "hub.rules.proposals.decide",
    "hub.secrets.list",
    "hub.secrets.rotate_pending_dismiss",
    "hub.recovery.status",
    "hub.backup.get",
    "hub.backup.set",
    "hub.backup.test",
    "hub.backup.run",
    "hub.backup.remote.list",
    "github.status",
    "hub.update.check",
    "hub.update.start",
    "hub.update.rollback",
];

/// Pushes do hub repassados ao navegador (spec da PWA §4.3). `env.push` já
/// chega filtrado pela lista do canal (`ct_http::sync::CHANNEL_PUSHES`).
pub const WEB_PUSHES: &[&str] = &[
    "hub_sync_status",
    "hub.proposals.changed",
    "hub.rules.changed",
    "env.online",
    "env.push",
    "hub.update.status",
];

/// Erro de método fora da lista do `/web/ws`.
pub const WEB_METHOD_UNAVAILABLE: i64 = -32601;

pub fn web_allowed(method: &str) -> bool {
    WEB_METHODS.contains(&method)
}

pub(crate) fn web_push(v: &Value) -> bool {
    v.get("event")
        .and_then(Value::as_str)
        .is_some_and(|e| WEB_PUSHES.contains(&e))
}

/// Endereço TCP de quem conectou, quando o servidor guarda o
/// `ConnectInfo` (o `ct_http::serve` guarda); `None` = desconhecido (vale
/// como fora do loopback).
#[derive(Debug, Clone, Copy)]
pub struct Peer(pub Option<SocketAddr>);

impl<S: Send + Sync> FromRequestParts<S> for Peer {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Self::Rejection> {
        Ok(Peer(
            parts
                .extensions
                .get::<ConnectInfo<SocketAddr>>()
                .map(|c| c.0),
        ))
    }
}

/// Configuração das rotas web no `HubState`.
#[derive(Clone)]
pub struct WebOptions {
    pub enabled: bool,
    /// `Origin` aceitos; vazio = `http://127.0.0.1:<port>` e `http://localhost:<port>`.
    pub origins: Vec<String>,
    /// Porta do hub, para as origens padrão.
    pub port: u16,
    pub auth: Arc<WebAuth>,
}

impl WebOptions {
    /// Rotas web do `[web]` do config.
    pub fn new(
        cfg: &crate::config::WebConfig,
        port: u16,
        hub: ct_ledger::SharedHub,
        ui_token: &str,
    ) -> Self {
        WebOptions {
            enabled: cfg.enabled,
            origins: cfg.origins.clone(),
            port,
            auth: Arc::new(WebAuth::new(hub, ui_token, cfg)),
        }
    }

    /// Rotas web desligadas (o padrão do `[web]`).
    pub fn disabled(hub: ct_ledger::SharedHub, ui_token: &str) -> Self {
        Self::new(
            &crate::config::WebConfig::default(),
            crate::config::DEFAULT_PORT,
            hub,
            ui_token,
        )
    }

    /// `Origin` aceitos no upgrade do `/web/ws`.
    pub fn allowed_origins(&self) -> Vec<String> {
        if self.origins.is_empty() {
            return vec![
                format!("http://127.0.0.1:{}", self.port),
                format!("http://localhost:{}", self.port),
            ];
        }
        self.origins
            .iter()
            .map(|o| o.trim().trim_end_matches('/').to_string())
            .collect()
    }

    /// `Origin` presente e igual a um dos aceitos (ausente é recusado).
    pub fn origin_ok(&self, headers: &HeaderMap) -> bool {
        let Some(o) = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()) else {
            return false;
        };
        self.allowed_origins().iter().any(|a| a == o)
    }
}

/// Cabeçalhos de segurança das respostas web (spec da PWA §3.3).
fn security_headers(h: &mut HeaderMap) {
    h.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'self'; connect-src 'self'; img-src 'self' data:; frame-ancestors 'none'",
        ),
    );
    h.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    h.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
}

fn refuse(status: StatusCode, msg: &'static str) -> Response {
    let mut r = (status, msg).into_response();
    security_headers(r.headers_mut());
    r
}

/// Resposta JSON das rotas `/web/*`, nunca guardada em cache.
fn json_reply(status: StatusCode, body: Value) -> Response {
    let mut r = (status, axum::Json(body)).into_response();
    let h = r.headers_mut();
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    security_headers(h);
    r
}

fn denied(d: Denied) -> Response {
    match d {
        Denied::Proxy => refuse(StatusCode::FORBIDDEN, "acesso negado"),
        Denied::Session => refuse(StatusCode::UNAUTHORIZED, "sessão web inválida"),
    }
}

/// Roda `f` com o `WebAuth` fora do runtime (SQLite e, com o Access, a
/// busca das chaves). `None` = a tarefa falhou (já no log): `500`.
async fn blocking<T: Send + 'static>(
    st: &HubState,
    f: impl FnOnce(&WebAuth) -> T + Send + 'static,
) -> Option<T> {
    let auth = st.web.auth.clone();
    tokio::task::spawn_blocking(move || f(&auth))
        .await
        .inspect_err(|e| tracing::error!("rota web: {e}"))
        .ok()
}

fn internal() -> Response {
    refuse(StatusCode::INTERNAL_SERVER_ERROR, "erro interno")
}

fn origin_refused(st: &HubState, headers: &HeaderMap, route: &str) -> Option<Response> {
    if st.web.origin_ok(headers) {
        return None;
    }
    let origin = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok());
    tracing::warn!("{route} recusado: Origin {origin:?} fora da lista");
    Some(refuse(StatusCode::FORBIDDEN, "origin não permitido"))
}

/// `GET /web/ws`: `Origin` da lista, sem `Authorization` (o caminho com
/// token é o `/ws`), identidade do proxy (se configurado) e sessão web.
pub(crate) async fn upgrade(
    State(st): State<HubState>,
    Peer(peer): Peer,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    if let Some(r) = origin_refused(&st, &headers, "/web/ws") {
        return r;
    }
    if headers.contains_key(header::AUTHORIZATION) {
        return refuse(
            StatusCode::BAD_REQUEST,
            "o /web/ws não aceita Authorization: use o /ws",
        );
    }
    let session = match blocking(&st, move |a| a.authorize(&headers, peer)).await {
        Some(Ok(s)) => s,
        Some(Err(d)) => return denied(d),
        None => return internal(),
    };
    ws.on_upgrade(move |socket| {
        crate::ws::serve(socket, st, crate::ws::Channel::Web(session.id.into()))
    })
}

/// Corpo do `POST /web/login`. Sem `Debug`: o token não vai a log.
#[derive(serde::Deserialize)]
struct LoginBody {
    token: String,
}

/// Teto do corpo do `POST /web/login`.
pub(crate) const LOGIN_BODY_LIMIT: usize = 4096;

/// `POST /web/login { token }`: com o token de UI certo, responde
/// `{ expires_at, identity }` e o `Set-Cookie` do `ct_web`. O token não
/// volta em resposta nem erro.
pub(crate) async fn login(
    State(st): State<HubState>,
    Peer(peer): Peer,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    if let Some(r) = origin_refused(&st, &headers, "/web/login") {
        return r;
    }
    let Ok(LoginBody { token }) = serde_json::from_slice::<LoginBody>(&body) else {
        return json_reply(
            StatusCode::BAD_REQUEST,
            json!({ "error": "corpo inválido: esperado { token }" }),
        );
    };
    let r = blocking(&st, move |a| {
        let r = a.login(&token, &headers, peer);
        r.map(|s| (a.set_cookie(&s), s))
    })
    .await;
    match r {
        None => internal(),
        Some(Ok((cookie, s))) => {
            let mut r = json_reply(
                StatusCode::OK,
                json!({ "expires_at": s.expires_at, "authenticated": true }),
            );
            if let Ok(v) = HeaderValue::from_str(&cookie) {
                r.headers_mut().insert(header::SET_COOKIE, v);
            }
            r
        }
        Some(Err(LoginError::Proxy)) => denied(Denied::Proxy),
        Some(Err(LoginError::BadToken)) => json_reply(
            StatusCode::UNAUTHORIZED,
            json!({ "error": "token inválido" }),
        ),
        Some(Err(LoginError::Locked(left))) => {
            let secs = left.as_secs().max(1);
            let mut r = json_reply(
                StatusCode::TOO_MANY_REQUESTS,
                json!({ "error": "muitas tentativas: aguarde", "retry_after": secs }),
            );
            r.headers_mut()
                .insert(header::RETRY_AFTER, HeaderValue::from(secs));
            r
        }
        Some(Err(LoginError::Internal(e))) => {
            tracing::error!("/web/login: {e}");
            refuse(StatusCode::INTERNAL_SERVER_ERROR, "erro interno")
        }
    }
}

/// `POST /web/logout`: revoga a sessão do cookie e apaga o cookie.
pub(crate) async fn logout(
    State(st): State<HubState>,
    Peer(peer): Peer,
    headers: HeaderMap,
) -> Response {
    if let Some(r) = origin_refused(&st, &headers, "/web/logout") {
        return r;
    }
    match blocking(&st, move |a| a.logout(&headers, peer)).await {
        None => internal(),
        Some(Err(d)) => denied(d),
        Some(Ok(revoked)) => {
            let mut r = json_reply(StatusCode::OK, json!({ "revoked": revoked }));
            if let Ok(v) = HeaderValue::from_str(&WebAuth::clear_cookie()) {
                r.headers_mut().insert(header::SET_COOKIE, v);
            }
            r
        }
    }
}

/// `GET /web/me`: `{ authenticated, identity, expires_at }`.
pub(crate) async fn me(
    State(st): State<HubState>,
    Peer(peer): Peer,
    headers: HeaderMap,
) -> Response {
    match blocking(&st, move |a| a.me(&headers, peer)).await {
        None => internal(),
        Some(Err(d)) => denied(d),
        Some(Ok(me)) => json_reply(StatusCode::OK, json!(me)),
    }
}

/// Caminhos das rotas do hub: nunca viram o `index.html`.
fn hub_route(path: &str) -> bool {
    path == "ws"
        || path == "healthz"
        || ["web", "sync", "agent"]
            .iter()
            .any(|p| path == *p || path.starts_with(&format!("{p}/")))
}

/// Assets com hash no nome (o Vite os põe em `assets/`).
fn hashed(path: &str) -> bool {
    path.starts_with("assets/")
}

fn not_found() -> Response {
    refuse(StatusCode::NOT_FOUND, "não encontrado")
}

/// Fallback do router com a web ligada: arquivo do bundle ou, para os
/// demais caminhos de navegação, o `index.html` (rotas do lado da UI).
pub(crate) async fn asset(
    State(st): State<HubState>,
    Peer(peer): Peer,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
) -> Response {
    if method != Method::GET && method != Method::HEAD {
        return not_found();
    }
    let path = uri.path().trim_start_matches('/');
    if hub_route(path) {
        return not_found();
    }
    // Com proxy, nem o bundle sai sem a identidade dele.
    if st.web.auth.proxy_configured() {
        let h = headers.clone();
        match blocking(&st, move |a| a.identity(&h, peer)).await {
            Some(Ok(_)) => {}
            Some(Err(d)) => return denied(d),
            None => return internal(),
        }
    }
    let path = if path.is_empty() { "index.html" } else { path };
    let (path, file) = match Bundle::get(path) {
        Some(f) => (path, f),
        // Asset com hash que não existe é 404: o index.html no lugar de um
        // .js quebraria a página (e o service worker o guardaria).
        None if hashed(path) => return not_found(),
        None => match Bundle::get("index.html") {
            Some(f) => ("index.html", f),
            None => return not_found(),
        },
    };
    let etag = format!(
        "\"{}\"",
        file.metadata
            .sha256_hash()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    );
    let cache = if hashed(path) {
        "public, max-age=31536000, immutable"
    } else {
        // index.html, sw.js, manifest.webmanifest e os demais sem hash.
        "no-cache"
    };
    let fresh = headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.split(',').any(|t| t.trim() == etag));
    let mut r = if fresh {
        StatusCode::NOT_MODIFIED.into_response()
    } else {
        file.data.into_owned().into_response()
    };
    let h = r.headers_mut();
    if let Ok(v) = HeaderValue::from_str(file.metadata.mimetype()) {
        h.insert(header::CONTENT_TYPE, v);
    }
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
    if let Ok(v) = HeaderValue::from_str(&etag) {
        h.insert(header::ETAG, v);
    }
    security_headers(h);
    r
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::router;
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use serde_json::json;
    use tower::ServiceExt;

    fn st(enabled: bool) -> HubState {
        let hub = ct_ledger::SharedHub::new(ct_ledger::Hub::open_in_memory().unwrap());
        let mut st = HubState::new(hub.clone(), "ui", "h", "hub-id");
        let cfg = crate::config::WebConfig {
            enabled,
            ..Default::default()
        };
        st.web = WebOptions::new(&cfg, 7879, hub, "ui");
        st
    }

    async fn get(st: &HubState, path: &str, h: &[(&str, &str)]) -> Response {
        let mut req = Request::get(path);
        for (k, v) in h {
            req = req.header(*k, *v);
        }
        router(st.clone())
            .oneshot(req.body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    fn hv<'a>(r: &'a Response, k: &str) -> &'a str {
        r.headers()
            .get(k)
            .map(|v| v.to_str().unwrap())
            .unwrap_or("")
    }

    async fn body(r: Response) -> String {
        String::from_utf8(r.into_body().collect().await.unwrap().to_bytes().to_vec()).unwrap()
    }

    /// Um asset com hash do bundle embutido, se houver (sem `ui/dist-web` só
    /// existe a página de aviso).
    fn some_hashed_asset() -> Option<String> {
        Bundle::iter()
            .find(|p| hashed(p) && p.ends_with(".js"))
            .map(|p| p.to_string())
    }

    #[tokio::test]
    async fn index_e_fallback_spa_sem_cache() {
        let st = st(true);
        for path in ["/", "/index.html", "/hub/propostas", "/item?go=hub:envs"] {
            let r = get(&st, path, &[]).await;
            assert_eq!(r.status(), StatusCode::OK, "{path}");
            assert_eq!(hv(&r, "cache-control"), "no-cache", "{path}");
            assert!(hv(&r, "content-type").starts_with("text/html"), "{path}");
            assert_eq!(hv(&r, "x-content-type-options"), "nosniff");
            assert_eq!(hv(&r, "referrer-policy"), "no-referrer");
            assert!(hv(&r, "content-security-policy").contains("frame-ancestors 'none'"));
            let b = body(r).await;
            assert!(b.contains("Control Tower"), "{path}: {b}");
            if !bundle_present() {
                assert!(b.contains("npm run build:web"), "{b}");
            }
        }
        // ETag: a revalidação do no-cache responde 304 sem corpo.
        let r = get(&st, "/", &[]).await;
        let etag = hv(&r, "etag").to_string();
        assert!(etag.starts_with('"'), "{etag}");
        let r = get(&st, "/", &[("if-none-match", &etag)]).await;
        assert_eq!(r.status(), StatusCode::NOT_MODIFIED);
        assert_eq!(hv(&r, "cache-control"), "no-cache");
    }

    #[tokio::test]
    async fn assets_com_hash_com_cache_longo() {
        let st = st(true);
        // Asset com hash ausente é 404, não o index.html.
        let r = get(&st, "/assets/index-naoexiste.js", &[]).await;
        assert_eq!(r.status(), StatusCode::NOT_FOUND);
        let Some(asset) = some_hashed_asset() else {
            assert!(!bundle_present(), "bundle sem assets/*.js");
            return;
        };
        let r = get(&st, &format!("/{asset}"), &[]).await;
        assert_eq!(r.status(), StatusCode::OK);
        assert_eq!(
            hv(&r, "cache-control"),
            "public, max-age=31536000, immutable"
        );
        assert!(hv(&r, "content-type").contains("javascript"), "{asset}");
        assert_eq!(hv(&r, "x-content-type-options"), "nosniff");
    }

    /// Os cabeçalhos por tipo de arquivo, sem depender do bundle compilado.
    #[test]
    fn classificacao_dos_caminhos() {
        assert!(hashed("assets/index-3f9a1c.js"));
        for p in [
            "index.html",
            "sw.js",
            "manifest.webmanifest",
            "theme-boot.js",
        ] {
            assert!(!hashed(p), "{p}");
        }
        for p in [
            "ws",
            "healthz",
            "web/ws",
            "web/login",
            "sync/push",
            "agent/v1/x",
        ] {
            assert!(hub_route(p), "{p}");
        }
        for p in ["", "hub", "websites", "syncronia", "assets/x.js"] {
            assert!(!hub_route(p), "{p}");
        }
    }

    #[tokio::test]
    async fn rotas_do_hub_nao_viram_index() {
        let st = st(true);
        for path in ["/sync/inventada", "/agent/v1/inventada", "/web/inventada"] {
            let r = get(&st, path, &[]).await;
            assert_eq!(r.status(), StatusCode::NOT_FOUND, "{path}");
        }
        let r = get(&st, "/healthz", &[]).await;
        assert_eq!(r.status(), StatusCode::OK);
        assert!(body(r).await.contains("\"role\":\"hub\""));
        // POST fora de uma rota do hub não serve o bundle.
        let r = router(st.clone())
            .oneshot(Request::post("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn web_desligada_responde_404() {
        let st = st(false);
        for path in [
            "/",
            "/index.html",
            "/hub",
            "/web/ws",
            "/web/me",
            "/web/login",
        ] {
            let r = get(&st, path, &[]).await;
            assert_eq!(r.status(), StatusCode::NOT_FOUND, "{path}");
        }
    }

    // ---- login web (#200) ----

    const ORIGIN: &str = "http://127.0.0.1:7879";

    async fn post(st: &HubState, path: &str, h: &[(&str, &str)], body: &str) -> Response {
        let mut req = Request::post(path);
        for (k, v) in h {
            req = req.header(*k, *v);
        }
        router(st.clone())
            .oneshot(req.body(Body::from(body.to_string())).unwrap())
            .await
            .unwrap()
    }

    fn login_body(token: &str) -> String {
        json!({ "token": token }).to_string()
    }

    /// `ct_web=<valor>` do `Set-Cookie`.
    fn cookie_of(r: &Response) -> String {
        hv(r, "set-cookie").split(';').next().unwrap().to_string()
    }

    #[tokio::test]
    async fn login_cria_cookie_e_me_e_logout() {
        let st = st(true);
        let r = get(&st, "/web/me", &[]).await;
        assert_eq!(r.status(), StatusCode::OK);
        assert_eq!(hv(&r, "cache-control"), "no-store");
        let v: Value = serde_json::from_str(&body(r).await).unwrap();
        assert_eq!(
            v,
            json!({ "authenticated": false, "identity": null, "expires_at": null })
        );

        let r = post(&st, "/web/login", &[("origin", ORIGIN)], &login_body("ui")).await;
        assert_eq!(r.status(), StatusCode::OK);
        let sc = hv(&r, "set-cookie").to_string();
        for attr in [
            "HttpOnly",
            "Secure",
            "SameSite=Strict",
            "Path=/",
            "Max-Age=2592000",
        ] {
            assert!(sc.contains(attr), "{sc}");
        }
        let cookie = cookie_of(&r);
        let b = body(r).await;
        assert!(!b.contains("\"ui\""), "o token não volta: {b}");

        let r = get(&st, "/web/me", &[("cookie", &cookie)]).await;
        let v: Value = serde_json::from_str(&body(r).await).unwrap();
        assert_eq!(v["authenticated"], true, "{v}");
        assert!(v["expires_at"].as_i64().unwrap() > ct_core::now_ms());

        // Logout exige Origin, revoga e apaga o cookie.
        let r = post(&st, "/web/logout", &[("cookie", &cookie)], "").await;
        assert_eq!(r.status(), StatusCode::FORBIDDEN);
        let r = post(
            &st,
            "/web/logout",
            &[("origin", ORIGIN), ("cookie", &cookie)],
            "",
        )
        .await;
        assert_eq!(r.status(), StatusCode::OK);
        assert!(hv(&r, "set-cookie").contains("Max-Age=0"));
        assert!(body(r).await.contains("\"revoked\":true"));
        let r = get(&st, "/web/me", &[("cookie", &cookie)]).await;
        assert!(body(r).await.contains("\"authenticated\":false"));
    }

    #[tokio::test]
    async fn login_confere_origin_corpo_e_token() {
        let st = st(true);
        for h in [
            vec![],
            vec![("origin", "https://evil.example")],
            vec![("origin", "http://localhost:1420")],
        ] {
            let r = post(&st, "/web/login", &h, &login_body("ui")).await;
            assert_eq!(r.status(), StatusCode::FORBIDDEN, "{h:?}");
            assert!(hv(&r, "set-cookie").is_empty());
        }
        let r = post(&st, "/web/login", &[("origin", ORIGIN)], "token=ui").await;
        assert_eq!(r.status(), StatusCode::BAD_REQUEST);
        let r = post(
            &st,
            "/web/login",
            &[("origin", ORIGIN)],
            &login_body("segredo-errado"),
        )
        .await;
        assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
        assert!(hv(&r, "set-cookie").is_empty());
        let b = body(r).await;
        assert!(!b.contains("segredo-errado"), "{b}");
        // Corpo grande demais.
        let big = login_body(&"x".repeat(LOGIN_BODY_LIMIT));
        let r = post(&st, "/web/login", &[("origin", ORIGIN)], &big).await;
        assert_eq!(r.status(), StatusCode::PAYLOAD_TOO_LARGE);
        // GET não é login.
        let r = get(&st, "/web/login", &[("origin", ORIGIN)]).await;
        assert_eq!(r.status(), StatusCode::METHOD_NOT_ALLOWED);
    }

    #[tokio::test]
    async fn login_bloqueado_apos_falhas_seguidas() {
        let st = st(true);
        for _ in 0..crate::web_auth::LOCK_AFTER {
            let r = post(
                &st,
                "/web/login",
                &[("origin", ORIGIN)],
                &login_body("errado"),
            )
            .await;
            assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
        }
        let r = post(&st, "/web/login", &[("origin", ORIGIN)], &login_body("ui")).await;
        assert_eq!(r.status(), StatusCode::TOO_MANY_REQUESTS);
        let secs: u64 = hv(&r, "retry-after").parse().unwrap();
        assert!((1..=60).contains(&secs), "{secs}");
        assert!(hv(&r, "set-cookie").is_empty());
        // O bloqueio é só do login: o /healthz e o resto seguem.
        assert_eq!(get(&st, "/healthz", &[]).await.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn cookie_vencido_recusado() {
        let st = st(true);
        let r = post(&st, "/web/login", &[("origin", ORIGIN)], &login_body("ui")).await;
        let cookie = cookie_of(&r);
        // Validade absoluta passada.
        st.hub
            .lock()
            .conn()
            .execute(
                "UPDATE web_session SET expires_at = ?1",
                [ct_core::now_ms() - 1],
            )
            .unwrap();
        let r = get(&st, "/web/me", &[("cookie", &cookie)]).await;
        assert!(body(r).await.contains("\"authenticated\":false"));
    }

    fn with_proxy(st: &mut HubState, cfg: crate::config::WebConfig) {
        st.web.auth = Arc::new(WebAuth::new(st.hub.clone(), "ui", &cfg));
    }

    async fn get_from(st: &HubState, path: &str, peer: &str, h: &[(&str, &str)]) -> Response {
        let mut req = Request::get(path);
        for (k, v) in h {
            req = req.header(*k, *v);
        }
        let mut req = req.body(Body::empty()).unwrap();
        req.extensions_mut()
            .insert(ConnectInfo::<SocketAddr>(peer.parse().unwrap()));
        router(st.clone()).oneshot(req).await.unwrap()
    }

    #[tokio::test]
    async fn tailscale_vale_so_do_loopback_e_sem_cf() {
        let mut st = st(true);
        with_proxy(
            &mut st,
            crate::config::WebConfig {
                enabled: true,
                tailscale: Some(crate::config::TailscaleConfig {
                    logins: vec!["eu@github".into()],
                }),
                ..Default::default()
            },
        );
        let ts = ("tailscale-user-login", "eu@github");
        let lo = "127.0.0.1:40000";
        assert_eq!(get_from(&st, "/", lo, &[ts]).await.status(), StatusCode::OK);
        let r = get_from(&st, "/web/me", lo, &[ts]).await;
        assert!(body(r).await.contains("\"identity\":\"eu@github\""));
        // Sem o header, de fora do loopback, sem ConnectInfo ou com Cf-*.
        for (peer, h) in [
            (lo, vec![]),
            ("100.64.0.9:40000", vec![ts]),
            (lo, vec![ts, ("cf-connecting-ip", "203.0.113.9")]),
            (lo, vec![ts, ("cf-ray", "x")]),
            (lo, vec![("tailscale-user-login", "outro@github")]),
        ] {
            let r = get_from(&st, "/", peer, &h).await;
            assert_eq!(r.status(), StatusCode::FORBIDDEN, "{peer} {h:?}");
        }
        assert_eq!(get(&st, "/", &[ts]).await.status(), StatusCode::FORBIDDEN);
        // O login com o proxy ainda exige o token.
        let mut req = Request::post("/web/login")
            .header("origin", ORIGIN)
            .header(ts.0, ts.1)
            .body(Body::from(login_body("errado")))
            .unwrap();
        req.extensions_mut()
            .insert(ConnectInfo::<SocketAddr>(lo.parse().unwrap()));
        let r = router(st.clone()).oneshot(req).await.unwrap();
        assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn cloudflare_access_exige_jwt_valido_e_o_token() {
        use crate::web_auth::tests::{claims, jwks_server, jwt, AUD};
        let (certs, _) = jwks_server().await;
        let mut st = st(true);
        let cfg = crate::config::WebConfig {
            enabled: true,
            cloudflare: Some(crate::config::CloudflareConfig {
                team_domain: "equipe.cloudflareaccess.com".into(),
                aud: AUD.into(),
                emails: vec!["eu@exemplo.com".into()],
            }),
            ..Default::default()
        };
        st.web.auth = Arc::new(
            WebAuth::new(st.hub.clone(), "ui", &cfg)
                .with_cloudflare(crate::web_auth::tests::cf(&certs)),
        );
        let good = jwt("a", "ka", claims("eu@exemplo.com", AUD));
        let cf = |t: &str| ("cf-access-jwt-assertion", t.to_string());
        let login = |h: Vec<(&'static str, String)>, token: &'static str| {
            let st = st.clone();
            async move {
                let mut req = Request::post("/web/login").header("origin", ORIGIN);
                for (k, v) in &h {
                    req = req.header(*k, v);
                }
                router(st)
                    .oneshot(req.body(Body::from(login_body(token))).unwrap())
                    .await
                    .unwrap()
            }
        };
        // Sem JWT, com assinatura errada, AUD errado ou e-mail fora: 403.
        for (h, why) in [
            (vec![], "sem JWT"),
            (
                vec![cf(&jwt("b", "ka", claims("eu@exemplo.com", AUD)))],
                "assinatura",
            ),
            (
                vec![cf(&jwt("a", "ka", claims("eu@exemplo.com", "outro")))],
                "aud",
            ),
            (
                vec![cf(&jwt("a", "ka", claims("intruso@exemplo.com", AUD)))],
                "email",
            ),
        ] {
            let r = login(h, "ui").await;
            assert_eq!(r.status(), StatusCode::FORBIDDEN, "{why}");
            let b = body(r).await;
            assert_eq!(b, "acesso negado", "o motivo não vai na resposta: {why}");
        }
        // JWT válido sem o token: recusado; com o token: cookie.
        let r = login(vec![cf(&good)], "errado").await;
        assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
        let r = login(vec![cf(&good)], "ui").await;
        assert_eq!(r.status(), StatusCode::OK);
        let cookie = cookie_of(&r);
        // O cookie sozinho não basta: precisa do JWT junto.
        let r = get(&st, "/web/me", &[("cookie", &cookie)]).await;
        assert_eq!(r.status(), StatusCode::FORBIDDEN);
        let r = get(
            &st,
            "/web/me",
            &[("cookie", &cookie), ("cf-access-jwt-assertion", &good)],
        )
        .await;
        let v: Value = serde_json::from_str(&body(r).await).unwrap();
        assert_eq!(v["authenticated"], true);
        assert_eq!(v["identity"], "eu@exemplo.com");
        let ident: Option<String> = st
            .hub
            .lock()
            .conn()
            .query_row("SELECT identity FROM web_session", [], |r| r.get(0))
            .unwrap();
        assert_eq!(ident.as_deref(), Some("eu@exemplo.com"));
        // O bundle também exige o JWT.
        assert_eq!(get(&st, "/", &[]).await.status(), StatusCode::FORBIDDEN);
        assert_eq!(
            get(&st, "/", &[("cf-access-jwt-assertion", &good)])
                .await
                .status(),
            StatusCode::OK
        );
    }

    #[test]
    fn origens_padrao_e_configuradas() {
        let hub = ct_ledger::SharedHub::new(ct_ledger::Hub::open_in_memory().unwrap());
        let mut o = WebOptions::disabled(hub, "ui");
        o.port = 7879;
        let h = |v: &str| {
            let mut h = HeaderMap::new();
            h.insert(header::ORIGIN, v.parse().unwrap());
            h
        };
        assert!(o.origin_ok(&h("http://127.0.0.1:7879")));
        assert!(o.origin_ok(&h("http://localhost:7879")));
        assert!(!o.origin_ok(&h("http://localhost:1420")));
        assert!(!o.origin_ok(&h("https://evil.example")));
        assert!(!o.origin_ok(&HeaderMap::new()), "Origin ausente");
        o.origins = vec!["https://hub.exemplo.ts.net/".into()];
        assert!(o.origin_ok(&h("https://hub.exemplo.ts.net")));
        assert!(!o.origin_ok(&h("http://127.0.0.1:7879")));
    }

    #[test]
    fn pushes_da_web() {
        use serde_json::json;
        assert!(web_push(&json!({ "event": "hub_sync_status", "data": {} })));
        assert!(web_push(
            &json!({ "event": "hub.proposals.changed", "data": {} })
        ));
        assert!(web_push(
            &json!({ "event": "env.online", "data": { "env_id": "e", "online": true } })
        ));
        assert!(web_push(&json!({ "event": "env.push", "data": {} })));
        assert!(!web_push(&json!({ "event": "outro", "data": {} })));
        assert!(!web_push(&json!({})));
    }

    #[test]
    fn listas_sem_sobreposicao() {
        for m in WEB_METHODS {
            assert!(
                !WEB_SENSITIVE.contains(m) && !WEB_REFUSED.contains(m),
                "{m}"
            );
        }
        for m in WEB_SENSITIVE {
            assert!(!WEB_REFUSED.contains(m), "{m}");
            assert!(!web_allowed(m), "{m}");
        }
    }

    /// Todo método atendido pelo `call` do `/ws` (os braços `"hub.…" =>` e
    /// `"github.…" =>` de `ws.rs`) está classificado aqui, e nada além
    /// deles: método novo sem classificação quebra este teste.
    #[test]
    fn todo_metodo_do_ws_esta_classificado() {
        let src = include_str!("ws.rs");
        let code = &src[..src.find("#[cfg(test)]").unwrap()];
        let mut found = std::collections::BTreeSet::new();
        for prefix in ["\"hub.", "\"github."] {
            for (i, _) in code.match_indices(prefix) {
                let lit = &code[i + 1..];
                let Some(j) = lit.find('"') else { continue };
                let (lit, after) = (&lit[..j], lit[j + 1..].trim_start());
                if !lit.ends_with('.') && (after.starts_with("=>") || after.starts_with('|')) {
                    found.insert(lit.to_string());
                }
            }
        }
        assert!(found.contains("hub.dashboard.get"), "{found:?}");
        assert!(found.contains("github.clear_token"), "{found:?}");
        let classified: std::collections::BTreeSet<String> = WEB_METHODS
            .iter()
            .chain(WEB_SENSITIVE)
            .chain(WEB_REFUSED)
            .map(|m| m.to_string())
            .collect();
        let missing: Vec<_> = found.difference(&classified).collect();
        assert!(missing.is_empty(), "sem classificação na web: {missing:?}");
        let stale: Vec<_> = classified.difference(&found).collect();
        assert!(
            stale.is_empty(),
            "classificados sem método no /ws: {stale:?}"
        );
    }

    /// A `WebDaemonApi` (`ui/src/api/web.ts`, `ENV_METHODS`) recusa sem ir
    /// ao hub o que está fora da lista do canal: as duas listas são iguais.
    #[test]
    fn lista_de_metodos_do_ambiente_da_ui_web_e_a_do_canal() {
        let src = include_str!("../../../ui/src/api/web.ts");
        let start = src.find("ENV_METHODS").expect("ENV_METHODS na web.ts");
        let body = &src[start..];
        let body = &body[body.find('[').unwrap() + 1..body.find(']').unwrap()];
        let ui: std::collections::BTreeSet<&str> = body
            .split(',')
            .map(|m| m.trim().trim_matches('"'))
            .filter(|m| !m.is_empty())
            .collect();
        let hub: std::collections::BTreeSet<&str> =
            ct_http::sync::CHANNEL_METHODS.iter().copied().collect();
        assert_eq!(ui, hub);
    }
}
