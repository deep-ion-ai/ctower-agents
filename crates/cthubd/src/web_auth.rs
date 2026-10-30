//! Sessão web do hub (issue #200, spec da PWA §3): login com o token de UI
//! que vira o cookie `ct_web`, validade e revogação (`hub.db`, tabela
//! `web_session`), bloqueio global após falhas e a identidade do proxy
//! (Cloudflare Access ou `tailscale serve`) como porta a mais.
//!
//! Regras:
//!
//! - **Proxy:** com `[web.cloudflare]` ou `[web.tailscale]` configurado, toda
//!   rota web exige uma identidade válida de um deles (senão `403`); sem
//!   nenhum, vale só o cookie. A identidade nunca substitui o login: o token
//!   de UI é sempre exigido para criar o cookie.
//! - **Cloudflare Access:** JWT RS256 em `Cf-Access-Jwt-Assertion`, com as
//!   chaves de `https://<team_domain>/cdn-cgi/access/certs` (cache de 1 h,
//!   recarregado com `kid` desconhecido), `iss`, `aud`, `exp`/`nbf` e
//!   `email` da lista. O motivo da recusa vai só ao log.
//! - **Tailscale:** `Tailscale-User-Login` só vale com a conexão vinda do
//!   loopback e sem nenhum header `Cf-*` (o `cloudflared` também fala do
//!   loopback); fora disso, é ignorado.
//! - **Bloqueio:** a partir da 5ª falha seguida de login, 1 min, dobrando a
//!   cada nova falha até 1 h; um acerto zera. Só o `/web/login` é afetado.

use crate::config::WebConfig;
use axum::http::{header, HeaderMap};
use ct_ledger::{IssuedWebSession, SharedHub};
use jsonwebtoken::{Algorithm, DecodingKey, Validation};
use serde::Deserialize;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const COOKIE: &str = "ct_web";
pub const CF_JWT_HEADER: &str = "cf-access-jwt-assertion";
pub const CF_CONNECTING_IP: &str = "cf-connecting-ip";
pub const TAILSCALE_LOGIN_HEADER: &str = "tailscale-user-login";

const DAY_MS: i64 = 24 * 3600 * 1000;

/// Falhas seguidas até o primeiro bloqueio.
pub const LOCK_AFTER: u32 = 5;
const LOCK_FIRST: Duration = Duration::from_secs(60);
const LOCK_MAX: Duration = Duration::from_secs(3600);

/// Validade do cache das chaves do Access.
const JWKS_TTL: Duration = Duration::from_secs(3600);
/// Intervalo mínimo entre buscas das chaves por causa de um `kid`
/// desconhecido (um JWT forjado não faz o hub martelar a Cloudflare).
const JWKS_RETRY: Duration = Duration::from_secs(30);
const JWKS_TIMEOUT: Duration = Duration::from_secs(5);

/// Contador global de falhas do `/web/login` (atrás de proxy, o IP visto é
/// sempre `127.0.0.1`: não há contador por cliente).
#[derive(Debug, Default)]
pub(crate) struct Lockout {
    failures: u32,
    until: Option<Instant>,
}

impl Lockout {
    /// Tempo restante do bloqueio, se houver.
    pub(crate) fn blocked(&self, now: Instant) -> Option<Duration> {
        self.until
            .and_then(|u| u.checked_duration_since(now))
            .filter(|d| !d.is_zero())
    }

    /// Registra uma falha; devolve o bloqueio que ela abriu, se abriu.
    pub(crate) fn fail(&mut self, now: Instant) -> Option<Duration> {
        self.failures = self.failures.saturating_add(1);
        if self.failures < LOCK_AFTER {
            return None;
        }
        let doublings = (self.failures - LOCK_AFTER).min(16);
        let d = LOCK_FIRST.saturating_mul(1 << doublings).min(LOCK_MAX);
        self.until = Some(now + d);
        Some(d)
    }

    pub(crate) fn success(&mut self) {
        *self = Lockout::default();
    }
}

/// Chave do JWKS do Access (só os campos usados).
#[derive(Deserialize)]
struct Jwk {
    kid: Option<String>,
    kty: String,
    n: Option<String>,
    e: Option<String>,
}

#[derive(Deserialize)]
struct Jwks {
    keys: Vec<Jwk>,
}

#[derive(Default)]
struct KeyCache {
    keys: HashMap<String, DecodingKey>,
    fetched_at: Option<Instant>,
    last_attempt: Option<Instant>,
}

