//! Daemon do Control Tower: dono das sessões, do ledger e do servidor HTTP
//! daquele ambiente. Roda como serviço systemd de usuário. O hub é outro
//! binário (`cthubd`); este envia a ele (`hub_url`) a execução e puxa dele a
//! configuração dos projetos atribuídos.

use clap::Parser;
use ct_gh::{GhClient, GhConfig};
use ct_ledger::Ledger;
use ct_server::cleanup::{spawn_periodic as spawn_cleanup_check, Cleanup};
use ct_server::env_config::EnvConfig;
use ct_server::hub_config::{spawn_puller, ConfigOptions};
use ct_server::hub_sync::{spawn_sender, HubSender, HubTarget};
use ct_server::update::{spawn_periodic as spawn_update_check, Updates};
use ct_server::{notifier, router, AppState};
use ct_sessions::cleanup::Cleaner;
use ct_sessions::config::{DaemonConfig, HUB_REMOVED};
use ct_sessions::github::next_delay;
use ct_sessions::update::Build;
use ct_sessions::{
    ensure_env_id, ensure_token, Dirs, GithubSync, JiraGitlabSync, Sessions, SessionsConfig,
    TrackerSyncConfig,
};
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tokio::sync::broadcast;

const SPOOL_EVERY: Duration = Duration::from_secs(30);
const BACKUP_EVERY: Duration = Duration::from_secs(5 * 60);
/// Timeout de cada envio ao hub.
const HUB_TIMEOUT: Duration = Duration::from_secs(30);

/// Sem subcomandos: só `--help` e `--version`; qualquer outro argumento é
/// recusado antes de abrir porta ou ledger (issue #247).
#[derive(Parser)]
#[command(
    name = "ctowerd",
    version = version(),
    about = "Daemon do Control Tower (sessões, ledger e servidor HTTP do ambiente)"
)]
struct Cli {}

/// Versão com o commit do build, como o `Build` do `/healthz`.
fn version() -> &'static str {
    static V: OnceLock<String> = OnceLock::new();
    V.get_or_init(|| match ct_core::BUILD_COMMIT {
        "" => format!("{} (commit desconhecido)", ct_core::VERSION),
        c => format!("{} ({c})", ct_core::VERSION),
    })
}

