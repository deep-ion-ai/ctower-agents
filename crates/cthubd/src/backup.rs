//! Snapshots do `hub.db` (spec do hub como plano de controle §6.5): o hub é
//! a única cópia do histórico, então o backup é crítico e os problemas dele
//! viram avisos em `hub.status`.

use crate::remote::{self, Credentials, RemoteKind, RemoteSettings};
use crate::secrets::HubSecrets;
use ct_ledger::SharedHub;
use serde::{Deserialize, Serialize};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, UNIX_EPOCH};
use tokio::sync::Notify;

/// Estado dos snapshots para `hub.info` e `hub.status`.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct BackupStatus {
    pub dir: String,
    /// `0` = snapshots desligados.
    pub interval_hours: u64,
    pub keep: usize,
    /// Snapshot mais novo em `dir` (mtime, ms).
    pub newest_at: Option<i64>,
    pub last_ok_at: Option<i64>,
    pub last_error: Option<String>,
    /// `dir` está no mesmo disco do `hub.db`.
    pub same_device: bool,
    /// Destino remoto (#263); `none` = só o disco local.
    pub remote: RemoteStatus,
}

/// Estado do envio remoto para `hub.info`/`hub.status`/`hub.backup.get`.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct RemoteStatus {
    pub kind: RemoteKind,
    /// Onde os arquivos vão (`s3://bucket/prefixo`, pasta do Drive); sem credenciais.
    pub target: String,
    pub encrypt: bool,
    /// Credenciais do destino já gravadas (nunca o valor).
    pub credentials_configured: bool,
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub last_ok_at: Option<i64>,
    pub last_name: Option<String>,
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub last_size: Option<u64>,
    pub last_error: Option<String>,
    /// Falhas seguidas; próximo reenvio com backoff.
    pub failures: u32,
}

/// Estado do backup dos docs (#442) para `hub.backup.get|run`, separado
/// do `hub.db`: pacote completo de `docs_dir` no `backup_dir` e no mesmo
/// destino remoto.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct DocsBackupStatus {
    /// `docs_dir` do hub; `None` = docs desligados (o backup não roda).
    pub docs_dir: Option<String>,
    /// `0` = backup dos docs desligado.
    pub interval_hours: u64,
    pub keep: usize,
    /// Tamanho máximo dos docs (soma dos arquivos) para o pacote rodar.
    pub max_mb: u64,
    /// Pacote mais novo no `backup_dir` (mtime, ms) e o nome dele.
    pub newest_at: Option<i64>,
    pub newest_name: Option<String>,
    pub last_ok_at: Option<i64>,
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub last_size: Option<u64>,
    pub last_error: Option<String>,
    /// Por que a última rodada não gerou pacote (pasta vazia ou ausente,
    /// acima do limite); `None` se gerou ou falhou.
    pub last_skipped: Option<String>,
    /// Tamanho dos docs na última rodada que passou do limite.
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub too_large_bytes: Option<u64>,
    /// Envio ao destino remoto do backup (o mesmo do `hub.db`).
    pub remote: DocsRemoteStatus,
}

/// Último envio do pacote dos docs ao destino remoto.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct DocsRemoteStatus {
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub last_ok_at: Option<i64>,
    pub last_name: Option<String>,
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub last_size: Option<u64>,
    pub last_error: Option<String>,
    pub failures: u32,
}

/// O que uma rodada do backup dos docs fez.
#[derive(Debug, Clone, PartialEq)]
pub enum DocsRun {
    /// Pacote gerado (e os antigos removidos pela retenção).
    Done { path: PathBuf, removed: usize },
    /// Não rodou: sem `docs_dir`, pasta vazia/ausente ou acima do limite.
    Skipped(String),
}

/// Aviso de `hub.status`. `kind` é estável para a UI: os do backup
/// (inclusive os dos docs: `docs_backup_failed`, `docs_backup_stale`,
/// `docs_backup_too_large`),
/// `push_quarantine` (um por projeto com linhas em quarentena), os da chave
/// de recuperação (`recovery_not_configured`, `recovery_reverify_due`) e os
/// dos snapshots de secrets (`secrets_snapshot_stale`,
/// `secrets_snapshot_unreachable`, `secrets_snapshot_same_device`) e
/// `secrets_rotate_pending` (secrets a trocar na origem depois de restaurar
/// com a chave de recuperação, #170).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Warning {
    #[cfg_attr(feature = "ts", ts(type = "string"))]
    pub kind: &'static str,
    pub message: String,
}

#[derive(Default)]
struct Last {
    ok_at: Option<i64>,
    error: Option<String>,
    remote: RemoteLast,
    docs: DocsLast,
}

#[derive(Default)]
struct DocsLast {
    ok_at: Option<i64>,
    size: Option<u64>,
    error: Option<String>,
    skipped: Option<String>,
    too_large: Option<u64>,
    /// A última rodada achou a pasta vazia ou ausente: sem aviso de atraso.
    empty: bool,
    remote: RemoteLast,
}

#[derive(Default)]
struct RemoteLast {
    ok_at: Option<i64>,
    name: Option<String>,
    size: Option<u64>,
    error: Option<String>,
    /// Erro de autenticação: vira o aviso de credencial inválida/revogada.
    credential: bool,
    failures: u32,
}

#[derive(Clone)]
struct Settings {
    dir: PathBuf,
    every: Duration,
    keep: usize,
    same_device: bool,
    remote: RemoteSettings,
    docs: DocsSettings,
}

/// Backup dos docs (#442): `dir` vem do `docs_dir` do config.toml; o resto
/// também do `backup.json`.
#[derive(Clone)]
struct DocsSettings {
    dir: Option<PathBuf>,
    every: Duration,
    keep: usize,
    max_mb: u64,
}

impl Default for DocsSettings {
    fn default() -> Self {
        DocsSettings {
            dir: None,
            every: Duration::from_secs(DEFAULT_DOCS_INTERVAL_HOURS * 3600),
            keep: DEFAULT_DOCS_KEEP,
            max_mb: DEFAULT_DOCS_MAX_MB,
        }
    }
}

pub const DEFAULT_DOCS_INTERVAL_HOURS: u64 = 24;
pub const DEFAULT_DOCS_KEEP: usize = 7;
pub const DEFAULT_DOCS_MAX_MB: u64 = 1024;
/// Teto do `docs_backup_max_mb`: o pacote vai inteiro para a memória no
/// envio (`Remote::put`/`seal`) e o download tem teto de 4 GiB.
const MAX_DOCS_MAX_MB: u64 = 4096;

/// `backup.json` (em `<config>`): o que a UI configura, por cima do
/// `config.toml`, que segue valendo como fallback. Sem credenciais: essas
/// vão para o store de secrets.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(default)]
pub struct BackupUpdate {
    pub dir: Option<String>,
    pub interval_hours: Option<u64>,
    pub keep: Option<usize>,
    pub remote: Option<RemoteSettings>,
    /// Backup dos docs (#442); ausentes = o config.toml.
    pub docs_interval_hours: Option<u64>,
    pub docs_keep: Option<usize>,
    pub docs_max_mb: Option<u64>,
}

struct RemoteCtx {
    file: PathBuf,
    secrets: Arc<HubSecrets>,
}

pub struct Backups {
    settings: Mutex<Settings>,
    db: Option<PathBuf>,
    started: Instant,
    last: Mutex<Last>,
    ctx: Option<RemoteCtx>,
    /// Nomes das credenciais já gravadas. `status()` roda com o hub pego
    /// (`hub.status`) e o lock do store vem antes do do hub: por isso um
    /// cache, em vez de perguntar ao store.
    have: Mutex<std::collections::HashSet<&'static str>>,
    /// Acorda o laço periódico quando a configuração muda.
    wake: Notify,
    /// O mesmo para o laço do backup dos docs.
    docs_wake: Notify,
}

