//! GitHub falso para testes: servidor HTTP/1.1 mínimo em `127.0.0.1`, com
//! ETag, paginação e falhas programadas; e um `gh` falso que imprime o token.

use serde_json::{json, Value};
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, VecDeque};
use std::hash::{Hash, Hasher};
use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Falha programada: status e cabeçalhos.
type Failure = (u16, Vec<(String, String)>);

struct Route {
    body: Value,
    etag: String,
    link: Option<String>,
}

#[derive(Default)]
struct State {
    token: String,
    routes: HashMap<String, Route>,
    fails: HashMap<String, VecDeque<Failure>>,
    delay: Duration,
    requests: Vec<(String, Option<String>)>,
    /// (método, caminho) de cada requisição.
    calls: Vec<(String, String)>,
    /// (caminho, corpo JSON) de cada `POST`.
    posted: Vec<(String, Value)>,
}

/// Servidor GitHub falso. As rotas são casadas pelo caminho completo com a
/// query, exatamente como o cliente envia. `DELETE` de uma rota publicada a
/// remove (`204`); o de `.../git/refs/heads/<b>` remove também a leitura
/// `.../git/ref/heads/<b>`, como no GitHub. `DELETE` do que não existe dá o
/// `422` "Reference does not exist". `POST` numa rota publicada responde
/// `201` com o corpo dela e guarda o enviado ([`FakeGitHub::posted`]).
#[derive(Clone)]
pub struct FakeGitHub {
    addr: SocketAddr,
    state: Arc<Mutex<State>>,
}

impl FakeGitHub {
    /// Sobe o servidor; só aceita `Authorization: Bearer <token>`.
    pub fn start(token: &str) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let state = Arc::new(Mutex::new(State {
            token: token.to_string(),
            ..State::default()
        }));
        let st = Arc::clone(&state);
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let st = Arc::clone(&st);
                std::thread::spawn(move || {
                    let _ = handle(stream, &st);
                });
            }
        });
        FakeGitHub { addr, state }
    }

    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// Publica `body` em `path`. O ETag muda quando o corpo muda.
    pub fn set(&self, path: &str, body: Value) {
        let mut st = self.state.lock().unwrap();
        let link = st.routes.remove(path).and_then(|r| r.link);
        st.routes.insert(
            path.to_string(),
            Route {
                etag: etag_of(&body),
                body,
                link,
            },
        );
    }

    /// Publica uma listagem em várias páginas: a primeira em `path`, as
    /// seguintes em `path&page=N`, ligadas por `Link: rel="next"`.
    pub fn set_pages(&self, path: &str, pages: Vec<Value>) {
        let sep = if path.contains('?') { '&' } else { '?' };
        let key = |i: usize| match i {
            0 => path.to_string(),
            i => format!("{path}{sep}page={}", i + 1),
        };
        let n = pages.len();
        for (i, body) in pages.into_iter().enumerate() {
            self.set(&key(i), body);
            let link = (i + 1 < n).then(|| format!("{}{}", self.url(), key(i + 1)));
            self.state
                .lock()
                .unwrap()
                .routes
                .get_mut(&key(i))
                .unwrap()
                .link = link;
        }
    }

    /// Força o `Link: rel="next"` de uma rota (ex.: para outro host).
    pub fn set_link(&self, path: &str, next: &str) {
        self.state
            .lock()
            .unwrap()
            .routes
            .get_mut(path)
            .expect("rota publicada")
            .link = Some(next.to_string());
    }

    /// A próxima requisição a `path` recebe `status` com estes cabeçalhos.
    pub fn fail_next(&self, path: &str, status: u16, headers: &[(&str, &str)]) {
        let h = headers
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        self.state
            .lock()
            .unwrap()
            .fails
            .entry(path.to_string())
            .or_default()
            .push_back((status, h));
    }

    /// Atraso antes de cada resposta.
    pub fn set_delay(&self, d: Duration) {
        self.state.lock().unwrap().delay = d;
    }

    /// Requisições recebidas: (caminho com query, `If-None-Match`).
    pub fn requests(&self) -> Vec<(String, Option<String>)> {
        self.state.lock().unwrap().requests.clone()
    }

    pub fn clear_requests(&self) {
        let mut st = self.state.lock().unwrap();
        st.requests.clear();
        st.calls.clear();
    }

    /// Requisições recebidas: (método, caminho com query).
    pub fn calls(&self) -> Vec<(String, String)> {
        self.state.lock().unwrap().calls.clone()
    }

    /// Corpos dos `POST` recebidos: (caminho, JSON).
    pub fn posted(&self) -> Vec<(String, Value)> {
        self.state.lock().unwrap().posted.clone()
    }
}

