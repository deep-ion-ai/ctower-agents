//! Log em arquivo dos daemons e a leitura pela UI (#65).
//!
//! [`init`] instala o subscriber do `tracing`: o stderr de sempre (journald
//! no Linux/WSL, `~/Library/Logs/*.log` pelo launchd no macOS) e uma camada
//! que grava cada evento como uma linha JSON ([`LogLine`]) em
//! `<dir>/<daemon>.jsonl`, com rotação por tamanho (`.1` … `.N-1`). A mesma
//! camada repassa as linhas novas a quem acompanha (modo seguir).
//!
//! Tokens e secrets saem mascarados ([`mask`]) antes de gravar e de novo ao
//! ler, para as linhas gravadas por uma versão anterior da máscara.
//!
//! Protocolo (o mesmo nos dois daemons; no hub com o prefixo `hub.`):
//! - `logs.tail { limit?, before?, level?, q?, since?, until? }` →
//!   [`TailPage`]: as últimas linhas, da mais antiga para a mais nova;
//!   `before` (o `id` da linha mais antiga já vista) pagina para trás.
//! - `logs.follow { level?, q? }`: a conexão passa a receber o push
//!   `{event, data: {lines}}` com as linhas novas; `logs.unfollow` para.

use axum::extract::ws::Message;
use ct_core::LogsConfig;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::borrow::Cow;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex, OnceLock};
use tokio::sync::{broadcast, mpsc};
use tracing::field::{Field, Visit};
use tracing::{Event, Subscriber};
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::Layer;

use crate::rpc::{invalid, RpcResult};

/// Linhas devolvidas por `logs.tail` sem `limit`, e o máximo.
pub const TAIL_DEFAULT: usize = 200;
pub const TAIL_MAX: usize = 2000;
/// Linhas por push do modo seguir.
const FOLLOW_BATCH: usize = 200;
/// Erro de `logs.*` com o arquivo de log desligado (`max_file_mb = 0`).
pub const LOGS_DISABLED: i64 = -32040;

/// Uma linha do log.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LogLine {
    /// Crescente (µs desde a época, nunca repetido no processo): o cursor
    /// de `before`.
    pub id: u64,
    /// Momento do evento (ms desde a época).
    pub ts: i64,
    /// `error` | `warn` | `info` | `debug` | `trace`.
    pub level: String,
    /// Módulo que gerou o evento (`ct_server::hub_sync`).
    pub target: String,
    pub message: String,
    /// Campos estruturados do evento, além da mensagem.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub fields: Map<String, Value>,
}

/// Resposta de `logs.tail`.
#[derive(Debug, Clone, Serialize)]
pub struct TailPage {
    /// Da mais antiga para a mais nova.
    pub lines: Vec<LogLine>,
    /// Há linhas mais antigas que casam com o filtro (`before` = `lines[0].id`).
    pub more: bool,
    /// Arquivo atual, para quem quiser ir à máquina.
    pub file: String,
}

/// Filtro de `logs.tail` e `logs.follow`.
#[derive(Debug, Clone, Default)]
pub struct Filter {
    /// Nível mínimo (`warn` = `error` + `warn`); `None` = todos.
    pub level: Option<u8>,
    /// Texto, sem caixa, na mensagem, no target ou nos campos.
    pub q: Option<String>,
    pub since: Option<i64>,
    pub until: Option<i64>,
}

impl Filter {
    /// Lê `level`, `q`, `since` e `until` dos parâmetros.
    pub fn from_params(p: &Value) -> Result<Self, (i64, String)> {
        let level = match p.get("level") {
            None | Some(Value::Null) => None,
            Some(Value::String(s)) if s.is_empty() => None,
            Some(Value::String(s)) => Some(level_rank(s).ok_or_else(|| {
                invalid(format!(
                    "level inválido: {s} (aceitos: error, warn, info, debug, trace)"
                ))
            })?),
            Some(_) => return Err(invalid("level deve ser texto")),
        };
        let q = match p.get("q") {
            None | Some(Value::Null) => None,
            Some(Value::String(s)) if s.trim().is_empty() => None,
            Some(Value::String(s)) => Some(s.trim().to_lowercase()),
            Some(_) => return Err(invalid("q deve ser texto")),
        };
        let ms = |k: &str| match p.get(k) {
            None | Some(Value::Null) => Ok(None),
            Some(v) => v
                .as_i64()
                .map(Some)
                .ok_or_else(|| invalid(format!("{k} deve ser um instante em ms"))),
        };
        Ok(Filter {
            level,
            q,
            since: ms("since")?,
            until: ms("until")?,
        })
    }

