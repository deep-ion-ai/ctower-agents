//! `cthubd`: o hub do Control Tower. Roda como serviço systemd de usuário
//! (`cthubd serve`); os demais subcomandos administram o `hub.db`.

use clap::{Parser, Subcommand};
use cthubd::config::{Dirs, HubConfig};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "cthubd",
    version,
    about = "Hub do Control Tower (plano de controle)"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Sobe o hub (padrão sem subcomando).
    Serve,
    /// Gera (ou troca) o token de sync de um ambiente e o imprime.
    AddEnv {
        name: String,
        /// Não troca o token de um ambiente que já existe: sai com 3.
        #[arg(long)]
        no_replace: bool,
        /// Prende o token a este `env_id` (o de um ambiente cujo ledger já
        /// existe). Sem ele, um nome novo ganha um `env_id` gerado aqui e um
        /// nome existente mantém o seu.
        #[arg(long)]
        env_id: Option<String>,
    },
    /// Revoga o token de sync de um ambiente (e o de agente dele).
    RevokeEnv { name: String },
    /// Gera (ou troca) o token de agente de um ambiente e o imprime: só lê
    /// /agent/v1 e registra propostas. O anterior deixa de valer na hora.
    AgentToken { name: String },
    /// Revoga só o token de agente de um ambiente; o sync continua.
    RevokeAgentToken { name: String },
    /// Sessões web da PWA (cookie `ct_web`).
    Web {
        #[command(subcommand)]
        cmd: WebCmd,
    },
    /// Restaura o hub.db a partir de um snapshot (com o serviço parado).
    /// Todas as sessões web saem revogadas. Com --to, extrai um pacote dos
    /// docs (docs-*.tar.zst[.age]) numa pasta nova ou vazia, sem tocar no
    /// docs_dir nem no hub.db (roda com o serviço no ar).
    Restore {
        /// Arquivo do snapshot ou do pacote dos docs; com --remote, o nome
        /// no destino remoto ou `latest`. Sem nome, --remote lista os
        /// snapshots e os pacotes dos docs do destino.
        snapshot: Option<PathBuf>,
        /// Pasta (nova ou vazia, fora do docs_dir) onde extrair o pacote
        /// dos docs.
        #[arg(long)]
        to: Option<PathBuf>,
        /// Busca o snapshot no destino remoto configurado (baixa e decifra).
        #[arg(long)]
        remote: bool,
        /// Chave de recuperação (AGE-SECRET-KEY-1...) se a do hub não abrir
        /// o backup remoto cifrado.
        #[arg(long)]
        identity: Option<PathBuf>,
    },
    /// Migra o hub antigo (ctowerd com `hub = true`) para este cthubd, com
    /// o ctowerd e o cthubd parados. O hub.db antigo fica intocado.
    Migrate {
        /// Diretório de dados do ctowerd que era o hub (ex.: ~/.local/share/ctower).
        #[arg(long)]
        from: PathBuf,
        /// Diretório de config do ctowerd (padrão: CTOWER_CONFIG_DIR ou ~/.config/ctower).
        #[arg(long)]
        ctower_config: Option<PathBuf>,
        /// Nome do ctowerd desta máquina no hub (padrão: env_name do config
        /// dele ou o nome da máquina).
        #[arg(long)]
        env_name: Option<String>,
        /// Substitui o hub.db que o cthubd já tem (guardado como
        /// hub.db.pre-migrate-*).
        #[arg(long)]
        force: bool,
    },
    /// Store de secrets: restaurar de um snapshot e rotacionar as chaves
    /// (com o serviço parado).
    Secrets {
        #[command(subcommand)]
        cmd: SecretsCmd,
    },
    /// Atualiza o cthubd a partir do ref configurado (`update_ref`): compila,
    /// guarda o binário e um snapshot do hub.db, reinicia o serviço e volta
    /// à versão anterior se ele não ficar saudável.
    Update {
        /// Só mostra a versão instalada e a disponível.
        #[arg(long)]
        check: bool,
        /// Volta para a versão anterior guardada pela última atualização.
        #[arg(long)]
        rollback: bool,
        /// No rollback, aceita voltar o hub.db do snapshot (o que entrou
        /// depois dele fica só na cópia hub.db.pre-rollback-*).
        #[arg(long, requires = "rollback")]
        force: bool,
    },
}

