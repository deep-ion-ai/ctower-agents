//! `cthubd migrate --from <data_dir do ctowerd>` (spec do hub como plano de
//! controle §11, issue #182): troca o hub do #64 (`ctowerd` com
//! `hub = true`) pelo `cthubd`.
//!
//! - copia o `hub.db` com `VACUUM INTO` (o original fica intocado) e aplica
//!   as migrações novas; `env`, `env_token` (hashes e `env_id`) e o
//!   histórico seguem iguais, então os tokens de sync continuam valendo;
//! - promove os projetos a origem e cria as atribuições a partir de
//!   `project_location` ([`ct_ledger::Hub::adopt_legacy`]);
//! - cria o `config.toml` do `cthubd` a partir de `bind` e `hub_backup_*` do
//!   `ctowerd` e o token de UI do hub;
//! - transforma o `ctowerd` local num ambiente comum: token de sync preso ao
//!   `env_id` que ele já usava (o hub antigo gravava o ledger dele em
//!   processo com esse `env_id`, sem token), `hub.token`, e `hub = true`
//!   trocado por `hub_url` (o config antigo vai para `config.toml.pre-cthub`).
//!
//! Tudo o que pode falhar roda antes de tocar nos arquivos finais: o
//! `hub.db` novo é montado ao lado (`hub.db.migrate`) e só troca de nome no
//! fim. Se algo falhar depois, rodar de novo retoma: um `hub.db` já migrado
//! (`hub_meta.migrated_at`) com o `ctowerd` ainda em `hub = true` só refaz a
//! parte do `ctowerd`, desde que o `hub.db` antigo esteja igual ao copiado
//! (impressão digital em `hub_meta`); se mudou (voltou-se atrás e o hub
//! antigo recebeu algo), recusa sem `--force`. Com o `ctowerd` já
//! convertido, não faz nada. `--force` refaz tudo a partir do `hub.db`
//! antigo (o do `cthubd` fica ao lado).
//!
//! Lê o config do `ctowerd` como TOML cru: o `cthubd` não depende de
//! `ct-sessions`.

use crate::config::{Dirs, HubConfig, DEFAULT_PORT};
use ct_ledger::Hub;
use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Porta padrão do `ctowerd` (para conferir que ele está parado).
const CTOWER_DEFAULT_PORT: u16 = 7878;

/// Chaves do papel de hub que saem do config do `ctowerd`.
const LEGACY_KEYS: [&str; 4] = [
    "hub",
    "hub_backup_dir",
    "hub_backup_interval_hours",
    "hub_backup_keep",
];

/// Chaves do `ctowerd` que passam iguais para o `cthubd` (mesmo nome).
const SHARED_KEYS: [&str; 6] = [
    "bind",
    "gh_sync_interval_secs",
    "gh_api_url",
    "gh_bin",
    "update_repo",
    "update_ref",
];

#[derive(Debug, Clone)]
pub struct MigrateOpts {
    /// Diretório de dados do `ctowerd` (onde está o `hub.db` antigo).
    pub from: PathBuf,
    /// Diretório de config do `ctowerd` (`config.toml`, `env_id`, `hub.token`).
    pub ctower_config: PathBuf,
    /// Nome do `ctowerd` local no hub; `None` = `env_name` do config dele ou
    /// o nome da máquina.
    pub env_name: Option<String>,
    /// Substitui um `hub.db` do `cthubd` que já existe (ele fica ao lado
    /// como `hub.db.pre-migrate-<AAAAMMDD-HHMM>`).
    pub force: bool,
    /// Porta do `cthubd` (vai no `hub_url` do `ctowerd` local).
    pub hub_port: u16,
}

impl MigrateOpts {
    /// `--from`, com o config do `ctowerd` em `CTOWER_CONFIG_DIR` (ou
    /// `~/.config/ctower`) e a porta do config do `cthubd`.
    pub fn new(from: PathBuf, hub_port: u16) -> io::Result<Self> {
        Ok(MigrateOpts {
            from,
            ctower_config: ctower_config_dir()?,
            env_name: None,
            force: false,
            hub_port,
        })
    }
}

