//! CLI auxiliar do Control Tower: ponto de entrada dos hooks, diagnóstico e
//! manutenção do ambiente.

mod clean;
mod hook;
mod hub;
mod secrets;
mod status;
mod statusline;
mod update;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "ctower", version, about = "CLI auxiliar do Control Tower")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Ponto de entrada dos hooks do Claude Code (lê o JSON do hook no stdin).
    Hook {
        #[arg(value_enum)]
        event: hook::HookEvent,
    },
    /// Mostra se o ctowerd está no ar e as sessões vivas.
    Status,
    /// Reinicia o serviço ctowerd (recusa com sessões vivas, salvo --force).
    Restart {
        #[arg(long)]
        force: bool,
    },
    /// Atualiza ctowerd e ctower a partir do ref configurado (`update_ref`).
    Update {
        /// Só mostra a versão instalada e a disponível.
        #[arg(long)]
        check: bool,
        /// Volta para a versão anterior guardada pela última atualização.
        #[arg(long)]
        rollback: bool,
        /// Segue mesmo com sessões vivas (o restart as encerra).
        #[arg(long)]
        force: bool,
        /// (interno) Executa o atualizador aqui mesmo, sem unidade do systemd.
        #[arg(long, hide = true)]
        run: bool,
    },
    /// Envio ao hub: token de sync, resync e envio agora. Os tokens dos
    /// ambientes são gerados no hub (`cthubd add-env`).
    Hub {
        #[command(subcommand)]
        cmd: HubCmd,
    },
    /// Limpa o que os work items concluídos deixaram (sessões, worktree,
    /// target/, branches), via ctowerd. Mostra o plano em tabela e o espaço
    /// liberado; só executa as etapas pendentes, nunca as bloqueadas.
    Clean {
        /// Só mostra o plano (pendentes e bloqueados com o motivo); não apaga nada.
        #[arg(long)]
        dry_run: bool,
        /// Só o work item da issue <n>.
        #[arg(long, value_name = "n")]
        item: Option<i64>,
        /// Projeto do --item quando o número existe em mais de um (owner/repo).
        #[arg(long, value_name = "owner/repo")]
        repo: Option<String>,
    },
    /// Statusline do Claude Code: grava o uso da sessão (lê o JSON no stdin).
    /// `ctower statusline install` o registra no ambiente.
    Statusline {
        /// Versão do registro (vem no comando instalado).
        #[arg(long = "v", hide = true)]
        v: Option<u32>,
        #[command(subcommand)]
        cmd: Option<StatuslineCmd>,
    },
    /// Secrets e variáveis entregues pelo hub a este ambiente.
    Secrets {
        #[command(subcommand)]
        cmd: SecretsCmd,
    },
}

#[derive(Subcommand)]
enum StatuslineCmd {
    /// Registra o statusline no settings.json do Claude Code do ambiente
    /// (idempotente; um statusLine de terceiros passa a rodar dentro dele).
    Install {
        /// Pasta de configuração do Claude Code (padrão: CLAUDE_CONFIG_DIR
        /// ou ~/.claude).
        #[arg(long)]
        claude_dir: Option<std::path::PathBuf>,
        /// Caminho do ctower no comando registrado (padrão: este binário).
        #[arg(long)]
        ctower_bin: Option<String>,
    },
}

#[derive(Subcommand)]
enum SecretsCmd {
    /// (no ambiente) Troca a chave do ambiente (env.key), recifra o cache e
    /// manda a chave pública nova ao hub, que reentrega os secrets.
    RotateEnvKey,
}

#[derive(Subcommand)]
enum HubCmd {
    /// (no ambiente) Grava o token de sync lido do stdin em hub.token (0600).
    SetToken,
    /// (no ambiente) Grava o token de agente lido do stdin em hub-agent.token
    /// (0600), para as ferramentas portfolio_* do gestor.
    SetAgentToken,
    /// (no ambiente) Reenfileira o ledger local (só o que não foi podado) para
    /// reenviar ao hub.
    Resync,
    /// (no ambiente) Envia agora ao hub o que está no outbox, via ctowerd.
    Sync,
}

fn main() {
    // Hooks nunca bloqueiam o agente: erro de argumento sai com 0 (só
    // `--help`/`--version` mantêm o comportamento padrão do clap).
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(e) => {
            use clap::error::ErrorKind;
            if matches!(e.kind(), ErrorKind::DisplayHelp | ErrorKind::DisplayVersion) {
                e.exit();
            }
            let _ = e.print();
            std::process::exit(0);
        }
    };
    let code = match cli.cmd {
        // Panic também não pode bloquear: sai com 0.
        Cmd::Hook { event } => std::panic::catch_unwind(|| hook::run(event)).unwrap_or(0),
        Cmd::Status => status::status(),
        Cmd::Restart { force } => status::restart(force),
        Cmd::Update {
            check,
            rollback,
            force,
            run,
        } => {
            if check {
                update::check()
            } else if run {
                update::run(rollback)
            } else {
                update::update(force, rollback)
            }
        }
        Cmd::Hub { cmd } => match cmd {
            HubCmd::SetToken => hub::set_token(),
            HubCmd::SetAgentToken => hub::set_agent_token(),
            HubCmd::Resync => hub::resync(),
            HubCmd::Sync => hub::sync(),
        },
        Cmd::Clean {
            dry_run,
            item,
            repo,
        } => clean::clean(dry_run, item, repo),
        Cmd::Statusline { v: _, cmd } => match cmd {
            None => std::panic::catch_unwind(statusline::run).unwrap_or(0),
            Some(StatuslineCmd::Install {
                claude_dir,
                ctower_bin,
            }) => statusline::install(claude_dir, ctower_bin),
        },
        Cmd::Secrets { cmd } => match cmd {
            SecretsCmd::RotateEnvKey => secrets::rotate_env_key(),
        },
    };
    std::process::exit(code);
}
