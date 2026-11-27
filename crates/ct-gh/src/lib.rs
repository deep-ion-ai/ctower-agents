//! Cliente do GitHub (spec §7.5): leitura com ETag e paginação, e uma única
//! escrita, a remoção de branch da limpeza de issues concluídas ([`refs`],
//! #286). Token: o de um [`TokenProvider`] (no hub e no ambiente, o
//! resolvedor de credencial a partir da conexão `github` do projeto, #275),
//! um token fixo ou, sem nenhum dos dois, o do `gh auth token`, obtido sob
//! demanda.
//! Síncrono; quem chama roda fora do runtime assíncrono.

pub mod deliveries;
pub mod refs;
pub mod release;
pub mod reviews;
pub mod sync;
pub mod time;

#[cfg(any(test, feature = "test-support"))]
pub mod fake;

use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Prazo padrão para o `gh auth token` ([`GhConfig::token_timeout`]).
pub const DEFAULT_TOKEN_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone)]
pub struct GhConfig {
    /// Raiz da API, sem barra no fim (`https://api.github.com`).
    pub api_url: String,
    /// Binário do `gh`: o `gh auth token` do cliente sem [`TokenProvider`]
    /// e a fonte `cli` das conexões (`<gh_bin> auth token`).
    pub gh_bin: String,
    pub timeout: Duration,
    /// Prazo para o `gh auth token` terminar (padrão
    /// [`DEFAULT_TOKEN_TIMEOUT`]); passado dele, o `gh` é morto.
    pub token_timeout: Duration,
    /// Token fixo (o exclusivo da sessão de release, #204): nunca cai para o
    /// `gh auth token`.
    pub token: Option<String>,
}

impl GhConfig {
    pub fn new(api_url: &str, gh_bin: &str) -> Self {
        GhConfig {
            api_url: api_url.trim_end_matches('/').to_string(),
            gh_bin: gh_bin.to_string(),
            timeout: Duration::from_secs(30),
            token_timeout: DEFAULT_TOKEN_TIMEOUT,
            token: None,
        }
    }

    /// Usa sempre este token ([`GhConfig::token`]).
    pub fn with_token(mut self, token: String) -> Self {
        self.token = Some(token);
        self
    }

    pub fn with_token_timeout(mut self, timeout: Duration) -> Self {
        self.token_timeout = timeout;
        self
    }

    /// Comando da fonte `cli` das conexões: o `gh auth token` do `gh`
    /// configurado, com o caminho como palavra do shell.
    pub fn cli_cmd(&self) -> String {
        format!("{} auth token", ct_core::provider::shell_word(&self.gh_bin))
    }
}

/// De onde o cliente tira o token (#275): no hub e no ambiente, o
/// resolvedor de credencial com a fonte da conexão `github` do projeto. O
/// cache é de quem implementa; o cliente pede o token a cada requisição.
pub trait TokenProvider: Send + Sync {
    /// O token de agora (do cache ou resolvido agora). O erro não leva o
    /// valor.
    fn token(&self) -> Result<String, GhError>;
    /// O GitHub recusou o token (`401`): a próxima chamada resolve de novo.
    fn invalidate(&self);
}

/// Origem do token de um [`GhClient`].
#[derive(Clone)]
enum Auth {
    /// [`GhConfig::token`].
    Fixed(String),
    /// `gh auth token` do [`GhConfig::gh_bin`], guardado aqui.
    Gh(Arc<Mutex<Option<String>>>),
    Provider(Arc<dyn TokenProvider>),
}

