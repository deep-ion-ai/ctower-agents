//! Subcomandos do `cthubd` (spec do hub como plano de controle §10), sem um
//! CLI separado: `serve` (padrão do serviço), `add-env`, `revoke-env`,
//! `agent-token`, `revoke-agent-token`, `web revoke-all` (#200), `restore` e `secrets restore|rotate-keys`
//! (#170; [`secrets_restore`] e [`secrets_rotate_keys`]) (`migrate` em
//! [`crate::migrate`], `update` em
//! [`crate::update`]). Os de token acessam o `hub.db` direto (SQLite com
//! WAL), então funcionam com o serviço no ar; os de restauração e rotação
//! exigem o serviço parado.

use crate::backup::{self, Backups};
use crate::config::{Dirs, HubConfig};
use crate::github::{self, HubGithub};
use crate::HubState;
use ct_gh::{GhClient, GhConfig};
use ct_ledger::{Hub, IssuedEnvToken, SharedHub};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

fn open_hub(dirs: &Dirs) -> Result<Hub, String> {
    dirs.ensure().map_err(|e| e.to_string())?;
    Hub::open(&dirs.hub_db()).map_err(|e| e.to_string())
}

/// Gera (ou troca) o token de sync do ambiente `name`, já preso a um
/// `env_id` (#195): o do ambiente, se o nome já existe; um novo, se não.
/// Devolve o token em claro (o hub guarda só o hash) e o `env_id`.
pub fn add_env(dirs: &Dirs, name: &str) -> Result<IssuedEnvToken, String> {
    open_hub(dirs)?
        .issue_env_token(name)
        .map_err(|e| e.to_string())
}

/// Como [`add_env`], mas prende o token ao `env_id` que o ambiente já usa
/// (`--env-id`): para um ambiente cujo ledger já existe. Recusa um `env_id`
/// que não seja UUID e um nome já preso a outro.
pub fn add_env_with_id(dirs: &Dirs, name: &str, env_id: &str) -> Result<IssuedEnvToken, String> {
    let env_id = uuid::Uuid::parse_str(env_id.trim())
        .map_err(|_| format!("env_id inválido: {env_id}"))?
        .to_string();
    open_hub(dirs)?
        .issue_env_token_with_env_id(name, &env_id)
        .map_err(|e| e.to_string())
}

/// Já há token de sync para o ambiente `name`?
pub fn env_exists(dirs: &Dirs, name: &str) -> Result<bool, String> {
    let names = open_hub(dirs)?
        .env_token_names()
        .map_err(|e| e.to_string())?;
    Ok(names.iter().any(|n| n == name.trim()))
}

/// Revoga o token de sync do ambiente `name` (corta o envio) e o de agente.
pub fn revoke_env(dirs: &Dirs, name: &str) -> Result<(), String> {
    if open_hub(dirs)?
        .revoke_env_token(name)
        .map_err(|e| e.to_string())?
    {
        Ok(())
    } else {
        Err(format!("nenhum token para \"{}\"", name.trim()))
    }
}

/// Gera (ou troca) o token de agente do ambiente `name` (spec do gestor
/// §6.1). Exige o token de sync do mesmo nome. Devolve o token em claro.
pub fn agent_token(dirs: &Dirs, name: &str) -> Result<String, String> {
    open_hub(dirs)?
        .add_agent_token(name)
        .map_err(|e| e.to_string())
}

/// Revoga só o token de agente do ambiente `name`; o sync continua.
pub fn revoke_agent_token(dirs: &Dirs, name: &str) -> Result<(), String> {
    if open_hub(dirs)?
        .revoke_agent_token(name)
        .map_err(|e| e.to_string())?
    {
        Ok(())
    } else {
        Err(format!("nenhum token de agente para \"{}\"", name.trim()))
    }
}

/// `cthubd web revoke-all`: revoga todas as sessões web direto no `hub.db`
/// (funciona com o hub parado ou no ar). Devolve quantas estavam ativas.
pub fn web_revoke_all(dirs: &Dirs) -> Result<usize, String> {
    open_hub(dirs)?
        .revoke_all_web_sessions()
        .map_err(|e| e.to_string())
}

/// Restaura o `hub.db` a partir de um snapshot, com o serviço parado. O
/// `hub.db` atual (com `-wal`/`-shm`) fica ao lado como
/// `hub.db.pre-restore-<AAAAMMDD-HHMM>`. Devolve esse caminho, se havia um.
/// Recusa se houver algo escutando em `127.0.0.1:port`.
pub fn restore(dirs: &Dirs, port: u16, snapshot: &Path) -> Result<Option<PathBuf>, String> {
    ensure_port_free(port)?;
    restore_files(dirs, snapshot)
}

/// Destino remoto e credenciais para `restore --remote`: o `backup.json` e
/// o store de secrets do hub; na falta do store (máquina nova), as mesmas
/// variáveis `CTHUB_BACKUP_*` do ambiente.
fn remote_context(
    dirs: &Dirs,
) -> Result<(crate::remote::RemoteSettings, crate::remote::Credentials), String> {
    use crate::remote::{credential_fields, secret_name, Credentials};
    let file = dirs.backup_settings_file();
    let text = std::fs::read_to_string(&file)
        .map_err(|e| format!("{}: {e} (destino remoto não configurado)", file.display()))?;
    let u: crate::backup::BackupUpdate =
        serde_json::from_str(&text).map_err(|e| format!("{}: {e}", file.display()))?;
    let settings = u.remote.unwrap_or_default();
    if settings.kind == crate::remote::RemoteKind::None {
        return Err("o backup remoto não está configurado".into());
    }
    let store = ct_secrets::read_identity(&dirs.secrets_key_file())
        .and_then(|id| ct_secrets::SecretsStore::open(dirs.secrets_file(), id, &[]))
        .ok();
    let mut c = Credentials::default();
    for f in credential_fields(settings.kind) {
        let name = secret_name(settings.kind, f).unwrap_or_default();
        let v = store
            .as_ref()
            .and_then(|s| s.get(&ct_secrets::Scope::Global, name))
            .map(|e| e.value.expose().to_string())
            .or_else(|| std::env::var(name).ok())
            .unwrap_or_default();
        match *f {
            "access_key" => c.access_key = v,
            "secret_key" => c.secret_key = v,
            "client_id" => c.client_id = v,
            "client_secret" => c.client_secret = v,
            _ => c.refresh_token = v,
        }
    }
    Ok((settings, c))
}