#[tokio::main]
async fn main() {
    Cli::parse();
    // O `[logs]` vem do config.toml; erro de leitura aparece no `run`.
    let logs = Dirs::from_env()
        .ok()
        .and_then(|d| DaemonConfig::load(&d).ok())
        .map(|c| c.logs)
        .unwrap_or_default();
    ct_http::logs::init("ctowerd", &logs, "CTOWER_DATA_DIR");
    if let Err(e) = run().await {
        tracing::error!("ctowerd encerrado com erro: {e}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let dirs = Dirs::from_env()?;
    dirs.ensure()?;
    let cfg = DaemonConfig::load(&dirs)?;
    if cfg.legacy_hub {
        return Err(HUB_REMOVED.into());
    }
    let ui_token = ensure_token(&dirs.config.join("ui.token"))?;
    let hook_token = ensure_token(&dirs.config.join("hook.token"))?;
    let env_id = ensure_env_id(&dirs.env_id_file())?;
    let env_name = cfg.env_name();
    tracing::info!("ambiente {env_name} ({env_id})");
    let ledger = Ledger::open(&dirs.ledger_db(), &env_id)?;

    let (events, _) = broadcast::channel(1024);
    let sessions = Sessions::new(
        SessionsConfig {
            dirs: dirs.clone(),
            port: cfg.port,
            hook_token: hook_token.clone(),
            claude_bin: cfg.claude_bin.clone(),
            ctower_bin: cfg.ctower_bin.clone(),
            worktrees_root: cfg.worktrees_root(),
            gestor_dir: cfg.gestor_dir(&dirs),
            scrollback: cfg.scrollback_bytes,
            claude_json: cfg
                .pre_trust_worktrees
                .then(ct_sessions::trust::claude_json_path)
                .flatten(),
            claude_projects: ct_sessions::trust::claude_projects_dir(),
            secrets_max_age: cfg.secrets_max_age(),
            process_env: None,
            shell: None,
            docs_root: cfg.docs_root(),
        },
        ledger,
        notifier(events.clone()),
    );

    let interrupted = {
        let s = sessions.clone();
        tokio::task::spawn_blocking(move || s.boot()).await??
    };
    if !interrupted.is_empty() {
        tracing::warn!(
            "{} sessão(ões) interrompida(s) aguardando recuperação: {interrupted:?}",
            interrupted.len()
        );
    }

    spawn_periodic(sessions.clone(), SPOOL_EVERY, "spool", |s| {
        s.drain_spool().map(|n| n as u64)
    });
    spawn_periodic(sessions.clone(), BACKUP_EVERY, "backup", |s| {
        s.backup_running().map(|n| n as u64)
    });

    let github = Arc::new(GithubSync::new(
        sessions.clone(),
        GhClient::new(
            GhConfig::new(&cfg.gh_api_url, &cfg.gh_bin)
                .with_token_file(dirs.config.join("github.token")),
        ),
    ));

    // Projetos Jira + GitLab (#349, #350): o jira-cli e o glab do ambiente,
    // com os comandos de credencial do cadastro; o laço do GitHub pula esses
    // projetos.
    let tracker_cfg = TrackerSyncConfig::new(&cfg.jira_bin)
        .with_glab_bin(&cfg.glab_bin)
        .with_jira_deadlines(
            Duration::from_secs(cfg.jira_view_timeout_secs.max(1)),
            Duration::from_secs(cfg.jira_list_timeout_secs.max(1)),
        );
    let gitlab = Arc::new(tracker_cfg.gitlab());
    let tracker = Arc::new(JiraGitlabSync::new(sessions.clone(), tracker_cfg));

    // Sem hub o ambiente sobe, mas sem projetos e sem iniciar sessões;
    // hooks, MCP e `session.resume` continuam (spec do hub §7).
    let target = match cfg.hub_url.trim() {
        "" => {
            tracing::warn!(
                "{}: sem projetos; sessões existentes continuam (retomada, hooks e MCP)",
                ct_server::hub_sync::NOT_LINKED
            );
            None
        }
        url => match std::fs::read_to_string(dirs.hub_token_file()) {
            Ok(t) if !t.trim().is_empty() => Some(HubTarget {
                url: url.to_string(),
                token: t.trim().to_string(),
            }),
            _ => {
                tracing::warn!(
                    "hub_url configurado, mas {} não existe: ambiente não ligado a um hub, sem \
                     projetos (gere o token com `cthubd add-env` no hub e grave com \
                     `ctower hub set-token`)",
                    dirs.hub_token_file().display()
                );
                None
            }
        },
    };
    let config = ConfigOptions {
        repos_root: cfg.repos_root(),
        pull_every: Duration::from_secs(cfg.config_pull_interval_secs.max(1)),
        github: Some(Arc::clone(&github)),
    };
    if cfg.retention_days == 0 {
        tracing::info!("poda dos work items inativos desligada (retention_days = 0)");
    }
    let hub_sender = target.map(|t| {
        Arc::new(
            HubSender::new(sessions.clone(), t, HUB_TIMEOUT)
                .with_config(config)
                .with_retention_days(cfg.retention_days),
        )
    });
    if let Some(s) = &hub_sender {
        // Pull antes do envio: o relato da aplicação já vai no primeiro lote.
        if cfg.config_pull_interval_secs > 0 {
            spawn_puller(Arc::clone(s));
        } else {
            tracing::info!(
                "pull periódico da configuração do hub desligado (config_pull_interval_secs = 0)"
            );
        }
        spawn_sender(Arc::clone(s));
    }

    let updates = Arc::new(Updates::new(
        ct_sessions::update::source(&cfg, &dirs),
        Build::current(),
        events.clone(),
    ));
    // `update_ref` e `gh_sync_interval_secs` editáveis pela UI sem restart.
    let env_config = Arc::new(EnvConfig::new(
        dirs.config.join("config.toml"),
        cfg.gh_sync_interval_secs,
        Some(Arc::clone(&updates)),
    ));
    if cfg.gh_sync_interval_secs == 0 {
        tracing::info!("sync periódico com o GitHub desligado (gh_sync_interval_secs = 0)");
    }
    spawn_github_sync(Arc::clone(&github), env_config.gh_sync());
    if cfg.jira_sync_interval_secs > 0 {
        spawn_tracker_sync(
            Arc::clone(&tracker),
            Duration::from_secs(cfg.jira_sync_interval_secs),
        );
    } else {
        tracing::info!("sync periódico do Jira desligado (jira_sync_interval_secs = 0)");
    }
    if cfg.update_check_interval_hours > 0 {
        spawn_update_check(
            Arc::clone(&updates),
            Duration::from_secs(cfg.update_check_interval_hours * 3600),
        );
    } else {
        tracing::info!("verificação periódica de atualização desligada");
    }

    // Limpeza após a conclusão do work item (issue #287): o job só monta o
    // plano, salvo `cleanup.auto`; o acionamento manual vale sempre.
    let cleanup = Arc::new(
        Cleanup::new(
            sessions.clone(),
            Cleaner::standard(cfg.cleanup.clone())
                .with_github(Some(Arc::clone(&github)))
                .with_gitlab(Some(gitlab)),
            events.clone(),
        )
        .with_hub(hub_sender.clone()),
    );
    if cfg.cleanup.check_interval_hours > 0 {
        spawn_cleanup_check(
            Arc::clone(&cleanup),
            Duration::from_secs(cfg.cleanup.check_interval_hours.saturating_mul(3600)),
        );
    } else {
        tracing::info!(
            "verificação periódica de limpezas desligada (cleanup.check_interval_hours = 0)"
        );
    }

    let state = AppState {
        sessions,
        ui_token: ui_token.into(),
        hook_token: hook_token.into(),
        events,
        github: Some(github),
        tracker: Some(tracker),
        env_name: env_name.into(),
        hub_sender,
        updates: Some(updates),
        cleanup: Some(cleanup),
        env_config: Some(env_config),
        permissions: Default::default(),
    };
    // Canal reverso com o hub (#198): o hub chama métodos da lista e recebe
    // os pushes em tempo real; sem hub ligado, não abre.
    ct_server::hub_channel::spawn(state.clone(), Default::default());
    let app = router(state);
    // Hooks, MCP e `ctower` falam com 127.0.0.1: o loopback fica sempre no
    // ar; `bind` explícito (ex.: IP Tailscale) escuta além dele.
    ct_http::serve(app, &cfg.bind, cfg.port, "ctowerd").await?;
    tracing::info!("ctowerd encerrado");
    Ok(())
}

/// Roda `f` a cada `every` (fora do runtime), registrando falhas sem parar.
fn spawn_periodic(
    sessions: Sessions,
    every: Duration,
    name: &'static str,
    f: fn(&Sessions) -> ct_sessions::Result<u64>,
) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(every);
        tick.tick().await; // o primeiro tick é imediato; o boot já cuidou disso
        loop {
            tick.tick().await;
            let s = sessions.clone();
            match tokio::task::spawn_blocking(move || f(&s)).await {
                Ok(Ok(n)) if n > 0 => tracing::info!("{name}: {n} item(ns) processado(s)"),
                Ok(Ok(_)) => {}
                Ok(Err(e)) => tracing::warn!("{name} falhou: {e}"),
                Err(e) => tracing::warn!("{name} falhou: {e}"),
            }
        }
    });
}