/// Resposta de um GET condicional.
#[derive(Debug, Clone, PartialEq)]
pub enum Fetched<T> {
    /// `304`: nada mudou desde o ETag informado.
    NotModified,
    Modified {
        body: T,
        etag: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum GhError {
    #[error("token do GitHub indisponível: {0}")]
    Token(String),
    #[error("falha de rede: {0}")]
    Network(String),
    #[error("GitHub respondeu {status}: {message}")]
    Http { status: u16, message: String },
    #[error("rate limit do GitHub esgotado")]
    RateLimited {
        /// Quando o limite volta (ms Unix), se informado.
        reset_at: Option<i64>,
    },
    #[error("resposta inesperada do GitHub: {0}")]
    Parse(String),
}

/// Cliente da API. Clonar compartilha a conexão HTTP e o cache de logins;
/// [`GhClient::with_provider`] dá um cliente com outra origem de token.
#[derive(Clone)]
pub struct GhClient {
    cfg: GhConfig,
    agent: ureq::Agent,
    auth: Auth,
    /// Token → login dos `GET /user` que deram certo (#149): o login só é
    /// pedido de novo quando o token muda. Compartilhado entre os clientes
    /// derivados (um por projeto, #275).
    login: Arc<Mutex<HashMap<String, String>>>,
}

/// Teto de logins guardados: passou, o cache recomeça.
const MAX_LOGINS: usize = 32;

struct Page {
    body: Value,
    etag: Option<String>,
    next: Option<String>,
}

impl GhClient {
    pub fn new(cfg: GhConfig) -> Self {
        let agent = ureq::AgentBuilder::new().timeout(cfg.timeout).build();
        let auth = match &cfg.token {
            Some(t) => Auth::Fixed(t.clone()),
            None => Auth::Gh(Arc::new(Mutex::new(None))),
        };
        GhClient {
            cfg,
            agent,
            auth,
            login: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// O mesmo cliente (API, conexão HTTP e cache de logins) com o token de
    /// `provider` (#275).
    pub fn with_provider(&self, provider: Arc<dyn TokenProvider>) -> GhClient {
        GhClient {
            auth: Auth::Provider(provider),
            ..self.clone()
        }
    }

    pub fn config(&self) -> &GhConfig {
        &self.cfg
    }

    /// GET de uma listagem (`path` relativo à API, com a query). Segue
    /// `Link: rel="next"` e concatena os arrays; só a primeira página leva
    /// `If-None-Match`, e o ETag devolvido é o dela.
    pub fn get_all(&self, path: &str, etag: Option<&str>) -> Result<Fetched<Vec<Value>>, GhError> {
        let url = format!("{}{}", self.cfg.api_url, path);
        let Some(first) = self.get(&url, etag)? else {
            return Ok(Fetched::NotModified);
        };
        let mut items = into_array(first.body)?;
        let mut next = first.next;
        while let Some(n) = next {
            // O token só vai para a API configurada.
            if !n.starts_with(&format!("{}/", self.cfg.api_url)) {
                return Err(GhError::Parse(format!(
                    "link de paginação fora da API: {n}"
                )));
            }
            let page = self
                .get(&n, None)?
                .ok_or_else(|| GhError::Parse("304 sem If-None-Match".into()))?;
            items.extend(into_array(page.body)?);
            next = page.next;
        }
        Ok(Fetched::Modified {
            body: items,
            etag: first.etag,
        })
    }

    /// GET condicional de um recurso só (`path` relativo à API), sem seguir
    /// paginação: os checks do commit (#129), que vêm num objeto.
    pub fn get_one(&self, path: &str, etag: Option<&str>) -> Result<Fetched<Value>, GhError> {
        let url = format!("{}{}", self.cfg.api_url, path);
        Ok(match self.get(&url, etag)? {
            None => Fetched::NotModified,
            Some(page) => Fetched::Modified {
                body: page.body,
                etag: page.etag,
            },
        })
    }

    /// Trecho final do log de um job do GitHub Actions (#129, para a #133):
    /// `GET /repos/{owner}/{repo}/actions/jobs/{job_id}/logs`, que redireciona
    /// para um endereço temporário (seguido sem o token). Lê o log inteiro em
    /// fluxo e guarda só os últimos `max_bytes`, a partir do começo de uma
    /// linha. `None` = o log não existe mais ou o token não tem acesso
    /// (`404`/`410`).
    pub fn job_log_tail(
        &self,
        owner: &str,
        repo: &str,
        job_id: i64,
        max_bytes: usize,
    ) -> Result<Option<String>, GhError> {
        let url = format!(
            "{}{}",
            self.cfg.api_url,
            sync::job_logs_path(owner, repo, job_id)
        );
        let mut renewed = false;
        let resp = loop {
            match self.request("GET", &url)?.call() {
                Ok(r) => break r,
                Err(ureq::Error::Status(401, _)) if !renewed => {
                    self.discard_token();
                    renewed = true;
                }
                Err(ureq::Error::Status(404 | 410, _)) => return Ok(None),
                Err(ureq::Error::Status(code, r)) => return Err(status_error(code, r)),
                Err(ureq::Error::Transport(t)) => return Err(GhError::Network(t.to_string())),
            }
        };
        let tail = read_tail(resp.into_reader(), max_bytes)
            .map_err(|e| GhError::Network(e.to_string()))?;
        Ok(Some(tail))
    }

    /// GET de um recurso (`path` relativo à API). `None` = `404` (não existe
    /// ou o token não tem acesso).
    pub fn get_json(&self, path: &str) -> Result<Option<Value>, GhError> {
        let url = format!("{}{}", self.cfg.api_url, path);
        match self.get(&url, None) {
            Ok(Some(page)) => Ok(Some(page.body)),
            Ok(None) => Err(GhError::Parse("304 sem If-None-Match".into())),
            Err(GhError::Http { status: 404, .. }) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// DELETE de um recurso (`path` relativo à API). `Ok(true)` = apagado
    /// (`2xx`); `Ok(false)` = já não existia (`404`, ou o `422` "Reference
    /// does not exist" das referências). Um `401` descarta o token e tenta
    /// uma vez mais.
    pub fn delete(&self, path: &str) -> Result<bool, GhError> {
        let url = format!("{}{}", self.cfg.api_url, path);
        let mut renewed = false;
        loop {
            return match self.request("DELETE", &url)?.call() {
                Ok(_) => Ok(true),
                Err(ureq::Error::Status(401, _)) if !renewed => {
                    self.discard_token();
                    renewed = true;
                    continue;
                }
                Err(ureq::Error::Status(code, r)) => match status_error(code, r) {
                    GhError::Http { status: 404, .. } => Ok(false),
                    GhError::Http {
                        status: 422,
                        message,
                    } if message.contains("does not exist") => Ok(false),
                    e => Err(e),
                },
                Err(ureq::Error::Transport(t)) => Err(GhError::Network(t.to_string())),
            };
        }
    }

    /// POST de `body` (`path` relativo à API); devolve a resposta. Um `401`
    /// descarta o token e tenta uma vez mais.
    pub fn post_json(&self, path: &str, body: &Value) -> Result<Value, GhError> {
        self.post_url(&format!("{}{}", self.cfg.api_url, path), body)
    }

    /// PUT de `body` (`path` relativo à API); devolve a resposta (o merge de
    /// PR, #204). Um `401` descarta o token e tenta uma vez mais.
    pub fn put_json(&self, path: &str, body: &Value) -> Result<Value, GhError> {
        self.send_json("PUT", path, body)
    }

    /// PATCH de `body` (`path` relativo à API); devolve a resposta.
    pub fn patch_json(&self, path: &str, body: &Value) -> Result<Value, GhError> {
        self.send_json("PATCH", path, body)
    }

    /// Requisição com corpo JSON ([`json_or_null`]).
    fn send_json(&self, method: &str, path: &str, body: &Value) -> Result<Value, GhError> {
        let url = format!("{}{}", self.cfg.api_url, path);
        let mut renewed = false;
        loop {
            return match self.request(method, &url)?.send_json(body) {
                Ok(r) => json_or_null(r),
                Err(ureq::Error::Status(401, _)) if !renewed => {
                    self.discard_token();
                    renewed = true;
                    continue;
                }
                Err(ureq::Error::Status(code, r)) => Err(status_error(code, r)),
                Err(ureq::Error::Transport(t)) => Err(GhError::Network(t.to_string())),
            };
        }
    }

    /// Endpoint GraphQL da API configurada: `<api>/graphql` no github.com,
    /// `<host>/api/graphql` no GitHub Enterprise (REST em `<host>/api/v3`).
    pub fn graphql_url(&self) -> String {
        match self.cfg.api_url.strip_suffix("/api/v3") {
            Some(host) => format!("{host}/api/graphql"),
            None => format!("{}/graphql", self.cfg.api_url),
        }
    }

    /// Consulta GraphQL; devolve o `data`. Resposta com `errors` vira erro:
    /// `RATE_LIMITED` em [`GhError::RateLimited`], `NOT_FOUND` num `404` e o
    /// resto num [`GhError::Parse`] com as mensagens.
    pub fn graphql(&self, query: &str, variables: &Value) -> Result<Value, GhError> {
        let v = self.post_url(
            &self.graphql_url(),
            &serde_json::json!({ "query": query, "variables": variables }),
        )?;
        let errors: Vec<&Value> = v
            .get("errors")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .collect();
        if !errors.is_empty() {
            let kind = |t: &str| {
                errors
                    .iter()
                    .any(|e| e.get("type").and_then(Value::as_str) == Some(t))
            };
            let message = errors
                .iter()
                .filter_map(|e| e.get("message").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("; ");
            return Err(if kind("RATE_LIMITED") {
                GhError::RateLimited { reset_at: None }
            } else if kind("NOT_FOUND") {
                GhError::Http {
                    status: 404,
                    message,
                }
            } else {
                GhError::Parse(format!("GraphQL: {}", truncate(&message, 300)))
            });
        }
        v.get("data")
            .filter(|d| !d.is_null())
            .cloned()
            .ok_or_else(|| GhError::Parse("GraphQL sem data".into()))
    }

    fn post_url(&self, url: &str, body: &Value) -> Result<Value, GhError> {
        let mut renewed = false;
        loop {
            return match self.request("POST", url)?.send_json(body) {
                Ok(r) => json_or_null(r),
                Err(ureq::Error::Status(401, _)) if !renewed => {
                    self.discard_token();
                    renewed = true;
                    continue;
                }
                Err(ureq::Error::Status(code, r)) => Err(status_error(code, r)),
                Err(ureq::Error::Transport(t)) => Err(GhError::Network(t.to_string())),
            };
        }
    }

    /// Comenta no PR `number` de `owner/repo` (comentário de issue, o da
    /// conversa do PR). Devolve o id e a URL do comentário.
    pub fn comment_pr(
        &self,
        owner: &str,
        repo: &str,
        number: i64,
        body: &str,
    ) -> Result<PrComment, GhError> {
        let v = self.post_json(
            &format!("/repos/{owner}/{repo}/issues/{number}/comments"),
            &serde_json::json!({ "body": body }),
        )?;
        Ok(PrComment {
            id: v.get("id").and_then(Value::as_i64).unwrap_or_default(),
            url: v
                .get("html_url")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        })
    }

    /// Requisição autenticada para a API.
    fn request(&self, method: &str, url: &str) -> Result<ureq::Request, GhError> {
        let token = self.token()?;
        Ok(self
            .agent
            .request(method, url)
            .set("Authorization", &format!("Bearer {token}"))
            .set("Accept", "application/vnd.github+json")
            .set("X-GitHub-Api-Version", "2022-11-28")
            .set("User-Agent", concat!("ctowerd/", env!("CARGO_PKG_VERSION"))))
    }

    /// Um GET; `None` = `304`. Um `401` descarta o token e tenta uma vez mais.
    fn get(&self, url: &str, etag: Option<&str>) -> Result<Option<Page>, GhError> {
        let mut renewed = false;
        loop {
            let mut req = self.request("GET", url)?;
            if let Some(e) = etag {
                req = req.set("If-None-Match", e);
            }
            return match req.call() {
                Ok(r) if r.status() == 304 => Ok(None),
                Ok(r) => {
                    let etag = r.header("etag").map(String::from);
                    let next = r.header("link").and_then(next_link);
                    let body = r
                        .into_json::<Value>()
                        .map_err(|e| GhError::Parse(e.to_string()))?;
                    Ok(Some(Page { body, etag, next }))
                }
                Err(ureq::Error::Status(401, _)) if !renewed => {
                    self.discard_token();
                    renewed = true;
                    continue;
                }
                Err(ureq::Error::Status(code, r)) => Err(status_error(code, r)),
                Err(ureq::Error::Transport(t)) => Err(GhError::Network(t.to_string())),
            };
        }
    }

    /// Token da origem do cliente: o fixo, o do provedor ou o do `gh auth
    /// token` (guardado até um `401`).
    pub fn token(&self) -> Result<String, GhError> {
        match &self.auth {
            Auth::Fixed(t) => Ok(t.clone()),
            Auth::Provider(p) => p.token(),
            Auth::Gh(cache) => {
                let mut t = cache.lock().unwrap();
                if let Some(tok) = t.as_ref() {
                    return Ok(tok.clone());
                }
                let tok = gh_token(&self.cfg.gh_bin, self.cfg.token_timeout)?;
                *t = Some(tok.clone());
                Ok(tok)
            }
        }
    }

    /// O GitHub recusou o token (`401`): a próxima requisição pede outro.
    fn discard_token(&self) {
        match &self.auth {
            Auth::Fixed(_) => {}
            Auth::Provider(p) => p.invalidate(),
            Auth::Gh(cache) => *cache.lock().unwrap() = None,
        }
    }

    /// Login da conta do token em uso: o "eu" do GitHub no filtro "só
    /// minhas" (#149), que o sync grava no `sync_state.me`. Pede `GET /user`
    /// só quando o token muda; a falha não fica guardada.
    pub fn login(&self) -> Result<String, GhError> {
        let token = self.token()?;
        if let Some(l) = self.login.lock().unwrap().get(&token) {
            return Ok(l.clone());
        }
        let login = self.login_of(&token)?;
        let mut cache = self.login.lock().unwrap();
        if cache.len() >= MAX_LOGINS {
            cache.clear();
        }
        cache.insert(token, login.clone());
        Ok(login)
    }

    /// Login da conta dona de `token` (GET /user): confere um token antes
    /// de gravá-lo.
    pub fn login_of(&self, token: &str) -> Result<String, GhError> {
        let r = self
            .agent
            .get(&format!("{}/user", self.cfg.api_url))
            .set("Authorization", &format!("Bearer {token}"))
            .set("Accept", "application/vnd.github+json")
            .set("User-Agent", concat!("ctowerd/", env!("CARGO_PKG_VERSION")))
            .call();
        match r {
            Ok(r) => r
                .into_json::<Value>()
                .ok()
                .and_then(|v| v.get("login").and_then(Value::as_str).map(String::from))
                .ok_or_else(|| GhError::Parse("GET /user sem login".into())),
            Err(ureq::Error::Status(code, r)) => Err(status_error(code, r)),
            Err(ureq::Error::Transport(t)) => Err(GhError::Network(t.to_string())),
        }
    }
}

/// Comentário criado por [`GhClient::comment_pr`] (`github.pr.comment`).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct PrComment {
    pub id: i64,
    /// `html_url` do comentário.
    pub url: String,
}

/// Os últimos `max` bytes de `r`, em fluxo, a partir do começo de uma
/// linha quando o trecho foi cortado. Bytes inválidos em UTF-8 viram `�`.
fn read_tail(mut r: impl Read, max: usize) -> std::io::Result<String> {
    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 64 * 1024];
    let mut cut = false;
    loop {
        let n = r.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        // Descarta o excesso só de vez em quando (o dobro do teto).
        if buf.len() > max.saturating_mul(2).max(chunk.len()) {
            let drop = buf.len() - max;
            buf.drain(..drop);
            cut = true;
        }
    }
    if buf.len() > max {
        buf.drain(..buf.len() - max);
        cut = true;
    }
    if cut {
        if let Some(nl) = buf.iter().position(|b| *b == b'\n') {
            buf.drain(..=nl);
        }
    }
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

fn into_array(v: Value) -> Result<Vec<Value>, GhError> {
    match v {
        Value::Array(a) => Ok(a),
        other => Err(GhError::Parse(format!(
            "esperada uma lista, veio {}",
            truncate(&other.to_string(), 200)
        ))),
    }
}

fn status_error(code: u16, r: ureq::Response) -> GhError {
    let remaining = r.header("x-ratelimit-remaining").map(str::to_string);
    let reset = r
        .header("x-ratelimit-reset")
        .and_then(|v| v.parse::<i64>().ok());
    let retry_after = r.header("retry-after").and_then(|v| v.parse::<i64>().ok());
    if matches!(code, 403 | 429) && (remaining.as_deref() == Some("0") || retry_after.is_some()) {
        let reset_at = reset
            .map(|s| s * 1000)
            .or_else(|| retry_after.map(|s| ct_core::now_ms() + s * 1000));
        return GhError::RateLimited { reset_at };
    }
    let text = r.into_string().unwrap_or_default();
    let message = serde_json::from_str::<Value>(&text)
        .ok()
        .and_then(|v| api_message(&v))
        .unwrap_or_else(|| truncate(&text, 200));
    GhError::Http {
        status: code,
        message,
    }
}

/// `message` do corpo de erro da API, com o detalhe de `errors[].message`
/// quando houver: o `422` diz só "Validation Failed" no `message`, e o motivo
/// (ex.: "No commits between develop and wip/autonoma") vem em `errors`.
fn api_message(v: &Value) -> Option<String> {
    let message = v.get("message").and_then(Value::as_str)?;
    let details: Vec<&str> = v
        .get("errors")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|e| e.get("message").and_then(Value::as_str))
        .filter(|m| !m.is_empty())
        .collect();
    Some(if details.is_empty() {
        message.to_string()
    } else {
        format!("{message}: {}", details.join("; "))
    })
}

/// URL com `rel="next"` de um cabeçalho `Link`.
fn next_link(h: &str) -> Option<String> {
    h.split(',').find_map(|part| {
        let (url, params) = part.trim().split_once(';')?;
        params
            .split(';')
            .any(|p| p.trim() == "rel=\"next\"")
            .then(|| {
                url.trim()
                    .trim_start_matches('<')
                    .trim_end_matches('>')
                    .to_string()
            })
    })
}

fn truncate(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

/// Corpo JSON da resposta; vazio (`204`, ou o `201` sem corpo do
/// `.../approve` de um run) vira `null`.
fn json_or_null(r: ureq::Response) -> Result<Value, GhError> {
    let text = r.into_string().map_err(|e| GhError::Parse(e.to_string()))?;
    if text.trim().is_empty() {
        Ok(Value::Null)
    } else {
        serde_json::from_str(&text).map_err(|e| GhError::Parse(e.to_string()))
    }
}

/// `gh auth token`, com prazo.
fn gh_token(gh: &str, timeout: Duration) -> Result<String, GhError> {
    let mut tentativas = 0;
    let mut child = loop {
        let spawned = Command::new(gh)
            .args(["auth", "token"])
            .env("GH_PROMPT_DISABLED", "1")
            .env("GH_NO_UPDATE_NOTIFIER", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn();
        match spawned {
            Ok(c) => break c,
            // ETXTBSY: o executável acabou de ser gravado e o descritor de
            // escrita ainda vive num `fork` concorrente; some após o `exec`.
            Err(e) if e.raw_os_error() == Some(26) && tentativas < 50 => {
                tentativas += 1;
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(e) => return Err(GhError::Token(format!("{gh}: {e}"))),
        }
    };
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(GhError::Token(format!("{gh} auth token: prazo esgotado")));
            }
            Err(e) => return Err(GhError::Token(e.to_string())),
        }
    };
    let mut out = String::new();
    if let Some(mut so) = child.stdout.take() {
        let _ = so.read_to_string(&mut out);
    }
    let tok = out.trim();
    if !status.success() || tok.is_empty() {
        return Err(GhError::Token(format!(
            "{gh} auth token falhou ({status}); rode `gh auth login` no ambiente do daemon"
        )));
    }
    Ok(tok.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::{fake_gh, fake_gh_calls, FakeGitHub};
    use serde_json::json;

    /// Provedor de token de teste: devolve o token da vez e conta os
    /// descartes.
    struct Provedor {
        token: Mutex<String>,
        descartes: Mutex<usize>,
    }

    impl Provedor {
        fn new(token: &str) -> Arc<Self> {
            Arc::new(Provedor {
                token: Mutex::new(token.into()),
                descartes: Mutex::new(0),
            })
        }
    }

    impl TokenProvider for Provedor {
        fn token(&self) -> Result<String, GhError> {
            Ok(self.token.lock().unwrap().clone())
        }
        fn invalidate(&self) {
            *self.descartes.lock().unwrap() += 1;
        }
    }

    /// #149: o login do token é pedido uma vez e de novo só quando o token
    /// muda; a falha não fica guardada. O cache vale entre os clientes
    /// derivados (#275).
    #[test]
    fn login_do_token_em_cache() {
        let gh = FakeGitHub::start("ghp_ficticio_a");
        let base = GhClient::new(GhConfig::new(&gh.url(), "/nao/existe/gh"));
        let p = Provedor::new("ghp_ficticio_a");
        let c = base.with_provider(p.clone());
        assert!(c.login().is_err(), "sem /user publicado");
        gh.set("/user", json!({ "login": "fulano" }));
        assert_eq!(c.login().unwrap(), "fulano");
        assert_eq!(c.login().unwrap(), "fulano");
        let outro = base.with_provider(Provedor::new("ghp_ficticio_a"));
        assert_eq!(outro.login().unwrap(), "fulano");
        let users = || gh.requests().iter().filter(|(p, _)| p == "/user").count();
        assert_eq!(users(), 2, "a falha e o primeiro sucesso");
        *p.token.lock().unwrap() = "ghp_ficticio_b".into();
        assert!(c.login().is_err(), "token novo: pede de novo");
        assert_eq!(users(), 3);
    }

    /// #275: o cliente derivado usa o token do provedor, não o `gh`, e o
    /// `401` descarta o token no provedor e tenta uma vez mais.
    #[test]
    fn provedor_de_token_e_401() {
        let d = tempfile::tempdir().unwrap();
        let gh = FakeGitHub::start("ghp_ficticio_conexao");
        gh.set("/repos/acme/acme-web/issues", json!([]));
        let bin = fake_gh(d.path(), "ghp_ficticio_do_gh");
        let base = GhClient::new(
            GhConfig::new(&gh.url(), bin.to_str().unwrap())
                .with_token_timeout(crate::fake::FAKE_GH_TOKEN_TIMEOUT),
        );
        let p = Provedor::new("ghp_ficticio_conexao");
        let c = base.with_provider(p.clone());
        assert!(c.get_all("/repos/acme/acme-web/issues", None).is_ok());
        assert_eq!(fake_gh_calls(d.path()), 0, "o gh não é consultado");
        assert_eq!(*p.descartes.lock().unwrap(), 0);

        gh.fail_next("/repos/acme/acme-web/issues", 401, &[]);
        assert!(c.get_all("/repos/acme/acme-web/issues", None).is_ok());
        assert_eq!(*p.descartes.lock().unwrap(), 1);

        // Token recusado sempre: um descarte por chamada, e o 401 volta.
        *p.token.lock().unwrap() = "ghp_ficticio_velho".into();
        let err = c.get_all("/repos/acme/acme-web/issues", None).unwrap_err();
        assert!(matches!(err, GhError::Http { status: 401, .. }), "{err}");
        assert_eq!(*p.descartes.lock().unwrap(), 2);
        // O cliente base segue com o `gh`.
        assert!(base.get_all("/repos/acme/acme-web/issues", None).is_err());
        assert_eq!(fake_gh_calls(d.path()), 2);
    }

    #[test]
    fn sem_gh_o_token_falha() {
        let gh = FakeGitHub::start("tok");
        let c = GhClient::new(GhConfig::new(&gh.url(), "/nao/existe/gh"));
        assert!(matches!(c.token(), Err(GhError::Token(_))));
        assert!(matches!(c.get_all("/x", None), Err(GhError::Token(_))));
    }

    #[test]
    fn comando_da_fonte_cli_usa_o_gh_configurado() {
        assert_eq!(GhConfig::new("x", "gh").cli_cmd(), "gh auth token");
        assert_eq!(
            GhConfig::new("x", "/opt/acme tools/gh").cli_cmd(),
            "'/opt/acme tools/gh' auth token"
        );
    }

    #[test]
    fn gh_travado_estoura_o_prazo_do_config() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let bin = d.path().join("gh");
        std::fs::write(&bin, "#!/bin/sh\nexec sleep 30\n").unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        let c = GhClient::new(
            GhConfig::new("http://127.0.0.1:9", bin.to_str().unwrap())
                .with_token_timeout(Duration::from_millis(200)),
        );
        let t = Instant::now();
        let err = c.get_all("/x", None).unwrap_err();
        assert!(
            matches!(&err, GhError::Token(m) if m.ends_with("prazo esgotado")),
            "{err:?}"
        );
        assert!(t.elapsed() < Duration::from_secs(10), "{:?}", t.elapsed());
    }

    fn client(gh: &FakeGitHub, dir: &std::path::Path, token: &str) -> GhClient {
        let bin = fake_gh(dir, token);
        GhClient::new(
            GhConfig::new(&gh.url(), bin.to_str().unwrap())
                .with_token_timeout(crate::fake::FAKE_GH_TOKEN_TIMEOUT),
        )
    }

    #[test]
    fn get_condicional_com_etag() {
        let d = tempfile::tempdir().unwrap();
        let gh = FakeGitHub::start("tok");
        gh.set("/repos/o/r/issues?state=open", json!([{ "number": 1 }]));
        let c = client(&gh, d.path(), "tok");
        let Fetched::Modified { body, etag } =
            c.get_all("/repos/o/r/issues?state=open", None).unwrap()
        else {
            panic!("esperado 200")
        };
        assert_eq!(body, vec![json!({ "number": 1 })]);
        let etag = etag.expect("etag");
        assert_eq!(
            c.get_all("/repos/o/r/issues?state=open", Some(&etag))
                .unwrap(),
            Fetched::NotModified
        );
        assert_eq!(gh.requests()[1].1.as_deref(), Some(etag.as_str()));
        // Corpo mudou: ETag antigo não serve mais.
        gh.set("/repos/o/r/issues?state=open", json!([{ "number": 2 }]));
        assert!(matches!(
            c.get_all("/repos/o/r/issues?state=open", Some(&etag))
                .unwrap(),
            Fetched::Modified { .. }
        ));
        // Token buscado uma vez só.
        assert_eq!(fake_gh_calls(d.path()), 1);
    }

    #[test]
    fn segue_paginacao() {
        let d = tempfile::tempdir().unwrap();
        let gh = FakeGitHub::start("tok");
        gh.set_pages(
            "/x?per_page=2",
            vec![json!([1, 2]), json!([3, 4]), json!([5])],
        );
        let c = client(&gh, d.path(), "tok");
        let Fetched::Modified { body, .. } = c.get_all("/x?per_page=2", None).unwrap() else {
            panic!()
        };
        assert_eq!(body, vec![json!(1), json!(2), json!(3), json!(4), json!(5)]);
        // Só a primeira página com If-None-Match.
        let r = gh.requests();
        assert_eq!(r.len(), 3);
        assert!(r[1].1.is_none() && r[2].1.is_none());
    }

    #[test]
    fn link_para_outro_host_nao_e_seguido() {
        let d = tempfile::tempdir().unwrap();
        let gh = FakeGitHub::start("tok");
        gh.set("/x", json!([1]));
        gh.set_link("/x", "https://evil.example/x?page=2");
        let c = client(&gh, d.path(), "tok");
        let err = c.get_all("/x", None).unwrap_err();
        assert!(
            matches!(err, GhError::Parse(ref m) if m.contains("evil.example")),
            "{err}"
        );
    }

    #[test]
    fn renova_token_apos_401() {
        let d = tempfile::tempdir().unwrap();
        let gh = FakeGitHub::start("tok");
        gh.set("/x", json!([]));
        let c = client(&gh, d.path(), "tok");
        gh.fail_next("/x", 401, &[]);
        assert!(c.get_all("/x", None).is_ok());
        assert_eq!(fake_gh_calls(d.path()), 2);
    }

    #[test]
    fn token_errado_persistente_e_401() {
        let d = tempfile::tempdir().unwrap();
        let gh = FakeGitHub::start("tok");
        gh.set("/x", json!([]));
        let c = client(&gh, d.path(), "outro");
        let err = c.get_all("/x", None).unwrap_err();
        assert!(matches!(err, GhError::Http { status: 401, .. }), "{err}");
        assert_eq!(fake_gh_calls(d.path()), 2);
    }

    #[test]
    fn rate_limit_e_erros_http() {
        let d = tempfile::tempdir().unwrap();
        let gh = FakeGitHub::start("tok");
        gh.set("/x", json!([]));
        let c = client(&gh, d.path(), "tok");
        gh.fail_next(
            "/x",
            403,
            &[
                ("x-ratelimit-remaining", "0"),
                ("x-ratelimit-reset", "1792108032"),
            ],
        );
        assert_eq!(
            c.get_all("/x", None).unwrap_err(),
            GhError::RateLimited {
                reset_at: Some(1_792_108_032_000)
            }
        );
        gh.fail_next("/x", 429, &[("retry-after", "60")]);
        assert!(matches!(
            c.get_all("/x", None).unwrap_err(),
            GhError::RateLimited { reset_at: Some(_) }
        ));
        // 403 sem sinal de rate limit é erro comum, com a mensagem da API.
        gh.fail_next("/x", 403, &[]);
        assert!(matches!(
            c.get_all("/x", None).unwrap_err(),
            GhError::Http { status: 403, .. }
        ));
        let err = c.get_all("/nao-existe", None).unwrap_err();
        assert_eq!(
            err,
            GhError::Http {
                status: 404,
                message: "Not Found".into()
            }
        );
        // 422 de validação: o motivo em `errors[].message` entra na mensagem.
        gh.fail_next_with(
            "/x",
            422,
            json!({ "message": "Validation Failed", "errors": [
                { "resource": "PullRequest", "code": "custom",
                  "message": "No commits between develop and wip/autonoma" }
            ] }),
        );
        assert_eq!(
            c.get_all("/x", None).unwrap_err(),
            GhError::Http {
                status: 422,
                message: "Validation Failed: No commits between develop and wip/autonoma".into()
            }
        );
    }

    #[test]
    fn sem_gh_ou_sem_servidor() {
        let c = GhClient::new(GhConfig::new("http://127.0.0.1:9", "/nao/existe/gh"));
        assert!(matches!(c.get_all("/x", None), Err(GhError::Token(_))));
        let d = tempfile::tempdir().unwrap();
        let bin = fake_gh(d.path(), "tok");
        // Porta fechada: falha de rede.
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let c = GhClient::new(
            GhConfig::new(&format!("http://127.0.0.1:{port}"), bin.to_str().unwrap())
                .with_token_timeout(crate::fake::FAKE_GH_TOKEN_TIMEOUT),
        );
        assert!(matches!(c.get_all("/x", None), Err(GhError::Network(_))));
    }

    #[test]
    fn comenta_no_pr() {
        let d = tempfile::tempdir().unwrap();
        let gh = FakeGitHub::start("tok");
        let path = "/repos/o/r/issues/7/comments";
        gh.set(
            path,
            json!({ "id": 99, "html_url": "https://github.com/o/r/pull/7#issuecomment-99" }),
        );
        let c = client(&gh, d.path(), "tok");
        let got = c.comment_pr("o", "r", 7, "texto").unwrap();
        assert_eq!(got.id, 99);
        assert!(got.url.ends_with("issuecomment-99"));
        assert_eq!(
            gh.posted(),
            vec![(path.to_string(), json!({ "body": "texto" }))]
        );
        assert!(matches!(
            c.comment_pr("o", "r", 8, "x").unwrap_err(),
            GhError::Http { status: 404, .. }
        ));
    }

    #[test]
    fn link_next() {
        let h = r#"<https://api.github.com/x?page=2>; rel="next", <https://api.github.com/x?page=9>; rel="last""#;
        assert_eq!(
            next_link(h).as_deref(),
            Some("https://api.github.com/x?page=2")
        );
        assert_eq!(next_link(r#"<https://a/x?page=1>; rel="prev""#), None);
    }

    fn client_for(gh: &FakeGitHub, d: &std::path::Path) -> GhClient {
        let bin = fake_gh(d, "tok");
        GhClient::new(
            GhConfig::new(&gh.url(), bin.to_str().unwrap())
                .with_token_timeout(crate::fake::FAKE_GH_TOKEN_TIMEOUT),
        )
    }

    /// #129 (para a #133): o log do job segue o redirecionamento, sem levar
    /// o token, e devolve só o fim, a partir do começo de uma linha.
    #[test]
    fn trecho_final_do_log_do_job() {
        let d = tempfile::tempdir().unwrap();
        let gh = FakeGitHub::start("tok");
        let c = client_for(&gh, d.path());
        let log: String = (1..=50)
            .map(|i| format!("2026-11-12T10:00:{i:02}Z linha {i}\n"))
            .collect();
        gh.set_redirect(
            &sync::job_logs_path("acme", "acme-web", 457),
            "/blob/logs/457.txt",
        );
        gh.set_text("/blob/logs/457.txt", &log);
        let tail = c
            .job_log_tail("acme", "acme-web", 457, 100)
            .unwrap()
            .unwrap();
        assert!(tail.len() <= 100, "{tail:?}");
        assert!(tail.starts_with("2026-11-12T10:00:"), "{tail:?}");
        assert!(tail.ends_with("linha 50\n"), "{tail:?}");
        // Cabe inteiro: vem inteiro.
        let all = c
            .job_log_tail("acme", "acme-web", 457, 1 << 20)
            .unwrap()
            .unwrap();
        assert_eq!(all, log);
        // Log expirado ou sem acesso.
        assert_eq!(c.job_log_tail("acme", "acme-web", 999, 100).unwrap(), None);
    }

    #[test]
    fn get_one_com_etag() {
        let d = tempfile::tempdir().unwrap();
        let gh = FakeGitHub::start("tok");
        let c = client_for(&gh, d.path());
        gh.set("/repos/o/r/commits/abc/status", json!({ "statuses": [] }));
        let Fetched::Modified { body, etag } =
            c.get_one("/repos/o/r/commits/abc/status", None).unwrap()
        else {
            panic!("esperado 200");
        };
        assert_eq!(body, json!({ "statuses": [] }));
        assert_eq!(
            c.get_one("/repos/o/r/commits/abc/status", etag.as_deref())
                .unwrap(),
            Fetched::NotModified
        );
    }

    #[test]
    fn read_tail_corta_na_linha() {
        let r = |s: &str, max| read_tail(s.as_bytes(), max).unwrap();
        assert_eq!(r("a\nb\nc\n", 100), "a\nb\nc\n");
        assert_eq!(r("aaaa\nbb\ncc\n", 6), "cc\n");
        assert_eq!(r("", 10), "");
    }
}