    pub fn matches(&self, l: &LogLine) -> bool {
        if let Some(max) = self.level {
            if level_rank(&l.level).unwrap_or(u8::MAX) > max {
                return false;
            }
        }
        if self.since.is_some_and(|s| l.ts < s) || self.until.is_some_and(|u| l.ts > u) {
            return false;
        }
        match &self.q {
            None => true,
            Some(q) => {
                l.message.to_lowercase().contains(q)
                    || l.target.to_lowercase().contains(q)
                    || (!l.fields.is_empty()
                        && Value::Object(l.fields.clone())
                            .to_string()
                            .to_lowercase()
                            .contains(q))
            }
        }
    }
}

/// `error` = 1 … `trace` = 5.
pub fn level_rank(level: &str) -> Option<u8> {
    match level.to_ascii_lowercase().as_str() {
        "error" => Some(1),
        "warn" | "warning" => Some(2),
        "info" => Some(3),
        "debug" => Some(4),
        "trace" => Some(5),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Máscara

/// Valores com cara de token ou secret, em qualquer lugar do texto.
static SECRET_VALUES: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        // GitHub (clássico, fine-grained, app e OAuth), GitLab, Anthropic/OpenAI.
        r"\b(?:gh[pousr]_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,}|glpat-[A-Za-z0-9_\-]{16,}",
        r"|sk-ant-[A-Za-z0-9_\-]{16,}|sk-[A-Za-z0-9_\-]{20,}|xox[abprs]-[A-Za-z0-9\-]{10,}",
        // Chave privada do age e JWT.
        r"|AGE-SECRET-KEY-1[0-9A-Z]{20,}|eyJ[A-Za-z0-9_\-]{8,}\.[A-Za-z0-9_\-]{8,}\.[A-Za-z0-9_\-]+",
        // Tokens do Control Tower (ui.token, hook.token, hub.token): 64 hex.
        r"|[0-9a-fA-F]{64})\b",
    ))
    .expect("regex dos secrets")
});

/// `Bearer <x>` e `Basic <x>`.
static AUTH_SCHEME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(bearer|basic)\s+[A-Za-z0-9._~+/=\-]+").expect("regex do bearer")
});

/// Usuário e senha na URL (`https://user:senha@host`).
static URL_USERINFO: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b([a-z][a-z0-9+.\-]*://)[^/\s:@]+:[^/\s@]+@").expect("regex da url")
});

/// `token=x`, `?secret=x`, `"password": "x"`: chave com cara de secret.
static SECRET_PAIRS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?i)(["']?[A-Za-z0-9_.\-]*(?:token|secret|passw(?:or)?d|api[_\-]?key|credential|authorization|private[_\-]?key)[A-Za-z0-9_.\-]*["']?)(\s*=\s*|\s*:\s*["'])([^\s"'&,;}]+)"#,
    )
    .expect("regex dos pares")
});

/// Nome de campo estruturado cujo valor é secret.
static SECRET_FIELD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)(token|secret|passw(or)?d|api[_\-]?key|credential|authorization|private[_\-]?key)",
    )
    .expect("regex dos campos")
});

const MASK: &str = "***";

/// Texto com tokens e secrets trocados por `***`.
pub fn mask(s: &str) -> Cow<'_, str> {
    let u = URL_USERINFO.replace_all(s, |c: &regex::Captures| format!("{}{MASK}@", &c[1]));
    let a = SECRET_PAIRS.replace_all(&u, |c: &regex::Captures| {
        format!("{}{}{MASK}", &c[1], &c[2])
    });
    let b = AUTH_SCHEME.replace_all(&a, |c: &regex::Captures| format!("{} {MASK}", &c[1]));
    let c = SECRET_VALUES.replace_all(&b, MASK);
    let untouched = [&u, &a, &b, &c]
        .iter()
        .all(|x| matches!(x, Cow::Borrowed(_)));
    if untouched {
        Cow::Borrowed(s)
    } else {
        Cow::Owned(c.into_owned())
    }
}

/// O nome do campo indica um secret (o valor sai inteiro como `***`).
pub fn secret_field(name: &str) -> bool {
    SECRET_FIELD.is_match(name)
}

