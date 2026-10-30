//! Atualização do ctowerd/ctower por build local (issue #90). O núcleo
//! (verificação, estado, atualizador) fica no crate `ct-update`, comum com o
//! `cthubd update`; aqui ficam o `Source` do ambiente e o disparo do
//! `ctower update --run` numa unidade transitória do systemd.

use crate::config::DaemonConfig;
use crate::{Dirs, Result, SessionsError};
use std::path::{Path, PathBuf};
use std::process::Command;

pub use ct_update::{
    git, log_file, read_state, resolve_bin, run, running, state_file, tail, workspace_version,
    write_state, Available, Build, Phase, Relation, Source, UpdateState,
};

impl From<ct_update::Error> for SessionsError {
    fn from(e: ct_update::Error) -> Self {
        SessionsError::Invalid(e.0)
    }
}

/// De onde vêm as atualizações do ambiente (`update_repo`, `update_ref`,
/// `<data>/update`).
pub fn source(cfg: &DaemonConfig, dirs: &Dirs) -> Source {
    Source {
        repo: cfg.update_repo.trim().to_string(),
        git_ref: cfg.update_ref.trim().to_string(),
        dir: dirs.update_dir(),
    }
}

/// Unidade transitória do systemd onde o atualizador roda.
pub const UNIT: &str = "ctower-update";

/// Variáveis repassadas ao atualizador (a unidade não herda o ambiente).
const PASS_ENV: &[&str] = &[
    "PATH",
    "HOME",
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
    "CTOWER_CONFIG_DIR",
    "CTOWER_DATA_DIR",
    "CTOWER_PORT",
    "CTOWER_BIN_DIR",
    "CTOWER_CARGO",
    "CTOWER_GIT",
    "CTOWER_SYSTEMCTL",
    "CTOWER_UPDATE_VERIFY_SECS",
];

/// Rótulo do LaunchAgent do ctowerd no macOS (`packaging/launchd/<rótulo>.plist`).
pub const LAUNCHD_LABEL: &str = "ai.deep-ion.ctowerd";

/// Gerenciador do serviço do ctowerd.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Manager {
    Systemd,
    Launchd,
}

impl Manager {
    /// `CTOWER_SERVICE_MANAGER` (`systemd`/`launchd`); sem ele, systemd quando
    /// `CTOWER_SYSTEMCTL` troca o binário (testes) e, fora isso, launchd no
    /// macOS e systemd de usuário no resto.
    pub fn from_env() -> Self {
        match std::env::var("CTOWER_SERVICE_MANAGER").as_deref() {
            Ok("launchd") => Manager::Launchd,
            Ok("systemd") => Manager::Systemd,
            _ if std::env::var_os("CTOWER_SYSTEMCTL").is_some() => Manager::Systemd,
            _ if cfg!(target_os = "macos") => Manager::Launchd,
            _ => Manager::Systemd,
        }
    }