/// `cthubd restore --remote` sem nome: os snapshots do `hub.db` e depois
/// os pacotes dos docs (#442) do destino, cada grupo do mais novo para o
/// mais velho.
pub fn remote_list(dirs: &Dirs) -> Result<Vec<crate::remote::RemoteObject>, String> {
    let (s, c) = remote_context(dirs)?;
    let objs = crate::remote::open(&s, &c)?.list()?;
    let mut out = crate::remote::hub_snapshots(objs.clone());
    out.extend(crate::remote::docs_packages(objs));
    Ok(out)
}

/// Decifra `data` com a chave do hub ou, se ela não abrir, com `identity`
/// (a chave de recuperação).
fn open_sealed(dirs: &Dirs, data: &[u8], identity: Option<&Path>) -> Result<Vec<u8>, String> {
    let ids: Vec<_> = [
        Some(dirs.secrets_key_file()),
        identity.map(Path::to_path_buf),
    ]
    .into_iter()
    .flatten()
    .filter_map(|p| ct_secrets::read_identity(&p).ok())
    .collect();
    ids.iter()
        .find_map(|id| ct_secrets::decrypt(data, id).ok())
        .map(|p| p.to_vec())
        .ok_or_else(|| {
            "não foi possível decifrar: a chave do hub não abre este backup \
             (passe a chave de recuperação com --identity)"
                .to_string()
        })
}

/// De onde vem o pacote dos docs no `restore --to`.
pub enum DocsSource<'a> {
    /// Nome no destino remoto (`docs-<...>.tar.zst.age`) ou `latest`.
    Remote(&'a str),
    /// Pacote local (`docs-<...>.tar.zst`, ou `.age` baixado à mão).
    Local(&'a Path),
}

/// `cthubd restore [--remote] docs-<...> --to <pasta>` (#442): baixa (ou
/// lê), decifra e extrai o pacote dos docs em `to`, uma pasta nova ou vazia
/// fora do `docs_dir`, que nunca é tocado. Não mexe no `hub.db`, então
/// roda com o serviço no ar. Devolve o nome do pacote e quantos arquivos
/// saíram dele.
pub fn docs_restore(
    dirs: &Dirs,
    docs_dir: Option<&Path>,
    src: DocsSource<'_>,
    to: &Path,
    identity: Option<&Path>,
) -> Result<(String, usize), String> {
    use crate::docs_backup;
    let (name, data) = match src {
        DocsSource::Remote(name) => {
            let (s, c) = remote_context(dirs)?;
            let store = crate::remote::open(&s, &c)?;
            let name = if name == "latest" {
                crate::remote::docs_packages(store.list()?)
                    .into_iter()
                    .next()
                    .ok_or("o destino remoto não tem pacotes dos docs")?
                    .name
            } else {
                name.to_string()
            };
            if !name.starts_with(docs_backup::PREFIX) || name.contains('/') || name.contains("..") {
                return Err(format!("nome inválido de pacote dos docs: {name}"));
            }
            // Antes de baixar: um destino recusado não custa o download.
            docs_backup::prepare_target(to, docs_dir).map(|created| {
                if created {
                    let _ = std::fs::remove_dir(to);
                }
            })?;
            let data = store.get(&name)?;
            (name, data)
        }
        DocsSource::Local(p) => {
            let data = std::fs::read(p).map_err(|e| format!("{}: {e}", p.display()))?;
            let name = p
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            (name, data)
        }
    };
    let data = if name.ends_with(".age") {
        open_sealed(dirs, &data, identity)?
    } else {
        data
    };
    let created = docs_backup::prepare_target(to, docs_dir)?;
    match docs_backup::extract(&data[..], to) {
        Ok(n) => Ok((name, n)),
        Err(e) => {
            // Nada pela metade: a pasta criada aqui sai; uma que já existia
            // (vazia) volta a ficar vazia.
            if created {
                let _ = std::fs::remove_dir_all(to);
            } else if let Ok(rd) = std::fs::read_dir(to) {
                for e in rd.flatten() {
                    let p = e.path();
                    let _ = if p.is_dir() && !p.is_symlink() {
                        std::fs::remove_dir_all(&p)
                    } else {
                        std::fs::remove_file(&p)
                    };
                }
            }
            Err(e)
        }
    }
}

/// Baixa o snapshot `name` (`latest` = o mais novo) do destino remoto para
/// `<data>/restore-remote/` e o decifra com a chave do hub, ou com
/// `identity` (a chave de recuperação) se a do hub não abrir. Devolve o
/// caminho do `.db`, pronto para [`restore`].
pub fn remote_download(
    dirs: &Dirs,
    name: &str,
    identity: Option<&Path>,
) -> Result<PathBuf, String> {
    let (s, c) = remote_context(dirs)?;
    let store = crate::remote::open(&s, &c)?;
    let name = if name == "latest" {
        crate::remote::hub_snapshots(store.list()?)
            .into_iter()
            .next()
            .ok_or("o destino remoto não tem snapshots")?
            .name
    } else {
        name.to_string()
    };
    if name.contains('/') || name.contains("..") {
        return Err(format!("nome inválido: {name}"));
    }
    let mut data = store.get(&name)?;
    let out_name = match name.strip_suffix(".age") {
        Some(base) if name.starts_with("hub-") => {
            data = open_sealed(dirs, &data, identity)?;
            base.to_string()
        }
        // `secrets-*.age` já é o formato do `secrets restore`; `.db` sem
        // cifra é usado como veio.
        _ => name.clone(),
    };
    let dir = dirs.data.join("restore-remote");
    ct_core::files::private_dir(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(out_name);
    ct_core::files::write_private_atomic(&path, &data).map_err(|e| e.to_string())?;
    Ok(path)
}

/// Recusa se houver um serviço escutando em `127.0.0.1:port`.
pub fn ensure_port_free(port: u16) -> Result<(), String> {
    if std::net::TcpStream::connect_timeout(
        &std::net::SocketAddr::from(([127, 0, 0, 1], port)),
        Duration::from_millis(500),
    )
    .is_ok()
    {
        return Err(format!(
            "há um serviço em 127.0.0.1:{port}: pare o cthubd antes de restaurar \
             (systemctl --user stop cthubd)"
        ));
    }
    Ok(())
}

/// A troca de arquivos do `restore`, depois da checagem da porta.
fn restore_files(dirs: &Dirs, snapshot: &Path) -> Result<Option<PathBuf>, String> {
    if !snapshot.is_file() {
        return Err(format!("snapshot não encontrado: {}", snapshot.display()));
    }
    dirs.ensure().map_err(|e| e.to_string())?;
    let db = dirs.hub_db();
    // Copia para o lado e valida (aplica as migrações) antes de trocar: o
    // snapshot em si nunca é alterado.
    let tmp = dirs.data.join("hub.db.restore");
    let _ = std::fs::remove_file(&tmp);
    ct_ledger::snapshot_db(snapshot, &tmp).map_err(|e| format!("snapshot ilegível: {e}"))?;
    // Época nova da configuração: o hub restaurado pode ter o mesmo
    // `config_rev` de antes com outro conteúdo, e os ambientes precisam
    // receber o conjunto completo no próximo pull (spec §4.1).
    // Sessões web do snapshot podem já ter sido revogadas depois dele: todas
    // saem revogadas (spec da PWA §9.1).
    if let Err(e) = Hub::open(&tmp).and_then(|h| {
        h.renew_config_epoch()?;
        h.revoke_all_web_sessions().map(|_| ())
    }) {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("snapshot não é um hub.db válido: {e}"));
    }
    for ext in ["-wal", "-shm"] {
        let _ = std::fs::remove_file(dirs.data.join(format!("hub.db.restore{ext}")));
    }
    let backup = if db.exists() {
        let prev = free_backup_name(
            &dirs.data,
            "hub.db.pre-restore",
            &ct_core::utc_stamp(ct_core::now_ms()),
        );
        for ext in ["", "-wal", "-shm"] {
            let from = PathBuf::from(format!("{}{ext}", db.display()));
            if from.exists() {
                std::fs::rename(&from, format!("{}{ext}", prev.display()))
                    .map_err(|e| e.to_string())?;
            }
        }
        Some(prev)
    } else {
        None
    };
    std::fs::rename(&tmp, &db).map_err(|e| e.to_string())?;
    Ok(backup)
}

/// Desliga o eco do terminal enquanto vive, se o stdin for um terminal
/// (a chave de recuperação colada no `secrets restore`).
struct NoEcho(Option<libc::termios>);

impl NoEcho {
    fn new() -> Self {
        // SAFETY: chamadas termios no fd 0 com uma struct válida, inicializada
        // pelo `tcgetattr` antes de ser lida.
        unsafe {
            if libc::isatty(0) != 1 {
                return NoEcho(None);
            }
            let mut t = std::mem::MaybeUninit::<libc::termios>::uninit();
            if libc::tcgetattr(0, t.as_mut_ptr()) != 0 {
                return NoEcho(None);
            }
            let old = t.assume_init();
            let mut quiet = old;
            quiet.c_lflag &= !libc::ECHO;
            quiet.c_lflag |= libc::ECHONL;
            if libc::tcsetattr(0, libc::TCSANOW, &quiet) != 0 {
                return NoEcho(None);
            }
            NoEcho(Some(old))
        }
    }
}

impl Drop for NoEcho {
    fn drop(&mut self) {
        if let Some(old) = &self.0 {
            // SAFETY: devolve ao fd 0 o estado lido em `new`.
            unsafe {
                libc::tcsetattr(0, libc::TCSANOW, old);
            }
        }
    }
}

/// Lê uma linha de `input` (sem eco se o stdin é um terminal; fora de um
/// terminal, como nos testes e num pipe, só lê).
fn read_secret_line(
    input: &mut dyn std::io::BufRead,
    prompt: &str,
) -> Result<ct_secrets::Zeroizing<String>, String> {
    eprint!("{prompt}");
    let _quiet = NoEcho::new();
    let mut line = ct_secrets::Zeroizing::new(String::new());
    input.read_line(&mut line).map_err(|e| e.to_string())?;
    if line.trim().is_empty() {
        return Err("nenhuma chave de recuperação informada".into());
    }
    Ok(line)
}

fn scope_text(s: &ct_secrets::Scope) -> &str {
    match s {
        ct_secrets::Scope::Global => "global",
        ct_secrets::Scope::Project(k) => k,
    }
}

fn kind_text(k: ct_secrets::Kind) -> &'static str {
    match k {
        ct_secrets::Kind::Secret => "secret",
        ct_secrets::Kind::Variable => "variável",
    }
}