fn mask_value(name: &str, v: Value) -> Value {
    if secret_field(name) {
        return Value::String(MASK.into());
    }
    match v {
        Value::String(s) => Value::String(mask(&s).into_owned()),
        Value::Object(m) => Value::Object(
            m.into_iter()
                .map(|(k, v)| (k.clone(), mask_value(&k, v)))
                .collect(),
        ),
        Value::Array(a) => Value::Array(a.into_iter().map(|v| mask_value("", v)).collect()),
        v => v,
    }
}

/// A linha com a máscara aplicada à mensagem e aos campos.
pub fn mask_line(mut l: LogLine) -> LogLine {
    l.message = mask(&l.message).into_owned();
    l.fields = l
        .fields
        .into_iter()
        .map(|(k, v)| (k.clone(), mask_value(&k, v)))
        .collect();
    l
}

// ---------------------------------------------------------------------------
// Arquivo com rotação

/// Pasta padrão dos logs. Com a pasta de dados trocada por `data_env`
/// (`CTOWER_DATA_DIR`, `CTHUB_DATA_DIR`: testes e instalações paralelas),
/// `<data>/logs`; senão `~/Library/Logs/ctower` no macOS e
/// `$XDG_STATE_HOME/ctower/logs` (`~/.local/state/ctower/logs`) no resto.
pub fn default_dir(data_env: &str) -> Option<PathBuf> {
    let var = |k: &str| {
        std::env::var_os(k)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    if let Some(d) = var(data_env) {
        return Some(d.join("logs"));
    }
    let home = var("HOME")?;
    if cfg!(target_os = "macos") {
        return Some(home.join("Library/Logs/ctower"));
    }
    Some(
        var("XDG_STATE_HOME")
            .unwrap_or_else(|| home.join(".local/state"))
            .join("ctower/logs"),
    )
}

/// Pasta dos logs pela configuração: `dir` (com `~/`) ou o padrão.
pub fn dir_from_config(cfg: &LogsConfig, data_env: &str) -> Option<PathBuf> {
    let d = cfg.dir.trim();
    if d.is_empty() {
        return default_dir(data_env);
    }
    match (d.strip_prefix("~/"), std::env::var_os("HOME")) {
        (Some(rest), Some(h)) => Some(PathBuf::from(h).join(rest)),
        _ => Some(PathBuf::from(d)),
    }
}

struct Sink {
    file: Option<File>,
    size: u64,
}

struct Inner {
    path: PathBuf,
    max_bytes: u64,
    max_files: u32,
    sink: Mutex<Sink>,
    tx: broadcast::Sender<Arc<LogLine>>,
    last_id: AtomicU64,
    /// Já avisou no stderr que não conseguiu gravar (avisa uma vez).
    warned: std::sync::atomic::AtomicBool,
}

/// O arquivo de log de um daemon: grava, lê e repassa as linhas novas.
#[derive(Clone)]
pub struct Logs(Arc<Inner>);

impl std::fmt::Debug for Logs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Logs").field("path", &self.0.path).finish()
    }
}

#[cfg(unix)]
fn private_open(path: &Path) -> io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(path)
}

#[cfg(not(unix))]
fn private_open(path: &Path) -> io::Result<File> {
    OpenOptions::new().create(true).append(true).open(path)
}