/// `CTOWER_CONFIG_DIR`, senão `$XDG_CONFIG_HOME/ctower` ou `~/.config/ctower`
/// (o mesmo do `ctowerd`).
pub fn ctower_config_dir() -> io::Result<PathBuf> {
    let var = |k: &str| {
        std::env::var_os(k)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    if let Some(d) = var("CTOWER_CONFIG_DIR") {
        return Ok(d);
    }
    let home =
        var("HOME").ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "HOME não definido"))?;
    Ok(var("XDG_CONFIG_HOME")
        .unwrap_or_else(|| home.join(".config"))
        .join("ctower"))
}

/// O que a migração fez, para a saída do comando.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MigrateReport {
    /// Já estava migrado: nada mudou.
    pub already: bool,
    /// Retomada de uma migração interrompida (o `hub.db` já estava migrado).
    pub resumed: bool,
    pub hub_db: PathBuf,
    /// `hub.db` do `cthubd` que existia e foi guardado (`--force`).
    pub replaced: Option<PathBuf>,
    pub projects: usize,
    pub assignments: usize,
    pub config_rev: i64,
    /// Nomes dos tokens de sync no hub (os antigos continuam valendo).
    pub env_tokens: Vec<String>,
    /// `config.toml` do `cthubd` gerado agora (`None` = já existia, mantido).
    pub hub_config: Option<PathBuf>,
    /// O `ctowerd` local virou ambiente: nome no hub e `env_id`.
    pub local_env: Option<(String, Option<String>)>,
    /// Avisos (o que ficou para fazer à mão).
    pub warnings: Vec<String>,
}

/// `cthubd migrate`: recusa com o `ctowerd` (porta do config dele) ou um
/// `cthubd` (`hub_port`) no ar e faz a migração.
pub fn migrate(dirs: &Dirs, o: &MigrateOpts) -> Result<MigrateReport, String> {
    let old = read_ctower_config(&o.ctower_config)?;
    let ctower_port = match old.get("port") {
        None => CTOWER_DEFAULT_PORT,
        Some(v) => v
            .as_integer()
            .and_then(|p| u16::try_from(p).ok())
            .ok_or_else(|| "port inválida no config do ctowerd".to_string())?,
    };
    if listening(ctower_port) {
        return Err(format!(
            "o ctowerd está no ar em 127.0.0.1:{ctower_port}: pare-o antes de migrar \
             (systemctl --user stop ctowerd), para nenhuma mudança ficar só no hub.db antigo"
        ));
    }
    if listening(o.hub_port) {
        return Err(format!(
            "há um serviço em 127.0.0.1:{}: pare o cthubd antes de migrar \
             (systemctl --user stop cthubd)",
            o.hub_port
        ));
    }
    migrate_files(dirs, o)
}

/// Há algo escutando em `127.0.0.1:port`?
fn listening(port: u16) -> bool {
    std::net::TcpStream::connect_timeout(
        &std::net::SocketAddr::from(([127, 0, 0, 1], port)),
        Duration::from_millis(500),
    )
    .is_ok()
}