#[derive(Subcommand)]
enum WebCmd {
    /// Revoga todas as sessões web, direto no hub.db (com o hub no ar ou
    /// parado). Trocar o token de UI não derruba as sessões: use isto.
    RevokeAll,
}

#[derive(Subcommand)]
enum SecretsCmd {
    /// Lista escopo, nome e tipo das entradas (nunca valores). Com --env,
    /// só as entregues a esse ambiente: as que estavam no cache dele.
    List {
        /// Nome do ambiente (o de `add-env`). Rode antes do `revoke-env`.
        #[arg(long)]
        env: Option<String>,
    },
    /// Restaura o store de secrets de um snapshot (secrets-*.age). Abre com a
    /// chave do hub desta máquina ou, sem ela, com a chave de recuperação
    /// (--identity ou colada no stdin, sem eco). Lista nomes, escopos e
    /// tipos (nunca valores) e pede confirmação.
    Restore {
        snapshot: PathBuf,
        /// Arquivo com a chave de recuperação (AGE-SECRET-KEY-1...).
        #[arg(long)]
        identity: Option<PathBuf>,
        /// Não pede confirmação.
        #[arg(long)]
        yes: bool,
    },
    /// Gera chave nova do hub e/ou de recuperação (sem flags: as duas),
    /// recifra o store e os snapshots e remove os que ficariam presos à
    /// chave antiga.
    RotateKeys {
        /// Só a chave do hub (secrets.key).
        #[arg(long)]
        hub: bool,
        /// Só a chave de recuperação (fica pendente até a verificação na UI).
        #[arg(long)]
        recovery: bool,
    },
}