/// Claims do JWT do Access que o hub usa (`iss`, `aud`, `exp` e `nbf` são
/// conferidos pela `Validation`).
#[derive(Deserialize)]
struct CfClaims {
    email: Option<String>,
}

/// Conferência do JWT do Cloudflare Access (`[web.cloudflare]`).
pub struct CfAccess {
    issuer: String,
    aud: String,
    emails: Vec<String>,
    certs_url: String,
    retry: Duration,
    cache: Mutex<KeyCache>,
}

impl CfAccess {
    pub fn new(team_domain: &str, aud: &str, emails: &[String]) -> Self {
        let team = team_domain
            .trim()
            .trim_start_matches("https://")
            .trim_end_matches('/');
        let issuer = format!("https://{team}");
        let certs_url = format!("{issuer}/cdn-cgi/access/certs");
        Self::with_certs_url(&issuer, aud, emails, &certs_url)
    }

    /// Como [`CfAccess::new`], com o endereço das chaves explícito (testes).
    pub fn with_certs_url(issuer: &str, aud: &str, emails: &[String], certs_url: &str) -> Self {
        CfAccess {
            issuer: issuer.to_string(),
            aud: aud.trim().to_string(),
            emails: emails
                .iter()
                .map(|e| e.trim().to_lowercase())
                .filter(|e| !e.is_empty())
                .collect(),
            certs_url: certs_url.to_string(),
            retry: JWKS_RETRY,
            cache: Mutex::new(KeyCache::default()),
        }
    }

    #[cfg(test)]
    fn with_retry(mut self, retry: Duration) -> Self {
        self.retry = retry;
        self
    }

    fn fetch(&self) -> Result<HashMap<String, DecodingKey>, String> {
        let jwks: Jwks = ureq::get(&self.certs_url)
            .timeout(JWKS_TIMEOUT)
            .call()
            .map_err(|e| format!("chaves do Access em {}: {e}", self.certs_url))?
            .into_json()
            .map_err(|e| format!("chaves do Access ilegíveis: {e}"))?;
        let mut keys = HashMap::new();
        for k in jwks.keys {
            let (Some(kid), Some(n), Some(e)) = (k.kid, k.n, k.e) else {
                continue;
            };
            if k.kty != "RSA" {
                continue;
            }
            match DecodingKey::from_rsa_components(&n, &e) {
                Ok(key) => {
                    keys.insert(kid, key);
                }
                Err(err) => tracing::warn!("chave {kid} do Access ignorada: {err}"),
            }
        }
        if keys.is_empty() {
            return Err("nenhuma chave RSA no JWKS do Access".into());
        }
        Ok(keys)
    }

    /// Chave do `kid`, do cache ou de uma busca nova (cache vencido ou
    /// `kid` desconhecido). Com a busca falhando, o cache antigo continua.
    fn key(&self, kid: &str) -> Result<DecodingKey, String> {
        let mut c = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        let fresh = c
            .fetched_at
            .is_some_and(|t| now.duration_since(t) < JWKS_TTL);
        let known = c.keys.contains_key(kid);
        let may_retry = c
            .last_attempt
            .is_none_or(|t| now.duration_since(t) >= self.retry);
        if (!fresh || !known) && may_retry {
            c.last_attempt = Some(now);
            match self.fetch() {
                Ok(keys) => {
                    c.keys = keys;
                    c.fetched_at = Some(now);
                }
                Err(e) => tracing::warn!("{e}"),
            }
        }
        c.keys
            .get(kid)
            .cloned()
            .ok_or_else(|| format!("kid desconhecido: {kid}"))
    }

    /// E-mail do JWT, se tudo confere; senão o motivo (só para o log).
    pub fn verify(&self, jwt: &str) -> Result<String, String> {
        let h = jsonwebtoken::decode_header(jwt).map_err(|e| format!("JWT ilegível: {e}"))?;
        if h.alg != Algorithm::RS256 {
            return Err(format!("algoritmo {:?} recusado", h.alg));
        }
        let kid = h.kid.ok_or("JWT sem kid")?;
        let key = self.key(&kid)?;
        let mut v = Validation::new(Algorithm::RS256);
        v.set_issuer(&[&self.issuer]);
        v.set_audience(&[&self.aud]);
        v.validate_nbf = true;
        v.set_required_spec_claims(&["exp", "iss", "aud"]);
        let data = jsonwebtoken::decode::<CfClaims>(jwt, &key, &v)
            .map_err(|e| format!("JWT recusado: {e}"))?;
        let email = data
            .claims
            .email
            .map(|e| e.trim().to_lowercase())
            .filter(|e| !e.is_empty())
            .ok_or("JWT sem email")?;
        if !self.emails.contains(&email) {
            return Err(format!("e-mail {email} fora da lista"));
        }
        Ok(email)
    }
}