/// A migração em si, sem a checagem das portas (testes que já prenderam a
/// porta do hub).
pub fn migrate_files(dirs: &Dirs, o: &MigrateOpts) -> Result<MigrateReport, String> {
    let cfg_path = o.ctower_config.join("config.toml");
    let old = read_ctower_config(&o.ctower_config)?;
    let legacy = old.get("hub").and_then(toml::Value::as_bool) == Some(true);
    let hub_db = dirs.hub_db();
    let old_db = o.from.join("hub.db");
    let migrated = if hub_db.exists() {
        Hub::migrated_at_path(&hub_db)
            .map_err(|e| format!("{}: {e}", hub_db.display()))?
            .is_some()
    } else {
        false
    };
    let mut rep = MigrateReport {
        hub_db: hub_db.clone(),
        ..Default::default()
    };

    if migrated && !legacy && !o.force {
        rep.already = true;
        let hub = Hub::open(&hub_db).map_err(|e| e.to_string())?;
        fill_counts(&hub, &mut rep)?;
        return Ok(rep);
    }
    // `--force` refaz do hub.db antigo mesmo sobre um já migrado (nova
    // tentativa depois de voltar atrás).
    let rebuild = !migrated || o.force;
    if rebuild {
        if hub_db.exists() && !o.force {
            return Err(format!(
                "o cthubd já tem um hub.db ({}): para substituí-lo pelo do ctowerd, rode de novo \
                 com --force (o atual fica ao lado como hub.db.pre-migrate-<data>)",
                hub_db.display()
            ));
        }
        if !old_db.is_file() {
            return Err(format!(
                "não há hub.db em {}: passe em --from o diretório de dados do ctowerd que era o \
                 hub (hub = true), em geral ~/.local/share/ctower",
                o.from.display()
            ));
        }
        if same_file(&old_db, &hub_db) {
            return Err(format!(
                "o hub.db de origem e o do cthubd são o mesmo arquivo ({}): ajuste CTHUB_DATA_DIR",
                hub_db.display()
            ));
        }
    }
    if !rebuild {
        // Retomada só se o hub antigo não recebeu nada depois da cópia: depois
        // de voltar atrás e usar o hub antigo, o hub.db do cthubd está velho.
        if !old_db.is_file() {
            return Err(format!(
                "{} já foi migrado, mas não há hub.db em {} para conferir se o hub antigo \
                 mudou depois: confira --from, ou rode com --force para refazer",
                hub_db.display(),
                o.from.display()
            ));
        }
        let unchanged = Hub::open(&hub_db)
            .and_then(|h| h.legacy_source_unchanged(&old_db))
            .map_err(|e| e.to_string())?;
        if !unchanged {
            return Err(format!(
                "o hub.db antigo ({}) mudou depois da migração anterior (o hub antigo voltou a \
                 ser usado): retomar perderia o que ele recebeu. Rode com --force para migrar \
                 de novo a partir dele (o hub.db atual do cthubd fica ao lado como \
                 hub.db.pre-migrate-<data>)",
                old_db.display()
            ));
        }
    }
    if !legacy {
        rep.warnings.push(format!(
            "o config do ctowerd ({}) não tem hub = true: não o converti. Para ligar o ctowerd \
             desta máquina ao cthubd, gere o token com \"cthubd add-env <nome>\", grave-o com \
             \"ctower hub set-token\" e ponha hub_url = \"http://127.0.0.1:{}\" no config",
            cfg_path.display(),
            o.hub_port
        ));
    }

    // --- checagens do ctowerd local, antes de qualquer escrita ---
    let local = if legacy {
        Some(LocalEnv::prepare(o, &old)?)
    } else {
        None
    };

    let hub_cfg = dirs.config.join("config.toml");
    let hub_cfg_text = if hub_cfg.exists() {
        None
    } else {
        Some(hub_config_from(&old, &cfg_path, o.hub_port)?)
    };

    // --- hub.db novo, montado ao lado ---
    dirs.ensure().map_err(|e| e.to_string())?;
    let tmp = dirs.data.join("hub.db.migrate");
    let mut token = None;
    if rebuild {
        remove_db_files(&tmp);
        let built = (|| -> Result<(), String> {
            ct_ledger::snapshot_db(&old_db, &tmp)
                .map_err(|e| format!("{}: {e}", old_db.display()))?;
            let hub = Hub::open(&tmp).map_err(|e| format!("hub.db antigo inválido: {e}"))?;
            let a = hub
                .adopt_legacy(ct_core::now_ms())
                .map_err(|e| e.to_string())?;
            (rep.projects, rep.assignments, rep.config_rev) =
                (a.projects, a.assignments, a.config_rev);
            if let Some(l) = &local {
                token = Some(l.token(&hub)?);
            }
            rep.env_tokens = hub.env_token_names().map_err(|e| e.to_string())?;
            Ok(())
        })();
        if let Err(e) = built {
            remove_db_files(&tmp);
            return Err(e);
        }
        for ext in ["-wal", "-shm"] {
            let _ = fs::remove_file(format!("{}{ext}", tmp.display()));
        }
    } else {
        rep.resumed = true;
        let hub = Hub::open(&hub_db).map_err(|e| e.to_string())?;
        if let Some(l) = &local {
            token = Some(l.token(&hub)?);
        }
        fill_counts(&hub, &mut rep)?;
    }

    // --- arquivos do cthubd ---
    match hub_cfg_text {
        None => rep.warnings.push(format!(
            "{} já existia: mantido (confira bind e backup_dir)",
            hub_cfg.display()
        )),
        Some(text) => {
            write_atomic(&hub_cfg, text.as_bytes(), 0o644).map_err(|e| e.to_string())?;
            rep.hub_config = Some(hub_cfg);
        }
    }
    ct_core::files::ensure_token(&dirs.ui_token_file()).map_err(|e| e.to_string())?;
    let gh_old = o.ctower_config.join("github.token");
    let gh_new = dirs.github_token_file();
    if gh_old.is_file() && !gh_new.exists() {
        let t = fs::read(&gh_old).map_err(|e| e.to_string())?;
        write_atomic(&gh_new, &t, 0o600).map_err(|e| e.to_string())?;
    }

    // --- troca do hub.db ---
    if rebuild {
        if hub_db.exists() {
            let prev = crate::cli::free_backup_name(
                &dirs.data,
                "hub.db.pre-migrate",
                &ct_core::utc_stamp(ct_core::now_ms()),
            );
            for ext in ["", "-wal", "-shm"] {
                let from = PathBuf::from(format!("{}{ext}", hub_db.display()));
                if from.exists() {
                    fs::rename(&from, format!("{}{ext}", prev.display()))
                        .map_err(|e| e.to_string())?;
                }
            }
            rep.replaced = Some(prev);
        }
        fs::rename(&tmp, &hub_db).map_err(|e| e.to_string())?;
    }

    // --- ctowerd local ---
    if let (Some(l), Some(token)) = (local, token) {
        l.commit(&token).map_err(|e| e.to_string())?;
        rep.local_env = Some((l.name, Some(l.env_id)));
    }
    Ok(rep)
}