impl Logs {
    /// Abre (ou cria) `<dir>/<name>.jsonl`. Não instala nada no `tracing`.
    pub fn open(dir: &Path, name: &str, max_bytes: u64, max_files: u32) -> io::Result<Logs> {
        fs::create_dir_all(dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o700));
        }
        let path = dir.join(format!("{name}.jsonl"));
        let file = private_open(&path)?;
        let size = file.metadata().map(|m| m.len()).unwrap_or(0);
        let (tx, _) = broadcast::channel(1024);
        Ok(Logs(Arc::new(Inner {
            path,
            max_bytes: max_bytes.max(1024),
            max_files: max_files.max(1),
            sink: Mutex::new(Sink {
                file: Some(file),
                size,
            }),
            tx,
            last_id: AtomicU64::new(0),
            warned: Default::default(),
        })))
    }

    pub fn path(&self) -> &Path {
        &self.0.path
    }

    /// Arquivos, do atual ao mais antigo.
    fn files(&self) -> Vec<PathBuf> {
        let mut v = vec![self.0.path.clone()];
        for i in 1..self.0.max_files {
            v.push(rotated(&self.0.path, i));
        }
        v
    }

    /// `id` crescente: µs de agora, ou o anterior + 1.
    fn next_id(&self) -> u64 {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_micros() as u64)
            .unwrap_or(0);
        let mut prev = self.0.last_id.load(Ordering::Relaxed);
        loop {
            let id = now.max(prev + 1);
            match self.0.last_id.compare_exchange_weak(
                prev,
                id,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => return id,
                Err(p) => prev = p,
            }
        }
    }

    /// Grava a linha (já mascarada) e a repassa a quem acompanha.
    pub fn write(&self, mut line: LogLine) {
        if line.id == 0 {
            line.id = self.next_id();
        }
        let line = mask_line(line);
        let mut text = serde_json::to_string(&line).unwrap_or_default();
        text.push('\n');
        if let Err(e) = self.append(text.as_bytes()) {
            if !self.0.warned.swap(true, Ordering::Relaxed) {
                eprintln!("log em arquivo ({}) falhou: {e}", self.0.path.display());
            }
        }
        let _ = self.0.tx.send(Arc::new(line));
    }

    fn append(&self, bytes: &[u8]) -> io::Result<()> {
        let mut s = self.0.sink.lock().unwrap_or_else(|e| e.into_inner());
        if s.size > 0 && s.size + bytes.len() as u64 > self.0.max_bytes {
            s.file = None;
            self.rotate()?;
            s.size = 0;
        }
        if s.file.is_none() {
            s.file = Some(private_open(&self.0.path)?);
        }
        s.file.as_mut().expect("aberto").write_all(bytes)?;
        s.size += bytes.len() as u64;
        Ok(())
    }

    /// `.N-2` → `.N-1` … atual → `.1`; o mais antigo sai.
    fn rotate(&self) -> io::Result<()> {
        let p = &self.0.path;
        let n = self.0.max_files;
        if n <= 1 {
            return fs::remove_file(p).or_else(ignore_missing);
        }
        fs::remove_file(rotated(p, n - 1)).or_else(ignore_missing)?;
        for i in (1..n - 1).rev() {
            fs::rename(rotated(p, i), rotated(p, i + 1)).or_else(ignore_missing)?;
        }
        fs::rename(p, rotated(p, 1)).or_else(ignore_missing)
    }

    /// Linhas novas a partir de agora (modo seguir).
    pub fn subscribe(&self) -> broadcast::Receiver<Arc<LogLine>> {
        self.0.tx.subscribe()
    }

    /// As últimas `limit` linhas que casam com `f`, antes de `before`.
    pub fn tail(&self, f: &Filter, limit: usize, before: Option<u64>) -> io::Result<TailPage> {
        let mut found: Vec<LogLine> = Vec::new();
        let mut more = false;
        'files: for path in self.files() {
            let text = match fs::read_to_string(&path) {
                Ok(t) => t,
                Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                Err(e) if e.kind() == io::ErrorKind::InvalidData => {
                    String::from_utf8_lossy(&fs::read(&path)?).into_owned()
                }
                Err(e) => return Err(e),
            };
            for raw in text.lines().rev() {
                let Ok(l) = serde_json::from_str::<LogLine>(raw) else {
                    continue;
                };
                if before.is_some_and(|b| l.id >= b) || !f.matches(&l) {
                    continue;
                }
                if found.len() == limit {
                    more = true;
                    break 'files;
                }
                found.push(l);
            }
        }
        found.reverse();
        Ok(TailPage {
            lines: found.into_iter().map(mask_line).collect(),
            more,
            file: self.0.path.display().to_string(),
        })
    }
}

fn rotated(p: &Path, i: u32) -> PathBuf {
    let mut s = p.as_os_str().to_owned();
    s.push(format!(".{i}"));
    PathBuf::from(s)
}

fn ignore_missing(e: io::Error) -> io::Result<()> {
    if e.kind() == io::ErrorKind::NotFound {
        Ok(())
    } else {
        Err(e)
    }
}

// ---------------------------------------------------------------------------
// tracing

#[derive(Default)]
struct Fields {
    message: String,
    fields: Map<String, Value>,
}

impl Fields {
    fn put(&mut self, f: &Field, v: Value) {
        if f.name() == "message" {
            self.message = match v {
                Value::String(s) => s,
                v => v.to_string(),
            };
        } else {
            self.fields.insert(f.name().to_string(), v);
        }
    }
}

