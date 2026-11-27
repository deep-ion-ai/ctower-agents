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
pub mod epic_plan;
pub mod epic_run;
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

pub use ct_agent::AgentHome;
use ct_agent::AgentRuntime;
use ct_core::Tool;
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

/// Agente das sessões novas e dono de [`SessionsConfig::agent_home`]: por
/// enquanto toda sessão é do Claude Code (épico #225).
pub const AGENT_TOOL: Tool = Tool::Claude;

/// [`AgentHome`] do agente de [`AGENT_TOOL`] pelas variáveis do processo,
/// para o boot. `pre_trust = false` (`pre_trust_worktrees`) tira o arquivo
/// da confiança: as sessões sobem sem pré-aceitá-la.
pub fn agent_home_from_env(pre_trust: bool) -> AgentHome {
    let mut home = ct_agent::runtime(AGENT_TOOL)
        .map(|r| r.home_from_env())
        .unwrap_or_default();
    if !pre_trust {
        home.config_file = None;
    }
    home
}

/// Runtime registrado para a ferramenta (#240); ferramenta sem runtime vira
/// erro.
pub(crate) fn agent_runtime(tool: Tool) -> Result<&'static dyn AgentRuntime> {
    ct_agent::runtime(tool)
        .ok_or_else(|| SessionsError::Invalid(format!("agente sem runtime: {}", tool.as_str())))
}

