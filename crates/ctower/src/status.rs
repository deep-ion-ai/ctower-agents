//! `ctower status` e `ctower restart [--force]` (spec §12).

use ct_sessions::config::DaemonConfig;
use ct_sessions::Dirs;
use serde_json::Value;
use std::time::Duration;

/// Porta do ctowerd local (config + `CTOWER_PORT`).
fn port() -> Result<u16, String> {
    let dirs = Dirs::from_env().map_err(|e| e.to_string())?;
    Ok(DaemonConfig::load(&dirs).map_err(|e| e.to_string())?.port)
}

/// URL do ctowerd local e token de UI.
pub(crate) fn daemon() -> Result<(String, String), String> {
    let dirs = Dirs::from_env().map_err(|e| e.to_string())?;
    let cfg = DaemonConfig::load(&dirs).map_err(|e| e.to_string())?;
    let token = std::fs::read_to_string(dirs.config.join("ui.token"))
        .map_err(|e| format!("token de UI ilegível: {e}"))?;
    Ok((
        format!("http://127.0.0.1:{}", cfg.port),
        token.trim().to_string(),
    ))
}

/// Sessões vivas que um restart do ctowerd encerraria.
pub(crate) enum Live {
    /// Nenhuma, ou o ctowerd está fora do ar.
    None,
    Some(Vec<String>),
    /// Há algo escutando na porta, mas `/status` não respondeu (token
    /// ilegível ou recusado): não dá para saber (#283).
    Unknown(String),
}

pub(crate) fn live_sessions() -> Live {
    match fetch_status() {
        Ok(st) => match live_lines(&st) {
            l if l.is_empty() => Live::None,
            l => Live::Some(l),
        },
        Err(e) => {
            let up = port().is_ok_and(|p| {
                let addr = std::net::SocketAddr::from(([127, 0, 0, 1], p));
                std::net::TcpStream::connect_timeout(&addr, Duration::from_secs(1)).is_ok()
            });
            if up {
                Live::Unknown(e)
            } else {
                Live::None
            }
        }
    }
}

pub(crate) fn fetch_status() -> Result<Value, String> {
    let (url, token) = daemon()?;
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(3))
        .build();
    let resp = agent
        .get(&format!("{url}/status"))
        .set("Authorization", &format!("Bearer {token}"))
        .call()
        .map_err(|e| format!("ctowerd fora do ar ({e})"))?;
    let body = resp.into_string().map_err(|e| e.to_string())?;
    serde_json::from_str(&body).map_err(|e| e.to_string())
}

pub(crate) fn live_lines(st: &Value) -> Vec<String> {
    st["live_sessions"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|s| {
                    format!(
                        "  {}  {:<8}  {}",
                        s["id"].as_str().unwrap_or("?"),
                        s["state"].as_str().unwrap_or("?"),
                        s["cwd"].as_str().unwrap_or("?")
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

pub fn status() -> i32 {
    match fetch_status() {
        Ok(st) => {
            let live = live_lines(&st);
            println!(
                "ctowerd {} no ar — {} sessão(ões) viva(s), {} interrompida(s)",
                st["version"].as_str().unwrap_or("?"),
                live.len(),
                st["interrupted"]
            );
            for l in live {
                println!("{l}");
            }
            0
        }
        Err(e) => {
            eprintln!("{e}");
            if let Some(prog) = missing_program() {
                eprintln!(
                    "executável do serviço ausente: {} (restaure o binário e rode `ctower restart`)",
                    prog.display()
                );
            }
            1
        }
    }
}

/// O executável que o serviço instalado sobe, se ele não existir: o caso em
/// que o launchd só registra exit 78 em loop (#360).
fn missing_program() -> Option<std::path::PathBuf> {
    let prog = ct_sessions::update::Manager::from_env().program()?;
    (!prog.exists()).then_some(prog)
}

/// Reinicia o serviço. Com sessões vivas, recusa sem `--force` (reiniciar
/// mata as sessões, que voltam como `interrupted`).
pub fn restart(force: bool) -> i32 {
    if !force {
        match live_sessions() {
            Live::None => {}
            Live::Some(live) => {
                eprintln!(
                    "Recusado: {} sessão(ões) viva(s) seriam encerradas:",
                    live.len()
                );
                for l in live {
                    eprintln!("{l}");
                }
                eprintln!("Use `ctower restart --force` para reiniciar mesmo assim.");
                return 1;
            }
            Live::Unknown(e) => {
                eprintln!(
                    "Recusado: o ctowerd está no ar, mas não consegui ver as sessões vivas ({e})."
                );
                eprintln!("Use `ctower restart --force` para reiniciar mesmo assim.");
                return 1;
            }
        }
    }
    match ct_sessions::update::Manager::from_env()
        .command("restart")
        .status()
    {
        Ok(s) if s.success() => {
            println!("ctowerd reiniciado.");
            0
        }
        Ok(s) => s.code().unwrap_or(1),
        Err(e) => {
            eprintln!("falha ao reiniciar o serviço: {e}");
            1
        }
    }
}
