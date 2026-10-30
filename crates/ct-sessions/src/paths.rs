//! Diretórios XDG do daemon e tokens de acesso (spec §11, §13).

pub use ct_core::files::{ensure_env_id, ensure_token, private_dir, write_private};
use std::io;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Dirs {
    pub config: PathBuf,
    pub data: PathBuf,
}

impl Dirs {
    /// `CTOWER_CONFIG_DIR`/`CTOWER_DATA_DIR`, senão `~/.config/ctower` e
    /// `~/.local/share/ctower` (respeitando `XDG_*_HOME`).
    pub fn from_env() -> io::Result<Self> {
        let var = |k: &str| {
            std::env::var_os(k)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
        };
        let home = var("HOME")
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "HOME não definido"))?;
        let config = var("CTOWER_CONFIG_DIR").unwrap_or_else(|| {
            var("XDG_CONFIG_HOME")
                .unwrap_or_else(|| home.join(".config"))
                .join("ctower")
        });
        let data = var("CTOWER_DATA_DIR").unwrap_or_else(|| {
            var("XDG_DATA_HOME")
                .unwrap_or_else(|| home.join(".local/share"))
                .join("ctower")
        });
        Ok(Dirs { config, data })
    }

    pub fn env_id_file(&self) -> PathBuf {
        self.config.join("env_id")
    }

    /// Token de sync deste ambiente com o hub (0600).
    pub fn hub_token_file(&self) -> PathBuf {
        self.config.join("hub.token")
    }

    /// Token de agente do hub (`/agent/v1`, spec do gestor §6.1), lido a
    /// cada chamada repassada pelas ferramentas `portfolio_*`.
    pub fn hub_agent_token_file(&self) -> PathBuf {
        self.config.join("hub-agent.token")
    }

    /// Identidade age do ambiente para a entrega de secrets do hub (0600,
    /// issue #184).
    pub fn env_key_file(&self) -> PathBuf {
        self.config.join("env.key")
    }

    /// Cache dos secrets entregues pelo hub, cifrado para `env.key` (0600,
    /// issue #184). Hub fora do ar: vale o último.
    pub fn secrets_cache_file(&self) -> PathBuf {
        self.data.join("secrets-cache.age")
    }

    pub fn ledger_db(&self) -> PathBuf {
        self.data.join("ledger.db")
    }

    pub fn transcripts(&self) -> PathBuf {
        self.data.join("transcripts")
    }

    pub fn spool(&self) -> PathBuf {
        self.data.join("hooks-spool")
    }

    /// Clone, build, backups e estado das atualizações (`ctower update`).
    pub fn update_dir(&self) -> PathBuf {
        self.data.join("update")
    }

    pub fn session_dir(&self, session_id: &str) -> PathBuf {
        self.data.join("sessions").join(session_id)
    }

    /// Cria os diretórios privados (0700).
    pub fn ensure(&self) -> io::Result<()> {
        for d in [&self.config, &self.data, &self.transcripts(), &self.spool()] {
            private_dir(d)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;

    fn mode(p: &Path) -> u32 {
        std::fs::metadata(p).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn ensure_cria_diretorios_privados() {
        let d = tempfile::tempdir().unwrap();
        let dirs = Dirs {
            config: d.path().join("c"),
            data: d.path().join("d"),
        };
        dirs.ensure().unwrap();
        assert_eq!(mode(&dirs.spool()), 0o700);
        assert_eq!(mode(&dirs.transcripts()), 0o700);
    }
}
