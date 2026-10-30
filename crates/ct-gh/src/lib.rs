//! Cliente do GitHub (spec §7.5): leitura com ETag e paginação, e uma única
//! escrita, a remoção de branch da limpeza de issues concluídas ([`refs`],
//! #286). Token: o configurado pela UI (`github.token`, 0600, spec §11) ou,
//! na falta dele, o do `gh auth token`, obtido sob demanda.
//! Síncrono; quem chama roda fora do runtime assíncrono.

pub mod refs;
pub mod sync;
pub mod time;

#[cfg(any(test, feature = "test-support"))]
pub mod fake;

use serde::Serialize;
use serde_json::Value;
use std::io::{Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Prazo padrão para o `gh auth token` ([`GhConfig::token_timeout`]).
pub const DEFAULT_TOKEN_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone)]
pub struct GhConfig {
    /// Raiz da API, sem barra no fim (`https://api.github.com`).
    pub api_url: String,
    /// Binário do `gh` (fonte do token).
    pub gh_bin: String,
    pub timeout: Duration,
    /// Prazo para o `gh auth token` terminar (padrão
    /// [`DEFAULT_TOKEN_TIMEOUT`]); passado dele, o `gh` é morto.
    pub token_timeout: Duration,
    /// Token configurado (`~/.config/ctower/github.token`); tem prioridade sobre o `gh`.
    pub token_file: Option<PathBuf>,
}

impl GhConfig {
    pub fn new(api_url: &str, gh_bin: &str) -> Self {
        GhConfig {
            api_url: api_url.trim_end_matches('/').to_string(),
            gh_bin: gh_bin.to_string(),
            timeout: Duration::from_secs(30),
            token_timeout: DEFAULT_TOKEN_TIMEOUT,
            token_file: None,
        }
    }

    pub fn with_token_file(mut self, path: PathBuf) -> Self {
        self.token_file = Some(path);
        self
    }

