//! `ctower statusline` (issue #106): o statusline do Claude Code registrado
//! no ambiente. Lê o JSON da sessão no stdin, grava o arquivo de uso e
//! imprime a linha (ou a do statusline de terceiros encadeado). Nunca falha:
//! sai com 0 e, sem os diretórios, só não grava.

use ct_sessions::usage::{claude_config_dir, install_statusline, run_statusline};
use ct_sessions::{Dirs, StatuslineInstall};
use std::io::{Read, Write};
use std::path::PathBuf;

/// Statusline em si (chamado pelo Claude Code a cada atualização).
pub fn run() -> i32 {
    let mut input = Vec::new();
    let _ = std::io::stdin()
        .take(4 * 1024 * 1024)
        .read_to_end(&mut input);
    if let Ok(dirs) = Dirs::from_env() {
        let line = run_statusline(&dirs, &input, ct_core::now_ms());
        let mut out = std::io::stdout();
        let _ = out.write_all(line.as_bytes());
        let _ = out.flush();
    }
    0
}

/// `ctower statusline install`: registra o statusline no `settings.json` da
/// pasta de configuração do Claude Code (idempotente).
pub fn install(claude_dir: Option<PathBuf>, ctower_bin: Option<String>) -> i32 {
    let r = (|| -> Result<StatuslineInstall, String> {
        let dirs = Dirs::from_env().map_err(|e| e.to_string())?;
        let claude_dir = claude_dir.or_else(claude_config_dir).ok_or(
            "pasta de configuração do Claude Code desconhecida (defina CLAUDE_CONFIG_DIR)",
        )?;
        let bin = match ctower_bin {
            Some(b) => b,
            None => std::env::current_exe()
                .map_err(|e| e.to_string())?
                .to_string_lossy()
                .into_owned(),
        };
        install_statusline(&claude_dir, &dirs, &bin).map_err(|e| e.to_string())
    })();
    match r {
        Ok(o) => {
            println!(
                "{}",
                match o {
                    StatuslineInstall::Unchanged => "statusline já instalado na versão atual",
                    StatuslineInstall::Installed => "statusline instalado",
                    StatuslineInstall::Updated => "statusline atualizado",
                    StatuslineInstall::Chained =>
                        "statusline instalado; o que já existia roda dentro dele, com a mesma saída",
                }
            );
            0
        }
        Err(e) => {
            eprintln!("erro: {e}");
            1
        }
    }
}