/// Argumentos do `secrets restore`.
#[derive(Debug, Clone, Copy)]
pub struct SecretsRestoreArgs<'a> {
    /// O snapshot (`secrets-*.age`).
    pub file: &'a Path,
    /// Arquivo com a chave de recuperação; `None` = pede no stdin.
    pub identity: Option<&'a Path>,
    /// Pula a confirmação.
    pub yes: bool,
}

/// O que o `secrets restore` terminou fazendo.
#[derive(Debug)]
pub enum SecretsRestored {
    Done(crate::secrets::restore::RestoreReport),
    /// Sem confirmação: nada mudou.
    Cancelled,
}

/// `cthubd secrets restore <arquivo> [--identity <arquivo>] [--yes]` (#170),
/// com o serviço parado: abre o snapshot (chave do hub, senão a de
/// recuperação de `identity` ou lida de `input`), lista escopo, nome e tipo
/// em `out` (nunca valores), pede confirmação em `input` (pulada com `yes`)
/// e restaura. Veja [`crate::secrets::restore`].
pub fn secrets_restore(
    dirs: &Dirs,
    port: u16,
    snapshot_dir: &Path,
    args: &SecretsRestoreArgs<'_>,
    input: &mut dyn std::io::BufRead,
    out: &mut dyn std::io::Write,
) -> Result<SecretsRestored, String> {
    use crate::secrets::restore::{self, OpenedWith};
    let SecretsRestoreArgs {
        file,
        identity,
        yes,
    } = *args;
    ensure_port_free(port)?;
    if !file.is_file() {
        return Err(format!("snapshot não encontrado: {}", file.display()));
    }
    let w = |e: std::io::Error| e.to_string();
    let opened = restore::open_snapshot(dirs, file, || match identity {
        Some(p) => std::fs::read_to_string(p)
            .map(ct_secrets::Zeroizing::new)
            .map_err(|e| format!("{}: {e}", p.display())),
        None => read_secret_line(
            input,
            "chave de recuperação (AGE-SECRET-KEY-1..., do gerenciador de senhas; não aparece \
             na tela): ",
        ),
    })?;
    let with = match opened.opened_with() {
        OpenedWith::HubKey => "a chave do hub desta máquina",
        OpenedWith::RecoveryKey => "a chave de recuperação",
    };
    writeln!(
        out,
        "{} aberto com {with} (rev {}), {} entrada(s):",
        file.display(),
        opened.store_rev(),
        opened.items().len()
    )
    .map_err(w)?;
    let width = opened
        .items()
        .iter()
        .map(|i| scope_text(&i.scope).len())
        .max()
        .unwrap_or(0);
    for i in opened.items() {
        writeln!(
            out,
            "  {:<width$}  {}  ({})",
            scope_text(&i.scope),
            i.name,
            kind_text(i.kind)
        )
        .map_err(w)?;
    }
    let current = dirs.secrets_file().exists();
    writeln!(
        out,
        "{}",
        match (opened.opened_with(), current) {
            (OpenedWith::HubKey, true) => "O store atual vai ser substituído (fica uma cópia).",
            (OpenedWith::HubKey, false) => "O hub ainda não tem store: este vira o store.",
            (OpenedWith::RecoveryKey, _) =>
                "Com a chave de recuperação: o hub ganha uma chave nova, a de recuperação deixa \
                 de valer (configure outra na tela Hub) e os secrets acima entram na lista a \
                 rotacionar na origem.",
        }
    )
    .map_err(w)?;
    if !yes {
        write!(out, "Restaurar? [s/N] ").map_err(w)?;
        out.flush().map_err(w)?;
        let mut answer = String::new();
        input.read_line(&mut answer).map_err(w)?;
        if !matches!(answer.trim(), "s" | "S" | "sim" | "Sim" | "SIM") {
            writeln!(out, "nada foi alterado").map_err(w)?;
            return Ok(SecretsRestored::Cancelled);
        }
    }
    restore::apply_restore(dirs, opened, snapshot_dir).map(SecretsRestored::Done)
}