/// Depois do `SIGHUP` de [`Sessions::kill`], quanto esperar o agente sair
/// antes do `SIGKILL` (#312). Depois do `SIGHUP`, o agente pode precisar de
/// alguns segundos para gravar o transcript e encerrar os filhos; 5 s dão
/// essa folga, inclusive numa máquina carregada, sem deixar a sessão viva
/// quando o agente ignora o sinal. Menor que o prazo da limpeza para a
/// sessão encerrada sair do estado vivo (`cleanup_sessions::END_TIMEOUT`).
pub(crate) const KILL_GRACE: Duration = Duration::from_secs(5);

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
    /// Onde o agente guarda o próprio estado (confiança nas pastas e
    /// transcripts por pasta, #16/#285), do runtime de [`AGENT_TOOL`].
    pub agent_home: AgentHome,
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
    /// Conexões do hub do último pull e as sobrescritas de credencial deste
    /// ambiente (`[connections]` do `config.toml`, #274; o `[credentials]`
    /// legado, #143).
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
    /// Última entrada humana registrada por sessão da execução de épico
    /// (#301), para o debounce do evento `human_input`.
    human_input: Mutex<HashMap<String, i64>>,
    notify: Notifier,
    /// Prazo da cauda do mascaramento nos PTYs das sessões novas
    /// ([`Sessions::set_mask_hold`]); `None` = o de produção
    /// ([`ct_pty::MASK_HOLD`]).
    #[cfg(any(test, feature = "test-support"))]
    mask_hold: Mutex<Option<Duration>>,
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
                human_input: Mutex::new(HashMap::new()),
                notify,
                #[cfg(any(test, feature = "test-support"))]
                mask_hold: Mutex::new(None),
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

    /// Fonte do token do tipo `kind` do projeto neste ambiente (#274,
    /// #275): o comando de `[credentials]` que sobrou da conversão (#143),
    /// senão a da conexão do hub (a referenciada ou a padrão do tipo) com a
    /// sobrescrita `[connections]`, senão a de reserva do descritor
    /// ([`ct_core::SharedCredentials::source`]). `None` = nenhum lado do
    /// projeto é do tipo. Só para resolver aqui: o que vai ao hub sai do
    /// ledger, sem a sobrescrita.
    pub fn source(
        &self,
        project: &ct_ledger::Project,
        kind: ct_core::provider::ProviderKind,
    ) -> Option<ct_core::EnvSource> {
        self.inner.cfg.credentials.source(
            &project.gh_owner,
            &project.gh_repo,
            &project.tracker,
            &project.forge,
            kind,
        )
    }

    /// As fontes de todos os tipos que o projeto usa, sem repetir o tipo
    /// (o GitHub nos dois lados conta uma vez).
    pub fn sources(
        &self,
        project: &ct_ledger::Project,
    ) -> Vec<(ct_core::provider::ProviderKind, ct_core::EnvSource)> {
        let mut out: Vec<(ct_core::provider::ProviderKind, ct_core::EnvSource)> = Vec::new();
        for side in ct_core::provider::project_sides(&project.tracker, &project.forge) {
            if out.iter().any(|(k, _)| *k == side.kind) {
                continue;
            }
            out.extend(self.source(project, side.kind).map(|s| (side.kind, s)));
        }
        out
    }

    /// Fonte do token do Jira do projeto ([`Self::source`]). `None` = o
    /// rastreador não é Jira.
    pub fn jira_source(&self, project: &ct_ledger::Project) -> Option<ct_core::EnvSource> {
        self.source(project, ct_core::provider::ProviderKind::Jira)
    }

    /// Fonte do token do GitLab do projeto ([`Self::source`]). `None` = a
    /// forja não é GitLab.
    pub fn gitlab_source(&self, project: &ct_ledger::Project) -> Option<ct_core::EnvSource> {
        self.source(project, ct_core::provider::ProviderKind::Gitlab)
    }

    /// Token da conexão do tipo `kind` do projeto pelo resolvedor do
    /// ambiente (cache, sobrescrita `[connections]`, secret do cache): o
    /// mesmo do sync, e por isso no mascaramento das sessões. Usado no
    /// clone do GitLab por HTTPS (#281). Pode rodar o comando de credencial.
    pub fn connection_token(
        &self,
        project: &ct_ledger::Project,
        kind: ct_core::provider::ProviderKind,
    ) -> std::result::Result<String, String> {
        let source = self
            .source(project, kind)
            .map_or(ct_core::provider::CredentialSource::None, |s| s.source);
        let lookup = self.secret_lookup(project);
        self.credentials()
            .token(resolver::Ask {
                key: &resolver::cache_key(kind, project.id),
                what: ct_core::provider::descriptor(kind).label(),
                source: &source,
                lookup: &lookup,
                deadline: ct_proc::CREDENTIAL_DEADLINE,
                cli: None,
            })
            .map(|t| t.expose().to_string())
            .map_err(|e| e.to_string())
    }

    /// Fonte do token do GitHub do projeto ([`Self::source`]): sem a
    /// conexão no pull, o `gh auth token`. `None` = o projeto não usa o
    /// GitHub.
    pub fn github_source(&self, project: &ct_ledger::Project) -> Option<ct_core::EnvSource> {
        self.source(project, ct_core::provider::ProviderKind::Github)
    }

    /// As conexões do projeto neste ambiente (#277): as do último pull, com
    /// a fonte efetiva daqui ([`ct_core::SharedCredentials::resolve`]). É
    /// daqui que o sync, as sessões, os templates e os links leem a URL do
    /// Jira, o host do GitLab, o `config_file` do jira-cli e os prefixos.
    pub fn project_connections(
        &self,
        project: &ct_ledger::Project,
    ) -> ct_core::ResolvedConnections {
        self.inner
            .cfg
            .credentials
            .resolve(
                &project.gh_owner,
                &project.gh_repo,
                &project.tracker,
                &project.forge,
            )
            .0
    }

    /// O projeto com os endereços e prefixos das conexões deste ambiente
    /// (`endpoints`, #277), como a UI o recebe.
    pub fn with_endpoints(&self, mut project: ct_ledger::Project) -> ct_ledger::Project {
        project.endpoints = Some(self.project_connections(&project).endpoints());
        project
    }

    /// Tira do cache do resolvedor os tokens dos projetos que usam a conexão
    /// `name` (#276): a sobrescrita editada pela UI vale no próximo uso,
    /// mesmo quando o comando não muda (o token por trás dele mudou).
    pub fn invalidate_connection(&self, name: &str) -> Result<()> {
        let (conns, _) = self.inner.cfg.credentials.connections();
        let Some(c) = conns.iter().find(|c| c.name == name) else {
            return Ok(());
        };
        let projects = self.ledger().list_projects()?;
        for p in projects {
            if ct_core::provider::uses_connection(&p.tracker, &p.forge, &conns, c) {
                self.credentials()
                    .invalidate(&resolver::cache_key(c.kind, p.id));
            }
        }
        Ok(())
    }

    /// Acesso direto ao ledger (consultas do servidor).
    pub fn ledger(&self) -> MutexGuard<'_, Ledger> {
        self.inner.ledger.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// PTY de uma sessão viva neste processo do daemon.
    pub fn pty(&self, session_id: &str) -> Option<PtySession> {
        self.inner.live.lock().unwrap().get(session_id).cloned()
    }

    /// Encerra a sessão a pedido do humano: `SIGHUP` e, se o agente não
    /// sair em [`KILL_GRACE`], `SIGKILL` (#312). Não espera a saída: o
    /// estado passa a `exited` quando o processo sai.
    pub fn kill(&self, session_id: &str) -> Result<()> {
        match self.pty(session_id) {
            Some(p) => {
                self.inner
                    .killed
                    .lock()
                    .unwrap()
                    .insert(session_id.to_string());
                Ok(p.terminate(KILL_GRACE)?)
            }
            None => Err(SessionsError::Invalid(format!(
                "sessão {session_id} não está rodando neste daemon"
            ))),
        }
    }

    /// Troca o prazo da cauda do mascaramento das sessões iniciadas daqui
    /// em diante. Só para testes: um teste que parte um secret em duas
    /// escritas não pode depender de a segunda chegar dentro do
    /// [`ct_pty::MASK_HOLD`] (#313).
    #[cfg(any(test, feature = "test-support"))]
    pub fn set_mask_hold(&self, hold: Duration) {
        *self.inner.mask_hold.lock().unwrap() = Some(hold);
    }

    /// Sobe o PTY de uma sessão, com o prazo da cauda dos testes, se houver
    /// ([`Sessions::set_mask_hold`]).
    pub(crate) fn spawn_pty(&self, spec: ct_pty::SpawnSpec) -> std::io::Result<PtySession> {
        let scrollback = self.inner.cfg.scrollback;
        #[cfg(any(test, feature = "test-support"))]
        if let Some(hold) = *self.inner.mask_hold.lock().unwrap() {
            return PtySession::spawn_with_hold(spec, scrollback, hold);
        }
        PtySession::spawn(spec, scrollback)
    }

    /// Push para as UIs conectadas (`{ "event": ..., "data": ... }`).
    pub fn notify(&self, event: &str, data: Value) {
        (self.inner.notify)(event, data);
    }
}

#[cfg(any(test, feature = "test-support"))]
pub mod test_support;