/// Sync com o GitHub logo após o boot e depois a cada `interval` segundos,
/// com backoff exponencial enquanto algum projeto falhar (spec §7.5).
/// `interval` vem do `env.config.set`: `0` pausa o laço, e uma mudança
/// acorda a espera (#328).
fn spawn_github_sync(sync: Arc<GithubSync>, mut interval: tokio::sync::watch::Receiver<u64>) {
    tokio::spawn(async move {
        let mut failures = 0u32;
        loop {
            let secs = *interval.borrow_and_update();
            if secs == 0 {
                if interval.changed().await.is_err() {
                    return;
                }
                continue;
            }
            let s = Arc::clone(&sync);
            let ok = match tokio::task::spawn_blocking(move || s.sync_all()).await {
                Ok(Ok(r)) => {
                    let changes: usize = r.iter().map(|p| p.status_changes).sum();
                    if changes > 0 {
                        tracing::info!("github: {changes} work item(s) mudaram de status");
                    }
                    for p in r.iter().filter(|p| !p.ok) {
                        tracing::warn!(
                            "github: projeto {} falhou: {}",
                            p.project_id,
                            p.error.as_deref().unwrap_or("")
                        );
                    }
                    r.iter().all(|p| p.ok)
                }
                Ok(Err(e)) => {
                    tracing::warn!("github: sync falhou: {e}");
                    false
                }
                Err(e) => {
                    tracing::warn!("github: sync falhou: {e}");
                    false
                }
            };
            failures = if ok { 0 } else { failures.saturating_add(1) };
            let delay = next_delay(Duration::from_secs(secs), failures);
            tokio::select! {
                _ = tokio::time::sleep(delay) => {}
                r = interval.changed() => if r.is_err() { return },
            }
        }
    });
}

/// Sync dos projetos Jira logo após o boot e depois a cada `interval`, com
/// o mesmo backoff do GitHub enquanto algum projeto falhar (comando de
/// credencial travado, Jira fora do ar): nunca em laço apertado (spec do
/// Jira e GitLab §6.5).
fn spawn_tracker_sync(sync: Arc<JiraGitlabSync>, interval: Duration) {
    tokio::spawn(async move {
        let mut failures = 0u32;
        loop {
            let s = Arc::clone(&sync);
            let ok = match tokio::task::spawn_blocking(move || s.sync_all()).await {
                Ok(Ok(r)) => {
                    let changes: usize = r.iter().map(|p| p.status_changes).sum();
                    if changes > 0 {
                        tracing::info!("jira: {changes} work item(s) mudaram de status");
                    }
                    for p in r.iter().filter(|p| !p.ok) {
                        tracing::warn!(
                            "jira: projeto {} falhou: {}",
                            p.project_id,
                            p.error.as_deref().unwrap_or("")
                        );
                    }
                    r.iter().all(|p| p.ok)
                }
                Ok(Err(e)) => {
                    tracing::warn!("jira: sync falhou: {e}");
                    false
                }
                Err(e) => {
                    tracing::warn!("jira: sync falhou: {e}");
                    false
                }
            };
            failures = if ok { 0 } else { failures.saturating_add(1) };
            tokio::time::sleep(next_delay(interval, failures)).await;
        }
    });
}