impl Visit for Fields {
    fn record_debug(&mut self, f: &Field, v: &dyn std::fmt::Debug) {
        self.put(f, Value::String(format!("{v:?}")));
    }
    fn record_str(&mut self, f: &Field, v: &str) {
        self.put(f, Value::String(v.to_string()));
    }
    fn record_i64(&mut self, f: &Field, v: i64) {
        self.put(f, json!(v));
    }
    fn record_u64(&mut self, f: &Field, v: u64) {
        self.put(f, json!(v));
    }
    fn record_bool(&mut self, f: &Field, v: bool) {
        self.put(f, json!(v));
    }
    fn record_f64(&mut self, f: &Field, v: f64) {
        self.put(f, json!(v));
    }
    fn record_error(&mut self, f: &Field, v: &(dyn std::error::Error + 'static)) {
        self.put(f, Value::String(v.to_string()));
    }
}

/// Camada que grava cada evento em [`Logs`].
pub struct FileLayer(pub Logs);

impl<S: Subscriber> Layer<S> for FileLayer {
    fn on_event(&self, ev: &Event<'_>, _: Context<'_, S>) {
        let mut v = Fields::default();
        ev.record(&mut v);
        let meta = ev.metadata();
        self.0.write(LogLine {
            id: 0,
            ts: ct_core::now_ms(),
            level: meta.level().as_str().to_ascii_lowercase(),
            target: meta.target().to_string(),
            message: v.message,
            fields: v.fields,
        });
    }
}

static GLOBAL: OnceLock<Logs> = OnceLock::new();

/// O arquivo de log do processo, se [`init`] (ou [`install`]) o abriu.
pub fn global() -> Option<&'static Logs> {
    GLOBAL.get()
}

/// Torna `logs` o arquivo do processo, sem mexer no `tracing` (o [`init`]
/// usa; os testes do `/ws` também). Só o primeiro vale.
pub fn install(logs: Logs) -> &'static Logs {
    GLOBAL.get_or_init(|| logs)
}

/// Liga o log deste processo: stderr com `RUST_LOG` (padrão `info`) e, se
/// `cfg.max_file_mb > 0`, o arquivo `<dir>/<name>.jsonl`. Falha ao abrir o
/// arquivo não impede o daemon de subir: fica o stderr, com o motivo.
pub fn init(name: &str, cfg: &LogsConfig, data_env: &str) {
    let filter =
        || tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into());
    let stderr = tracing_subscriber::fmt::layer().with_writer(io::stderr);
    let mut why = None;
    let logs = match (cfg.max_file_mb, dir_from_config(cfg, data_env)) {
        (0, _) => None,
        (_, None) => {
            why = Some("pasta de log indefinida (sem HOME)".to_string());
            None
        }
        (mb, Some(dir)) => {
            match Logs::open(&dir, name, mb.saturating_mul(1024 * 1024), cfg.max_files) {
                Ok(l) => Some(l),
                Err(e) => {
                    why = Some(format!("{}: {e}", dir.display()));
                    None
                }
            }
        }
    };
    let file = logs.clone().map(FileLayer);
    let _ = tracing_subscriber::registry()
        .with(filter())
        .with(stderr)
        .with(file)
        .try_init();
    match (&logs, why) {
        (Some(l), _) => {
            install(l.clone());
            tracing::info!("log em arquivo: {}", l.path().display());
        }
        (None, Some(w)) => tracing::warn!("log em arquivo desligado: {w}"),
        (None, None) => tracing::info!("log em arquivo desligado ([logs] max_file_mb = 0)"),
    }
}

// ---------------------------------------------------------------------------
// RPC

fn disabled() -> (i64, String) {
    (
        LOGS_DISABLED,
        "log em arquivo desligado neste daemon ([logs] max_file_mb = 0 ou pasta inacessível)"
            .into(),
    )
}

/// `logs.tail`.
pub async fn rpc_tail(logs: Option<&Logs>, p: &Value) -> RpcResult {
    let logs = logs.ok_or_else(disabled)?.clone();
    let f = Filter::from_params(p)?;
    let limit = match p.get("limit") {
        None | Some(Value::Null) => TAIL_DEFAULT,
        Some(v) => v
            .as_u64()
            .filter(|n| (1..=TAIL_MAX as u64).contains(n))
            .ok_or_else(|| invalid(format!("limit deve estar entre 1 e {TAIL_MAX}")))?
            as usize,
    };
    let before = match p.get("before") {
        None | Some(Value::Null) => None,
        Some(v) => Some(
            v.as_u64()
                .ok_or_else(|| invalid("before deve ser o id de uma linha"))?,
        ),
    };
    let page = tokio::task::spawn_blocking(move || logs.tail(&f, limit, before))
        .await
        .map_err(|e| (-32603, e.to_string()))?
        .map_err(|e| (-32000, format!("leitura do log falhou: {e}")))?;
    Ok(json!(page))
}