fn etag_of(v: &Value) -> String {
    let mut h = DefaultHasher::new();
    v.to_string().hash(&mut h);
    format!("\"{:016x}\"", h.finish())
}

fn handle(stream: TcpStream, st: &Mutex<State>) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let method = line.split_whitespace().next().unwrap_or("GET").to_string();
    let path = line.split_whitespace().nth(1).unwrap_or("/").to_string();
    let mut headers = HashMap::new();
    loop {
        let mut h = String::new();
        if reader.read_line(&mut h)? == 0 || h.trim().is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
        }
    }
    let if_none_match = headers.get("if-none-match").cloned();
    let len: usize = headers
        .get("content-length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let mut sent = vec![0u8; len];
    std::io::Read::read_exact(&mut reader, &mut sent)?;
    let delay = {
        let mut s = st.lock().unwrap();
        s.requests.push((path.clone(), if_none_match.clone()));
        s.calls.push((method.clone(), path.clone()));
        s.delay
    };
    if !delay.is_zero() {
        std::thread::sleep(delay);
    }

    let (status, extra, body) = {
        let mut s = st.lock().unwrap();
        let auth_ok = headers.get("authorization") == Some(&format!("Bearer {}", s.token));
        if let Some((code, h)) = s.fails.get_mut(&path).and_then(VecDeque::pop_front) {
            (
                code,
                h,
                json!({ "message": "falha programada" }).to_string(),
            )
        } else if !auth_ok {
            (
                401,
                vec![],
                json!({ "message": "Bad credentials" }).to_string(),
            )
        } else if method == "DELETE" {
            let read = path.replace("/git/refs/", "/git/ref/");
            let a = s.routes.remove(&path).is_some();
            let b = s.routes.remove(&read).is_some();
            if a || b {
                (204, vec![], String::new())
            } else {
                (
                    422,
                    vec![],
                    json!({ "message": "Reference does not exist" }).to_string(),
                )
            }
        } else if method == "POST" {
            let sent = serde_json::from_slice(&sent).unwrap_or(Value::Null);
            s.posted.push((path.clone(), sent));
            match s.routes.get(&path) {
                Some(r) => (201, vec![], r.body.to_string()),
                None => (404, vec![], json!({ "message": "Not Found" }).to_string()),
            }
        } else if let Some(r) = s.routes.get(&path) {
            let mut h = vec![("ETag".to_string(), r.etag.clone())];
            if let Some(l) = &r.link {
                h.push(("Link".into(), format!("<{l}>; rel=\"next\"")));
            }
            if if_none_match.as_deref() == Some(r.etag.as_str()) {
                (304, h, String::new())
            } else {
                (200, h, r.body.to_string())
            }
        } else {
            (404, vec![], json!({ "message": "Not Found" }).to_string())
        }
    };

    let mut out = stream;
    let mut resp = format!(
        "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    );
    for (k, v) in extra {
        resp.push_str(&format!("{k}: {v}\r\n"));
    }
    resp.push_str("\r\n");
    resp.push_str(&body);
    out.write_all(resp.as_bytes())?;
    out.flush()
}

/// Prazo do `gh auth token` para clientes com o [`fake_gh`] (ou outro `gh`
/// falso gravado pelo teste): no macOS, a primeira execução de um executável
/// recém-gravado passa pela avaliação do sistema (~300 ms, em fila única para
/// a máquina toda); com muitos testes criando executáveis em paralelo, a fila
/// passa dos 5 s do [`crate::DEFAULT_TOKEN_TIMEOUT`] de produção.
pub const FAKE_GH_TOKEN_TIMEOUT: Duration = Duration::from_secs(60);

/// `gh` falso em `dir`: imprime `token` e conta as chamadas em `dir/gh-calls`.
/// O cliente que o usa leva [`FAKE_GH_TOKEN_TIMEOUT`] no `GhConfig`.
pub fn fake_gh(dir: &Path, token: &str) -> PathBuf {
    let bin = dir.join("gh");
    std::fs::write(
        &bin,
        format!(
            "#!/bin/sh\necho x >> '{}'\nprintf '%s\\n' '{token}'\n",
            dir.join("gh-calls").display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    bin
}

/// Quantas vezes o `gh` falso de `dir` foi chamado.
pub fn fake_gh_calls(dir: &Path) -> usize {
    std::fs::read_to_string(dir.join("gh-calls"))
        .map(|s| s.lines().count())
        .unwrap_or(0)
}