    /// Comando de `restart`, `stop` ou `start` no serviço `ctowerd`.
    pub fn command(self, verb: &str) -> Command {
        match self {
            Manager::Systemd => {
                let bin = std::env::var("CTOWER_SYSTEMCTL").unwrap_or_else(|_| "systemctl".into());
                let mut c = Command::new(bin);
                c.args(["--user", verb, "ctowerd"]);
                c
            }
            Manager::Launchd => {
                let bin = std::env::var("CTOWER_LAUNCHCTL").unwrap_or_else(|_| "launchctl".into());
                let mut c = Command::new(bin);
                // SAFETY: getuid não falha nem tem pré-condições.
                let domain = format!("gui/{}", unsafe { libc::getuid() });
                match verb {
                    "restart" => c.args(["kickstart", "-k", &format!("{domain}/{LAUNCHD_LABEL}")]),
                    "stop" => c.args(["bootout", &format!("{domain}/{LAUNCHD_LABEL}")]),
                    _ => c.arg("bootstrap").arg(&domain).arg(plist_path()),
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

/// `~/.config/systemd/user/ctowerd.service`, onde o `provision.sh` instala.
pub fn unit_path() -> PathBuf {
    let home = std::env::var_os("HOME").unwrap_or_default();
    Path::new(&home).join(".config/systemd/user/ctowerd.service")
}

impl Manager {
    /// Executável que o serviço instalado sobe: o último `<string>` de
    /// `ProgramArguments` no plist (o plist passa o binário por último a um
    /// `/bin/sh`) ou o `ExecStart` da unidade, com `%h` expandido. `None` sem
    /// serviço instalado ou num formato que não reconhece.
    pub fn program(self) -> Option<PathBuf> {
        match self {
            Manager::Launchd => {
                let plist = std::fs::read_to_string(plist_path()).ok()?;
                plist_program(&plist).map(PathBuf::from)
            }
            Manager::Systemd => {
                let unit = std::fs::read_to_string(unit_path()).ok()?;
                let home = std::env::var("HOME").unwrap_or_default();
                unit_program(&unit).map(|p| PathBuf::from(p.replace("%h", &home)))
            }
        }
    }
}

fn plist_program(plist: &str) -> Option<&str> {
    let (_, rest) = plist.split_once("<key>ProgramArguments</key>")?;
    let (array, _) = rest.split_once("</array>")?;
    let (_, last) = array.rsplit_once("<string>")?;
    let (prog, _) = last.split_once("</string>")?;
    Some(prog.trim())
}

fn unit_program(unit: &str) -> Option<&str> {
    unit.lines()
        .find_map(|l| l.trim().strip_prefix("ExecStart="))?
        .trim_start_matches(['-', '@', '+', '!', ':'])
        .split_whitespace()
        .next()
}

/// Dispara `ctower update --run` fora do ctowerd, que o restart encerra
/// (`ct_update::launch`): no systemd, na unidade transitória [`UNIT`]; no
/// launchd, num processo com sessão nova.
pub fn spawn_runner(systemd_run: &str, ctower: &Path, rollback: bool) -> Result<()> {
    use ct_update::launch::{spawn, Launch};
    let args: &[&str] = if rollback {
        &["update", "--run", "--rollback"]
    } else {
        &["update", "--run"]
    };
    // Um `systemd-run` trocado (testes) sempre vale.
    let how = if systemd_run == "systemd-run" && Manager::from_env() == Manager::Launchd {
        Launch::Detached
    } else {
        Launch::Systemd {
            systemd_run,
            unit: UNIT,
            pass_env: PASS_ENV,
        }
    };
    Ok(spawn(how, ctower, args)?)
}

/// `systemd-run` (ou `CTOWER_SYSTEMD_RUN`, nos testes).
pub fn systemd_run_bin() -> String {
    std::env::var("CTOWER_SYSTEMD_RUN").unwrap_or_else(|_| "systemd-run".into())
}

#[cfg(test)]
mod launchd_tests {
    use super::*;

    fn args(c: &Command) -> Vec<String> {
        c.get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn launchd_traduz_os_verbos_para_launchctl() {
        // SAFETY: getuid não falha nem tem pré-condições.
        let domain = format!("gui/{}", unsafe { libc::getuid() });
        let label = format!("{domain}/{LAUNCHD_LABEL}");
        assert_eq!(
            args(&Manager::Launchd.command("restart")),
            ["kickstart", "-k", &label]
        );
        assert_eq!(args(&Manager::Launchd.command("stop")), ["bootout", &label]);
        let start = args(&Manager::Launchd.command("start"));
        assert_eq!(start[..2], ["bootstrap".to_string(), domain]);
        assert!(start[2].ends_with(&format!("LaunchAgents/{LAUNCHD_LABEL}.plist")));
    }

    #[test]
    fn programa_do_plist_e_da_unidade_do_pacote() {
        let plist = include_str!("../../../packaging/launchd/ai.deep-ion.ctowerd.plist");
        assert_eq!(plist_program(plist), Some("@HOME@/.cargo/bin/ctowerd"));
        let unit = include_str!("../../../packaging/systemd/ctowerd.service");
        assert_eq!(unit_program(unit), Some("%h/.cargo/bin/ctowerd"));
        assert_eq!(
            unit_program("ExecStart=-/usr/bin/x --a"),
            Some("/usr/bin/x")
        );
        assert_eq!(plist_program("<plist/>"), None);
    }
}