/// `logs.follow`: passa a empurrar as linhas novas pela conexão como
/// `{event, data: {lines}}` (ou `{lines: [], missed}` quando ficou para
/// trás). Quem chama guarda o handle e o aborta no `logs.unfollow` ou ao
/// fechar a conexão.
pub fn spawn_follow(
    logs: Option<&Logs>,
    p: &Value,
    out: mpsc::UnboundedSender<Message>,
    event: &'static str,
) -> Result<tokio::task::JoinHandle<()>, (i64, String)> {
    let logs = logs.ok_or_else(disabled)?;
    let f = Filter::from_params(p)?;
    let mut rx = logs.subscribe();
    Ok(tokio::spawn(async move {
        loop {
            let mut batch: Vec<Arc<LogLine>> = Vec::new();
            let mut missed = 0u64;
            match rx.recv().await {
                Ok(l) => batch.push(l),
                Err(broadcast::error::RecvError::Lagged(n)) => missed += n,
                Err(broadcast::error::RecvError::Closed) => return,
            }
            while batch.len() < FOLLOW_BATCH {
                match rx.try_recv() {
                    Ok(l) => batch.push(l),
                    Err(broadcast::error::TryRecvError::Lagged(n)) => missed += n,
                    Err(_) => break,
                }
            }
            let lines: Vec<&LogLine> = batch
                .iter()
                .map(|l| l.as_ref())
                .filter(|l| f.matches(l))
                .collect();
            if lines.is_empty() && missed == 0 {
                continue;
            }
            let mut data = json!({ "lines": lines });
            if missed > 0 {
                data["missed"] = json!(missed);
            }
            let msg = json!({ "event": event, "data": data });
            if out.send(Message::Text(msg.to_string().into())).is_err() {
                return;
            }
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(level: &str, msg: &str) -> LogLine {
        LogLine {
            id: 0,
            ts: ct_core::now_ms(),
            level: level.into(),
            target: "ct_test".into(),
            message: msg.into(),
            fields: Map::new(),
        }
    }

    #[test]
    fn mascara_tokens_e_pares() {
        let hex = "a".repeat(64);
        let cases = [
            (
                format!("ui token {hex} lido"),
                "ui token *** lido".to_string(),
            ),
            (
                "Authorization: Bearer abc.def-123".into(),
                "Authorization: Bearer ***".into(),
            ),
            (
                "gh token ghp_abcdefghijklmnopqrstuvwxyz0123".into(),
                "gh token ***".into(),
            ),
            (
                "url https://x/y?token=s3cr3t&a=1".into(),
                "url https://x/y?token=***&a=1".into(),
            ),
            (
                r#"{"password": "hunter2"}"#.into(),
                r#"{"password": "***"}"#.into(),
            ),
            ("hub_token=abc123 ok".into(), "hub_token=*** ok".into()),
            (
                "AGE-SECRET-KEY-1QQQQQQQQQQQQQQQQQQQQQQQQQQQQ".into(),
                "***".into(),
            ),
            (
                "glpat-abcdefghijklmnopqrst e sk-ant-api03-abcdefghijklmnop".into(),
                "*** e ***".into(),
            ),
            (
                "hub http://eu:s3nha@hub:7879/ws".into(),
                "hub http://***@hub:7879/ws".into(),
            ),
        ];
        for (i, o) in cases {
            assert_eq!(mask(&i), o, "{i}");
        }
        // Texto comum não muda (nem "secrets: chave …", nem um commit de 40 hex).
        let plain = "secrets: chave do ambiente trocada (age1xyz) commit 0123456789abcdef0123456789abcdef01234567";
        assert!(matches!(mask(plain), Cow::Borrowed(_)));
    }

    #[test]
    fn campos_com_nome_de_secret_saem_inteiros() {
        let mut l = line("info", "x");
        l.fields.insert("hub_token".into(), json!("qualquer"));
        l.fields.insert("env".into(), json!("dev"));
        l.fields.insert("url".into(), json!("http://h/?secret=abc"));
        let m = mask_line(l);
        assert_eq!(m.fields["hub_token"], "***");
        assert_eq!(m.fields["env"], "dev");
        assert_eq!(m.fields["url"], "http://h/?secret=***");
    }

    #[test]
    fn grava_rotaciona_e_le_para_tras() {
        let dir = tempfile::tempdir().unwrap();
        let logs = Logs::open(dir.path(), "d", 2048, 3).unwrap();
        for i in 0..100 {
            let lvl = if i % 10 == 0 { "warn" } else { "info" };
            logs.write(line(lvl, &format!("linha {i}")));
        }
        assert!(dir.path().join("d.jsonl.1").exists());
        assert!(dir.path().join("d.jsonl.2").exists());
        assert!(!dir.path().join("d.jsonl.3").exists());

        let all = Filter::default();
        let p = logs.tail(&all, 5, None).unwrap();
        let msgs: Vec<_> = p.lines.iter().map(|l| l.message.as_str()).collect();
        assert_eq!(
            msgs,
            ["linha 95", "linha 96", "linha 97", "linha 98", "linha 99"]
        );
        assert!(p.more);
        let p2 = logs.tail(&all, 3, Some(p.lines[0].id)).unwrap();
        let msgs: Vec<_> = p2.lines.iter().map(|l| l.message.as_str()).collect();
        assert_eq!(msgs, ["linha 92", "linha 93", "linha 94"]);

        let warns = Filter {
            level: level_rank("warn"),
            ..Default::default()
        };
        let p = logs.tail(&warns, 2, None).unwrap();
        let msgs: Vec<_> = p.lines.iter().map(|l| l.message.as_str()).collect();
        assert_eq!(msgs, ["linha 80", "linha 90"]);

        let q = Filter {
            q: Some("linha 9".into()),
            ..Default::default()
        };
        let p = logs.tail(&q, 100, None).unwrap();
        assert!(p.lines.iter().all(|l| l.message.starts_with("linha 9")));
        assert!(!p.more);
    }

    #[test]
    fn le_mascarando_linha_antiga_gravada_sem_mascara() {
        let dir = tempfile::tempdir().unwrap();
        let logs = Logs::open(dir.path(), "d", 1 << 20, 2).unwrap();
        let raw =
            serde_json::to_string(&line("info", &format!("token={}", "b".repeat(64)))).unwrap();
        fs::write(logs.path(), format!("{raw}\nlixo que não é json\n")).unwrap();
        let p = logs.tail(&Filter::default(), 10, None).unwrap();
        assert_eq!(p.lines.len(), 1);
        assert_eq!(p.lines[0].message, "token=***");
    }

    #[test]
    fn filtro_de_parametros() {
        assert!(Filter::from_params(&json!({ "level": "nada" })).is_err());
        let f = Filter::from_params(&json!({ "level": "error", "q": " Hub " })).unwrap();
        assert_eq!(f.level, Some(1));
        assert_eq!(f.q.as_deref(), Some("hub"));
        assert!(!f.matches(&line("warn", "hub")));
        assert!(f.matches(&line("error", "o HUB caiu")));
    }

    #[tokio::test]
    async fn segue_as_linhas_novas() {
        let dir = tempfile::tempdir().unwrap();
        let logs = Logs::open(dir.path(), "d", 1 << 20, 2).unwrap();
        let (out, mut rx) = mpsc::unbounded_channel();
        let h = spawn_follow(Some(&logs), &json!({ "level": "warn" }), out, "logs.lines").unwrap();
        tokio::task::yield_now().await;
        logs.write(line("info", "fica de fora"));
        logs.write(line(
            "error",
            &format!("falhou com Bearer {}", "c".repeat(10)),
        ));
        let Message::Text(t) = rx.recv().await.unwrap() else {
            panic!()
        };
        let v: Value = serde_json::from_str(&t).unwrap();
        assert_eq!(v["event"], "logs.lines");
        assert_eq!(v["data"]["lines"].as_array().unwrap().len(), 1);
        assert_eq!(v["data"]["lines"][0]["message"], "falhou com Bearer ***");
        h.abort();
    }

    #[tokio::test]
    async fn rpc_sem_arquivo_responde_desligado() {
        let e = rpc_tail(None, &json!({})).await.unwrap_err();
        assert_eq!(e.0, LOGS_DISABLED);
    }
}