/// Sessão web válida (cookie conferido no `hub.db`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebSession {
    pub id: String,
    pub expires_at: i64,
    /// Identidade do proxy nesta requisição.
    pub identity: Option<String>,
}

/// Por que uma rota web foi recusada.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Denied {
    /// Proxy configurado e sem identidade válida (`403`).
    Proxy,
    /// Sem sessão web válida (`401`).
    Session,
}

/// Falha do `POST /web/login`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginError {
    Proxy,
    /// Bloqueado: tempo restante.
    Locked(Duration),
    BadToken,
    Internal(String),
}

/// `GET /web/me`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct WebMe {
    pub authenticated: bool,
    pub identity: Option<String>,
    pub expires_at: Option<i64>,
}

/// Autenticação das rotas web do hub.
pub struct WebAuth {
    hub: SharedHub,
    ui_token: Arc<str>,
    session_ms: i64,
    idle_ms: i64,
    cloudflare: Option<CfAccess>,
    /// Logins aceitos do `tailscale serve` (minúsculas); `None` = não configurado.
    tailscale: Option<Vec<String>>,
    lockout: Mutex<Lockout>,
}

impl WebAuth {
    pub fn new(hub: SharedHub, ui_token: &str, cfg: &WebConfig) -> Self {
        WebAuth {
            hub,
            ui_token: ui_token.into(),
            session_ms: cfg.session_days.max(1) as i64 * DAY_MS,
            idle_ms: cfg.idle_days.max(1) as i64 * DAY_MS,
            cloudflare: cfg
                .cloudflare
                .as_ref()
                .map(|c| CfAccess::new(&c.team_domain, &c.aud, &c.emails)),
            tailscale: cfg.tailscale.as_ref().map(|t| {
                t.logins
                    .iter()
                    .map(|l| l.trim().to_lowercase())
                    .filter(|l| !l.is_empty())
                    .collect()
            }),
            lockout: Mutex::new(Lockout::default()),
        }
    }

    /// Troca a conferência do Access (testes com chaves servidas localmente).
    pub fn with_cloudflare(mut self, cf: CfAccess) -> Self {
        self.cloudflare = Some(cf);
        self
    }

    pub fn idle_ms(&self) -> i64 {
        self.idle_ms
    }

    pub fn proxy_configured(&self) -> bool {
        self.cloudflare.is_some() || self.tailscale.is_some()
    }

    /// Login do `tailscale serve`, se vale nesta requisição.
    fn tailscale_identity(&self, headers: &HeaderMap, peer: Option<SocketAddr>) -> Option<String> {
        let logins = self.tailscale.as_ref()?;
        let login = headers
            .get(TAILSCALE_LOGIN_HEADER)
            .and_then(|v| v.to_str().ok())
            .map(|v| v.trim().to_lowercase())
            .filter(|v| !v.is_empty())?;
        if !peer.is_some_and(|p| p.ip().is_loopback()) {
            tracing::warn!("Tailscale-User-Login {login} ignorado: conexão de fora do loopback");
            return None;
        }
        if headers.keys().any(|k| k.as_str().starts_with("cf-")) {
            tracing::warn!("Tailscale-User-Login {login} ignorado: requisição com headers Cf-*");
            return None;
        }
        if !logins.contains(&login) {
            tracing::warn!("Tailscale-User-Login {login} fora da lista");
            return None;
        }
        Some(login)
    }

    /// A porta do proxy: `Ok(None)` sem proxy configurado, `Ok(Some(id))`
    /// com uma identidade válida, `Err` com proxy e sem identidade.
    /// Bloqueante (pode buscar as chaves do Access).
    pub fn identity(
        &self,
        headers: &HeaderMap,
        peer: Option<SocketAddr>,
    ) -> Result<Option<String>, Denied> {
        if !self.proxy_configured() {
            return Ok(None);
        }
        if let Some(cf) = &self.cloudflare {
            if let Some(jwt) = headers.get(CF_JWT_HEADER).and_then(|v| v.to_str().ok()) {
                return match cf.verify(jwt.trim()) {
                    Ok(email) => Ok(Some(email)),
                    Err(why) => {
                        tracing::warn!("rota web recusada (Cloudflare Access): {why}");
                        Err(Denied::Proxy)
                    }
                };
            }
        }
        if let Some(login) = self.tailscale_identity(headers, peer) {
            return Ok(Some(login));
        }
        tracing::warn!("rota web recusada: proxy configurado e nenhuma identidade válida");
        Err(Denied::Proxy)
    }

