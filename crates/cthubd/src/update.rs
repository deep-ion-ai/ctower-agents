//! `cthubd update` (issue #181): atualização do cthubd por build local, como
//! o `ctower update` do ambiente (crate `ct-update`). Roda em primeiro plano,
//! no terminal: quem a chama não está no cgroup do serviço, então o restart
//! não a interrompe.
//!
//! - clone dedicado em `<data>/update/src`, no `update_ref` do config;
//! - antes de tudo, confere o Node (≥ 20) e o npm do bundle web (#199);
//! - `npm ci` e `npm run build:web` em `<src>/ui` e o build em
//!   `<data>/update/target` com `CTHUB_WEB_REQUIRED=1` (falha aqui não toca
//!   em nada);
//! - backup do binário em `update/prev/` e snapshot consistente do `hub.db`
//!   (`VACUUM INTO`, com o serviço no ar) em `update/db/`;
//! - instala, reinicia o serviço (systemd de usuário ou launchd) e confere a
//!   saúde: `/healthz` com o commit novo e `hub.info` pelo `/ws` com o token
//!   de UI do hub. Sem isso em `CTHUB_UPDATE_VERIFY_SECS` (60 s), volta o
//!   binário anterior e, se o esquema migrou, o `hub.db` do snapshot.
//!
//! Pela UI (#421), [`HubUpdates`] atende `hub.update.*`: verifica no boot e
//! a cada `update_check_interval_hours`, dispara o `cthubd update` fora do
//! serviço (`ct_update::launch`: unidade transitória [`UNIT`] no systemd,
//! processo com sessão nova no launchd) e acompanha o `state.json`, com o
//! push `hub.update.status` a cada mudança. O restart encerra este cthubd;
//! o novo lê o `state.json` no boot e segue acompanhando até o fim.

use crate::config::{Dirs, HubConfig};
use ct_update::launch::Launch;
use ct_update::runner::{short, Action, PreBuild, Runner, Target};
use ct_update::{read_state, running, Available, Build, Relation, Source, UpdateState};
use serde::Serialize;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::broadcast;

/// Rótulo do LaunchAgent no macOS (`packaging/launchd/<rótulo>.plist`).
pub const LAUNCHD_LABEL: &str = "ai.deep-ion.cthubd";

/// Quanto esperar cada pedido da sonda de saúde.
const PROBE_TIMEOUT: Duration = Duration::from_secs(3);

fn env_or(key: &str, default: impl FnOnce() -> String) -> String {
    std::env::var(key)
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(default)
}

/// De onde vêm as atualizações do hub.
pub fn source(cfg: &HubConfig, dirs: &Dirs) -> Source {
    Source {
        repo: cfg.update_repo.trim().to_string(),
        git_ref: cfg.update_ref.trim().to_string(),
        dir: dirs.update_dir(),
    }
}

/// Gerenciador do serviço: `CTHUB_SERVICE_MANAGER` (`systemd`/`launchd`) ou,
/// sem ele, launchd no macOS e systemd de usuário no resto.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Manager {
    Systemd,
    Launchd,
}

impl Manager {
    pub fn from_env() -> Self {
        match std::env::var("CTHUB_SERVICE_MANAGER").as_deref() {
            Ok("launchd") => Manager::Launchd,
            Ok("systemd") => Manager::Systemd,
            _ if cfg!(target_os = "macos") => Manager::Launchd,
            _ => Manager::Systemd,
        }
    }

    /// Comando para `a` no serviço do cthubd.
    pub fn command(self, a: Action) -> Command {
        match self {
            Manager::Systemd => {
                let mut c = Command::new(env_or("CTHUB_SYSTEMCTL", || "systemctl".into()));
                c.arg("--user")
                    .arg(match a {
                        Action::Restart => "restart",
                        Action::Stop => "stop",
                        Action::Start => "start",
                    })
                    .arg("cthubd");
                c
            }
            Manager::Launchd => {
                let mut c = Command::new(env_or("CTHUB_LAUNCHCTL", || "launchctl".into()));
                // SAFETY: getuid não falha nem tem pré-condições.
                let domain = format!("gui/{}", unsafe { libc::getuid() });
                let service = format!("{domain}/{LAUNCHD_LABEL}");
                match a {
                    Action::Restart => c.args(["kickstart", "-k", &service]),
                    Action::Stop => c.args(["bootout", &service]),
                    Action::Start => c.arg("bootstrap").arg(&domain).arg(plist_path()),
                };
                c
            }
        }
    }
}