/// Texto do resultado do `secrets restore`.
pub fn render_secrets_restore(dirs: &Dirs, r: &crate::secrets::restore::RestoreReport) -> String {
    use crate::secrets::restore::OpenedWith;
    use std::fmt::Write;
    let mut s = String::new();
    let _ = writeln!(
        s,
        "store de secrets restaurado em {} ({} entrada(s))",
        dirs.secrets_file().display(),
        r.entries
    );
    if let Some(p) = &r.previous_store {
        let _ = writeln!(s, "o store anterior ficou em {}", p.display());
    }
    if let Some(public) = &r.new_hub_public {
        let _ = writeln!(
            s,
            "chave nova do hub em {} (pública {public})",
            dirs.secrets_key_file().display()
        );
    }
    if let Some(p) = &r.previous_key {
        let _ = writeln!(s, "a chave anterior do hub ficou em {}", p.display());
    }
    let _ = writeln!(
        s,
        "config_rev {}: os ambientes recebem os secrets de novo no próximo pull",
        r.config_rev
    );
    match &r.snapshot {
        Ok(Some(p)) => {
            let _ = writeln!(s, "snapshot do store restaurado em {}", p.display());
        }
        Ok(None) => {}
        Err(e) => {
            let _ = writeln!(s, "aviso: o snapshot do store restaurado falhou: {e}");
        }
    }
    match r.with {
        OpenedWith::RecoveryKey => {
            let _ = writeln!(
                s,
                "a chave de recuperação usada foi exposta e deixou de valer: depois de subir o \
                 serviço, gere e verifique uma nova na tela Hub (Chave de recuperação)"
            );
            if !r.rotate_pending.is_empty() {
                let _ = writeln!(
                    s,
                    "secrets a rotacionar na origem (troque os valores em quem os emitiu e \
                     grave os novos no hub; a lista fica na tela Hub até você marcar):"
                );
                for i in &r.rotate_pending {
                    let _ = writeln!(s, "  {}  {}", scope_text(&i.scope), i.name);
                }
            }
        }
        OpenedWith::HubKey if !r.recovery_configured => {
            let _ = writeln!(
                s,
                "aviso: recuperação não configurada; gere a chave na tela Hub"
            );
        }
        OpenedWith::HubKey => {}
    }
    let _ = writeln!(s, "suba o serviço (systemctl --user start cthubd)");
    s
}

/// `cthubd secrets rotate-keys [--hub] [--recovery]` (#170), com o serviço
/// parado. Veja [`crate::secrets::rotate`].
pub fn secrets_rotate_keys(
    dirs: &Dirs,
    port: u16,
    snapshot_dir: &Path,
    opts: crate::secrets::rotate::RotateOpts,
) -> Result<crate::secrets::rotate::RotateReport, String> {
    ensure_port_free(port)?;
    crate::secrets::rotate::rotate_keys(dirs, opts, snapshot_dir)
}

/// Texto do resultado do `secrets rotate-keys`. Inclui a chave privada de
/// recuperação nova: imprima uma vez só.
pub fn render_rotate_keys(dirs: &Dirs, r: &crate::secrets::rotate::RotateReport) -> String {
    use std::fmt::Write;
    let mut s = String::new();
    if let Some(h) = &r.hub {
        let _ = writeln!(
            s,
            "chave do hub trocada em {} (pública {}); a antiga foi apagada",
            dirs.secrets_key_file().display(),
            h.public
        );
        let _ = writeln!(
            s,
            "store recifrado; snapshots: {} recifrado(s), {} removido(s) (nenhuma chave \
             conhecida abria ou não deu para recifrar)",
            h.snapshots_reencrypted,
            h.snapshots_removed.len()
        );
        for p in &h.snapshots_removed {
            let _ = writeln!(s, "  removido: {}", p.display());
        }
        match &h.snapshot {
            Ok(Some(p)) => {
                let _ = writeln!(s, "snapshot do store em {}", p.display());
            }
            Ok(None) => {}
            Err(e) => {
                let _ = writeln!(s, "aviso: o snapshot do store falhou: {e}");
            }
        }
        let _ = writeln!(
            s,
            "cópias antigas fora da pasta (histórico de versões do OneDrive, por exemplo) \
             continuam abrindo com a chave antiga do hub e a de recuperação da época"
        );
    }
    if let Some(rec) = &r.recovery {
        let _ = writeln!(
            s,
            "\nchave de recuperação NOVA (aparece só agora; guarde no seu gerenciador de \
             senhas):\n\n  {}\n\npública: {} (pendente em {})\nsuba o serviço e cole a chave \
             na tela Hub (Chave de recuperação) para verificar: até lá, a chave atual continua \
             valendo e o store não é cifrado para a nova",
            rec.private_key.as_str(),
            rec.public,
            dirs.secrets_recovery_pending_file().display()
        );
    }
    let _ = writeln!(s, "suba o serviço (systemctl --user start cthubd)");
    s
}