/// `dir` gravado no `backup.json` (`file`), se houver e não for vazio. Para
/// quem precisa do `backup_dir` em vigor sem um [`Backups`] (snapshots de
/// secrets no start e nos comandos com o serviço parado).
pub fn persisted_dir(file: &Path) -> Option<PathBuf> {
    let text = std::fs::read_to_string(file).ok()?;
    let u: BackupUpdate = serde_json::from_str(&text).ok()?;
    let d = u.dir?;
    let d = d.trim();
    (!d.is_empty()).then(|| crate::config::expand_home(d))
}

fn remote_fresh(st: &Settings, last: &Last) -> bool {
    let limit = st.every * 2;
    st.remote.kind != RemoteKind::None
        && last.remote.error.is_none()
        && last
            .remote
            .ok_at
            .is_some_and(|at| ct_core::now_ms().saturating_sub(at) <= limit.as_millis() as i64)
}

/// Teto do backoff de reenvio remoto, além do próprio intervalo.
const BACKOFF_BASE: Duration = Duration::from_secs(300);

impl Backups {
    /// `every` zero desliga. `db` = caminho do `hub.db` (`None` em memória).
    pub fn new(dir: PathBuf, every: Duration, keep: usize, db: Option<&Path>) -> Self {
        let same_device = db.is_some_and(|db| same_device(db, &dir));
        Backups {
            settings: Mutex::new(Settings {
                dir,
                every,
                keep,
                same_device,
                remote: RemoteSettings::default(),
                docs: DocsSettings::default(),
            }),
            db: db.map(Path::to_path_buf),
            started: Instant::now(),
            last: Mutex::new(Last::default()),
            ctx: None,
            have: Mutex::default(),
            wake: Notify::new(),
            docs_wake: Notify::new(),
        }
    }

    /// Liga o backup dos docs de `dir` (`docs_dir`; `None` = não roda).
    /// Chame antes de [`Backups::with_remote`], que aplica o `backup.json`
    /// por cima.
    pub fn with_docs(
        self,
        dir: Option<PathBuf>,
        every: Duration,
        keep: usize,
        max_mb: u64,
    ) -> Self {
        self.settings.lock().unwrap_or_else(|e| e.into_inner()).docs = DocsSettings {
            dir,
            every,
            keep,
            max_mb,
        };
        self
    }

    /// Snapshots desligados (testes, hub em memória).
    pub fn disabled() -> Self {
        Backups::new(PathBuf::new(), Duration::ZERO, 0, None)
    }

    /// Liga o destino remoto: `file` = `backup.json` e o store onde ficam as
    /// credenciais. Lê o `backup.json` se existir (ele vale sobre o config).
    pub fn with_remote(mut self, file: PathBuf, secrets: Arc<HubSecrets>) -> Self {
        for kind in [RemoteKind::S3, RemoteKind::B2, RemoteKind::Drive] {
            for f in remote::credential_fields(kind) {
                if let Some(n) = remote::secret_name(kind, f) {
                    if secrets.get_hub_only(n).is_some() {
                        self.have
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .insert(n);
                    }
                }
            }
        }
        self.ctx = Some(RemoteCtx { file, secrets });
        match self.read_persisted() {
            Ok(Some(u)) => {
                if let Err(e) = self.apply_in_memory(&u) {
                    tracing::warn!("backup.json ignorado: {e}");
                }
            }
            Ok(None) => {}
            Err(e) => tracing::warn!("backup.json ilegível, usando o config.toml: {e}"),
        }
        self
    }

