//! `ctower update` (issue #90): atualização do ctowerd/ctower por build local.

use ct_sessions::config::DaemonConfig;
use ct_sessions::update::{log_file, Build, Phase, Relation, UpdateState};
use ct_sessions::Dirs;
use ct_update::runner::{short, Action, Runner, Target};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Build do daemon no ar (`/healthz`) ou, fora do ar, o deste `ctower`.
fn installed(port: u16) -> Build {
    healthz(port).unwrap_or_else(Build::current)
}

/// Build informado pelo `/healthz`; `None` = daemon fora do ar.
fn healthz(port: u16) -> Option<Build> {
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(3))
        .build();
    agent
        .get(&format!("http://127.0.0.1:{port}/healthz"))
        .call()
        .ok()
        .and_then(|r| r.into_string().ok())
        .and_then(|b| serde_json::from_str::<serde_json::Value>(&b).ok())
        .and_then(|v| {
            Some(Build {
                version: v["version"].as_str()?.into(),
                commit: v["commit"].as_str().unwrap_or_default().into(),
            })
        })
}

/// `ctower update --check`: versão instalada × disponível no ref seguido.
pub fn check() -> i32 {
    let r = (|| -> Result<_, String> {
        let dirs = Dirs::from_env().map_err(|e| e.to_string())?;
        let cfg = DaemonConfig::load(&dirs).map_err(|e| e.to_string())?;
        let inst = installed(cfg.port);
        let source = ct_sessions::update::source(&cfg, &dirs);
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

// --- atualizador (`ctower update --run`) ------------------------------------

/// Binários instalados pela atualização.
const BINS: [&str; 2] = ["ctowerd", "ctower"];
/// Bancos com snapshot antes do restart (relativos ao diretório de dados).
/// O `hub.db` é o do hub antigo (`hub = true`), que fica no diretório do
/// ambiente até o `cthubd migrate`; sem ele, nada a copiar.
const DBS: [&str; 2] = ["ledger.db", "hub.db"];

fn env_or(key: &str, default: impl FnOnce() -> String) -> String {
    std::env::var(key)
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(default)
}

fn cargo() -> String {
    env_or("CTOWER_CARGO", || {
        let home = std::env::var("HOME").unwrap_or_default();
        let p = Path::new(&home).join(".cargo/bin/cargo");
        if p.exists() {
            p.display().to_string()
        } else {
            "cargo".into()
        }
    })
}

/// Diretório onde ficam ctowerd e ctower: o deste executável.
fn bin_dir() -> Result<PathBuf, String> {
    if let Ok(d) = std::env::var("CTOWER_BIN_DIR") {
        return Ok(PathBuf::from(d));
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    exe.parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| "diretório do ctower desconhecido".into())
}

fn verify_timeout() -> Duration {
    Duration::from_secs(
        std::env::var("CTOWER_UPDATE_VERIFY_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(60),
    )
}

fn runner(kind: &str) -> Result<Runner, String> {
    let dirs = Dirs::from_env().map_err(|e| e.to_string())?;
    let cfg = DaemonConfig::load(&dirs).map_err(|e| e.to_string())?;
    let source = ct_sessions::update::source(&cfg, &dirs);
    let port = cfg.port;
    let target = Target {
        service: "ctowerd".into(),
        bins: BINS.iter().map(|b| b.to_string()).collect(),
        dbs: DBS.iter().map(|d| dirs.data.join(d)).collect(),
        bin_dir: bin_dir()?,
        cargo: cargo(),
        release_prefix: Some("ctower-env".into()),
        verify_timeout: verify_timeout(),
        ctl: Box::new(|a| {
            ct_sessions::update::Manager::from_env().command(match a {
                Action::Restart => "restart",
                Action::Stop => "stop",
                Action::Start => "start",
            })
        }),
        probe: Box::new(move || healthz(port).ok_or_else(|| "/healthz não respondeu".into())),
        // O rollback do ambiente segue como antes (sem --force); o banco em
        // uso fica numa cópia .pre-rollback-* ao lado.
        after_db_restore: None,
        force_db_rollback: false,
        db_rollback_note: None,
        pre_build: None,
    };
    Runner::new(kind, source, target, installed(port)).map_err(|e| e.to_string())
}

/// `ctower update --run [--rollback]`: o atualizador em si. Normalmente roda
/// numa unidade transitória do systemd, fora do cgroup do ctowerd.
pub fn run(rollback: bool) -> i32 {
    let mut r = match runner(if rollback { "rollback" } else { "update" }) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("atualização: {e}");
            return 1;
        }
    };
    if rollback {
        r.rollback(true)
    } else {
        r.update()
    }
}

// --- disparo pelo usuário (`ctower update [--force] [--rollback]`) ----------

/// Quanto esperar o atualizador gravar o primeiro estado.
const START_TIMEOUT: Duration = Duration::from_secs(30);

/// Dispara o atualizador numa unidade do systemd e acompanha o
/// `state.json` até o fim. Com sessões vivas, recusa sem `--force`.
pub fn update(force: bool, rollback: bool) -> i32 {
    let what = if rollback { "rollback" } else { "update" };
    if !force {
        let flag = if rollback { " --rollback" } else { "" };
        match crate::status::live_sessions() {
            crate::status::Live::None => {}
            crate::status::Live::Some(live) => {
                eprintln!(
                    "Recusado: {} sessão(ões) viva(s) seriam encerradas pelo restart:",
                    live.len()
                );
                for l in live {
                    eprintln!("{l}");
                }
                eprintln!("Use `ctower update{flag} --force` para seguir mesmo assim.");
                return 1;
            }
            crate::status::Live::Unknown(e) => {
                eprintln!(
                    "Recusado: o ctowerd está no ar, mas não consegui ver as sessões vivas ({e})."
                );
                eprintln!("Use `ctower update{flag} --force` para seguir mesmo assim.");
                return 1;
            }
        }
    }
    let dir = match Dirs::from_env() {
        Ok(d) => d.update_dir(),
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    let t0 = ct_core::now_ms();
    if let Some(s) = ct_sessions::update::running(&dir, t0) {
        eprintln!("Uma atualização já está em andamento ({}).", label(s.phase));
        return 1;
    }
    let me = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    let run = ct_sessions::update::systemd_run_bin();
    if let Err(e) = ct_sessions::update::spawn_runner(&run, &me, rollback) {
        eprintln!("não foi possível iniciar o {what}: {e}");
        return 1;
    }
    println!(
        "{what} iniciado na unidade {} (log: {})",
        ct_sessions::update::UNIT,
        log_file(&dir).display()
    );
    follow(&dir, t0)
}

/// Imprime as fases até o estado terminal; devolve o código de saída.
fn follow(dir: &Path, t0: i64) -> i32 {
    let started = Instant::now();
    let mut last: Option<Phase> = None;
    loop {
        let st = ct_sessions::update::read_state(dir).filter(|s| s.started_at >= t0);
        match st {
            None if started.elapsed() > START_TIMEOUT => {
                eprintln!(
                    "O atualizador não começou. Veja {}.",
                    if ct_sessions::update::Manager::from_env()
                        == ct_sessions::update::Manager::Launchd
                    {
                        format!("{}", log_file(dir).display())
                    } else {
                        format!("`journalctl --user -u {}`", ct_sessions::update::UNIT)
                    }
                );
                return 1;
            }
            None => {}
            Some(s) => {
                if last != Some(s.phase) {
                    println!("  {}", label(s.phase));
                    last = Some(s.phase);
                }
                if s.phase.is_terminal() {
                    return report(&s);
                }
            }
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

fn label(p: Phase) -> String {
    ct_update::runner::label(p, "ctowerd")
}

fn report(s: &UpdateState) -> i32 {
    ct_update::runner::report(s, "ctowerd")
}
