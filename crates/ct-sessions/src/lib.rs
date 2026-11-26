//! Ciclo de vida das sessões: lançamento, hooks, backup de transcripts e
//! recuperação. Síncrono; o servidor chama daqui via `spawn_blocking`.

pub mod autonomy;
pub mod cleanup;
pub mod cleanup_branches;
pub mod cleanup_sessions;
pub mod cleanup_worktree;
pub mod config;
pub mod diff;
pub mod docs;
pub mod files;
pub mod github;
pub mod gitlab;
mod hooks;
mod launch;
pub mod paths;
mod prune;
mod recovery;
pub mod release;
pub mod remote;
mod resolver;
mod secrets;
pub mod shell;
pub mod tracker_sync;
pub mod transcript_view;
pub mod transcripts;
pub mod trust;
pub mod update;
pub mod usage;

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
pub use secrets::{SecretGetError, SESSION_TOKEN_ENV};
pub use shell::{Shell, SHELL_LIMIT};
pub use tracker_sync::{JiraGitlabSync, TrackerSyncConfig};
pub use transcript_view::{ContextUsage, TranscriptEntry, TranscriptSource, TranscriptTail};
pub use transcripts::TranscriptIntegrity;
pub use usage::{SessionUsage, StatuslineInstall};

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
    /// A branch de integração do modo autônomo não alinha com a base sem
    /// conflito (#181): a issue autônoma não começa.
    #[error("{0}")]
    AutonomyConflict(String),
    /// A issue não tem o rótulo de aprovação que o projeto exige para
    /// iniciar sessão (#124). A mensagem começa com
    /// `ct_core::APPROVAL_REFUSAL_PREFIX`.
    #[error("{0}")]
    ApprovalRequired(String),
    /// Já há uma sessão de release viva no projeto (#204).
    #[error("{0}")]
    ReleaseBusy(String),
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
    /// Comandos de credencial do Jira e do GitLab deste ambiente
    /// (`[credentials]` do `config.toml`, #143).
    pub credentials: ct_core::SharedCredentials,
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
    /// Token de cada sessão viva (#165) → id da sessão, só em memória.
    session_tokens: Mutex<HashMap<String, String>>,
    /// Shells livres vivos (#428), só em memória.
    shells: Mutex<HashMap<String, shell::LiveShell>>,
    /// Token de cada conexão (Jira, GitLab, GitHub), em cache: o comando de
    /// credencial não roda a cada ciclo (#236, #275).
    credentials: Arc<resolver::CredentialResolver>,
    /// Serializa a conferência "uma sessão de release viva por projeto" com
    /// a criação da sessão (#204).
    release_start: Mutex<()>,
    notify: Notifier,
}

/// Fachada do ciclo de vida das sessões. Clonar compartilha o estado.
#[derive(Clone)]
pub struct Sessions {
    inner: Arc<Inner>,
}

/// Aviso do resolvedor de token novo no cache (#266): o sync acabou de
/// resolver o token de uma conexão, e as sessões vivas passam a mascará-lo
/// ([`Sessions::refresh_masks`]). Roda em outra thread porque quem resolve
/// pode estar com o ledger ou outro lock do `Sessions`.
fn mask_refresher(weak: std::sync::Weak<Inner>) -> impl Fn() + Send + Sync + 'static {
    move || {
        let Some(inner) = weak.upgrade() else {
            return;
        };
        let sessions = Sessions { inner };
        let spawned = std::thread::Builder::new()
            .name("ct-mask-refresh".into())
            .spawn(move || sessions.refresh_masks());
        if let Err(e) = spawned {
            tracing::warn!("secrets: mascaramento das sessões não atualizado ({e})");
        }
    }
}

impl Sessions {
    pub fn new(cfg: SessionsConfig, ledger: Ledger, notify: Notifier) -> Self {
        Sessions {
            inner: Arc::new_cyclic(|weak: &std::sync::Weak<Inner>| Inner {
                cfg,
                ledger: Mutex::new(ledger),
                live: Mutex::new(HashMap::new()),
                killed: Mutex::new(HashSet::new()),
                masks: Mutex::new(HashMap::new()),
                session_tokens: Mutex::new(HashMap::new()),
                shells: Mutex::new(HashMap::new()),
                credentials: Arc::new(resolver::CredentialResolver::with_on_new(mask_refresher(
                    weak.clone(),
                ))),
                release_start: Mutex::new(()),
                notify,
            }),
        }
    }

    pub fn config(&self) -> &SessionsConfig {
        &self.inner.cfg
    }

    /// Resolvedor de credencial do ambiente (#236).
    pub(crate) fn credentials(&self) -> &resolver::CredentialResolver {
        &self.inner.credentials
    }

    /// Fonte do token do Jira do projeto neste ambiente (#274): a da conexão
    /// do hub, com a sobrescrita `[connections]`, ou o comando de
    /// `[credentials]` (#143). `None` = o rastreador não é Jira. Só para
    /// resolver aqui: o que vai ao hub sai do ledger, sem a sobrescrita.
    pub fn jira_source(&self, project: &ct_ledger::Project) -> Option<ct_core::EnvSource> {
        let j = project.tracker.jira()?;
        Some(
            self.inner
                .cfg
                .credentials
                .jira_source(&project.gh_owner, &project.gh_repo, j),
        )
    }

    /// Fonte do token do GitLab do projeto neste ambiente, como
    /// [`Self::jira_source`]. `None` = a forja não é GitLab.
    pub fn gitlab_source(&self, project: &ct_ledger::Project) -> Option<ct_core::EnvSource> {
        let g = project.forge.gitlab()?;
        Some(
            self.inner
                .cfg
                .credentials
                .gitlab_source(&project.gh_owner, &project.gh_repo, g),
        )
    }

    /// Fonte do token do GitHub do projeto neste ambiente (#275): a de
    /// [`Self::github_env_source`]. `None` = o projeto não usa o GitHub.
    pub fn github_source(&self, project: &ct_ledger::Project) -> Option<ct_core::EnvSource> {
        let github = project.tracker.kind() == ct_core::TrackerKind::Github
            || project.forge.kind() == ct_core::ForgeKind::Github;
        github.then(|| self.github_env_source(None))
    }

    /// Fonte do token do GitHub neste ambiente para a conexão `connection`
    /// (ou a `github` padrão, `None`): o token do ambiente
    /// (`github.token`, da tela de configurações do ambiente, até a #276),
    /// se houver, senão a conexão com a sobrescrita `[connections]`
    /// ([`ct_core::SharedCredentials::github_source`]).
    pub fn github_env_source(&self, connection: Option<i64>) -> ct_core::EnvSource {
        let file = self.inner.cfg.dirs.github_token_file();
        let has_token = std::fs::read_to_string(&file).is_ok_and(|t| !t.trim().is_empty());
        if has_token {
            return ct_core::EnvSource {
                source: ct_core::provider::CredentialSource::File {
                    path: file.to_string_lossy().into_owned(),
                },
                from_env: true,
            };
        }
        self.inner.cfg.credentials.github_source(connection)
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