fn main() {
    let cli = Cli::parse();
    let dirs = match Dirs::from_env() {
        Ok(d) => d,
        Err(e) => fail(e.to_string()),
    };
    match cli.cmd.unwrap_or(Cmd::Serve) {
        Cmd::Serve => serve(dirs),
        Cmd::AddEnv {
            name,
            no_replace,
            env_id,
        } => match no_replace
            .then(|| cthubd::cli::env_exists(&dirs, &name))
            .transpose()
        {
            Err(e) => fail(e),
            Ok(Some(true)) => {
                eprintln!(
                    "erro: o ambiente \"{}\" já tem token de sync; não troquei (sem --no-replace, \
                     add-env gera outro e o antigo para de valer)",
                    name.trim()
                );
                std::process::exit(3);
            }
            Ok(_) => match match &env_id {
                Some(id) => cthubd::cli::add_env_with_id(&dirs, &name, id),
                None => cthubd::cli::add_env(&dirs, &name),
            } {
                Ok(i) => {
                    // Duas linhas: o token e o env_id preso a ele; o
                    // `ctower hub set-token` grava os dois.
                    println!("{}\n{}", i.token, i.env_id);
                    eprintln!(
                    "token de sync de \"{}\" gerado (o hub guarda só o hash), preso ao env_id {}. \
                     No ambiente, grave as duas linhas (token e env_id):\n  \
                     printf '%s\\n%s\\n' <token> <env_id> | ctower hub set-token\n\
                     e configure `hub_url` no config.toml. Um ambiente que já tem ledger \
                     (~/.config/ctower/env_id) usa `cthubd add-env --env-id <env_id> <nome>`.",
                    name.trim(),
                    i.env_id
                );
                }
                Err(e) => fail(e),
            },
        },
        Cmd::RevokeEnv { name } => match cthubd::cli::revoke_env(&dirs, &name) {
            Ok(()) => println!("token de \"{}\" revogado", name.trim()),
            Err(e) => fail(e),
        },
        Cmd::AgentToken { name } => match cthubd::cli::agent_token(&dirs, &name) {
            Ok(token) => {
                println!("{token}");
                eprintln!(
                    "token de agente de \"{}\" gerado (o hub guarda só o hash; o anterior não \
                     vale mais). No ambiente, rode:\n  echo <token> | ctower hub set-agent-token",
                    name.trim()
                );
            }
            Err(e) => fail(e),
        },
        Cmd::RevokeAgentToken { name } => match cthubd::cli::revoke_agent_token(&dirs, &name) {
            Ok(()) => println!("token de agente de \"{}\" revogado", name.trim()),
            Err(e) => fail(e),
        },
        Cmd::Update {
            check,
            rollback,
            force,
        } => std::process::exit(if check {
            cthubd::update::check(&dirs)
        } else {
            cthubd::update::run(&dirs, rollback, force)
        }),
        Cmd::Migrate {
            from,
            ctower_config,
            env_name,
            force,
        } => {
            let port = match HubConfig::load(&dirs) {
                Ok(c) => c.port,
                Err(e) => fail(e.to_string()),
            };
            let mut o = match cthubd::migrate::MigrateOpts::new(from, port) {
                Ok(o) => o,
                Err(e) => fail(e.to_string()),
            };
            if let Some(c) = ctower_config {
                o.ctower_config = c;
            }
            o.env_name = env_name;
            o.force = force;
            match cthubd::migrate::migrate(&dirs, &o) {
                Ok(rep) => print!("{}", cthubd::migrate::render(&rep, &dirs, &o)),
                Err(e) => fail(e),
            }
        }
        Cmd::Secrets { cmd } => secrets(&dirs, cmd),
        Cmd::Web {
            cmd: WebCmd::RevokeAll,
        } => match cthubd::cli::web_revoke_all(&dirs) {
            Ok(n) => println!("{n} sessão(ões) web ativa(s) revogada(s)"),
            Err(e) => fail(e),
        },
        Cmd::Restore {
            snapshot,
            to: Some(to),
            remote,
            identity,
        } => {
            // Pacote dos docs (#442): extrai em outra pasta, com o hub no ar.
            let cfg = match HubConfig::load(&dirs) {
                Ok(c) => c,
                Err(e) => fail(e.to_string()),
            };
            let Some(snapshot) = snapshot else {
                fail(
                    "informe o pacote dos docs (docs-*.tar.zst ou, com --remote, o nome ou latest)"
                        .into(),
                )
            };
            let name = snapshot.to_string_lossy();
            let src = if remote {
                cthubd::cli::DocsSource::Remote(&name)
            } else {
                cthubd::cli::DocsSource::Local(&snapshot)
            };
            let docs_dir = cfg.docs_dir();
            match cthubd::cli::docs_restore(
                &dirs,
                docs_dir.as_deref(),
                src,
                &to,
                identity.as_deref(),
            ) {
                Ok((pkg, n)) => {
                    println!("{n} arquivo(s) de {pkg} extraído(s) em {}", to.display());
                    println!("o docs_dir não foi alterado: copie de lá só o que precisar");
                }
                Err(e) => fail(e),
            }
        }
        Cmd::Restore {
            snapshot,
            to: None,
            remote,
            identity,
        } => {
            let port = match HubConfig::load(&dirs) {
                Ok(c) => c.port,
                Err(e) => fail(e.to_string()),
            };
            if snapshot
                .as_deref()
                .and_then(|p| p.file_name())
                .is_some_and(|n| n.to_string_lossy().starts_with("docs-"))
            {
                fail("pacote dos docs: informe a pasta de destino com --to <pasta>".into());
            }
            let snapshot = match (remote, snapshot) {
                (false, Some(p)) => p,
                (false, None) => fail("informe o snapshot (ou use --remote)".into()),
                (true, None) => {
                    match cthubd::cli::remote_list(&dirs) {
                        Ok(v) if v.is_empty() => println!("o destino remoto não tem snapshots"),
                        Ok(v) => v
                            .iter()
                            .for_each(|o| println!("{}\t{} bytes", o.name, o.size)),
                        Err(e) => fail(e),
                    }
                    return;
                }
                (true, Some(name)) => {
                    // O serviço precisa estar parado antes de baixar.
                    if let Err(e) = cthubd::cli::ensure_port_free(port) {
                        fail(e);
                    }
                    match cthubd::cli::remote_download(
                        &dirs,
                        &name.to_string_lossy(),
                        identity.as_deref(),
                    ) {
                        Ok(p) if name.to_string_lossy().starts_with("secrets-") => {
                            // Cópia do store de secrets: não é um hub.db.
                            println!(
                                "baixado em {}; restaure com: cthubd secrets restore {}",
                                p.display(),
                                p.display()
                            );
                            return;
                        }
                        Ok(p) => p,
                        Err(e) => fail(e),
                    }
                }
            };
            match cthubd::cli::restore(&dirs, port, &snapshot) {
                Ok(prev) => {
                    println!(
                        "hub.db restaurado de {} em {}",
                        snapshot.display(),
                        dirs.hub_db().display()
                    );
                    if let Some(p) = prev {
                        println!("o hub.db anterior ficou em {}", p.display());
                    }
                    println!("suba o serviço (systemctl --user start cthubd)");
                }
                Err(e) => fail(e),
            }
        }
    }
}