    fn s(&self) -> Settings {
        self.settings
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn enabled(&self) -> bool {
        !self.s().every.is_zero()
    }

    /// `backup_dir`; vazio em `Backups::disabled()`. Também recebe o
    /// snapshot fora da rotação antes de excluir um projeto.
    pub fn dir(&self) -> PathBuf {
        self.s().dir
    }

    pub fn same_device(&self) -> bool {
        self.s().same_device
    }

    fn read_persisted(&self) -> Result<Option<BackupUpdate>, String> {
        let Some(c) = &self.ctx else { return Ok(None) };
        match std::fs::read_to_string(&c.file) {
            Ok(t) => serde_json::from_str(&t)
                .map(Some)
                .map_err(|e| e.to_string()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }

    /// Valida e aplica `u` na memória (sem gravar nada).
    fn apply_in_memory(&self, u: &BackupUpdate) -> Result<(), String> {
        let mut n = self.s();
        if let Some(d) = &u.dir {
            let d = d.trim();
            if !d.is_empty() {
                n.dir = crate::config::expand_home(d);
            }
        }
        if let Some(h) = u.interval_hours {
            if h > 24 * 90 {
                return Err("intervalo acima de 90 dias".into());
            }
            n.every = Duration::from_secs(h * 3600);
        }
        if let Some(k) = u.keep {
            if k == 0 {
                return Err("retenção precisa de pelo menos 1 snapshot".into());
            }
            n.keep = k;
        }
        if let Some(r) = &u.remote {
            n.remote = r.clone();
        }
        if let Some(h) = u.docs_interval_hours {
            if h > 24 * 90 {
                return Err("intervalo do backup dos docs acima de 90 dias".into());
            }
            n.docs.every = Duration::from_secs(h * 3600);
        }
        if let Some(k) = u.docs_keep {
            if k == 0 {
                return Err("retenção dos docs precisa de pelo menos 1 pacote".into());
            }
            n.docs.keep = k;
        }
        if let Some(m) = u.docs_max_mb {
            if m == 0 || m > MAX_DOCS_MAX_MB {
                return Err(format!(
                    "limite do backup dos docs precisa ficar entre 1 e {MAX_DOCS_MAX_MB} MB"
                ));
            }
            n.docs.max_mb = m;
        }
        n.same_device = self.db.as_deref().is_some_and(|db| same_device(db, &n.dir));
        *self.settings.lock().unwrap_or_else(|e| e.into_inner()) = n;
        Ok(())
    }

    /// Credenciais do destino lidas do store.
    pub fn credentials(&self, kind: RemoteKind) -> Credentials {
        let mut c = Credentials::default();
        let Some(ctx) = &self.ctx else { return c };
        for f in remote::credential_fields(kind) {
            let v = remote::secret_name(kind, f)
                .and_then(|n| ctx.secrets.get_hub_only(n))
                .unwrap_or_default();
            match *f {
                "access_key" => c.access_key = v,
                "secret_key" => c.secret_key = v,
                "client_id" => c.client_id = v,
                "client_secret" => c.client_secret = v,
                _ => c.refresh_token = v,
            }
        }
        c
    }

    fn credentials_configured(&self, kind: RemoteKind) -> bool {
        let have = self.have.lock().unwrap_or_else(|e| e.into_inner());
        let fields = remote::credential_fields(kind);
        !fields.is_empty()
            && fields
                .iter()
                .all(|f| remote::secret_name(kind, f).is_some_and(|n| have.contains(n)))
    }

    /// Credenciais que a UI mandou (`campo → valor`); o que vier vazio ou
    /// faltar mantém o valor guardado. Grava no store de secrets.
    pub fn set_credentials(
        &self,
        kind: RemoteKind,
        fields: &std::collections::BTreeMap<String, String>,
    ) -> Result<(), String> {
        let ctx = self.ctx.as_ref().ok_or("store de secrets indisponível")?;
        for (f, v) in fields {
            let name = remote::secret_name(kind, f)
                .ok_or_else(|| format!("campo de credencial desconhecido: {f}"))?;
            if v.is_empty() {
                continue;
            }
            ctx.secrets
                .set_hub_only(name, v)
                .map_err(|e| e.to_string())?;
            self.have
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(name);
        }
        Ok(())
    }

    /// Credenciais para um teste: as enviadas por cima das guardadas.
    pub fn merged_credentials(
        &self,
        kind: RemoteKind,
        fields: &std::collections::BTreeMap<String, String>,
    ) -> Credentials {
        let mut c = self.credentials(kind);
        for (f, v) in fields.iter().filter(|(_, v)| !v.is_empty()) {
            match f.as_str() {
                "access_key" => c.access_key = v.clone(),
                "secret_key" => c.secret_key = v.clone(),
                "client_id" => c.client_id = v.clone(),
                "client_secret" => c.client_secret = v.clone(),
                "refresh_token" => c.refresh_token = v.clone(),
                _ => {}
            }
        }
        c
    }

    /// Aplica e grava a configuração da UI. A validação do destino (campos
    /// e credenciais) vem antes de gravar qualquer coisa.
    pub fn update(&self, u: &BackupUpdate) -> Result<(), String> {
        let ctx = self
            .ctx
            .as_ref()
            .ok_or("configuração do backup indisponível")?;
        let before = self.s();
        self.apply_in_memory(u)?;
        let after = self.s();
        let check = (|| {
            if after.remote.kind != RemoteKind::None {
                remote::open(&after.remote, &self.credentials(after.remote.kind)).map(|_| ())?;
            }
            // Persiste o que está em vigor, não só o delta.
            let all = BackupUpdate {
                dir: Some(after.dir.display().to_string()),
                interval_hours: Some(after.every.as_secs() / 3600),
                keep: Some(after.keep),
                remote: Some(after.remote.clone()),
                docs_interval_hours: Some(after.docs.every.as_secs() / 3600),
                docs_keep: Some(after.docs.keep),
                docs_max_mb: Some(after.docs.max_mb),
            };
            let json = serde_json::to_vec_pretty(&all).map_err(|e| e.to_string())?;
            ct_core::files::write_private_atomic(&ctx.file, &json).map_err(|e| e.to_string())
        })();
        if let Err(e) = check {
            *self.settings.lock().unwrap_or_else(|e| e.into_inner()) = before;
            return Err(e);
        }
        if before.remote != after.remote {
            let mut last = self.last.lock().unwrap_or_else(|e| e.into_inner());
            last.remote = RemoteLast::default();
            last.docs.remote = RemoteLast::default();
        }
        self.wake.notify_one();
        self.docs_wake.notify_one();
        if before.dir != after.dir {
            ctx.secrets.follow_backup_dir(&after.dir);
        }
        Ok(())
    }

    /// Escreve e apaga um arquivo de prova no destino (`hub.backup.test`).
    pub fn test_remote(r: &RemoteSettings, c: &Credentials) -> Result<(), String> {
        let store = remote::open(r, c)?;
        let name = format!("ctower-teste-{}.tmp", ct_core::now_ms());
        store.put(&name, b"control-tower: teste de conexao")?;
        let back = store.get(&name)?;
        store.delete(&name)?;
        if back != b"control-tower: teste de conexao" {
            return Err("o destino devolveu um conteúdo diferente do enviado".into());
        }
        Ok(())
    }

    /// Tira um snapshot e roda a rotação; depois tenta o envio remoto, cuja
    /// falha só vira estado (aviso), nunca erro daqui. Síncrono: chame fora
    /// do runtime.
    pub fn run_once(&self, hub: &SharedHub) -> Result<(PathBuf, usize), String> {
        let st = self.s();
        let r = (|| -> Result<(PathBuf, usize), String> {
            ct_core::files::private_dir(&st.dir).map_err(|e| e.to_string())?;
            let p = hub
                .lock()
                .snapshot(&st.dir, ct_core::now_ms())
                .map_err(|e| e.to_string())?;
            let removed =
                ct_ledger::rotate_snapshots(&st.dir, st.keep).map_err(|e| e.to_string())?;
            Ok((p, removed.len()))
        })();
        {
            let mut last = self.last.lock().unwrap_or_else(|e| e.into_inner());
            match &r {
                Ok(_) => {
                    last.ok_at = Some(ct_core::now_ms());
                    last.error = None;
                }
                Err(e) => last.error = Some(e.clone()),
            }
        }
        if let Ok((p, _)) = &r {
            self.upload_remote(p);
        }
        r
    }

    /// Reenvia o snapshot local mais novo (retry depois de uma falha
    /// remota). Sem destino remoto ou sem snapshot, não faz nada.
    pub fn retry_remote(&self) {
        let st = self.s();
        if st.remote.kind == RemoteKind::None {
            return;
        }
        if let Ok(Some(p)) = ct_ledger::list_snapshots(&st.dir).map(|v| v.into_iter().next()) {
            self.upload_remote(&p);
        }
    }

    /// Envia `snapshot` e a cópia do `secrets.age`, e aplica a retenção no
    /// destino. Registra o resultado; nunca propaga erro.
    fn upload_remote(&self, snapshot: &Path) {
        let st = self.s();
        if st.remote.kind == RemoteKind::None {
            return;
        }
        let r = self.try_upload(&st, snapshot);
        let mut last = self.last.lock().unwrap_or_else(|e| e.into_inner());
        match r {
            Ok((name, size)) => {
                last.remote = RemoteLast {
                    ok_at: Some(ct_core::now_ms()),
                    name: Some(name),
                    size: Some(size),
                    ..RemoteLast::default()
                };
            }
            Err(e) => {
                tracing::warn!("hub: envio do backup para o destino remoto falhou: {e}");
                last.remote.credential = is_auth_error(&e);
                last.remote.error = Some(e);
                last.remote.failures += 1;
            }
        }
    }

    fn try_upload(&self, st: &Settings, snapshot: &Path) -> Result<(String, u64), String> {
        let store = remote::open(&st.remote, &self.credentials(st.remote.kind))?;
        let recipients = self
            .ctx
            .as_ref()
            .map(|c| c.secrets.backup_recipients())
            .unwrap_or_default();
        let file = snapshot
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or("nome de snapshot inválido")?;
        let mut data = std::fs::read(snapshot).map_err(|e| e.to_string())?;
        if st.remote.encrypt {
            if recipients.is_empty() {
                return Err("sem chave para cifrar o backup".into());
            }
            data = remote::seal(&data, &recipients)?;
        }
        let name = remote::remote_name(file, st.remote.encrypt);
        store.put(&name, &data)?;
        let size = data.len() as u64;
        // Cópia do store de secrets (já cifrado para o hub e a recuperação).
        if let Some(c) = &self.ctx {
            if let Ok(bytes) = std::fs::read(c.secrets.store_path()) {
                let stamp = ct_core::utc_stamp(ct_core::now_ms());
                store.put(&format!("secrets-{stamp}.age"), &bytes)?;
            }
        }
        // Retenção também no destino; uma falha aqui não invalida o envio.
        match store.list() {
            Ok(objs) => {
                for prefix in ["hub-", "secrets-"] {
                    let mut o: Vec<_> =
                        objs.iter().filter(|o| o.name.starts_with(prefix)).collect();
                    o.sort_by(|a, b| b.name.cmp(&a.name));
                    for old in o.into_iter().skip(st.keep.max(1)) {
                        if let Err(e) = store.delete(&old.name) {
                            tracing::warn!("retenção no destino remoto: {}: {e}", old.name);
                        }
                    }
                }
            }
            Err(e) => tracing::warn!("retenção no destino remoto: listagem falhou: {e}"),
        }
        Ok((name, size))
    }

    /// Idade do snapshot mais novo em `dir`.
    fn newest(&self) -> Option<(i64, Duration)> {
        let p = ct_ledger::list_snapshots(&self.dir())
            .ok()?
            .into_iter()
            .next()?;
        let m = std::fs::metadata(p).ok()?.modified().ok()?;
        let at = m.duration_since(UNIX_EPOCH).ok()?.as_millis() as i64;
        Some((at, m.elapsed().unwrap_or_default()))
    }

    fn remote_target(r: &RemoteSettings) -> String {
        match r.kind {
            RemoteKind::None => String::new(),
            RemoteKind::Drive => format!("drive:{}", r.drive_folder_id),
            RemoteKind::S3 | RemoteKind::B2 => {
                let p = r.prefix.trim().trim_matches('/');
                let host = r.endpoint.trim();
                let base = if host.is_empty() {
                    format!("s3://{}", r.bucket)
                } else {
                    format!("{host}/{}", r.bucket)
                };
                if p.is_empty() {
                    base
                } else {
                    format!("{base}/{p}")
                }
            }
        }
    }

    pub fn status(&self) -> BackupStatus {
        let st = self.s();
        let last = self.last.lock().unwrap_or_else(|e| e.into_inner());
        BackupStatus {
            dir: st.dir.display().to_string(),
            interval_hours: st.every.as_secs() / 3600,
            keep: st.keep,
            newest_at: self.newest().map(|(at, _)| at),
            last_ok_at: last.ok_at,
            last_error: last.error.clone(),
            same_device: st.same_device,
            remote: RemoteStatus {
                kind: st.remote.kind,
                target: Self::remote_target(&st.remote),
                encrypt: st.remote.encrypt,
                credentials_configured: self.credentials_configured(st.remote.kind),
                last_ok_at: last.remote.ok_at,
                last_name: last.remote.name.clone(),
                last_size: last.remote.size,
                last_error: last.remote.error.clone(),
                failures: last.remote.failures,
            },
        }
    }

    /// Configuração atual, para a UI preencher o formulário.
    pub fn settings_view(&self) -> RemoteSettings {
        self.s().remote
    }

    /// O último envio ao destino remoto deu certo e tem menos de 2 × o
    /// intervalo. Ele leva o `hub.db` e o `secrets.age` para fora da
    /// máquina: com ele em dia, snapshot local no mesmo disco não alarma.
    pub fn remote_fresh(&self) -> bool {
        let st = self.s();
        let last = self.last.lock().unwrap_or_else(|e| e.into_inner());
        remote_fresh(&st, &last)
    }

    /// Avisos do backup: desligado, no mesmo disco do `hub.db` (sem destino
    /// remoto em dia), último snapshot falhou, o mais novo passou de 2 × o
    /// intervalo, e os do envio remoto.
    pub fn warnings(&self) -> Vec<Warning> {
        let mut out = self.hub_warnings();
        out.extend(self.docs_warnings());
        out
    }

    fn hub_warnings(&self) -> Vec<Warning> {
        let st = self.s();
        if st.every.is_zero() {
            return vec![Warning {
                kind: "backup_disabled",
                message: "snapshots do hub.db desligados (backup_interval_hours = 0): \
                          o hub é a única cópia do histórico"
                    .into(),
            }];
        }
        let limit = st.every * 2;
        let last = self.last.lock().unwrap_or_else(|e| e.into_inner());
        let remote_on = st.remote.kind != RemoteKind::None;
        let mut out = Vec::new();
        if st.same_device && !remote_fresh(&st, &last) {
            out.push(Warning {
                kind: "backup_same_device",
                message: format!(
                    "backup no mesmo disco do hub: {} (ponha backup_dir em outro disco \
                     ou configure um destino remoto)",
                    st.dir.display()
                ),
            });
        }
        if let Some(e) = &last.error {
            out.push(Warning {
                kind: "backup_failed",
                message: format!("último snapshot do hub.db falhou: {e}"),
            });
        }
        if remote_on {
            if let Some(e) = &last.remote.error {
                let (kind, what) = if last.remote.credential {
                    (
                        "backup_remote_credentials",
                        "credencial do destino remoto inválida ou expirada",
                    )
                } else {
                    (
                        "backup_remote_failed",
                        "envio do backup para o destino remoto falhando",
                    )
                };
                out.push(Warning {
                    kind,
                    message: format!("{what}: {e}"),
                });
            } else {
                let stale = match last.remote.ok_at {
                    Some(at) => ct_core::now_ms().saturating_sub(at) > limit.as_millis() as i64,
                    None => self.started.elapsed() > limit,
                };
                if stale {
                    out.push(Warning {
                        kind: "backup_remote_stale",
                        message: format!(
                            "nenhum envio do backup ao destino remoto nas últimas {} h",
                            limit.as_secs() / 3600
                        ),
                    });
                }
            }
            if !st.remote.encrypt {
                out.push(Warning {
                    kind: "backup_remote_unencrypted",
                    message: "o backup remoto sobe sem cifra: o hub.db (histórico dos \
                              projetos) e o pacote dos docs vão em texto claro"
                        .into(),
                });
            }
        }
        drop(last);
        let stale = match self.newest() {
            Some((_, age)) => age > limit,
            None => self.started.elapsed() > limit,
        };
        if stale {
            out.push(Warning {
                kind: "backup_stale",
                message: format!(
                    "nenhum snapshot do hub.db nas últimas {} h",
                    limit.as_secs() / 3600
                ),
            });
        }
        out
    }

    // ───────────────────── backup dos docs (#442) ─────────────────────

    pub fn docs_enabled(&self) -> bool {
        let d = self.s().docs;
        d.dir.is_some() && !d.every.is_zero()
    }

    /// Gera o pacote de `docs_dir`, aplica a retenção local e tenta o envio
    /// remoto (cuja falha só vira estado). Pasta vazia ou ausente não roda
    /// nem gera aviso; acima de `docs_backup_max_mb` não roda e avisa.
    /// Síncrono: chame fora do runtime.
    pub fn run_docs_once(&self) -> Result<DocsRun, String> {
        self.run_docs_at(ct_core::now_ms())
    }

    /// [`Backups::run_docs_once`] com o carimbo do nome em `ms` (testes).
    pub(crate) fn run_docs_at(&self, ms: i64) -> Result<DocsRun, String> {
        let st = self.s();
        let Some(docs) = st.docs.dir.clone() else {
            return Ok(DocsRun::Skipped("docs_dir não configurado".into()));
        };
        let limit = st.docs.max_mb.saturating_mul(1 << 20);
        enum Out {
            Done(PathBuf, usize),
            Empty(String),
            TooLarge(u64),
        }
        let r = (|| -> Result<Out, String> {
            let tree = crate::docs_backup::measure(&docs, &st.dir)
                .map_err(|e| format!("{}: {e}", docs.display()))?;
            let bytes = match tree {
                crate::docs_backup::Tree::Missing => {
                    return Ok(Out::Empty(format!("{} não existe", docs.display())))
                }
                crate::docs_backup::Tree::Empty => {
                    return Ok(Out::Empty(format!("{} está vazia", docs.display())))
                }
                crate::docs_backup::Tree::Files { bytes, .. } => bytes,
            };
            if bytes > limit {
                return Ok(Out::TooLarge(bytes));
            }
            ct_core::files::private_dir(&st.dir).map_err(|e| e.to_string())?;
            let path = st.dir.join(crate::docs_backup::package_name(ms));
            crate::docs_backup::pack(&docs, &st.dir, &path)?;
            let removed = crate::docs_backup::rotate(&st.dir, st.docs.keep)
                .map_err(|e| format!("retenção dos pacotes dos docs: {e}"))?;
            Ok(Out::Done(path, removed.len()))
        })();
        let r = {
            let mut last = self.last.lock().unwrap_or_else(|e| e.into_inner());
            let l = &mut last.docs;
            if r.is_ok() {
                l.error = None;
                l.too_large = None;
                l.empty = false;
                l.skipped = None;
            }
            match r {
                Err(e) => {
                    l.error = Some(e.clone());
                    l.skipped = None;
                    Err(e)
                }
                Ok(Out::TooLarge(bytes)) => {
                    let msg = format!(
                        "docs com {} MB, acima de docs_backup_max_mb ({} MB)",
                        bytes.div_ceil(1 << 20),
                        st.docs.max_mb
                    );
                    l.too_large = Some(bytes);
                    l.skipped = Some(msg.clone());
                    Ok(DocsRun::Skipped(msg))
                }
                Ok(Out::Empty(why)) => {
                    l.empty = true;
                    l.skipped = Some(why.clone());
                    Ok(DocsRun::Skipped(why))
                }
                Ok(Out::Done(path, removed)) => {
                    l.size = std::fs::metadata(&path).ok().map(|m| m.len());
                    l.ok_at = Some(ct_core::now_ms());
                    Ok(DocsRun::Done { path, removed })
                }
            }
        };
        if let Ok(DocsRun::Done { path, .. }) = &r {
            self.upload_docs_remote(path);
        }
        r
    }

    /// Reenvia o pacote local mais novo dos docs depois de uma falha remota.
    pub fn retry_docs_remote(&self) {
        let st = self.s();
        if st.remote.kind == RemoteKind::None || st.docs.dir.is_none() {
            return;
        }
        if let Ok(Some(p)) =
            crate::docs_backup::list_packages(&st.dir).map(|v| v.into_iter().next())
        {
            self.upload_docs_remote(&p);
        }
    }

    fn upload_docs_remote(&self, package: &Path) {
        let st = self.s();
        if st.remote.kind == RemoteKind::None {
            return;
        }
        let r = self.try_upload_docs(&st, package);
        let mut last = self.last.lock().unwrap_or_else(|e| e.into_inner());
        match r {
            Ok((name, size)) => {
                last.docs.remote = RemoteLast {
                    ok_at: Some(ct_core::now_ms()),
                    name: Some(name),
                    size: Some(size),
                    ..RemoteLast::default()
                };
            }
            Err(e) => {
                tracing::warn!("hub: envio do pacote dos docs para o destino remoto falhou: {e}");
                last.docs.remote.credential = is_auth_error(&e);
                last.docs.remote.error = Some(e);
                last.docs.remote.failures += 1;
            }
        }
    }

    /// Envia o pacote (cifrado como `docs-<...>.tar.zst.age`, nas mesmas
    /// regras do `hub.db`) e aplica a retenção do prefixo `docs-` no destino.
    fn try_upload_docs(&self, st: &Settings, package: &Path) -> Result<(String, u64), String> {
        let store = remote::open(&st.remote, &self.credentials(st.remote.kind))?;
        let file = package
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or("nome de pacote inválido")?;
        let mut data = std::fs::read(package).map_err(|e| e.to_string())?;
        if st.remote.encrypt {
            let recipients = self
                .ctx
                .as_ref()
                .map(|c| c.secrets.backup_recipients())
                .unwrap_or_default();
            if recipients.is_empty() {
                return Err("sem chave para cifrar o backup".into());
            }
            data = remote::seal(&data, &recipients)?;
        }
        let name = remote::remote_name(file, st.remote.encrypt);
        store.put(&name, &data)?;
        let size = data.len() as u64;
        drop(data);
        match store.list() {
            Ok(objs) => {
                for old in remote::docs_packages(objs)
                    .into_iter()
                    .skip(st.docs.keep.max(1))
                {
                    if let Err(e) = store.delete(&old.name) {
                        tracing::warn!("retenção dos docs no destino remoto: {}: {e}", old.name);
                    }
                }
            }
            Err(e) => tracing::warn!("retenção dos docs no destino remoto: listagem falhou: {e}"),
        }
        Ok((name, size))
    }

    /// Pacote mais novo no `backup_dir`: nome, mtime (ms) e idade.
    fn docs_newest(&self) -> Option<(String, i64, Duration)> {
        let p = crate::docs_backup::list_packages(&self.dir())
            .ok()?
            .into_iter()
            .next()?;
        let m = std::fs::metadata(&p).ok()?.modified().ok()?;
        let at = m.duration_since(UNIX_EPOCH).ok()?.as_millis() as i64;
        let name = p.file_name()?.to_string_lossy().into_owned();
        Some((name, at, m.elapsed().unwrap_or_default()))
    }

    #[cfg(test)]
    fn update_docs_every_for_test(&self, every: Duration) {
        self.settings
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .docs
            .every = every;
    }

    pub fn docs_status(&self) -> DocsBackupStatus {
        let st = self.s();
        let newest = self.docs_newest();
        let last = self.last.lock().unwrap_or_else(|e| e.into_inner());
        let l = &last.docs;
        DocsBackupStatus {
            docs_dir: st.docs.dir.as_ref().map(|d| d.display().to_string()),
            interval_hours: st.docs.every.as_secs() / 3600,
            keep: st.docs.keep,
            max_mb: st.docs.max_mb,
            newest_at: newest.as_ref().map(|n| n.1),
            newest_name: newest.map(|n| n.0),
            last_ok_at: l.ok_at,
            last_size: l.size,
            last_error: l.error.clone(),
            last_skipped: l.skipped.clone(),
            too_large_bytes: l.too_large,
            remote: DocsRemoteStatus {
                last_ok_at: l.remote.ok_at,
                last_name: l.remote.name.clone(),
                last_size: l.remote.size,
                last_error: l.remote.error.clone(),
                failures: l.remote.failures,
            },
        }
    }

    /// Avisos do backup dos docs: falhou (local ou envio), acima do limite e
    /// nenhum pacote em 2 × o intervalo. Sem `docs_dir`, desligado ou com a
    /// pasta vazia/ausente, nada.
    fn docs_warnings(&self) -> Vec<Warning> {
        let st = self.s();
        if st.docs.dir.is_none() || st.docs.every.is_zero() {
            return Vec::new();
        }
        let limit = st.docs.every * 2;
        let newest_age = self.docs_newest().map(|n| n.2);
        let last = self.last.lock().unwrap_or_else(|e| e.into_inner());
        let l = &last.docs;
        let mut out = Vec::new();
        if let Some(e) = &l.error {
            out.push(Warning {
                kind: "docs_backup_failed",
                message: format!("último backup dos docs falhou: {e}"),
            });
        } else if st.remote.kind != RemoteKind::None {
            if let Some(e) = &l.remote.error {
                out.push(Warning {
                    kind: "docs_backup_failed",
                    message: format!("envio do pacote dos docs ao destino remoto falhando: {e}"),
                });
            }
        }
        if let Some(bytes) = l.too_large {
            out.push(Warning {
                kind: "docs_backup_too_large",
                message: format!(
                    "backup dos docs não rodou: {} MB, acima de docs_backup_max_mb ({} MB)",
                    bytes.div_ceil(1 << 20),
                    st.docs.max_mb
                ),
            });
        } else if !l.empty {
            let stale = match newest_age {
                Some(age) => age > limit,
                None => self.started.elapsed() > limit,
            };
            if stale {
                out.push(Warning {
                    kind: "docs_backup_stale",
                    message: format!(
                        "nenhum pacote dos docs nas últimas {} h",
                        limit.as_secs() / 3600
                    ),
                });
            }
        }
        out
    }
}

/// O erro vem de autenticação/autorização do provedor.
fn is_auth_error(e: &str) -> bool {
    e.contains("HTTP 401") || e.contains("HTTP 403") || e.contains("credencial")
}

/// `dir` (ou o ancestral mais próximo que já existe) e `db` no mesmo
/// dispositivo (`st_dev`). Também serve aos snapshots de secrets (#168).
pub(crate) fn same_device(db: &Path, dir: &Path) -> bool {
    let Ok(a) =
        std::fs::metadata(db).or_else(|_| std::fs::metadata(db.parent().unwrap_or(Path::new("/"))))
    else {
        return false;
    };
    let mut d = Some(dir);
    while let Some(p) = d {
        if let Ok(b) = std::fs::metadata(p) {
            return a.dev() == b.dev();
        }
        d = p.parent();
    }
    false
}

/// Backoff do reenvio remoto: 5 min, 10, 20... até o intervalo.
fn backoff(failures: u32, every: Duration) -> Duration {
    (BACKOFF_BASE * 2u32.saturating_pow(failures.saturating_sub(1).min(10))).min(every)
}

/// Snapshot a cada intervalo. No boot, só tira um se o último é mais velho
/// que o intervalo. Falha no envio remoto reagenda só o envio, com backoff,
/// sem atrasar o próximo snapshot local. A configuração pode mudar com o
/// laço rodando (`Backups::update` acorda ele).
pub fn spawn_periodic(hub: SharedHub, backups: Arc<Backups>) {
    tokio::spawn(async move {
        use tokio::time::{sleep_until, Instant as TInstant};
        let first = {
            let b = Arc::clone(&backups);
            tokio::task::spawn_blocking(move || b.newest().map(|(_, age)| age))
                .await
                .ok()
                .flatten()
        };
        let mut next_snap = match first.filter(|a| *a < backups.s().every) {
            Some(age) => Some(TInstant::now() + (backups.s().every - age)),
            None => Some(TInstant::now()),
        };
        let mut next_retry: Option<TInstant> = None;
        loop {
            let every = backups.s().every;
            if every.is_zero() {
                // Desligado: espera uma mudança de configuração.
                backups.wake.notified().await;
                next_snap = Some(TInstant::now());
                continue;
            }
            let due = [next_snap, next_retry].into_iter().flatten().min();
            let Some(due) = due else {
                backups.wake.notified().await;
                continue;
            };
            tokio::select! {
                _ = sleep_until(due) => {}
                _ = backups.wake.notified() => {
                    // Configuração nova: reavalia já (inclusive um destino
                    // remoto recém-criado).
                    next_snap = Some(next_snap.unwrap_or_else(TInstant::now).min(TInstant::now() + every));
                    next_retry = backups.s().remote.kind.ne(&RemoteKind::None).then(TInstant::now);
                    continue;
                }
            }
            let now = TInstant::now();
            let snap_due = next_snap.is_some_and(|t| t <= now);
            let b = Arc::clone(&backups);
            if snap_due {
                let h = hub.clone();
                match tokio::task::spawn_blocking(move || b.run_once(&h)).await {
                    Ok(Ok((p, n))) => tracing::info!(
                        "hub: snapshot em {} ({n} antigo(s) removido(s))",
                        p.display()
                    ),
                    Ok(Err(e)) => tracing::warn!("hub: snapshot falhou: {e}"),
                    Err(e) => tracing::warn!("hub: snapshot falhou: {e}"),
                }
                next_snap = Some(TInstant::now() + every);
            } else {
                let _ = tokio::task::spawn_blocking(move || b.retry_remote()).await;
            }
            let (kind, failed, n) = {
                let l = backups.last.lock().unwrap_or_else(|e| e.into_inner());
                (
                    backups.s().remote.kind,
                    l.remote.error.is_some(),
                    l.remote.failures,
                )
            };
            next_retry =
                (kind != RemoteKind::None && failed).then(|| TInstant::now() + backoff(n, every));
        }
    });
}

/// Pacote dos docs a cada intervalo (#442), no mesmo esquema do laço do
/// `hub.db`: no boot só gera se o mais novo passou do intervalo; falha no
/// envio reagenda só o envio, com backoff. Sem `docs_dir`, não sobe.
pub fn spawn_docs_periodic(backups: Arc<Backups>) {
    if backups.s().docs.dir.is_none() {
        return;
    }
    tokio::spawn(async move {
        use tokio::time::{sleep_until, Instant as TInstant};
        let first = {
            let b = Arc::clone(&backups);
            tokio::task::spawn_blocking(move || b.docs_newest().map(|n| n.2))
                .await
                .ok()
                .flatten()
        };
        let every = backups.s().docs.every;
        let mut next_run = match first.filter(|a| *a < every) {
            Some(age) => Some(TInstant::now() + (every - age)),
            None => Some(TInstant::now()),
        };
        let mut next_retry: Option<TInstant> = None;
        loop {
            let every = backups.s().docs.every;
            if every.is_zero() {
                backups.docs_wake.notified().await;
                next_run = Some(TInstant::now());
                continue;
            }
            let Some(due) = [next_run, next_retry].into_iter().flatten().min() else {
                backups.docs_wake.notified().await;
                continue;
            };
            tokio::select! {
                _ = sleep_until(due) => {}
                _ = backups.docs_wake.notified() => {
                    next_run = Some(next_run.unwrap_or_else(TInstant::now).min(TInstant::now() + every));
                    next_retry = backups.s().remote.kind.ne(&RemoteKind::None).then(TInstant::now);
                    continue;
                }
            }
            let now = TInstant::now();
            let b = Arc::clone(&backups);
            if next_run.is_some_and(|t| t <= now) {
                match tokio::task::spawn_blocking(move || b.run_docs_once()).await {
                    Ok(Ok(DocsRun::Done { path, removed })) => tracing::info!(
                        "hub: pacote dos docs em {} ({removed} antigo(s) removido(s))",
                        path.display()
                    ),
                    Ok(Ok(DocsRun::Skipped(why))) => {
                        tracing::info!("hub: backup dos docs não rodou: {why}")
                    }
                    Ok(Err(e)) => tracing::warn!("hub: backup dos docs falhou: {e}"),
                    Err(e) => tracing::warn!("hub: backup dos docs falhou: {e}"),
                }
                next_run = Some(TInstant::now() + every);
            } else {
                let _ = tokio::task::spawn_blocking(move || b.retry_docs_remote()).await;
            }
            let (kind, failed, n) = {
                let l = backups.last.lock().unwrap_or_else(|e| e.into_inner());
                (
                    backups.s().remote.kind,
                    l.docs.remote.error.is_some(),
                    l.docs.remote.failures,
                )
            };
            next_retry =
                (kind != RemoteKind::None && failed).then(|| TInstant::now() + backoff(n, every));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use ct_ledger::Hub;

    #[test]
    fn snapshot_rotacao_e_avisos() {
        let d = tempfile::tempdir().unwrap();
        let db = d.path().join("hub.db");
        let hub = SharedHub::new(Hub::open(&db).unwrap());
        let b = Backups::new(
            d.path().join("backups"),
            Duration::from_secs(3600),
            2,
            Some(&db),
        );
        // Mesmo tempdir = mesmo disco.
        assert!(b.same_device());
        let kinds = |b: &Backups| b.warnings().iter().map(|w| w.kind).collect::<Vec<_>>();
        assert_eq!(kinds(&b), vec!["backup_same_device"]);
        let (p, removed) = b.run_once(&hub).unwrap();
        assert!(p.exists() && removed == 0);
        let st = b.status();
        assert!(st.last_ok_at.is_some() && st.newest_at.is_some() && st.last_error.is_none());
        assert_eq!((st.interval_hours, st.keep), (1, 2));

        // Destino impossível: falha vira aviso.
        std::fs::write(d.path().join("arquivo"), "x").unwrap();
        let ruim = Backups::new(
            d.path().join("arquivo/sub"),
            Duration::from_secs(3600),
            2,
            None,
        );
        assert!(ruim.run_once(&hub).is_err());
        assert!(kinds(&ruim).contains(&"backup_failed"));
        assert!(!ruim.same_device());

        assert_eq!(kinds(&Backups::disabled()), vec!["backup_disabled"]);
    }

    use crate::config::Dirs;
    use crate::remote::{fake, RemoteKind, RemoteSettings};
    use std::collections::BTreeMap;

    /// Hub de teste com store de secrets e um `Backups` com destino S3 falso.
    fn com_remoto(
        d: &Path,
        url: &str,
        keep: usize,
        encrypt: bool,
    ) -> (SharedHub, Backups, Arc<HubSecrets>) {
        let ds = Dirs {
            config: d.join("config"),
            data: d.join("data"),
        };
        ds.ensure().unwrap();
        let hub = SharedHub::new(Hub::open(&ds.hub_db()).unwrap());
        let secrets = Arc::new(HubSecrets::open(&ds, hub.clone()).unwrap());
        let b = Backups::new(
            d.join("snaps"),
            Duration::from_secs(3600),
            keep,
            Some(&ds.hub_db()),
        )
        .with_docs(Some(d.join("docs")), Duration::from_secs(3600), 7, 1)
        .with_remote(ds.config.join("backup.json"), Arc::clone(&secrets));
        let creds: BTreeMap<_, _> = [("access_key", "AK-SECRETA"), ("secret_key", "SK-SECRETA")]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        b.set_credentials(RemoteKind::S3, &creds).unwrap();
        b.update(&BackupUpdate {
            remote: Some(RemoteSettings {
                kind: RemoteKind::S3,
                bucket: "bk".into(),
                prefix: "hub".into(),
                endpoint: url.into(),
                encrypt,
                ..Default::default()
            }),
            ..Default::default()
        })
        .unwrap();
        (hub, b, secrets)
    }

    #[test]
    fn envio_remoto_cifrado_com_retencao_e_sem_vazar_credencial() {
        let d = tempfile::tempdir().unwrap();
        let (url, st) = fake::serve();
        let (hub, b, secrets) = com_remoto(d.path(), &url, 2, true);

        // Três ciclos com nomes de snapshot distintos (o nome tem minuto).
        for i in 0..3 {
            let snap = hub
                .lock()
                .snapshot(&b.dir(), 1_700_000_000_000 + i * 120_000)
                .unwrap();
            b.upload_remote(&snap);
        }
        let s = b.status();
        assert!(s.remote.last_ok_at.is_some() && s.remote.last_error.is_none());
        assert_eq!(s.remote.kind, RemoteKind::S3);
        assert!(s.remote.credentials_configured && s.remote.encrypt);
        assert!(s.remote.last_name.unwrap().ends_with(".db.age"));

        let g = st.lock().unwrap();
        let hubs: Vec<_> = g
            .objects
            .keys()
            .filter(|k| k.starts_with("hub/hub-"))
            .collect();
        assert_eq!(hubs.len(), 2, "retenção no remoto: {:?}", g.objects.keys());
        // Cifrado: não é um SQLite legível; e abre com a chave do hub.
        let obj = &g.objects[hubs[0]];
        assert!(!obj.starts_with(b"SQLite format 3"));
        drop(g);

        // As credenciais ficam no store (hub-only), não no backup.json nem
        // no que vai para os ambientes.
        let json = std::fs::read_to_string(d.path().join("config/backup.json")).unwrap();
        assert!(!json.contains("SECRETA"), "{json}");
        assert!(!format!("{:?}", b.status()).contains("SECRETA"));
        assert_eq!(
            secrets
                .get_hub_only("CTHUB_BACKUP_S3_ACCESS_KEY")
                .as_deref(),
            Some("AK-SECRETA")
        );
        assert!(secrets.list().unwrap().items.is_empty());
    }

    #[test]
    fn falha_remota_nao_quebra_o_snapshot_local_e_vira_aviso() {
        let d = tempfile::tempdir().unwrap();
        let (url, st) = fake::serve();
        let (hub, b, _s) = com_remoto(d.path(), &url, 5, false);
        let kinds = |b: &Backups| b.warnings().iter().map(|w| w.kind).collect::<Vec<_>>();

        st.lock().unwrap().deny = true;
        let (p, _) = b
            .run_once(&hub)
            .expect("o snapshot local não depende do remoto");
        assert!(p.exists());
        let ks = kinds(&b);
        assert!(ks.contains(&"backup_remote_credentials"), "{ks:?}");
        assert!(ks.contains(&"backup_remote_unencrypted"), "{ks:?}");
        assert_eq!(b.status().remote.failures, 1);
        assert!(b.status().last_error.is_none());
        assert!(!b.remote_fresh());

        // Voltou: o reenvio limpa o aviso e o disco local deixa de alarmar.
        st.lock().unwrap().deny = false;
        b.retry_remote();
        assert!(b.status().remote.last_error.is_none());
        assert!(!kinds(&b).contains(&"backup_remote_credentials"));
        assert!(!kinds(&b).contains(&"backup_same_device"));
        assert!(b.remote_fresh());
        assert_eq!(
            backoff(1, Duration::from_secs(3600)),
            Duration::from_secs(300)
        );
        assert_eq!(
            backoff(3, Duration::from_secs(3600)),
            Duration::from_secs(1200)
        );
        assert_eq!(
            backoff(9, Duration::from_secs(3600)),
            Duration::from_secs(3600)
        );
    }

    fn docs_kinds(b: &Backups) -> Vec<&'static str> {
        b.warnings()
            .into_iter()
            .map(|w| w.kind)
            .filter(|k| k.starts_with("docs_"))
            .collect()
    }

    fn arvore_docs(docs: &Path) {
        std::fs::create_dir_all(docs.join("acme__app/epicos/7-busca")).unwrap();
        std::fs::write(docs.join("acme__app/epicos/7-busca/README.md"), "# Busca\n").unwrap();
    }

    #[test]
    fn docs_pacote_cifrado_com_retencao_de_7_no_local_e_no_remoto() {
        let d = tempfile::tempdir().unwrap();
        let (url, st) = fake::serve();
        let (hub, b, _s) = com_remoto(d.path(), &url, 2, true);
        arvore_docs(&d.path().join("docs"));
        b.run_once(&hub).unwrap();
        for i in 0..9 {
            let r = b.run_docs_at(1_700_000_000_000 + i * 120_000).unwrap();
            assert!(matches!(r, DocsRun::Done { .. }), "{r:?}");
        }
        let local = crate::docs_backup::list_packages(&b.dir()).unwrap();
        assert_eq!(local.len(), 7);
        // A retenção dos docs não mexe nos snapshots do hub.db, e vice-versa.
        assert_eq!(ct_ledger::list_snapshots(&b.dir()).unwrap().len(), 1);

        let g = st.lock().unwrap();
        let docs: Vec<_> = g
            .objects
            .keys()
            .filter(|k| k.starts_with("hub/docs-"))
            .collect();
        assert_eq!(docs.len(), 7, "{:?}", g.objects.keys());
        assert!(docs.iter().all(|k| k.ends_with(".tar.zst.age")));
        // Cifrado: não começa com o número mágico do zstd.
        assert!(!g.objects[docs[0]].starts_with(&[0x28, 0xb5, 0x2f, 0xfd]));
        assert_eq!(
            g.objects
                .keys()
                .filter(|k| k.starts_with("hub/hub-"))
                .count(),
            1
        );
        drop(g);

        let s = b.docs_status();
        assert_eq!((s.keep, s.interval_hours, s.max_mb), (7, 1, 1));
        assert!(s.last_ok_at.is_some() && s.last_error.is_none());
        assert!(s.newest_name.unwrap().starts_with("docs-"));
        assert!(s.remote.last_name.unwrap().ends_with(".tar.zst.age"));
        assert!(docs_kinds(&b).is_empty(), "{:?}", docs_kinds(&b));

        // Destino recusando: a falha do envio vira docs_backup_failed e o
        // reenvio limpa.
        st.lock().unwrap().deny = true;
        b.run_docs_at(1_800_000_000_000).unwrap();
        assert_eq!(docs_kinds(&b), vec!["docs_backup_failed"]);
        assert_eq!(b.docs_status().remote.failures, 1);
        st.lock().unwrap().deny = false;
        b.retry_docs_remote();
        assert!(docs_kinds(&b).is_empty());
    }

    #[test]
    fn docs_vazio_ou_inexistente_nao_roda_nem_avisa() {
        let d = tempfile::tempdir().unwrap();
        let (url, st) = fake::serve();
        let (_hub, b, _s) = com_remoto(d.path(), &url, 2, false);
        // Inexistente.
        assert!(matches!(b.run_docs_once().unwrap(), DocsRun::Skipped(_)));
        assert!(docs_kinds(&b).is_empty());
        // Só pastas, sem arquivo.
        std::fs::create_dir_all(d.path().join("docs/acme__app/_projeto")).unwrap();
        let DocsRun::Skipped(why) = b.run_docs_once().unwrap() else {
            panic!("rodou com a pasta vazia")
        };
        assert!(why.contains("vazia"), "{why}");
        assert!(docs_kinds(&b).is_empty());
        assert!(crate::docs_backup::list_packages(&b.dir())
            .unwrap()
            .is_empty());
        assert!(st
            .lock()
            .unwrap()
            .objects
            .keys()
            .all(|k| !k.contains("docs-")));
        assert!(b.docs_status().last_error.is_none());

        // Sem docs_dir, nada roda.
        let sem = Backups::new(d.path().join("x"), Duration::from_secs(3600), 2, None);
        assert!(matches!(sem.run_docs_once().unwrap(), DocsRun::Skipped(_)));
        assert!(docs_kinds(&sem).is_empty());
        assert_eq!(sem.docs_status().docs_dir, None);
    }

    #[test]
    fn docs_acima_do_limite_nao_envia_e_avisa() {
        let d = tempfile::tempdir().unwrap();
        let (url, st) = fake::serve();
        let (_hub, b, _s) = com_remoto(d.path(), &url, 2, true);
        let docs = d.path().join("docs");
        arvore_docs(&docs);
        // Limite de 1 MB.
        std::fs::write(docs.join("grande.bin"), vec![7u8; 2 << 20]).unwrap();
        assert!(matches!(b.run_docs_once().unwrap(), DocsRun::Skipped(_)));
        assert_eq!(docs_kinds(&b), vec!["docs_backup_too_large"]);
        assert_eq!(b.docs_status().too_large_bytes, Some((2 << 20) + 8));
        assert!(crate::docs_backup::list_packages(&b.dir())
            .unwrap()
            .is_empty());
        assert!(st
            .lock()
            .unwrap()
            .objects
            .keys()
            .all(|k| !k.contains("docs-")));

        // Subir o limite pela UI resolve.
        b.update(&BackupUpdate {
            docs_max_mb: Some(10),
            ..Default::default()
        })
        .unwrap();
        assert!(matches!(b.run_docs_once().unwrap(), DocsRun::Done { .. }));
        assert!(docs_kinds(&b).is_empty());
        let json = std::fs::read_to_string(d.path().join("config/backup.json")).unwrap();
        assert!(json.contains("\"docs_max_mb\": 10"), "{json}");
        for ruim in [
            BackupUpdate {
                docs_max_mb: Some(0),
                ..Default::default()
            },
            BackupUpdate {
                docs_keep: Some(0),
                ..Default::default()
            },
        ] {
            assert!(b.update(&ruim).is_err());
        }
    }

    #[test]
    fn docs_sem_pacote_em_2x_o_intervalo_avisa() {
        let d = tempfile::tempdir().unwrap();
        let docs = d.path().join("docs");
        arvore_docs(&docs);
        let b = Backups::new(d.path().join("bk"), Duration::from_secs(3600), 2, None).with_docs(
            Some(docs),
            Duration::from_millis(5),
            7,
            10,
        );
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(docs_kinds(&b), vec!["docs_backup_stale"]);
        // Desligado (intervalo 0): sem aviso.
        b.update_docs_every_for_test(Duration::ZERO);
        assert!(docs_kinds(&b).is_empty());
    }

    #[test]
    fn secrets_acompanham_o_backup_dir_da_ui() {
        let d = tempfile::tempdir().unwrap();
        let ds = Dirs {
            config: d.path().join("config"),
            data: d.path().join("data"),
        };
        ds.ensure().unwrap();
        let hub = SharedHub::new(Hub::open(&ds.hub_db()).unwrap());
        let abre = |segue: bool| {
            let s = HubSecrets::open(&ds, hub.clone())
                .unwrap()
                .with_snapshot_dir(d.path().join("antes"));
            Arc::new(if segue { s.following_backup_dir() } else { s })
        };
        let novo = BackupUpdate {
            dir: Some(d.path().join("depois").display().to_string()),
            ..Default::default()
        };

        let secrets = abre(true);
        secrets.set_hub_only("CTHUB_TESTE", "valor").unwrap();
        let b = Backups::new(d.path().join("antes"), Duration::from_secs(3600), 5, None)
            .with_remote(ds.backup_settings_file(), Arc::clone(&secrets));
        b.update(&novo).unwrap();
        assert_eq!(secrets.snapshot_dir(), Some(d.path().join("depois")));
        // A conferência na troca já tira o snapshot no destino novo.
        assert!(std::fs::read_dir(d.path().join("depois"))
            .unwrap()
            .any(|e| e
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("secrets-")));

        // Com secrets_snapshot_dir explícito, o destino não muda.
        std::fs::remove_file(ds.backup_settings_file()).unwrap();
        let secrets = abre(false);
        let b = Backups::new(d.path().join("antes"), Duration::from_secs(3600), 5, None)
            .with_remote(ds.backup_settings_file(), Arc::clone(&secrets));
        b.update(&novo).unwrap();
        assert_eq!(secrets.snapshot_dir(), Some(d.path().join("antes")));
    }

    #[test]
    fn update_valida_e_testa_conexao() {
        let d = tempfile::tempdir().unwrap();
        let (url, _st) = fake::serve();
        let (_hub, b, _s) = com_remoto(d.path(), &url, 5, true);

        // Sem bucket: recusa e mantém a configuração anterior.
        let ruim = BackupUpdate {
            remote: Some(RemoteSettings {
                kind: RemoteKind::S3,
                ..Default::default()
            }),
            ..Default::default()
        };
        assert!(b.update(&ruim).is_err());
        assert_eq!(b.status().remote.target, format!("{url}/bk/hub"));
        assert!(b
            .update(&BackupUpdate {
                keep: Some(0),
                ..Default::default()
            })
            .is_err());

        let r = b.settings_view();
        Backups::test_remote(&r, &b.credentials(RemoteKind::S3)).unwrap();
        let mut errada = b.credentials(RemoteKind::S3);
        errada.secret_key.clear();
        assert!(Backups::test_remote(&r, &errada).is_err());

        // A configuração persiste e volta num hub novo.
        let again = std::fs::read_to_string(d.path().join("config/backup.json")).unwrap();
        assert!(again.contains("\"s3\""));
    }
}