    pub fn with_token_timeout(mut self, timeout: Duration) -> Self {
        self.token_timeout = timeout;
        self
    }
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

pub struct GhClient {
    cfg: GhConfig,
    agent: ureq::Agent,
    token: Mutex<Option<String>>,
}

struct Page {
    body: Value,
    etag: Option<String>,
    next: Option<String>,
}

impl GhClient {
    pub fn new(cfg: GhConfig) -> Self {
        let agent = ureq::AgentBuilder::new().timeout(cfg.timeout).build();
        GhClient {
            cfg,
            agent,
            token: Mutex::new(None),
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
                    *self.token.lock().unwrap() = None;
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
        let url = format!("{}{}", self.cfg.api_url, path);
        let mut renewed = false;
        loop {
            return match self.request("POST", &url)?.send_json(body) {
                Ok(r) => r
                    .into_json::<Value>()
                    .map_err(|e| GhError::Parse(e.to_string())),
                Err(ureq::Error::Status(401, _)) if !renewed => {
                    *self.token.lock().unwrap() = None;
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
                    *self.token.lock().unwrap() = None;
                    renewed = true;
                    continue;
                }
                Err(ureq::Error::Status(code, r)) => Err(status_error(code, r)),
                Err(ureq::Error::Transport(t)) => Err(GhError::Network(t.to_string())),
            };
        }
    }

    /// Token em memória ou, se ainda não há, do arquivo configurado e, na
    /// falta dele, do `gh auth token`.
    fn token(&self) -> Result<String, GhError> {
        let mut t = self.token.lock().unwrap();
        if let Some(tok) = t.as_ref() {
            return Ok(tok.clone());
        }
        let tok = match self.file_token() {
            Some(tok) => tok,
            None => gh_token(&self.cfg.gh_bin, self.cfg.token_timeout)?,
        };
        *t = Some(tok.clone());
        Ok(tok)
    }

    /// Token do arquivo configurado, se houver.
    fn file_token(&self) -> Option<String> {
        let path = self.cfg.token_file.as_ref()?;
        let t = std::fs::read_to_string(path).ok()?;
        let t = t.trim();
        (!t.is_empty()).then(|| t.to_string())
    }

    /// Token configurado para ferramentas externas (`GH_TOKEN` do `gh repo
    /// clone`); `None` = usar o login do próprio `gh`.
    pub fn configured_token(&self) -> Option<String> {
        self.file_token()
    }

    /// Login da conta dona de `token` (GET /user).
    fn login_of(&self, token: &str) -> Result<String, GhError> {
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

    /// Valida `token` no GitHub e, se valer, grava no arquivo configurado
    /// (0600) e passa a usá-lo. Devolve o login da conta.
    pub fn set_token(&self, token: &str) -> Result<String, GhError> {
        let token = token.trim();
        if token.is_empty() || token.contains(char::is_whitespace) {
            return Err(GhError::Token("token vazio ou com espaços".into()));
        }
        let path = self
            .cfg
            .token_file
            .as_ref()
            .ok_or_else(|| GhError::Token("daemon sem arquivo de token configurado".into()))?;
        let login = self.login_of(token)?;
        write_private(path, token).map_err(|e| GhError::Token(e.to_string()))?;
        *self.token.lock().unwrap() = Some(token.to_string());
        Ok(login)
    }

    /// Apaga o token configurado; volta a usar o `gh auth token`.
    pub fn clear_token(&self) -> Result<(), GhError> {
        if let Some(path) = &self.cfg.token_file {
            match std::fs::remove_file(path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(GhError::Token(e.to_string())),
            }
        }
        *self.token.lock().unwrap() = None;
        Ok(())
    }

    /// De onde vem o token e de qual conta ele é (nunca o token).
    pub fn status(&self) -> TokenStatus {
        let source = if self.file_token().is_some() {
            "token"
        } else {
            "gh"
        };
        match self.token().and_then(|t| self.login_of(&t)) {
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
}

/// Situação do token do GitHub para a UI.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct TokenStatus {
    /// `token` (configurado no Control Tower), `gh` (`gh auth token`) ou `none`.
    pub source: String,
    pub login: Option<String>,
    pub error: Option<String>,
}

/// Comentário criado por [`GhClient::comment_pr`] (`github.pr.comment`).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct PrComment {
    pub id: i64,
    /// `html_url` do comentário.
    pub url: String,
}

/// Grava `contents` em `path` com permissão 0600 (temporário + rename).
fn write_private(path: &std::path::Path, contents: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)?;
    f.write_all(contents.as_bytes())?;
    f.sync_all()?;
    std::fs::rename(&tmp, path)
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
        .and_then(|v| v.get("message").and_then(Value::as_str).map(String::from))
        .unwrap_or_else(|| truncate(&text, 200));
    GhError::Http {
        status: code,
        message,
    }
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

    #[test]
    fn token_configurado_tem_prioridade_e_gh_e_reserva() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        // O GitHub falso só aceita o token configurado; o `gh` dá outro.
        let gh = FakeGitHub::start("tok-arquivo");
        gh.set("/user", json!({ "login": "octocat" }));
        gh.set("/repos/o/r/issues", json!([]));
        let file = d.path().join("cfg/github.token");
        let bin = fake_gh(d.path(), "tok-do-gh");
        let c = GhClient::new(
            GhConfig::new(&gh.url(), bin.to_str().unwrap())
                .with_token_timeout(crate::fake::FAKE_GH_TOKEN_TIMEOUT)
                .with_token_file(file.clone()),
        );
        let st = c.status();
        assert_eq!(st.source, "gh");
        assert!(st.login.is_none() && st.error.unwrap().contains("401"));

        // Token inválido não é gravado.
        assert!(c.set_token("errado").is_err());
        assert!(!file.exists());
        assert!(c.set_token("  ").is_err());

        assert_eq!(c.set_token("tok-arquivo").unwrap(), "octocat");
        let mode = std::fs::metadata(&file).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        assert!(c.get_all("/repos/o/r/issues", None).is_ok());
        assert_eq!(c.configured_token().as_deref(), Some("tok-arquivo"));
        let st = c.status();
        assert_eq!(
            (st.source.as_str(), st.login.as_deref()),
            ("token", Some("octocat"))
        );
        // Um cliente novo (daemon reiniciado) lê o arquivo antes do gh.
        let c2 = GhClient::new(
            GhConfig::new(&gh.url(), bin.to_str().unwrap())
                .with_token_timeout(crate::fake::FAKE_GH_TOKEN_TIMEOUT)
                .with_token_file(file.clone()),
        );
        assert!(c2.get_all("/repos/o/r/issues", None).is_ok());
        assert_eq!(fake_gh_calls(d.path()), 1, "só o status inicial usou o gh");

        c.clear_token().unwrap();
        assert!(!file.exists());
        assert_eq!(c.configured_token(), None);
        assert_eq!(c.status().source, "gh");
        c.clear_token().unwrap();
    }

    #[test]
    fn sem_gh_e_sem_token_status_none() {
        let gh = FakeGitHub::start("tok");
        let c = GhClient::new(GhConfig::new(&gh.url(), "/nao/existe/gh"));
        let st = c.status();
        assert_eq!(st.source, "none");
        assert!(st.error.is_some());
        assert!(c.set_token("tok").is_err(), "sem arquivo configurado");
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
}