fn secrets(dirs: &Dirs, cmd: SecretsCmd) {
    let cfg = match HubConfig::load(dirs) {
        Ok(c) => c,
        Err(e) => fail(e.to_string()),
    };
    let snapshot_dir = cfg.secrets_snapshot_dir(dirs);
    match cmd {
        SecretsCmd::List { env } => match cthubd::cli::secrets_list(dirs, env.as_deref()) {
            Ok(s) => print!("{s}"),
            Err(e) => fail(e),
        },
        SecretsCmd::Restore {
            snapshot,
            identity,
            yes,
        } => {
            let mut input = std::io::stdin().lock();
            let mut out = std::io::stdout();
            match cthubd::cli::secrets_restore(
                dirs,
                cfg.port,
                &snapshot_dir,
                &cthubd::cli::SecretsRestoreArgs {
                    file: &snapshot,
                    identity: identity.as_deref(),
                    yes,
                },
                &mut input,
                &mut out,
            ) {
                Ok(cthubd::cli::SecretsRestored::Done(r)) => {
                    print!("{}", cthubd::cli::render_secrets_restore(dirs, &r))
                }
                Ok(cthubd::cli::SecretsRestored::Cancelled) => std::process::exit(2),
                Err(e) => fail(e),
            }
        }
        SecretsCmd::RotateKeys { hub, recovery } => {
            let both = !hub && !recovery;
            let opts = cthubd::secrets::rotate::RotateOpts {
                hub: hub || both,
                recovery: recovery || both,
            };
            match cthubd::cli::secrets_rotate_keys(dirs, cfg.port, &snapshot_dir, opts) {
                Ok(r) => print!("{}", cthubd::cli::render_rotate_keys(dirs, &r)),
                Err(e) => fail(e),
            }
        }
    }
}

fn serve(dirs: Dirs) {
    // O `[logs]` vem do config.toml; erro de leitura aparece no `serve`.
    let logs = HubConfig::load(&dirs).map(|c| c.logs).unwrap_or_default();
    ct_http::logs::init("cthubd", &logs, "CTHUB_DATA_DIR");
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("runtime do tokio");
    if let Err(e) = rt.block_on(cthubd::cli::serve(dirs)) {
        tracing::error!("cthubd encerrado com erro: {e}");
        std::process::exit(1);
    }
}

fn fail(msg: String) -> ! {
    eprintln!("erro: {msg}");
    std::process::exit(1);
}