    /// Valor do cookie `ct_web`, se houver.
    pub fn cookie(headers: &HeaderMap) -> Option<String> {
        headers
            .get_all(header::COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .flat_map(|v| v.split(';'))
            .filter_map(|c| c.trim().split_once('='))
            .find(|(k, _)| *k == COOKIE)
            .map(|(_, v)| v.trim().to_string())
            .filter(|v| !v.is_empty())
    }

    fn cookie_session(&self, headers: &HeaderMap) -> Option<ct_ledger::HubWebSession> {
        let c = Self::cookie(headers)?;
        match self.hub.lock().check_web_session(&c, self.idle_ms) {
            Ok(s) => s,
            Err(e) => {
                tracing::error!("sessão web: {e}");
                None
            }
        }
    }

    /// Proxy e cookie: a sessão web da requisição. Bloqueante.
    pub fn authorize(
        &self,
        headers: &HeaderMap,
        peer: Option<SocketAddr>,
    ) -> Result<WebSession, Denied> {
        let identity = self.identity(headers, peer)?;
        let s = self.cookie_session(headers).ok_or(Denied::Session)?;
        Ok(WebSession {
            id: s.id,
            expires_at: s.expires_at,
            identity,
        })
    }

    /// A sessão `id` ainda vale (conexão aberta do `/web/ws`); marca o uso.
    pub fn still_valid(&self, id: &str) -> bool {
        matches!(
            self.hub.lock().web_session_by_id(id, self.idle_ms),
            Ok(Some(_))
        )
    }

    /// `POST /web/login`: proxy, bloqueio e o token de UI em tempo
    /// constante. O token não aparece em log, resposta nem erro.
    pub fn login(
        &self,
        token: &str,
        headers: &HeaderMap,
        peer: Option<SocketAddr>,
    ) -> Result<IssuedWebSession, LoginError> {
        let identity = self
            .identity(headers, peer)
            .map_err(|_| LoginError::Proxy)?;
        let who = identity.as_deref().unwrap_or("sem proxy");
        let mut lock = self.lockout.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        if let Some(left) = lock.blocked(now) {
            tracing::warn!("/web/login bloqueado ({who}): faltam {}s", left.as_secs());
            return Err(LoginError::Locked(left));
        }
        if !token_ok(token, &self.ui_token) {
            let blocked = lock.fail(now);
            tracing::warn!(
                "/web/login: token errado ({who}), {} falha(s) seguida(s){}",
                lock.failures,
                blocked
                    .map(|d| format!(", bloqueado por {}s", d.as_secs()))
                    .unwrap_or_default()
            );
            return Err(LoginError::BadToken);
        }
        lock.success();
        drop(lock);
        let ua = headers
            .get(header::USER_AGENT)
            .and_then(|v| v.to_str().ok());
        let s = self
            .hub
            .lock()
            .create_web_session(identity.as_deref(), ua, self.session_ms)
            .map_err(|e| LoginError::Internal(e.to_string()))?;
        tracing::info!("sessão web {} criada ({who})", s.id);
        Ok(s)
    }

    /// `POST /web/logout`: revoga a sessão do cookie, se válida.
    pub fn logout(&self, headers: &HeaderMap, peer: Option<SocketAddr>) -> Result<bool, Denied> {
        self.identity(headers, peer)?;
        let Some(s) = self.cookie_session(headers) else {
            return Ok(false);
        };
        let r = self.hub.lock().revoke_web_session(&s.id).unwrap_or(false);
        if r {
            tracing::info!("sessão web {} encerrada (logout)", s.id);
        }
        Ok(r)
    }

    /// `GET /web/me`.
    pub fn me(&self, headers: &HeaderMap, peer: Option<SocketAddr>) -> Result<WebMe, Denied> {
        let identity = self.identity(headers, peer)?;
        let s = self.cookie_session(headers);
        Ok(WebMe {
            authenticated: s.is_some(),
            identity: identity.or_else(|| s.as_ref().and_then(|s| s.identity.clone())),
            expires_at: s.map(|s| s.expires_at),
        })
    }

    /// `Set-Cookie` da sessão nova.
    pub fn set_cookie(&self, s: &IssuedWebSession) -> String {
        format!(
            "{COOKIE}={}; HttpOnly; Secure; SameSite=Strict; Path=/; Max-Age={}",
            s.cookie,
            self.session_ms / 1000
        )
    }

    /// `Set-Cookie` que apaga o cookie.
    pub fn clear_cookie() -> String {
        format!("{COOKIE}=; HttpOnly; Secure; SameSite=Strict; Path=/; Max-Age=0")
    }

    #[cfg(test)]
    pub(crate) fn lockout_failures(&self) -> u32 {
        self.lockout.lock().unwrap().failures
    }
}

/// Comparação em tempo constante (como o `ct_http::bearer_ok`).
fn token_ok(got: &str, expected: &str) -> bool {
    let (a, b) = (got.as_bytes(), expected.as_bytes());
    !b.is_empty()
        && a.len() == b.len()
        && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::config::{CloudflareConfig, TailscaleConfig};
    use jsonwebtoken::{EncodingKey, Header};
    use serde_json::json;

    pub(crate) const UI: &str = "ui-secreto";
    pub(crate) const AUD: &str = "aud-da-aplicacao";
    pub(crate) const ISS: &str = "https://equipe.cloudflareaccess.com";

    pub(crate) fn hub() -> SharedHub {
        SharedHub::new(ct_ledger::Hub::open_in_memory().unwrap())
    }

    fn n(k: &str) -> String {
        match k {
            "a" => include_str!("testdata/cf-access-a.n"),
            _ => include_str!("testdata/cf-access-b.n"),
        }
        .trim()
        .to_string()
    }

    fn private(k: &str) -> EncodingKey {
        EncodingKey::from_rsa_der(match k {
            "a" => include_bytes!("testdata/cf-access-a.der"),
            _ => include_bytes!("testdata/cf-access-b.der"),
        })
    }

    /// JWT assinado pela chave `key` (`a` ou `b`) com o `kid` dado.
    pub(crate) fn jwt(key: &str, kid: &str, claims: serde_json::Value) -> String {
        let mut h = Header::new(Algorithm::RS256);
        h.kid = Some(kid.into());
        jsonwebtoken::encode(&h, &claims, &private(key)).unwrap()
    }

    pub(crate) fn claims(email: &str, aud: &str) -> serde_json::Value {
        let now = ct_core::now_ms() / 1000;
        json!({ "iss": ISS, "aud": [aud], "email": email, "exp": now + 600,
                "nbf": now - 10, "iat": now - 10, "sub": "u1" })
    }

    /// Servidor local com o JWKS (só a chave `a`, `kid` = `ka`); devolve o
    /// endereço das chaves e o contador de buscas.
    pub(crate) async fn jwks_server() -> (String, Arc<std::sync::atomic::AtomicUsize>) {
        use axum::routing::get;
        let hits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let h = hits.clone();
        let body = json!({ "keys": [
            { "kid": "ka", "kty": "RSA", "alg": "RS256", "use": "sig", "e": "AQAB", "n": n("a") },
            { "kid": "ec", "kty": "EC", "crv": "P-256", "x": "x", "y": "y" },
        ], "public_cert": { "kid": "ka", "cert": "-----BEGIN CERTIFICATE-----" } });
        let app = axum::Router::new().route(
            "/cdn-cgi/access/certs",
            get(move || {
                h.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let b = body.clone();
                async move { axum::Json(b) }
            }),
        );
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
        (format!("http://{addr}/cdn-cgi/access/certs"), hits)
    }

    pub(crate) fn cf(certs: &str) -> CfAccess {
        CfAccess::with_certs_url(ISS, AUD, &["Eu@Exemplo.com".into()], certs)
    }

    async fn verify(cf: &Arc<CfAccess>, t: String) -> Result<String, String> {
        let cf = cf.clone();
        tokio::task::spawn_blocking(move || cf.verify(&t))
            .await
            .unwrap()
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn jwt_do_access_confere_assinatura_aud_iss_e_email() {
        let (certs, hits) = jwks_server().await;
        let cf = Arc::new(cf(&certs));
        let ok = verify(&cf, jwt("a", "ka", claims("eu@exemplo.com", AUD))).await;
        assert_eq!(ok.unwrap(), "eu@exemplo.com");
        // Maiúsculas no e-mail do JWT não importam.
        assert!(verify(&cf, jwt("a", "ka", claims("EU@exemplo.com", AUD)))
            .await
            .is_ok());
        assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 1, "cache");

        // Assinatura de outra chave com o mesmo kid.
        let e = verify(&cf, jwt("b", "ka", claims("eu@exemplo.com", AUD)))
            .await
            .unwrap_err();
        assert!(e.contains("JWT recusado"), "{e}");
        // AUD errado.
        let e = verify(&cf, jwt("a", "ka", claims("eu@exemplo.com", "outro")))
            .await
            .unwrap_err();
        assert!(e.contains("JWT recusado"), "{e}");
        // E-mail fora da lista.
        let e = verify(&cf, jwt("a", "ka", claims("outro@exemplo.com", AUD)))
            .await
            .unwrap_err();
        assert!(e.contains("fora da lista"), "{e}");
        // iss de outro time.
        let mut c = claims("eu@exemplo.com", AUD);
        c["iss"] = json!("https://outra.cloudflareaccess.com");
        assert!(verify(&cf, jwt("a", "ka", c)).await.is_err());
        // Vencido.
        let mut c = claims("eu@exemplo.com", AUD);
        c["exp"] = json!(ct_core::now_ms() / 1000 - 3600);
        assert!(verify(&cf, jwt("a", "ka", c)).await.is_err());
        // Ainda não vale (nbf no futuro).
        let mut c = claims("eu@exemplo.com", AUD);
        c["nbf"] = json!(ct_core::now_ms() / 1000 + 3600);
        assert!(verify(&cf, jwt("a", "ka", c)).await.is_err());
        // Sem e-mail.
        let mut c = claims("eu@exemplo.com", AUD);
        c.as_object_mut().unwrap().remove("email");
        assert!(verify(&cf, jwt("a", "ka", c)).await.is_err());
        // Lixo e HS256.
        assert!(verify(&cf, "nao.e.jwt".into()).await.is_err());
        let mut h = Header::new(Algorithm::HS256);
        h.kid = Some("ka".into());
        let hs = jsonwebtoken::encode(
            &h,
            &claims("eu@exemplo.com", AUD),
            &EncodingKey::from_secret(b"AQAB"),
        )
        .unwrap();
        let e = verify(&cf, hs).await.unwrap_err();
        assert!(e.contains("algoritmo"), "{e}");

        // kid desconhecido logo depois de uma busca: não busca de novo
        // (um JWT forjado não faz o hub martelar a Cloudflare).
        let before = hits.load(std::sync::atomic::Ordering::SeqCst);
        let e = verify(&cf, jwt("b", "kb", claims("eu@exemplo.com", AUD)))
            .await
            .unwrap_err();
        assert!(e.contains("kid desconhecido"), "{e}");
        assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), before);
        // Passado o intervalo, o kid desconhecido recarrega as chaves.
        let cf = Arc::new(self::cf(&certs).with_retry(Duration::ZERO));
        assert!(verify(&cf, jwt("a", "ka", claims("eu@exemplo.com", AUD)))
            .await
            .is_ok());
        let before = hits.load(std::sync::atomic::Ordering::SeqCst);
        assert!(verify(&cf, jwt("b", "kb", claims("eu@exemplo.com", AUD)))
            .await
            .is_err());
        assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), before + 1);
        // Com a chave em cache, o kid conhecido não busca nada.
        assert!(verify(&cf, jwt("a", "ka", claims("eu@exemplo.com", AUD)))
            .await
            .is_ok());
        assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), before + 1);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn jwks_fora_do_ar_recusa() {
        let cf = Arc::new(cf("http://127.0.0.1:9/cdn-cgi/access/certs"));
        let e = verify(&cf, jwt("a", "ka", claims("eu@exemplo.com", AUD)))
            .await
            .unwrap_err();
        assert!(e.contains("kid desconhecido"), "{e}");
    }

    #[test]
    fn team_domain_vira_iss_e_endereco_das_chaves() {
        for d in [
            "equipe.cloudflareaccess.com",
            "https://equipe.cloudflareaccess.com/",
        ] {
            let c = CfAccess::new(d, "a", &["x@y".into()]);
            assert_eq!(c.issuer, ISS);
            assert_eq!(
                c.certs_url,
                "https://equipe.cloudflareaccess.com/cdn-cgi/access/certs"
            );
        }
    }

    #[test]
    fn bloqueio_dobra_de_1_min_ate_1_h_e_acerto_zera() {
        let t0 = Instant::now();
        let mut l = Lockout::default();
        for _ in 0..LOCK_AFTER - 1 {
            assert_eq!(l.fail(t0), None);
            assert_eq!(l.blocked(t0), None);
        }
        assert_eq!(l.fail(t0), Some(Duration::from_secs(60)));
        assert_eq!(l.blocked(t0), Some(Duration::from_secs(60)));
        assert_eq!(l.blocked(t0 + Duration::from_secs(60)), None);
        let mut t = t0 + Duration::from_secs(60);
        for want in [120, 240, 480, 960, 1920, 3600, 3600] {
            assert_eq!(l.fail(t), Some(Duration::from_secs(want)));
            t += Duration::from_secs(want);
        }
        l.success();
        assert_eq!(l.blocked(t), None);
        assert_eq!(l.fail(t), None);
    }

    fn cfg_ts(logins: &[&str]) -> WebConfig {
        WebConfig {
            tailscale: Some(TailscaleConfig {
                logins: logins.iter().map(|s| s.to_string()).collect(),
            }),
            ..WebConfig::default()
        }
    }

    fn headers(h: &[(&str, &str)]) -> HeaderMap {
        let mut m = HeaderMap::new();
        for (k, v) in h {
            m.append(
                axum::http::HeaderName::from_bytes(k.as_bytes()).unwrap(),
                v.parse().unwrap(),
            );
        }
        m
    }

    #[test]
    fn tailscale_so_do_loopback_e_sem_cf() {
        let a = WebAuth::new(hub(), UI, &cfg_ts(&["Eu@GitHub"]));
        let lo: Option<SocketAddr> = Some("127.0.0.1:50000".parse().unwrap());
        let lo6: Option<SocketAddr> = Some("[::1]:50000".parse().unwrap());
        let lan: Option<SocketAddr> = Some("100.64.0.7:50000".parse().unwrap());
        let ts = ("tailscale-user-login", "eu@github");
        assert_eq!(
            a.identity(&headers(&[ts]), lo),
            Ok(Some("eu@github".into()))
        );
        assert_eq!(
            a.identity(&headers(&[ts]), lo6),
            Ok(Some("eu@github".into()))
        );
        // De fora do loopback, sem endereço ou com Cf-*: ignorado.
        assert_eq!(a.identity(&headers(&[ts]), lan), Err(Denied::Proxy));
        assert_eq!(a.identity(&headers(&[ts]), None), Err(Denied::Proxy));
        for cf in ["cf-connecting-ip", "cf-ray", "Cf-Access-Jwt-Assertion"] {
            assert_eq!(
                a.identity(&headers(&[ts, (cf, "x")]), lo),
                Err(Denied::Proxy),
                "{cf}"
            );
        }
        // Login fora da lista ou ausente.
        assert_eq!(
            a.identity(&headers(&[("tailscale-user-login", "outro@github")]), lo),
            Err(Denied::Proxy)
        );
        assert_eq!(a.identity(&HeaderMap::new(), lo), Err(Denied::Proxy));
        // Sem [web.tailscale], o header não vale nada (nem recusa).
        let a = WebAuth::new(hub(), UI, &WebConfig::default());
        assert!(!a.proxy_configured());
        assert_eq!(a.identity(&headers(&[ts]), lan), Ok(None));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn com_os_dois_proxies_qualquer_identidade_valida_serve() {
        let (certs, _) = jwks_server().await;
        let cfg = WebConfig {
            cloudflare: Some(CloudflareConfig {
                team_domain: "equipe.cloudflareaccess.com".into(),
                aud: AUD.into(),
                emails: vec!["eu@exemplo.com".into()],
            }),
            ..cfg_ts(&["eu@github"])
        };
        let a = Arc::new(WebAuth::new(hub(), UI, &cfg).with_cloudflare(cf(&certs)));
        let lo: Option<SocketAddr> = Some("127.0.0.1:50000".parse().unwrap());
        let run = |h: HeaderMap| {
            let a = a.clone();
            tokio::task::spawn_blocking(move || a.identity(&h, lo))
        };
        let good = jwt("a", "ka", claims("eu@exemplo.com", AUD));
        let bad = jwt("b", "ka", claims("eu@exemplo.com", AUD));
        let r = run(headers(&[(CF_JWT_HEADER, &good)])).await.unwrap();
        assert_eq!(r, Ok(Some("eu@exemplo.com".into())));
        let r = run(headers(&[(CF_JWT_HEADER, &bad)])).await.unwrap();
        assert_eq!(r, Err(Denied::Proxy));
        // JWT inválido não é salvo pelo header do Tailscale.
        let r = run(headers(&[
            (CF_JWT_HEADER, &bad),
            (TAILSCALE_LOGIN_HEADER, "eu@github"),
        ]))
        .await
        .unwrap();
        assert_eq!(r, Err(Denied::Proxy));
        let r = run(headers(&[(TAILSCALE_LOGIN_HEADER, "eu@github")]))
            .await
            .unwrap();
        assert_eq!(r, Ok(Some("eu@github".into())));
        // Veio pela Cloudflare sem JWT: recusado.
        let r = run(headers(&[(CF_CONNECTING_IP, "1.2.3.4")]))
            .await
            .unwrap();
        assert_eq!(r, Err(Denied::Proxy));
    }

    #[test]
    fn login_com_token_cookie_e_bloqueio() {
        let a = WebAuth::new(hub(), UI, &WebConfig::default());
        let h = headers(&[("user-agent", "Mobile Safari")]);
        let s = a.login(UI, &h, None).unwrap();
        let sc = a.set_cookie(&s);
        assert!(sc.starts_with(&format!("ct_web={}.", s.id)), "{sc}");
        for attr in ["HttpOnly", "Secure", "SameSite=Strict", "Path=/"] {
            assert!(sc.contains(attr), "{sc}");
        }
        assert!(sc.ends_with(&format!("Max-Age={}", 30 * 24 * 3600)), "{sc}");
        let ok = headers(&[("cookie", &format!("x=1; ct_web={}", s.cookie))]);
        let got = a.authorize(&ok, None).unwrap();
        assert_eq!(got.id, s.id);
        assert!(a.still_valid(&s.id));
        assert_eq!(
            a.authorize(&headers(&[("cookie", "ct_web=abc.def")]), None),
            Err(Denied::Session)
        );
        assert_eq!(a.authorize(&HeaderMap::new(), None), Err(Denied::Session));

        // Cinco falhas seguidas bloqueiam, inclusive o token certo.
        for i in 1..=LOCK_AFTER {
            assert_eq!(
                a.login("errado", &h, None),
                Err(LoginError::BadToken),
                "{i}"
            );
        }
        assert_eq!(a.lockout_failures(), LOCK_AFTER);
        match a.login(UI, &h, None) {
            Err(LoginError::Locked(d)) => assert!(d <= Duration::from_secs(60)),
            r => panic!("{r:?}"),
        }
        // Durante o bloqueio, a tentativa nem é conferida nem conta.
        assert!(matches!(
            a.login("errado", &h, None),
            Err(LoginError::Locked(_))
        ));
        assert_eq!(a.lockout_failures(), LOCK_AFTER);
        // A sessão já criada segue valendo durante o bloqueio.
        assert!(a.authorize(&ok, None).is_ok());

        // Logout revoga a sessão.
        assert_eq!(a.logout(&ok, None), Ok(true));
        assert_eq!(a.authorize(&ok, None), Err(Denied::Session));
        assert_eq!(a.logout(&ok, None), Ok(false));
        assert!(WebAuth::clear_cookie().contains("Max-Age=0"));
    }

    #[test]
    fn me_responde_estado_da_sessao() {
        let a = WebAuth::new(hub(), UI, &WebConfig::default());
        let me = a.me(&HeaderMap::new(), None).unwrap();
        assert_eq!(
            me,
            WebMe {
                authenticated: false,
                identity: None,
                expires_at: None
            }
        );
        let s = a.login(UI, &HeaderMap::new(), None).unwrap();
        let me = a
            .me(
                &headers(&[("cookie", &format!("ct_web={}", s.cookie))]),
                None,
            )
            .unwrap();
        assert!(me.authenticated);
        assert_eq!(me.expires_at, Some(s.expires_at));
    }

    #[test]
    fn cookie_extraido_do_header() {
        let h = headers(&[("cookie", "a=1; ct_web=x.y ; b=2"), ("cookie", "c=3")]);
        assert_eq!(WebAuth::cookie(&h).as_deref(), Some("x.y"));
        let h = headers(&[("cookie", "ct_webx=1; xct_web=2")]);
        assert_eq!(WebAuth::cookie(&h), None);
        assert_eq!(WebAuth::cookie(&headers(&[("cookie", "ct_web=")])), None);
    }

    #[test]
    fn token_comparado_exato() {
        assert!(token_ok("abc", "abc"));
        assert!(!token_ok("abd", "abc"));
        assert!(!token_ok("abcd", "abc"));
        assert!(!token_ok("", ""));
    }
}