/// `~/Library/LaunchAgents/<rótulo>.plist`.
pub fn plist_path() -> PathBuf {
    let home = std::env::var_os("HOME").unwrap_or_default();
    Path::new(&home)
        .join("Library/LaunchAgents")
        .join(format!("{LAUNCHD_LABEL}.plist"))
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(PROBE_TIMEOUT)
        .timeout(PROBE_TIMEOUT)
        .build()
}

/// Build informado pelo `/healthz` do hub em `port`.
pub fn healthz(port: u16) -> Result<Build, String> {
    let body = agent()
        .get(&format!("http://127.0.0.1:{port}/healthz"))
        .call()
        .map_err(|e| format!("/healthz não respondeu: {e}"))?
        .into_string()
        .map_err(|e| format!("/healthz: {e}"))?;
    let v: Value = serde_json::from_str(&body).map_err(|e| format!("/healthz: {e}"))?;
    if v["role"] != "hub" {
        return Err(format!("/healthz não é de um hub: {body}"));
    }
    Ok(Build {
        version: v["version"].as_str().unwrap_or_default().into(),
        commit: v["commit"].as_str().unwrap_or_default().into(),
    })
}

/// `hub.info` pelo `/ws` com o token de UI: prova que o hub abriu o
/// `hub.db` e responde com um protocolo válido (`<major>.<minor>`).
pub fn hub_info(port: u16, ui_token: &str) -> Result<Value, String> {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    use tokio_tungstenite::tungstenite::Message;

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    rt.block_on(async {
        let call = async {
            let mut req = format!("ws://127.0.0.1:{port}/ws")
                .into_client_request()
                .map_err(|e| e.to_string())?;
            req.headers_mut().insert(
                "authorization",
                format!("Bearer {}", ui_token.trim())
                    .parse()
                    .map_err(|_| "token de UI inválido".to_string())?,
            );
            let (mut ws, _) = tokio_tungstenite::connect_async(req)
                .await
                .map_err(|e| e.to_string())?;
            let msg = serde_json::json!({ "id": 1, "method": "hub.info", "params": {} });
            ws.send(Message::Text(msg.to_string().into()))
                .await
                .map_err(|e| e.to_string())?;
            while let Some(m) = ws.next().await {
                let Message::Text(t) = m.map_err(|e| e.to_string())? else {
                    continue;
                };
                let v: Value = serde_json::from_str(&t).map_err(|e| e.to_string())?;
                if v["id"] != 1 {
                    continue; // push de evento
                }
                if let Some(e) = v.get("error") {
                    return Err(format!("erro {e}"));
                }
                let r = v["result"].clone();
                // Só exige um protocolo válido: um release pode subir a major.
                let valid = r["protocol"]
                    .as_str()
                    .and_then(|p| p.split('.').next())
                    .is_some_and(|m| m.parse::<u32>().is_ok());
                if !valid {
                    return Err(format!("protocolo inválido: {}", r["protocol"]));
                }
                return Ok(r);
            }
            Err("conexão fechada sem resposta".to_string())
        };
        tokio::time::timeout(PROBE_TIMEOUT, call)
            .await
            .map_err(|_| "sem resposta no prazo".to_string())?
    })
    .map_err(|e| format!("hub.info falhou: {e}"))
}

/// Saudável = `/healthz` de um hub e `hub.info` respondendo com um
/// protocolo válido. O build devolvido é o do `hub.info` (o commit que o
/// atualizador compara), com o do `/healthz` na falta dele.
fn probe(port: u16, token_file: &Path) -> Result<Build, String> {
    let b = healthz(port)?;
    let token = std::fs::read_to_string(token_file)
        .map_err(|e| format!("{}: {e}", token_file.display()))?;
    let info = hub_info(port, &token)?;
    Ok(Build {
        version: info["version"].as_str().unwrap_or(&b.version).into(),
        commit: info["commit"].as_str().unwrap_or(&b.commit).into(),
    })
}

/// Build no ar (`/healthz`) ou, fora do ar, o deste binário.
fn installed(port: u16) -> Build {
    healthz(port).unwrap_or_else(|_| Build::current())
}

/// Diretório onde fica o cthubd: `CTHUB_BIN_DIR` ou o deste executável.
fn bin_dir() -> Result<PathBuf, String> {
    if let Ok(d) = std::env::var("CTHUB_BIN_DIR") {
        return Ok(PathBuf::from(d));
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    exe.parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| "diretório do cthubd desconhecido".into())
}

