//! Ciclo de vida das sessões: lançamento, hooks, backup de transcripts e
//! recuperação. Síncrono; o servidor chama daqui via `spawn_blocking`.

pub mod cleanup;
pub mod cleanup_branches;
pub mod cleanup_sessions;
pub mod cleanup_worktree;
pub mod config;
pub mod docs;
pub mod github;
pub mod gitlab;
mod hooks;
mod launch;
pub mod paths;
mod prune;
mod recovery;
pub mod remote;
mod secrets;
pub mod shell;
pub mod tracker_sync;
pub mod transcript_view;
pub mod transcripts;
pub mod trust;
pub mod update;

pub use github::{GithubSync, ProjectSync};
pub use gitlab::GitlabRemote;
pub use hooks::{HookEnvelope, HookOutcome};
pub use launch::{
    gestor_scopes, AgentOptions, GestorScope, PtySize, StartRequest, GESTOR_LIMITS,
    PROJECT_WORK_ITEM,
};
pub use paths::{ensure_env_id, ensure_token, Dirs};
pub use prune::PruneOutcome;
pub use recovery::{
    recoverable_sessions, work_item_concluded, ArchivedItem, RecoveryItem, RESUME_PROMPT,
};
pub use remote::{RemoteControlEnable, RemoteControlStatus};
pub use shell::{Shell, SHELL_LIMIT};
pub use tracker_sync::{JiraGitlabSync, TrackerSyncConfig};
pub use transcript_view::{ContextUsage, TranscriptEntry, TranscriptSource, TranscriptTail};
pub use transcripts::TranscriptIntegrity;

use ct_ledger::Ledger;
use ct_pty::PtySession;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

#[derive(Debug, thiserror::Error)]
pub enum SessionsError {
    #[error(transparent)]
    Ledger(#[from] ct_ledger::LedgerError),
    #[error(transparent)]
    Git(#[from] ct_git::GitError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Invalid(String),
    /// O ambiente efetivo da sessão impede o Remote Control (issue #197).
    #[error("{0}")]
    RemoteControl(String),
}

pub type Result<T> = std::result::Result<T, SessionsError>;

#[derive(Debug, Clone)]
pub struct SessionsConfig {
    pub dirs: Dirs,
    /// Porta HTTP do daemon (para `CTOWER_URL` e o `mcp.json`).
    pub port: u16,
    pub hook_token: String,
    /// Binário do agente (`claude`); testes usam um script falso.
    pub claude_bin: String,
    /// Binário chamado pelos hooks (`ctower`).
    pub ctower_bin: String,
    pub worktrees_root: PathBuf,
    /// Pasta das sessões do gestor (`DaemonConfig::gestor_dir`).
    pub gestor_dir: PathBuf,
    pub scrollback: usize,
    /// `.claude.json` onde pré-aceitar a confiança nas pastas das sessões
    /// (`None` = não mexe; o humano responde ao diálogo no terminal).
    pub claude_json: Option<PathBuf>,
    /// `~/.claude/projects`, onde o Claude Code guarda os transcripts de
    /// cada pasta; a limpeza apaga a do worktree removido (#285). `None` =
    /// nunca apaga.
    pub claude_projects: Option<PathBuf>,
    /// Idade máxima da configuração do hub (secrets do cache) antes do aviso
    /// no spawn (`secrets_max_age_hours`); `None` = não avisa.
    pub secrets_max_age: Option<Duration>,
    /// Ambiente do processo considerado nos requisitos do Remote Control
    /// (#197); `None` = o do próprio `ctowerd`. Os testes fixam um, para não
    /// depender do ambiente de quem os roda.
    pub process_env: Option<Vec<(String, String)>>,
    /// Programa do shell livre (#428); `None` = o shell de login do
    /// usuário (`$SHELL`, ou o do `passwd`). Os testes fixam um.
    pub shell: Option<String>,
    /// Raiz da pasta de docs dos projetos (`docs_root`, #440); `None` = sem
    /// docs neste ambiente.
    pub docs_root: Option<PathBuf>,
}

/// Notificação para quem observa o daemon (o servidor repassa à UI):
/// nome do evento e dados.
pub type Notifier = Arc<dyn Fn(&str, Value) + Send + Sync>;

struct Inner {
    cfg: SessionsConfig,
    ledger: Mutex<Ledger>,
    live: Mutex<HashMap<String, PtySession>>,
    /// Sessões encerradas por `kill` (pedido humano): a saída é limpa.
    killed: Mutex<HashSet<String>>,
    /// Mascaramento dos secrets de cada sessão (#167), só em memória.
    masks: Mutex<HashMap<String, secrets::SessionMask>>,
    /// Shells livres vivos (#428), só em memória.
    shells: Mutex<HashMap<String, shell::LiveShell>>,
    notify: Notifier,
}

/// Fachada do ciclo de vida das sessões. Clonar compartilha o estado.
#[derive(Clone)]
pub struct Sessions {
    inner: Arc<Inner>,
}

impl Sessions {
    pub fn new(cfg: SessionsConfig, ledger: Ledger, notify: Notifier) -> Self {
        Sessions {
            inner: Arc::new(Inner {
                cfg,
                ledger: Mutex::new(ledger),
                live: Mutex::new(HashMap::new()),
                killed: Mutex::new(HashSet::new()),
                masks: Mutex::new(HashMap::new()),
                shells: Mutex::new(HashMap::new()),
                notify,
            }),
        }
    }

    pub fn config(&self) -> &SessionsConfig {
        &self.inner.cfg
    }

    /// Acesso direto ao ledger (consultas do servidor).
    pub fn ledger(&self) -> MutexGuard<'_, Ledger> {
        self.inner.ledger.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// PTY de uma sessão viva neste processo do daemon.
    pub fn pty(&self, session_id: &str) -> Option<PtySession> {
        self.inner.live.lock().unwrap().get(session_id).cloned()
    }

    pub fn kill(&self, session_id: &str) -> Result<()> {
        match self.pty(session_id) {
            Some(p) => {
                self.inner
                    .killed
                    .lock()
                    .unwrap()
                    .insert(session_id.to_string());
                Ok(p.kill()?)
            }
            None => Err(SessionsError::Invalid(format!(
                "sessão {session_id} não está rodando neste daemon"
            ))),
        }
    }

    /// Push para as UIs conectadas (`{ "event": ..., "data": ... }`).
    pub fn notify(&self, event: &str, data: Value) {
        (self.inner.notify)(event, data);
    }
}

#[cfg(any(test, feature = "test-support"))]
pub mod test_support;