/// `cthubd secrets list [--env <nome>]` (#170): escopo, nome e tipo das
/// entradas do store (nunca valores). Com `env`, só as que o hub entrega a
/// esse ambiente (globais e dos projetos atribuídos): o que estava no
/// cache dele e precisa ser rotacionado na origem se a máquina se perder.
/// Só lê: funciona com o serviço no ar. Rode **antes** do `revoke-env`
/// (sem o token, um ambiente que nunca enviou nada some do hub).
pub fn secrets_list(dirs: &Dirs, env: Option<&str>) -> Result<String, String> {
    use std::fmt::Write;
    let key = dirs.secrets_key_file();
    if !key.exists() {
        return Err(format!(
            "{} não existe: o hub ainda não tem store de secrets",
            key.display()
        ));
    }
    let id = ct_secrets::read_identity(&key).map_err(|e| e.to_string())?;
    let store =
        ct_secrets::SecretsStore::open(dirs.secrets_file(), id, &[]).map_err(|e| e.to_string())?;
    let projects: Option<Vec<String>> = match env {
        None => None,
        Some(name) => {
            let inputs = open_hub(dirs)?
                .secrets_delivery_inputs()
                .map_err(|e| e.to_string())?;
            let e = inputs
                .into_iter()
                .find(|e| e.env_name == name.trim())
                .ok_or_else(|| format!("ambiente desconhecido no hub: \"{}\"", name.trim()))?;
            Some(
                e.assignments
                    .into_iter()
                    .filter(|a| a.active)
                    .map(|a| a.project_key)
                    .collect(),
            )
        }
    };
    let mut s = String::new();
    for e in store.entries() {
        let entregue = match (&projects, &e.scope) {
            (None, _) | (Some(_), ct_secrets::Scope::Global) => true,
            (Some(ps), ct_secrets::Scope::Project(k)) => ps.contains(k),
        };
        if entregue {
            let _ = writeln!(
                s,
                "{}  {}  ({})",
                scope_text(&e.scope),
                e.name,
                kind_text(e.kind)
            );
        }
    }
    Ok(s)
}

/// `<prefix>-<stamp>` livre em `data` (ex.: `hub.db.pre-restore-<stamp>`):
/// se ele (ou o `-wal`/`-shm` dele) já existe, de um restore anterior no
/// mesmo minuto, tenta `-2`, `-3`... Nunca sobrescreve um backup anterior.
pub(crate) fn free_backup_name(data: &Path, prefix: &str, stamp: &str) -> PathBuf {
    let taken = |p: &Path| {
        ["", "-wal", "-shm"]
            .iter()
            .any(|ext| Path::new(&format!("{}{ext}", p.display())).exists())
    };
    let base = data.join(format!("{prefix}-{stamp}"));
    if !taken(&base) {
        return base;
    }
    (2u32..)
        .map(|n| data.join(format!("{prefix}-{stamp}-{n}")))
        .find(|p| !taken(p))
        .expect("algum sufixo livre")
}

/// Cria o marcador [`ct_core::docs::DOCS_ROOT_MARKER`] na raiz de
/// `docs_dir` se faltar (#441). Nunca cria `docs_dir` nem sobrescreve o
/// marcador; falha vira aviso no log. Devolve se criou.
pub(crate) fn ensure_docs_marker(docs_dir: &Path) -> bool {
    use ct_core::docs::{DOCS_ROOT_MARKER, DOCS_ROOT_MARKER_TEXT};
    if !docs_dir.is_dir() {
        tracing::warn!(
            "docs_dir {} não existe: crie a pasta (o hub não a cria)",
            docs_dir.display()
        );
        return false;
    }
    let marker = docs_dir.join(DOCS_ROOT_MARKER);
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&marker)
    {
        Ok(mut f) => {
            use std::io::Write;
            if let Err(e) = f.write_all(DOCS_ROOT_MARKER_TEXT.as_bytes()) {
                tracing::warn!("docs_dir: {} não gravado: {e}", marker.display());
                return false;
            }
            tracing::info!("docs_dir: marcador {} criado", marker.display());
            true
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => false,
        Err(e) => {
            tracing::warn!("docs_dir: {} não criado: {e}", marker.display());
            false
        }
    }
}