fn fill_counts(hub: &Hub, rep: &mut MigrateReport) -> Result<(), String> {
    rep.projects = hub.list_projects().map_err(|e| e.to_string())?.len();
    rep.assignments = hub
        .list_assignments(&Default::default())
        .map_err(|e| e.to_string())?
        .len();
    rep.config_rev = hub.config_rev().map_err(|e| e.to_string())?;
    rep.env_tokens = hub.env_token_names().map_err(|e| e.to_string())?;
    Ok(())
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

fn remove_db_files(p: &Path) {
    for ext in ["", "-wal", "-shm"] {
        let _ = fs::remove_file(format!("{}{ext}", p.display()));
    }
}

/// `config.toml` do `ctowerd` como tabela TOML (ausente = vazio).
fn read_ctower_config(dir: &Path) -> Result<toml::Table, String> {
    let path = dir.join("config.toml");
    match fs::read_to_string(&path) {
        Ok(s) => toml::from_str(&s).map_err(|e| format!("{}: {e}", path.display())),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(toml::Table::new()),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

/// `config.toml` do `cthubd` a partir do config do `ctowerd`: `bind`,
/// `hub_backup_*` (sem o prefixo) e as chaves do GitHub e do atualizador
/// que existirem. `hub_backup_dir` vazio (o padrão antigo,
/// `<data do ctowerd>/hub-backups`) fica vazio: o padrão novo é
/// `<data do cthubd>/backups`.
fn hub_config_from(old: &toml::Table, src: &Path, hub_port: u16) -> Result<String, String> {
    let mut t = toml::Table::new();
    if hub_port != DEFAULT_PORT {
        t.insert("port".into(), toml::Value::Integer(hub_port.into()));
    }
    for k in SHARED_KEYS {
        if let Some(v) = old.get(k) {
            t.insert(k.into(), v.clone());
        }
    }
    if let Some(v) = old.get("hub_backup_dir") {
        if v.as_str().is_some_and(|s| !s.trim().is_empty()) {
            t.insert("backup_dir".into(), v.clone());
        }
    }
    for k in ["backup_interval_hours", "backup_keep"] {
        if let Some(v) = old.get(&format!("hub_{k}")) {
            t.insert(k.into(), v.clone());
        }
    }
    let body = toml::to_string(&t).map_err(|e| e.to_string())?;
    toml::from_str::<HubConfig>(&body)
        .map_err(|e| format!("config do ctowerd com valor inválido para o cthubd: {e}"))?;
    Ok(format!(
        "# Gerado por \"cthubd migrate\" a partir de {} (bind, hub_backup_*,\n\
         # GitHub e atualizador). Chaves em docs/hub.md, seção 1.\n{body}",
        src.display()
    ))
}

/// O `ctowerd` desta máquina, que deixa de ser o hub.
struct LocalEnv {
    config_dir: PathBuf,
    name: String,
    /// `env_id` que ele já usava (`<config>/env_id`); o hub antigo gravou o
    /// ledger dele com esse id. Sem arquivo, um novo, preso ao token.
    env_id: String,
    /// `env_id` gerado agora (o ctowerd ainda não tinha um): o commit o grava.
    fresh_env_id: bool,
    /// Config atual e o reescrito.
    current: String,
    rewritten: String,
}

impl LocalEnv {
    /// Checagens e o config novo, sem escrever nada.
    fn prepare(o: &MigrateOpts, old: &toml::Table) -> Result<Self, String> {
        let cfg_name = old
            .get("env_name")
            .and_then(toml::Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let (name, add_env_name) = match (o.env_name.as_deref().map(str::trim), cfg_name) {
            (Some(arg), Some(c)) if arg != c => {
                return Err(format!(
                    "env_name = \"{c}\" no config do ctowerd e --env-name {arg}: use um só"
                ))
            }
            (_, Some(c)) => (c.to_string(), false),
            (Some(arg), None) => (arg.to_string(), true),
            (None, None) => (
                crate::config::hostname().unwrap_or_else(|| "ctower".into()),
                true,
            ),
        };
        if name.is_empty()
            || !name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        {
            return Err(format!(
                "nome de ambiente inválido: \"{name}\" (use só letras, números, '.', '_' e '-'; \
                 passe --env-name)"
            ));
        }
        let id_file = o.ctower_config.join("env_id");
        let existing_id = match fs::read_to_string(&id_file) {
            Ok(s) if s.trim().is_empty() => None,
            Ok(s) => {
                let s = s.trim().to_string();
                if !is_uuid(&s) {
                    return Err(format!("{}: env_id inválido", id_file.display()));
                }
                Some(s)
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => None,
            Err(e) => return Err(format!("{}: {e}", id_file.display())),
        };
        // Sem env_id, o token nasce preso a um novo (#195); o commit o grava.
        let (env_id, fresh_env_id) = match existing_id {
            Some(id) => (id, false),
            None => (uuid::Uuid::new_v4().to_string(), true),
        };
        let cfg = o.ctower_config.join("config.toml");
        let current = fs::read_to_string(&cfg).map_err(|e| format!("{}: {e}", cfg.display()))?;
        let pre = o.ctower_config.join("config.toml.pre-cthub");
        if let Ok(p) = fs::read_to_string(&pre) {
            if p != current {
                return Err(format!(
                    "{} já existe e é diferente do config atual: mova-o para outro lugar e rode \
                     de novo (ele guardaria o config de antes da migração)",
                    pre.display()
                ));
            }
        }
        let url = format!("http://127.0.0.1:{}", o.hub_port);
        let rewritten = rewrite_ctower_config(&current, &url, add_env_name.then_some(&name))?;
        Ok(LocalEnv {
            config_dir: o.ctower_config.clone(),
            name,
            env_id,
            fresh_env_id,
            current,
            rewritten,
        })
    }

    /// Token de sync do `ctowerd` local em `hub`: preso ao `env_id` dele.
    /// Um token já preso a esse `env_id` (migração retomada) é trocado e
    /// mantém o nome. Recusa um nome que já é de outro ambiente.
    fn token(&self, hub: &Hub) -> Result<String, String> {
        let e = |e: ct_ledger::LedgerError| e.to_string();
        let env_id = &self.env_id;
        if let Some(n) = hub.env_token_name_of(env_id).map_err(e)? {
            if n != self.name {
                return Err(format!(
                    "o ctowerd desta máquina (env_id {env_id}) já tem o token de sync \"{n}\" no \
                     hub: rode com --env-name {n}"
                ));
            }
        } else if hub.env_token_env_id(&self.name).map_err(e)?.is_some() {
            return Err(self.name_taken());
        }
        hub.add_bound_env_token(&self.name, env_id)
            .map(|i| i.token)
            .map_err(e)
    }

    fn name_taken(&self) -> String {
        format!(
            "o hub já tem um token de sync \"{}\" de outro ambiente: escolha outro nome para o \
             ctowerd desta máquina com --env-name",
            self.name
        )
    }

    /// Grava `hub.token`, guarda o config em `config.toml.pre-cthub` e
    /// troca o config.
    fn commit(&self, token: &str) -> io::Result<()> {
        ct_core::files::private_dir(&self.config_dir)?;
        if self.fresh_env_id {
            ct_core::files::write_private(&self.config_dir.join("env_id"), self.env_id.as_bytes())?;
        }
        write_atomic(
            &self.config_dir.join("hub.token"),
            format!("{token}\n").as_bytes(),
            0o600,
        )?;
        let cfg = self.config_dir.join("config.toml");
        let mode = fs::metadata(&cfg)
            .map(|m| m.permissions().mode() & 0o777)
            .unwrap_or(0o600);
        let pre = self.config_dir.join("config.toml.pre-cthub");
        if !pre.exists() {
            write_atomic(&pre, self.current.as_bytes(), mode)?;
        }
        write_atomic(&cfg, self.rewritten.as_bytes(), mode)
    }
}

/// Tira do config do `ctowerd` as chaves do papel de hub (e um `hub_url`
/// antigo) e põe `hub_url` (e `env_name`, se faltar) no topo, antes de
/// qualquer `[tabela]`. Edição por linha, para manter os comentários; o
/// resultado é conferido como TOML.
fn rewrite_ctower_config(
    src: &str,
    hub_url: &str,
    env_name: Option<&String>,
) -> Result<String, String> {
    let mut top = true;
    let mut kept = Vec::new();
    for line in src.lines() {
        let t = line.trim_start();
        if t.starts_with('[') {
            top = false;
        }
        let key = t.split('=').next().unwrap_or("").trim();
        if top && t.contains('=') && (LEGACY_KEYS.contains(&key) || key == "hub_url") {
            continue;
        }
        kept.push(line);
    }
    let mut out = String::from(
        "# Ligado ao cthubd local por \"cthubd migrate\"; o config de antes está em\n\
         # config.toml.pre-cthub.\n",
    );
    if let Some(n) = env_name {
        out.push_str(&format!("env_name = {}\n", toml::Value::String(n.clone())));
    }
    out.push_str(&format!(
        "hub_url = {}\n",
        toml::Value::String(hub_url.into())
    ));
    if kept.iter().any(|l| !l.trim().is_empty()) {
        out.push('\n');
        for l in kept {
            out.push_str(l);
            out.push('\n');
        }
    }
    let t: toml::Table = toml::from_str(&out).map_err(|e| {
        format!("não consegui reescrever o config do ctowerd ({e}); troque hub = true por hub_url à mão")
    })?;
    if LEGACY_KEYS.iter().any(|k| t.contains_key(*k))
        || t.get("hub_url").and_then(toml::Value::as_str) != Some(hub_url)
    {
        return Err(
            "não consegui reescrever o config do ctowerd; troque hub = true por hub_url à mão"
                .into(),
        );
    }
    Ok(out)
}

fn is_uuid(s: &str) -> bool {
    let parts: Vec<&str> = s.split('-').collect();
    parts.len() == 5
        && parts
            .iter()
            .zip([8, 4, 4, 4, 12])
            .all(|(p, n)| p.len() == n && p.chars().all(|c| c.is_ascii_hexdigit()))
}

/// Escreve `contents` em `path` por um temporário ao lado e `rename`, com
/// a permissão `mode`.
fn write_atomic(path: &Path, contents: &[u8], mode: u32) -> io::Result<()> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let tmp = path.with_file_name(format!(".{name}.migrate"));
    ct_core::files::write_private(&tmp, contents)?;
    fs::set_permissions(&tmp, fs::Permissions::from_mode(mode))?;
    fs::rename(&tmp, path)
}

/// Texto da saída do comando: o que foi feito e os próximos passos.
pub fn render(rep: &MigrateReport, dirs: &Dirs, o: &MigrateOpts) -> String {
    let mut s = String::new();
    let port = o.hub_port;
    if rep.already {
        s.push_str(&format!(
            "nada a fazer: {} já foi migrado e o ctowerd desta máquina já não é hub.\n",
            rep.hub_db.display()
        ));
    } else if rep.resumed {
        s.push_str(&format!(
            "migração retomada: {} já estava migrado; refiz a parte do ctowerd.\n",
            rep.hub_db.display()
        ));
    } else {
        s.push_str(&format!(
            "hub.db migrado: {} -> {} (o original fica intocado, como backup)\n",
            o.from.join("hub.db").display(),
            rep.hub_db.display()
        ));
    }
    if let Some(p) = &rep.replaced {
        s.push_str(&format!(
            "  o hub.db que o cthubd tinha ficou em {}\n",
            p.display()
        ));
    }
    s.push_str(&format!(
        "  {} projeto(s) no hub, {} atribuição(ões), config_rev {}\n",
        rep.projects, rep.assignments, rep.config_rev
    ));
    s.push_str(&format!(
        "  tokens de sync: {}\n",
        if rep.env_tokens.is_empty() {
            "nenhum".to_string()
        } else {
            rep.env_tokens.join(", ")
        }
    ));
    if let Some(p) = &rep.hub_config {
        s.push_str(&format!("config do cthubd: {}\n", p.display()));
    }
    s.push_str(&format!(
        "token de UI do hub: {}\n",
        dirs.ui_token_file().display()
    ));
    if let Some((name, id)) = &rep.local_env {
        s.push_str(&format!(
            "ctowerd desta máquina: ambiente \"{name}\"{} com token de sync em {}; \
             hub_url = \"http://127.0.0.1:{port}\" (o config antigo ficou em {})\n",
            id.as_deref()
                .map(|i| format!(" (env_id {i})"))
                .unwrap_or_default(),
            o.ctower_config.join("hub.token").display(),
            o.ctower_config.join("config.toml.pre-cthub").display()
        ));
    }
    for w in &rep.warnings {
        s.push_str(&format!("aviso: {w}\n"));
    }
    s.push_str(&format!(
        "\nPróximos passos (docs/hub.md, seção 5):\n\
         \x20 1. suba o cthubd: systemctl --user enable --now cthubd\n\
         \x20 2. atualize e suba o ctowerd desta máquina: ctower update (guarda o binário\n\
         \x20    anterior para ctower update --rollback)\n\
         \x20 3. nos outros ambientes, troque a porta de hub_url para {port} e rode ctower restart\n\
         \x20 4. confira: ctower hub sync em cada ambiente e, na UI (Configurações → Hub, com o\n\
         \x20    token de UI acima), os ambientes com 0 pendências e as atribuições aplicadas\n\
         Até conferir o passo 4, não use sessões nesta máquina: se precisar voltar atrás, o que\n\
         elas gravarem não chega ao hub antigo.\n\
         Para voltar atrás: pare o cthubd, copie {} sobre o config.toml do ctowerd e suba o\n\
         ctowerd antigo (ctower update --rollback, se já o atualizou). O hub.db em {} não mudou;\n\
         para migrar de novo depois de usar o hub antigo, rode com --force.\n",
        o.ctower_config.join("config.toml.pre-cthub").display(),
        o.from.display()
    ));
    s
}