fn cargo() -> String {
    env_or("CTHUB_CARGO", || {
        let home = std::env::var("HOME").unwrap_or_default();
        let p = Path::new(&home).join(".cargo/bin/cargo");
        if p.exists() {
            p.display().to_string()
        } else {
            "cargo".into()
        }
    })
}

/// Node mínimo do build web (o do `ui/package.json` e da CI).
pub const MIN_NODE_MAJOR: u32 = 20;

fn node() -> String {
    env_or("CTHUB_NODE", || "node".into())
}

fn npm() -> String {
    env_or("CTHUB_NPM", || "npm".into())
}

/// Major de `node --version` (`v22.11.0` → 22).
fn node_major(out: &str) -> Option<u32> {
    out.trim()
        .strip_prefix('v')?
        .split('.')
        .next()?
        .parse()
        .ok()
}

/// Saída de `<bin> --version`, ou o motivo de não ter rodado.
fn tool_version(bin: &str) -> Result<String, String> {
    let out = Command::new(bin)
        .arg("--version")
        .output()
        .map_err(|e| format!("`{bin} --version` não rodou: {e}"))?;
    if !out.status.success() {
        return Err(format!("`{bin} --version` falhou ({})", out.status));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// O hub embute a UI web (#199): sem Node ≥ 20 e npm, a atualização para
/// aqui, antes do fetch, de qualquer backup e da troca do binário.
pub fn check_node() -> Result<(), String> {
    let hint = format!(
        "o cthubd embute a UI web e o build dela precisa do Node {MIN_NODE_MAJOR} ou mais novo          com o npm: instale o Node (ex.: `brew install node` ou o NodeSource no Linux; veja o          docs/hub.md) e rode o `cthubd update` de novo. Nada foi alterado"
    );
    let v = tool_version(&node()).map_err(|e| format!("Node não encontrado ({e}); {hint}"))?;
    match node_major(&v) {
        Some(m) if m >= MIN_NODE_MAJOR => {}
        _ => return Err(format!("Node {v} é antigo demais ou ilegível; {hint}")),
    }
    tool_version(&npm()).map_err(|e| format!("npm não encontrado ({e}); {hint}"))?;
    Ok(())
}

/// Bundle web antes do `cargo install`: `npm ci` e `npm run build:web` em
/// `<src>/ui`; o build do cthubd falha se o bundle faltar.
fn web_pre_build() -> PreBuild {
    PreBuild {
        check: Box::new(check_node),
        commands: Box::new(|src| {
            let ui = src.join("ui");
            let mut ci = Command::new(npm());
            ci.current_dir(&ui).args(["ci", "--no-audit", "--no-fund"]);
            let mut build = Command::new(npm());
            build.current_dir(&ui).args(["run", "build:web"]);
            vec![ci, build]
        }),
        cargo_env: vec![("CTHUB_WEB_REQUIRED".into(), "1".into())],
    }
}

fn verify_timeout() -> Duration {
    Duration::from_secs(
        std::env::var("CTHUB_UPDATE_VERIFY_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(60),
    )
}

/// `cthubd update --check`: versão instalada × disponível no ref seguido.
pub fn check(dirs: &Dirs) -> i32 {
    let r = (|| -> Result<_, String> {
        let cfg = HubConfig::load(dirs).map_err(|e| e.to_string())?;
        let inst = installed(cfg.port);
        let source = source(&cfg, dirs);
        let a = source
            .check(&inst, ct_core::now_ms())
            .map_err(|e| e.to_string())?;
        Ok((inst, source, a))
    })();
    let (inst, source, a) = match r {
        Ok(v) => v,
        Err(e) => {
            eprintln!("verificação falhou: {e}");
            return 1;
        }
    };
    println!("instalado:  {} ({})", inst.version, short(&inst.commit));
    println!(
        "disponível: {} ({}) em {} de {}",
        a.build.version,
        short(&a.build.commit),
        a.git_ref,
        source.repo
    );
    match a.relation {
        Relation::Same => println!("Em dia."),
        Relation::Ahead => println!("O instalado está à frente de {}.", a.git_ref),
        Relation::Behind => {
            println!(
                "Atualização disponível: {} commit(s) novo(s).",
                a.behind.unwrap_or(a.commits.len() as u32)
            );
            for c in &a.commits {
                println!("  {c}");
            }
        }
        Relation::Diverged => println!(
            "Atualização disponível (o instalado não veio de {}).",
            a.git_ref
        ),
        Relation::Unknown => println!("Atualização disponível (commit instalado desconhecido)."),
    }
    0
}

/// `cthubd update [--rollback [--force]]`: atualiza (ou volta) e reinicia
/// o serviço. `force` aceita voltar o `hub.db` do snapshot no rollback manual.
pub fn run(dirs: &Dirs, rollback: bool, force: bool) -> i32 {
    let r = (|| -> Result<Runner, String> {
        dirs.ensure().map_err(|e| e.to_string())?;
        let cfg = HubConfig::load(dirs).map_err(|e| e.to_string())?;
        let source = source(&cfg, dirs);
        if let Some(s) = ct_update::running(&source.dir, ct_core::now_ms()) {
            return Err(format!(
                "uma atualização já está em andamento ({})",
                ct_update::runner::label(s.phase, "cthubd")
            ));
        }
        let port = cfg.port;
        let token_file = dirs.ui_token_file();
        let manager = Manager::from_env();
        let target = Target {
            service: "cthubd".into(),
            bins: vec!["cthubd".into()],
            dbs: vec![dirs.hub_db()],
            bin_dir: bin_dir()?,
            cargo: cargo(),
            release_prefix: None,
            verify_timeout: verify_timeout(),
            ctl: Box::new(move |a| manager.command(a)),
            probe: Box::new(move || probe(port, &token_file)),
            // O hub é a única cópia do histórico (os ambientes podam depois
            // do ack): o banco em uso fica num .pre-rollback-*, a época da
            // configuração muda e o rollback manual pede --force.
            after_db_restore: Some(Box::new(
                |path| match ct_ledger::Hub::renew_config_epoch_at(path) {
                    Ok(Some(_)) => Ok("config_epoch renovada: os ambientes recebem o \
                                       conjunto completo no próximo pull"
                        .into()),
                    Ok(None) => Ok(String::new()),
                    Err(e) => Err(format!("config_epoch não renovada: {e}")),
                },
            )),
            force_db_rollback: true,
            db_rollback_note: Some(
                "o que o hub novo aceitou depois do snapshot está só na cópia \
                 .pre-rollback-*; em cada ambiente, rode `ctower hub resync` e \
                 `ctower hub sync` para reenviar o que ainda não foi podado"
                    .into(),
            ),
            pre_build: Some(web_pre_build()),
        };
        let kind = if rollback { "rollback" } else { "update" };
        Runner::new(kind, source, target, installed(port)).map_err(|e| e.to_string())
    })();
    let mut r = match r {
        Ok(r) => r,
        Err(e) => {
            eprintln!("atualização: {e}");
            return 1;
        }
    };
    if rollback {
        r.rollback(force);
    } else {
        r.update();
    }
    ct_update::runner::report(r.state(), "cthubd")
}

// --- pela UI (`hub.update.*`, #421) -----------------------------------------

/// Unidade transitória do systemd onde o atualizador do hub roda.
pub const UNIT: &str = "cthubd-update";

/// Variáveis repassadas ao atualizador (a unidade não herda o ambiente).
const PASS_ENV: &[&str] = &[
    "PATH",
    "HOME",
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
    "CTHUB_CONFIG_DIR",
    "CTHUB_DATA_DIR",
    "CTHUB_PORT",
    "CTHUB_BIN_DIR",
    "CTHUB_CARGO",
    "CTHUB_NODE",
    "CTHUB_NPM",
    "CTHUB_SYSTEMCTL",
    "CTHUB_LAUNCHCTL",
    "CTHUB_SERVICE_MANAGER",
    "CTHUB_UPDATE_VERIFY_SECS",
    "CTOWER_GIT",
];

/// Por quanto tempo, no máximo, acompanhar o `state.json`.
const WATCH_FOR: Duration = Duration::from_secs(2 * 3600);

/// Quanto um disparo segura o próximo antes de o atualizador gravar o
/// primeiro estado (duplo clique, duas UIs).
const LAUNCH_GRACE_MS: i64 = 30_000;

/// Estado da atualização do hub (`hub.update.get`, push `hub.update.status`).
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct HubUpdateStatus {
    /// Build deste cthubd.
    pub installed: Build,
    /// Ref seguido (`update_ref`).
    #[serde(rename = "ref")]
    #[cfg_attr(feature = "ts", ts(rename = "ref"))]
    pub git_ref: String,
    /// Última verificação bem-sucedida; `None` = ainda não verificou.
    pub available: Option<Available>,
    /// Erro da última verificação (some na próxima que der certo).
    pub check_error: Option<String>,
    /// Atualização em andamento ou a última que terminou (`state.json`).
    pub state: Option<UpdateState>,
    /// Versão guardada pela última atualização, para onde o rollback
    /// manual volta; `None` = nada guardado.
    pub previous: Option<Build>,
}

pub struct HubUpdates {
    source: Source,
    /// Bancos com snapshot (`hub.db`), para a prévia do rollback.
    dbs: Vec<PathBuf>,
    /// `cthubd` disparado pelo atualizador (caminho absoluto).
    cthubd: PathBuf,
    systemd_run: String,
    installed: Build,
    /// Última verificação: `(available, check_error)`.
    checked: Mutex<(Option<Available>, Option<String>)>,
    /// Uma verificação por vez (o `git fetch` não gosta de concorrência).
    checking: tokio::sync::Mutex<()>,
    /// Último disparo (ms), até o atualizador gravar o estado.
    launched_at: Mutex<i64>,
    events: broadcast::Sender<Value>,
}

/// Erro de `hub.update.start|rollback` (a mensagem diz o motivo).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartError {
    /// O rollback volta o `hub.db` do snapshot: pede `force` (a mensagem diz
    /// o que se perde).
    NeedsForce(String),
    Other(String),
}

impl HubUpdates {
    pub fn new(cfg: &HubConfig, dirs: &Dirs, events: broadcast::Sender<Value>) -> Self {
        HubUpdates {
            source: source(cfg, dirs),
            dbs: vec![dirs.hub_db()],
            cthubd: ct_update::resolve_bin("cthubd"),
            systemd_run: std::env::var("CTHUB_SYSTEMD_RUN")
                .unwrap_or_else(|_| "systemd-run".into()),
            installed: Build::current(),
            checked: Mutex::new((None, None)),
            checking: tokio::sync::Mutex::new(()),
            launched_at: Mutex::new(0),
            events,
        }
    }

    /// `cthubd` a disparar (testes trocam por um falso).
    pub fn with_cthubd(mut self, cthubd: PathBuf) -> Self {
        self.cthubd = cthubd;
        self
    }

    /// `systemd-run` a usar (testes trocam por um falso).
    pub fn with_systemd_run(mut self, bin: &str) -> Self {
        self.systemd_run = bin.into();
        self
    }

    /// Build tido como instalado (testes).
    pub fn with_installed(mut self, b: Build) -> Self {
        self.installed = b;
        self
    }

    fn dir(&self) -> &Path {
        &self.source.dir
    }

    pub fn status(&self) -> HubUpdateStatus {
        let (available, check_error) = self.checked.lock().unwrap().clone();
        HubUpdateStatus {
            installed: self.installed.clone(),
            git_ref: self.source.git_ref.clone(),
            available,
            check_error,
            state: read_state(self.dir()),
            previous: ct_update::runner::previous(self.dir()),
        }
    }

    fn push(&self) {
        let _ = self
            .events
            .send(json!({ "event": "hub.update.status", "data": self.status() }));
    }

    /// Busca o ref remoto e compara com o instalado. Falha vira
    /// `check_error`, sem apagar o último `available`.
    pub async fn check(&self) -> HubUpdateStatus {
        let _guard = self.checking.lock().await;
        let (source, installed) = (self.source.clone(), self.installed.clone());
        let r = tokio::task::spawn_blocking(move || source.check(&installed, ct_core::now_ms()))
            .await
            .map_err(|e| e.to_string())
            .and_then(|r| r.map_err(|e| e.to_string()));
        {
            let mut c = self.checked.lock().unwrap();
            match r {
                Ok(a) => *c = (Some(a), None),
                Err(e) => c.1 = Some(e),
            }
        }
        self.push();
        self.status()
    }

    /// Dispara `cthubd update` (ou `--rollback`) fora do serviço e
    /// acompanha o `state.json`. No rollback, sem `force` e com o `hub.db`
    /// voltando do snapshot, recusa com [`StartError::NeedsForce`].
    pub async fn start(
        self: &Arc<Self>,
        rollback: bool,
        force: bool,
    ) -> Result<HubUpdateStatus, StartError> {
        let t0 = ct_core::now_ms();
        if running(self.dir(), t0).is_some() {
            return Err(StartError::Other(
                "uma atualização do hub já está em andamento".into(),
            ));
        }
        if rollback {
            let (dir, dbs) = (self.dir().to_path_buf(), self.dbs.clone());
            let loss = tokio::task::spawn_blocking(move || {
                ct_update::runner::db_rollback_loss(&dir, &dbs)
            })
            .await
            .map_err(|e| StartError::Other(e.to_string()))?
            .map_err(StartError::Other)?;
            if let (Some(l), false) = (loss, force) {
                return Err(StartError::NeedsForce(l));
            }
        }
        {
            let mut last = self.launched_at.lock().unwrap();
            let started = read_state(self.dir()).map_or(0, |s| s.started_at);
            if t0 - *last < LAUNCH_GRACE_MS && started < *last {
                return Err(StartError::Other(
                    "uma atualização do hub já foi disparada".into(),
                ));
            }
            *last = t0;
        }
        let mut args = vec!["update"];
        if rollback {
            args.push("--rollback");
            if force {
                args.push("--force");
            }
        }
        let (run, cthubd) = (self.systemd_run.clone(), self.cthubd.clone());
        let r = tokio::task::spawn_blocking(move || {
            // Um `systemd-run` trocado (testes) sempre vale.
            let how = if run == "systemd-run" && Manager::from_env() == Manager::Launchd {
                Launch::Detached
            } else {
                Launch::Systemd {
                    systemd_run: &run,
                    unit: UNIT,
                    pass_env: PASS_ENV,
                }
            };
            ct_update::launch::spawn(how, &cthubd, &args)
        })
        .await
        .map_err(|e| StartError::Other(e.to_string()))?;
        if let Err(e) = r {
            *self.launched_at.lock().unwrap() = 0;
            return Err(StartError::Other(e.0));
        }
        Arc::clone(self).watch(t0);
        Ok(self.status())
    }

    /// Push `hub.update.status` a cada mudança do `state.json` iniciado a
    /// partir de `since`, até o estado terminal (ou este cthubd sair, no
    /// restart: o novo retoma no boot).
    fn watch(self: Arc<Self>, since: i64) {
        tokio::spawn(async move {
            let deadline = tokio::time::Instant::now() + WATCH_FOR;
            let mut last: Option<UpdateState> = None;
            while tokio::time::Instant::now() < deadline {
                let cur = read_state(self.dir()).filter(|s| s.started_at >= since);
                if cur.is_some() && cur != last {
                    self.push();
                    if cur.as_ref().is_some_and(|s| s.phase.is_terminal()) {
                        break;
                    }
                    last = cur;
                }
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        });
    }

    /// No boot: com uma atualização em andamento (este é o cthubd novo,
    /// subindo antes da verificação de saúde), segue acompanhando até o fim.
    pub fn resume(self: &Arc<Self>) {
        if let Some(s) = running(self.dir(), ct_core::now_ms()) {
            Arc::clone(self).watch(s.started_at);
        }
    }
}

/// Verifica logo após o boot e depois a cada `every`.
pub fn spawn_periodic(updates: Arc<HubUpdates>, every: Duration) {
    tokio::spawn(async move {
        loop {
            let st = updates.check().await;
            match (&st.available, &st.check_error) {
                (_, Some(e)) => tracing::warn!("atualização do hub: verificação falhou: {e}"),
                (Some(a), None) if a.update_available => tracing::info!(
                    "atualização do hub disponível em {}: {} ({})",
                    a.git_ref,
                    a.build.version,
                    short(&a.build.commit)
                ),
                _ => {}
            }
            tokio::time::sleep(every).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(c: &Command) -> Vec<String> {
        c.get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn major_do_node() {
        assert_eq!(node_major("v22.11.0\n"), Some(22));
        assert_eq!(node_major("v18.0.0"), Some(18));
        assert_eq!(node_major("22.1"), None);
        assert_eq!(node_major(""), None);
    }

    #[test]
    fn comandos_do_gerenciador() {
        let c = Manager::Systemd.command(Action::Restart);
        assert_eq!(args(&c), ["--user", "restart", "cthubd"]);
        let uid = unsafe { libc::getuid() };
        let c = Manager::Launchd.command(Action::Restart);
        assert_eq!(
            args(&c),
            ["kickstart", "-k", &format!("gui/{uid}/{LAUNCHD_LABEL}")]
        );
        let c = Manager::Launchd.command(Action::Stop);
        assert_eq!(args(&c), ["bootout", &format!("gui/{uid}/{LAUNCHD_LABEL}")]);
        let c = Manager::Launchd.command(Action::Start);
        let a = args(&c);
        assert_eq!(a[..2], ["bootstrap".to_string(), format!("gui/{uid}")]);
        assert!(a[2].ends_with("Library/LaunchAgents/ai.deep-ion.cthubd.plist"));
    }
}