/// `cthubd serve`: sobe o hub com o config de `dirs`.
pub async fn serve(dirs: Dirs) -> Result<(), Box<dyn std::error::Error>> {
    dirs.ensure()?;
    let cfg = HubConfig::load(&dirs)?;
    let ui_token = ct_core::files::ensure_token(&dirs.ui_token_file())?;
    let hub_id = ct_core::files::ensure_env_id(&dirs.hub_id_file())?;
    let name = cfg.name();
    let db = dirs.hub_db();
    tracing::info!("hub {name} ({hub_id}): {}", db.display());
    let hub = SharedHub::new(Hub::open(&db)?);

    let mut state = HubState::new(hub.clone(), &ui_token, &name, &hub_id);
    state.health = Arc::new(cfg.health.clone());
    state.gh_sync_interval_secs = cfg.gh_sync_interval_secs;
    state.docs_dir = cfg.docs_dir().map(|d| d.display().to_string().into());
    if let Some(d) = cfg.docs_dir() {
        ensure_docs_marker(&d);
    }
    // UI web (#199) e a sessão web (#200).
    if cfg.web.enabled {
        cfg.web.validate()?;
    }
    state.web = crate::web::WebOptions::new(&cfg.web, cfg.port, hub.clone(), &ui_token);
    if cfg.web.enabled {
        tracing::info!(
            "UI web ligada; origens aceitas: {}",
            state.web.allowed_origins().join(", ")
        );
        tracing::info!(
            "sessão web: {} dias, {} sem uso; proxy: {}",
            cfg.web.session_days,
            cfg.web.idle_days,
            match (&cfg.web.cloudflare, &cfg.web.tailscale) {
                (Some(_), Some(_)) => "Cloudflare Access e Tailscale",
                (Some(_), None) => "Cloudflare Access",
                (None, Some(_)) => "Tailscale",
                (None, None) => "nenhum (só o cookie)",
            }
        );
        if !crate::web::bundle_present() {
            tracing::warn!(
                "UI web ligada, mas este cthubd foi compilado sem o bundle (página de aviso):                  rode `npm run build:web` em ui/ e compile de novo"
            );
        }
    }

    // Web Push (#203): chave VAPID gerada na primeira subida com a web ligada.
    if cfg.web.enabled && cfg.web.push.enabled {
        let vapid = crate::push::Vapid::load_or_create(
            &dirs.vapid_key_file(),
            &crate::push::subject(&cfg.web),
        )?;
        let push = Arc::new(
            crate::push::WebPush::new(
                vapid,
                hub.clone(),
                crate::push::PushOptions {
                    stale_ms: (cfg.web.push.env_stale_minutes * 60_000) as i64,
                    allow_http: false,
                },
            )
            .with_channels(Arc::clone(&state.channels)),
        );
        tracing::info!(
            "Web Push ligada; ambiente sem sync avisa depois de {} min{}",
            cfg.web.push.env_stale_minutes,
            if cfg.web.push.env_stale_minutes == 0 {
                " (desligado)"
            } else {
                ""
            }
        );
        Arc::clone(&push).spawn();
        state.push = Some(push);
    }

    // Chave do store de secrets: gerada aqui na primeira subida (#164).
    let mut secrets = crate::secrets::HubSecrets::open(&dirs, hub.clone())?
        .with_reverify_days(cfg.recovery_reverify_days)
        .with_snapshot_dir(cfg.secrets_snapshot_dir(&dirs));
    if cfg.secrets_snapshot_follows_backup() {
        secrets = secrets.following_backup_dir();
    }
    let st = secrets.status();
    if st.recovery_configured {
        tracing::info!(
            "store de secrets: {} entradas (rev {})",
            st.entries,
            st.store_rev
        );
    } else {
        tracing::warn!(
            "store de secrets: {} entradas (rev {}), recuperação não configurada",
            st.entries,
            st.store_rev
        );
    }
    if let Some(d) = secrets.snapshot_dir() {
        tracing::info!("snapshots de secrets em {}", d.display());
    }
    let secrets = Arc::new(secrets);
    // A primeira conferência já tira um snapshot se o mais recente não
    // bate com o store (#168).
    crate::secrets::spawn_snapshot_check(Arc::clone(&secrets));
    state.secrets = Some(Arc::clone(&secrets));

    // `backup.json` (UI) sobrepõe o config.toml; as credenciais do destino
    // remoto ficam no store de secrets.
    let backups = Arc::new(
        Backups::new(
            cfg.backup_dir(&dirs),
            Duration::from_secs(cfg.backup_interval_hours * 3600),
            cfg.backup_keep,
            Some(&db),
        )
        .with_docs(
            cfg.docs_dir(),
            Duration::from_secs(cfg.docs_backup_interval_hours * 3600),
            cfg.docs_backup_keep,
            cfg.docs_backup_max_mb,
        )
        .with_remote(dirs.backup_settings_file(), Arc::clone(&secrets)),
    );
    if !backups.enabled() {
        tracing::warn!("snapshots do hub.db desligados (backup_interval_hours = 0)");
    } else if backups.same_device() {
        tracing::warn!(
            "backup no mesmo disco do hub.db ({}): ponha backup_dir em outro disco",
            backups.dir().display()
        );
    }
    backup::spawn_periodic(hub.clone(), Arc::clone(&backups));
    if let Some(d) = cfg.docs_dir() {
        if backups.docs_enabled() {
            tracing::info!("backup dos docs de {} ligado", d.display());
        } else {
            tracing::warn!("backup dos docs desligado (docs_backup_interval_hours = 0)");
        }
    }
    backup::spawn_docs_periodic(Arc::clone(&backups));
    state.backups = backups;

    let gh = Arc::new(HubGithub::new(
        hub,
        GhClient::new(
            GhConfig::new(&cfg.gh_api_url, &cfg.gh_bin).with_token_file(dirs.github_token_file()),
        ),
        &hub_id,
        state.events.clone(),
    ));
    if cfg.gh_sync_interval_secs > 0 {
        github::spawn_periodic(
            Arc::clone(&gh),
            Duration::from_secs(cfg.gh_sync_interval_secs),
        );
    } else {
        tracing::info!("sync periódico com o GitHub desligado (gh_sync_interval_secs = 0)");
    }
    state.github = Some(gh);

    // Atualização pela UI (#421): retoma o acompanhamento de uma em
    // andamento (este pode ser o cthubd novo) e verifica periodicamente.
    let updates = Arc::new(crate::update::HubUpdates::new(
        &cfg,
        &dirs,
        state.events.clone(),
    ));
    updates.resume();
    if cfg.update_check_interval_hours > 0 {
        crate::update::spawn_periodic(
            Arc::clone(&updates),
            Duration::from_secs(cfg.update_check_interval_hours * 3600),
        );
    }
    state.updates = Some(updates);

    ct_http::serve(crate::router(state), &cfg.bind, cfg.port, "cthubd").await?;
    tracing::info!("cthubd encerrado");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Marcador de `docs_dir` (#441): criado se falta, nunca sobrescrito, e
    /// `docs_dir` inexistente não é criado.
    #[test]
    fn marcador_de_docs_dir_criado_sem_sobrescrever() {
        use ct_core::docs::{DOCS_ROOT_MARKER, DOCS_ROOT_MARKER_TEXT};
        let tmp = tempfile::tempdir().unwrap();
        let fora = tmp.path().join("nao-existe");
        assert!(!ensure_docs_marker(&fora));
        assert!(!fora.exists());

        assert!(ensure_docs_marker(tmp.path()));
        let m = tmp.path().join(DOCS_ROOT_MARKER);
        assert_eq!(std::fs::read_to_string(&m).unwrap(), DOCS_ROOT_MARKER_TEXT);
        std::fs::write(&m, "meu").unwrap();
        assert!(!ensure_docs_marker(tmp.path()));
        assert_eq!(std::fs::read_to_string(&m).unwrap(), "meu");
    }

    fn dirs(root: &Path) -> Dirs {
        Dirs {
            config: root.join("config"),
            data: root.join("data"),
        }
    }

    #[test]
    fn restaura_snapshot_e_guarda_o_atual() {
        let d = tempfile::tempdir().unwrap();
        let ds = dirs(d.path());
        // Snapshot com o token "a"; depois o hub ganha "b".
        let tok_a = add_env(&ds, "a").unwrap().token;
        let snap = Hub::open(&ds.hub_db())
            .unwrap()
            .snapshot(&d.path().join("bk"), ct_core::now_ms())
            .unwrap();
        add_env(&ds, "b").unwrap();

        let prev = restore_files(&ds, &snap).unwrap().unwrap();
        assert!(prev.exists());
        let h = Hub::open(&ds.hub_db()).unwrap();
        assert_eq!(h.env_token_names().unwrap(), vec!["a"]);
        assert!(h.authenticate(&tok_a).unwrap().is_some());
        let antigo = Hub::open(&prev).unwrap();
        assert_eq!(antigo.env_token_names().unwrap(), vec!["a", "b"]);
        assert!(snap.exists(), "o snapshot não é consumido");
    }

    /// Dois restores no mesmo minuto: o segundo não sobrescreve o backup
    /// do primeiro, nem um `-wal` que sobrou com o nome base.
    #[test]
    fn restores_no_mesmo_minuto_nao_sobrescrevem_o_backup() {
        let d = tempfile::tempdir().unwrap();
        let ds = dirs(d.path());
        add_env(&ds, "original").unwrap();
        let snap = Hub::open(&ds.hub_db())
            .unwrap()
            .snapshot(&d.path().join("bk"), ct_core::now_ms())
            .unwrap();
        add_env(&ds, "depois").unwrap();
        let epoch = Hub::open(&snap).unwrap().config_epoch().unwrap();
        let p1 = restore_files(&ds, &snap).unwrap().unwrap();
        // O restore sorteia uma época nova da configuração; o snapshot não muda.
        let restaurado = Hub::open(&ds.hub_db()).unwrap().config_epoch().unwrap();
        assert_ne!(restaurado, epoch);
        assert_eq!(Hub::open(&snap).unwrap().config_epoch().unwrap(), epoch);
        let p2 = restore_files(&ds, &snap).unwrap().unwrap();
        assert_ne!(p1, p2);
        // O primeiro backup ainda é o hub.db original (com "depois").
        let h1 = Hub::open(&p1).unwrap();
        assert_eq!(h1.env_token_names().unwrap(), vec!["depois", "original"]);
        assert!(p2.exists());

        // Nome base livre, mas com um -wal órfão: também pula.
        let stamp = "20261018-1200";
        let base = d.path().join(format!("hub.db.pre-restore-{stamp}"));
        assert_eq!(
            free_backup_name(d.path(), "hub.db.pre-restore", stamp),
            base
        );
        std::fs::write(format!("{}-wal", base.display()), "x").unwrap();
        assert_eq!(
            free_backup_name(d.path(), "hub.db.pre-restore", stamp),
            d.path().join(format!("hub.db.pre-restore-{stamp}-2"))
        );
    }

    /// Sessão web ativa no snapshot sai revogada do restore (pode ter sido
    /// revogada depois dele), e o `web revoke-all` grava direto no hub.db.
    #[test]
    fn restore_e_revoke_all_revogam_as_sessoes_web() {
        let d = tempfile::tempdir().unwrap();
        let ds = dirs(d.path());
        let idle = 7 * 24 * 3600 * 1000;
        let s = open_hub(&ds)
            .unwrap()
            .create_web_session(None, None, 3_600_000)
            .unwrap();
        let snap = Hub::open(&ds.hub_db())
            .unwrap()
            .snapshot(&d.path().join("bk"), ct_core::now_ms())
            .unwrap();
        restore_files(&ds, &snap).unwrap();
        let h = Hub::open(&ds.hub_db()).unwrap();
        assert!(h.check_web_session(&s.cookie, idle).unwrap().is_none());
        assert!(Hub::open(&snap)
            .unwrap()
            .check_web_session(&s.cookie, idle)
            .unwrap()
            .is_some());

        let s = h.create_web_session(None, None, 3_600_000).unwrap();
        drop(h);
        assert_eq!(web_revoke_all(&ds).unwrap(), 1);
        assert_eq!(web_revoke_all(&ds).unwrap(), 0);
        let h = Hub::open(&ds.hub_db()).unwrap();
        assert!(h.check_web_session(&s.cookie, idle).unwrap().is_none());
    }

    #[test]
    fn restore_recusa_com_servico_no_ar_ou_arquivo_ruim() {
        let d = tempfile::tempdir().unwrap();
        let ds = dirs(d.path());
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let snap = d.path().join("x.db");
        std::fs::write(&snap, "não é sqlite").unwrap();
        // O listener fica de pé durante a checagem: sem corrida de porta.
        let e = restore(&ds, port, &snap).unwrap_err();
        assert!(e.contains("pare o cthubd"), "{e}");
        assert_eq!(ensure_port_free(port).unwrap_err(), e);
        drop(l);
        let e = restore_files(&ds, &snap).unwrap_err();
        assert!(e.contains("snapshot"), "{e}");
        let e = restore_files(&ds, &d.path().join("nada.db")).unwrap_err();
        assert!(e.contains("não encontrado"), "{e}");
        assert!(!ds.hub_db().exists());
    }

    #[test]
    fn env_exists_pelo_nome() {
        let d = tempfile::tempdir().unwrap();
        let ds = dirs(d.path());
        assert!(!env_exists(&ds, "x").unwrap());
        add_env(&ds, "x").unwrap();
        assert!(env_exists(&ds, " x ").unwrap());
        assert!(!env_exists(&ds, "y").unwrap());
    }

    /// Perde o hub.db e volta do destino remoto cifrado: lista, baixa,
    /// decifra com a chave do hub e restaura (#263).
    #[test]
    fn restaura_do_destino_remoto_cifrado() {
        use crate::backup::{BackupUpdate, Backups};
        use crate::remote::{fake, RemoteKind, RemoteSettings};
        let d = tempfile::tempdir().unwrap();
        let ds = dirs(d.path());
        ds.ensure().unwrap();
        let (url, st) = fake::serve();
        let hub = SharedHub::new(Hub::open(&ds.hub_db()).unwrap());
        hub.lock()
            .add_project(&ct_ledger::NewHubProject {
                owner: "deep-ion-ai".into(),
                repo: "demo".into(),
                name: "demo".into(),
                base_branch: "main".into(),
                protected_branches: vec!["main".into()],
            })
            .unwrap();
        let secrets = Arc::new(crate::secrets::HubSecrets::open(&ds, hub.clone()).unwrap());
        let b = Backups::new(
            d.path().join("snaps"),
            Duration::from_secs(3600),
            5,
            Some(&ds.hub_db()),
        )
        .with_remote(ds.backup_settings_file(), secrets);
        let creds = [("access_key", "AK"), ("secret_key", "SK")]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        b.set_credentials(RemoteKind::S3, &creds).unwrap();
        b.update(&BackupUpdate {
            remote: Some(RemoteSettings {
                kind: RemoteKind::S3,
                bucket: "bk".into(),
                endpoint: url,
                ..Default::default()
            }),
            ..Default::default()
        })
        .unwrap();
        b.run_once(&hub).unwrap();
        assert!(st
            .lock()
            .unwrap()
            .objects
            .keys()
            .any(|k| k.ends_with(".db.age")));
        drop((hub, b));

        // Desastre: o hub.db some; só o destino remoto sobrou.
        for ext in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{ext}", ds.hub_db().display()));
        }
        std::fs::remove_dir_all(d.path().join("snaps")).unwrap();
        let lista = remote_list(&ds).unwrap();
        assert_eq!(lista.len(), 1);
        let snap = remote_download(&ds, "latest", None).unwrap();
        assert!(snap.extension().is_some_and(|e| e == "db"));
        restore_files(&ds, &snap).unwrap();
        let h = Hub::open(&ds.hub_db()).unwrap();
        assert!(h.find_project("deep-ion-ai", "demo").unwrap().is_some());

        assert!(remote_download(&ds, "../x", None).is_err());
    }

    /// Backup dos docs (#442): pacote cifrado no remoto, restaurado com
    /// `--to` pela chave do hub e, numa máquina sem ela, pela de
    /// recuperação; o `docs_dir` nunca é o destino.
    #[test]
    fn restaura_docs_com_chave_do_hub_e_de_recuperacao() {
        use crate::backup::{BackupUpdate, Backups, DocsRun};
        use crate::remote::{fake, RemoteKind, RemoteSettings};
        let d = tempfile::tempdir().unwrap();
        let ds = dirs(d.path());
        ds.ensure().unwrap();
        let rec = ct_secrets::Identity::generate();
        std::fs::write(ds.secrets_recovery_file(), format!("{}\n", rec.to_public())).unwrap();
        let docs = d.path().join("docs");
        std::fs::create_dir_all(docs.join("acme__app/_projeto/specs")).unwrap();
        std::fs::write(docs.join("acme__app/_projeto/specs/spec.md"), "# Spec\n").unwrap();
        let (url, st) = fake::serve();
        let hub = SharedHub::new(Hub::open(&ds.hub_db()).unwrap());
        let secrets = Arc::new(crate::secrets::HubSecrets::open(&ds, hub.clone()).unwrap());
        let b = Backups::new(
            d.path().join("snaps"),
            Duration::from_secs(3600),
            5,
            Some(&ds.hub_db()),
        )
        .with_docs(Some(docs.clone()), Duration::from_secs(86400), 7, 10)
        .with_remote(ds.backup_settings_file(), secrets);
        let creds = [("access_key", "AK"), ("secret_key", "SK")]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        b.set_credentials(RemoteKind::S3, &creds).unwrap();
        b.update(&BackupUpdate {
            remote: Some(RemoteSettings {
                kind: RemoteKind::S3,
                bucket: "bk".into(),
                endpoint: url,
                ..Default::default()
            }),
            ..Default::default()
        })
        .unwrap();
        let DocsRun::Done { path: local, .. } = b.run_docs_once().unwrap() else {
            panic!("o pacote não rodou")
        };
        let (remote_name, sealed) = {
            let g = st.lock().unwrap();
            let (k, v) = g
                .objects
                .iter()
                .find(|(k, _)| k.starts_with("docs-"))
                .unwrap();
            (k.clone(), v.clone())
        };
        assert!(remote_name.ends_with(".tar.zst.age"));
        assert!(remote_list(&ds)
            .unwrap()
            .iter()
            .any(|o| o.name == remote_name));

        // Pela chave do hub, do destino remoto (com o hub "no ar": não
        // mexe no hub.db).
        let to = d.path().join("rest-remoto");
        let (pkg, n) =
            docs_restore(&ds, Some(&docs), DocsSource::Remote("latest"), &to, None).unwrap();
        assert_eq!((pkg.as_str(), n), (remote_name.as_str(), 1));
        assert_eq!(
            std::fs::read_to_string(to.join("acme__app/_projeto/specs/spec.md")).unwrap(),
            "# Spec\n"
        );
        // Pacote local, sem cifra.
        let to2 = d.path().join("rest-local");
        docs_restore(&ds, Some(&docs), DocsSource::Local(&local), &to2, None).unwrap();
        assert!(to2.join("acme__app/_projeto/specs/spec.md").exists());
        // Nunca no docs_dir, nem numa pasta com conteúdo.
        for alvo in [&docs, &docs.join("acme__app/novo"), &to] {
            assert!(docs_restore(&ds, Some(&docs), DocsSource::Local(&local), alvo, None).is_err());
        }
        assert!(!docs.join("acme__app/novo").exists());
        assert!(docs_restore(&ds, Some(&docs), DocsSource::Remote("../x"), &to2, None).is_err());

        // Máquina nova: sem a chave do hub, só a de recuperação abre.
        drop((hub, b));
        let baixado = d.path().join(&remote_name);
        std::fs::write(&baixado, &sealed).unwrap();
        std::fs::remove_file(ds.secrets_key_file()).unwrap();
        let to3 = d.path().join("rest-recuperacao");
        let e =
            docs_restore(&ds, Some(&docs), DocsSource::Local(&baixado), &to3, None).unwrap_err();
        assert!(e.contains("--identity"), "{e}");
        assert!(!to3.exists());
        let id = d.path().join("recuperacao.key");
        ct_secrets::write_identity(&id, &rec, "recuperação").unwrap();
        let (_, n) = docs_restore(
            &ds,
            Some(&docs),
            DocsSource::Local(&baixado),
            &to3,
            Some(&id),
        )
        .unwrap();
        assert_eq!(n, 1);
        assert!(to3.join("acme__app/_projeto/specs/spec.md").exists());
    }
}
