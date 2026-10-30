//! Restauração do store de secrets a partir de um snapshot (issue #170,
//! épico #171): `cthubd secrets restore <arquivo> [--identity <arquivo>]
//! [--yes]`, com o serviço parado (a issue escrevia `cthub`; o binário é o
//! `cthubd`).
//!
//! 1. [`open_snapshot`] abre o snapshot com a `secrets.key` local, se ela
//!    existir e abrir o arquivo; senão, com a chave de recuperação (de
//!    `--identity` ou do stdin, sem eco). Só em memória: nada muda em disco.
//! 2. Quem chama lista escopo, nome e tipo ([`OpenedSnapshot::items`],
//!    nunca valores) e pede confirmação.
//! 3. [`apply_restore`] grava o snapshot como o novo `secrets.age` (escrita
//!    atômica), recifrado; o store anterior fica como
//!    `secrets.age.pre-restore-<carimbo>` (0600).
//!    - Aberto com a chave do hub: recifra para o hub e a recuperação atual
//!      (se configurada).
//!    - Aberto com a de recuperação: ela foi exposta (colada numa máquina
//!      nova). Gera uma `secrets.key` nova (a anterior, se havia, fica como
//!      `secrets.key.pre-restore-<carimbo>`), recifra só para ela, apaga
//!      `secrets.recovery.pub`, `secrets.recovery.pending.pub` e
//!      `secrets.recovery.verified` (a UI volta a pedir uma chave de
//!      recuperação, pelo fluxo da #169) e grava
//!      `secrets.rotate-pending.json` com escopo e nome dos `secret` (sem
//!      valores): a lista "Secrets a rotacionar na origem" da UI, até o
//!      usuário marcar que já rotacionou (`hub.secrets.rotate_pending_dismiss`).
//!    - Nos dois casos: o `config_rev` do `hub.db` passa do maior `rev` do
//!      store restaurado e sobe mais um ([`ct_ledger::Hub::bump_config_rev_past`]),
//!      para os ambientes receberem tudo de novo no próximo pull, e sai um
//!      snapshot do store restaurado no destino configurado.

use super::{HubSecrets, HubSecretsError};
use crate::backup::Warning;
use crate::config::Dirs;
use ct_ledger::{Hub, SharedHub};
use ct_secrets::{Identity, Kind, Scope, SecretsStore, Zeroizing};
use serde::{Deserialize, Serialize};
use std::io;
use std::path::{Path, PathBuf};

/// Um secret a trocar na origem (o token na API de quem o emitiu).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct RotateItem {
    pub scope: Scope,
    pub name: String,
}

/// `secrets.rotate-pending.json`: os secrets que estavam no store quando ele
/// foi restaurado com a chave de recuperação (perda da máquina do hub). Só
/// escopos e nomes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct RotatePending {
    /// Quando a restauração aconteceu (ms).
    pub restored_at: i64,
    pub items: Vec<RotateItem>,
}

/// Lê a lista; arquivo ausente = nenhuma. Ilegível vira aviso no log e
/// "nenhuma": a lista é um lembrete, não trava o hub.
pub(super) fn read_rotate_pending(path: &Path) -> Option<RotatePending> {
    match std::fs::read(path) {
        Ok(bytes) => match serde_json::from_slice(&bytes) {
            Ok(p) => Some(p),
            Err(e) => {
                tracing::warn!("{}: ilegível ({e})", path.display());
                None
            }
        },
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(e) => {
            tracing::warn!("{}: {e}", path.display());
            None
        }
    }
}

fn io_err(path: &Path, e: io::Error) -> HubSecretsError {
    HubSecretsError::Secrets(ct_secrets::SecretsError::Io {
        path: path.to_path_buf(),
        source: e,
    })
}

/// Apaga `path`; ausente não é erro.
fn remove_if_exists(path: &Path) -> io::Result<bool> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

impl HubSecrets {
    fn rotate_pending_state(&self) -> std::sync::MutexGuard<'_, Option<RotatePending>> {
        self.rotate_pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    /// A lista "Secrets a rotacionar na origem", se houver.
    pub fn rotate_pending(&self) -> Option<RotatePending> {
        self.rotate_pending_state().clone()
    }

    /// `hub.secrets.rotate_pending_dismiss`: o usuário já trocou os valores
    /// na origem. Apaga o arquivo e a lista some da UI e do `hub.status`.
    pub fn rotate_pending_dismiss(&self) -> Result<(), HubSecretsError> {
        let mut st = self.rotate_pending_state();
        remove_if_exists(&self.rotate_pending_path)
            .map_err(|e| io_err(&self.rotate_pending_path, e))?;
        *st = None;
        Ok(())
    }

    /// Aviso `secrets_rotate_pending` de `hub.status` enquanto houver lista.
    /// Só pega o lock dela: pode com o hub pego.
    pub fn rotate_pending_warnings(&self) -> Vec<Warning> {
        match &*self.rotate_pending_state() {
            Some(p) if !p.items.is_empty() => vec![Warning {
                kind: "secrets_rotate_pending",
                message: format!(
                    "{} secret(s) a rotacionar na origem: o store foi restaurado com a chave de \
                     recuperação depois da perda da máquina do hub. Troque os valores (tokens \
                     de API) em quem os emitiu, grave os novos no hub e marque como feito na \
                     tela Hub",
                    p.items.len()
                ),
            }],
            _ => Vec::new(),
        }
    }

    /// Snapshot do store agora (fim do `restore` e do `rotate-keys`).
    /// `Ok(None)` = desligados, store ausente ou o mais recente já igual.
    pub(super) fn snapshot_now(&self) -> Result<Option<PathBuf>, String> {
        let store = self.store();
        self.snapshots.take(store.path())
    }
}

/// Com qual chave o snapshot abriu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenedWith {
    /// A `secrets.key` desta máquina.
    HubKey,
    /// A chave de recuperação (do gerenciador de senhas).
    RecoveryKey,
}

/// Uma entrada do snapshot, sem valor: o que a confirmação lista.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoreItem {
    pub scope: Scope,
    pub name: String,
    pub kind: Kind,
}

/// Snapshot decifrado em memória, à espera da confirmação.
pub struct OpenedSnapshot {
    source: PathBuf,
    plain: Zeroizing<Vec<u8>>,
    with: OpenedWith,
    items: Vec<RestoreItem>,
    store_rev: i64,
}

impl std::fmt::Debug for OpenedSnapshot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenedSnapshot")
            .field("source", &self.source)
            .field("with", &self.with)
            .field("items", &self.items.len())
            .field("store_rev", &self.store_rev)
            .finish_non_exhaustive()
    }
}

impl OpenedSnapshot {
    pub fn opened_with(&self) -> OpenedWith {
        self.with
    }

    /// Escopo, nome e tipo de cada entrada, por escopo e nome.
    pub fn items(&self) -> &[RestoreItem] {
        &self.items
    }

    pub fn store_rev(&self) -> i64 {
        self.store_rev
    }
}

/// Abre `file` só em memória: com a `secrets.key` local, se ela existir e
/// abrir o arquivo; senão, com a identidade que `recovery_key` devolver (o
/// texto de `--identity` ou o colado no stdin). O snapshot precisa ser um
/// store válido. Nada muda em disco.
pub fn open_snapshot(
    dirs: &Dirs,
    file: &Path,
    recovery_key: impl FnOnce() -> Result<Zeroizing<String>, String>,
) -> Result<OpenedSnapshot, String> {
    let bytes = std::fs::read(file).map_err(|e| format!("{}: {e}", file.display()))?;
    let key_file = dirs.secrets_key_file();
    let hub = match ct_secrets::read_identity(&key_file) {
        Ok(id) => ct_secrets::decrypt(&bytes, &id).ok().map(|p| (id, p)),
        Err(ct_secrets::SecretsError::Io { source, .. })
            if source.kind() == io::ErrorKind::NotFound =>
        {
            None
        }
        Err(e) => {
            eprintln!("aviso: {e}; tentando a chave de recuperação");
            None
        }
    };
    let (identity, plain, with) = match hub {
        Some((id, plain)) => (id, plain, OpenedWith::HubKey),
        None => {
            let text = recovery_key()?;
            let id = ct_secrets::parse_identity(&text)
                .map_err(|r| format!("chave de recuperação inválida: {r}"))?;
            let plain = ct_secrets::decrypt(&bytes, &id).map_err(|_| {
                format!(
                    "{} não abre com a chave de recuperação informada{}",
                    file.display(),
                    if key_file.exists() {
                        " nem com a secrets.key desta máquina"
                    } else {
                        ""
                    }
                )
            })?;
            (id, plain, OpenedWith::RecoveryKey)
        }
    };
    // Valida o formato pelo próprio store (sem gravar: só lê o arquivo).
    let store = SecretsStore::open(file, identity, &[])
        .map_err(|e| format!("{}: não é um store de secrets válido: {e}", file.display()))?;
    let items = store
        .entries()
        .iter()
        .map(|e| RestoreItem {
            scope: e.scope.clone(),
            name: e.name.clone(),
            kind: e.kind,
        })
        .collect();
    Ok(OpenedSnapshot {
        source: file.to_path_buf(),
        plain,
        with,
        items,
        store_rev: store.store_rev(),
    })
}

/// O que [`apply_restore`] fez, para o terminal.
#[derive(Debug)]
pub struct RestoreReport {
    pub with: OpenedWith,
    pub entries: usize,
    /// O `secrets.age` anterior, preservado.
    pub previous_store: Option<PathBuf>,
    /// A `secrets.key` anterior, preservada (só com a de recuperação).
    pub previous_key: Option<PathBuf>,
    /// Chave pública nova do hub (só com a de recuperação).
    pub new_hub_public: Option<String>,
    /// A recuperação continua configurada (só com a chave do hub).
    pub recovery_configured: bool,
    /// Secrets a rotacionar na origem (só com a de recuperação).
    pub rotate_pending: Vec<RotateItem>,
    pub config_rev: i64,
    /// Snapshot do store restaurado; `Err` = falhou (a restauração vale).
    pub snapshot: Result<Option<PathBuf>, String>,
}

/// Grava o snapshot aberto como o store do hub (veja o módulo). Chame com o
/// serviço parado e depois da confirmação. `snapshot_dir` = destino dos
/// snapshots (`secrets_snapshot_dir` do config).
pub fn apply_restore(
    dirs: &Dirs,
    opened: OpenedSnapshot,
    snapshot_dir: &Path,
) -> Result<RestoreReport, String> {
    dirs.ensure().map_err(|e| e.to_string())?;
    let stamp = ct_core::utc_stamp(ct_core::now_ms());
    let key_file = dirs.secrets_key_file();
    let store_file = dirs.secrets_file();
    let s = |e: ct_secrets::SecretsError| e.to_string();

    // Destinatários e, com a de recuperação, a chave nova do hub (ainda só
    // em memória).
    let (hub_public, recovery, new_key) = match opened.with {
        OpenedWith::HubKey => {
            let id = ct_secrets::read_identity(&key_file).map_err(s)?;
            let rec = ct_secrets::read_recipient(&dirs.secrets_recovery_file()).map_err(s)?;
            (id.to_public(), rec, None)
        }
        OpenedWith::RecoveryKey => {
            let id = Identity::generate();
            (id.to_public(), None, Some(id))
        }
    };
    let mut recipients = vec![hub_public.clone()];
    recipients.extend(recovery.iter().cloned());
    let sealed = ct_secrets::encrypt(&opened.plain, &recipients).map_err(s)?;

    // Preserva o store e (com a de recuperação) a chave anteriores.
    let previous_store = preserve(&dirs.config, &store_file, "secrets.age.pre-restore", &stamp)?;
    let mut previous_key = None;
    if let Some(id) = &new_key {
        previous_key = preserve(&dirs.config, &key_file, "secrets.key.pre-restore", &stamp)?;
        ct_secrets::write_identity(&key_file, id, "chave do store de secrets do Control Tower")
            .map_err(s)?;
    }
    ct_core::files::write_private_atomic(&store_file, &sealed)
        .map_err(|e| format!("{}: {e}", store_file.display()))?;

    let mut rotate_pending = Vec::new();
    if new_key.is_some() {
        // A chave de recuperação foi exposta: a UI pede outra (#169).
        for p in [
            dirs.secrets_recovery_file(),
            dirs.secrets_recovery_pending_file(),
            dirs.secrets_recovery_verified_file(),
        ] {
            remove_if_exists(&p).map_err(|e| format!("{}: {e}", p.display()))?;
        }
        rotate_pending = opened
            .items
            .iter()
            .filter(|i| i.kind == Kind::Secret)
            .map(|i| RotateItem {
                scope: i.scope.clone(),
                name: i.name.clone(),
            })
            .collect();
        let pending = RotatePending {
            restored_at: ct_core::now_ms(),
            items: rotate_pending.clone(),
        };
        let path = dirs.secrets_rotate_pending_file();
        let bytes = serde_json::to_vec_pretty(&pending).expect("RotatePending serializa");
        ct_core::files::write_private_atomic(&path, &bytes)
            .map_err(|e| format!("{}: {e}", path.display()))?;
    }

    // Reentrega: o config_rev passa do maior rev do store restaurado.
    let hub = Hub::open(&dirs.hub_db()).map_err(|e| format!("hub.db: {e}"))?;
    let config_rev = hub
        .bump_config_rev_past(opened.store_rev)
        .map_err(|e| format!("hub.db: {e}"))?;

    // Confere que o store gravado abre com a chave do hub e tira o snapshot.
    let secrets = HubSecrets::open(dirs, SharedHub::new(hub))
        .map_err(|e| format!("o store restaurado não abre com a chave do hub: {e}"))?
        .with_snapshot_dir(snapshot_dir.to_path_buf());
    let snapshot = secrets.snapshot_now();
    let st = secrets.status();
    Ok(RestoreReport {
        with: opened.with,
        entries: st.entries,
        previous_store,
        previous_key,
        new_hub_public: new_key.map(|_| hub_public.to_string()),
        recovery_configured: st.recovery_configured,
        rotate_pending,
        config_rev,
        snapshot,
    })
}

/// Renomeia `path` (se existir) para `<prefix>-<stamp>` livre em `dir`, com
/// 0600. Devolve o novo caminho.
fn preserve(dir: &Path, path: &Path, prefix: &str, stamp: &str) -> Result<Option<PathBuf>, String> {
    if !path.exists() {
        return Ok(None);
    }
    let to = crate::cli::free_backup_name(dir, prefix, stamp);
    std::fs::rename(path, &to).map_err(|e| format!("{}: {e}", path.display()))?;
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&to, std::fs::Permissions::from_mode(0o600))
        .map_err(|e| format!("{}: {e}", to.display()))?;
    Ok(Some(to))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ct_secrets::SecretValue;
    use std::os::unix::fs::PermissionsExt;

    use crate::secrets::snapshots;

    fn dirs(root: &Path) -> Dirs {
        let d = Dirs {
            config: root.join("config"),
            data: root.join("data"),
        };
        d.ensure().unwrap();
        d
    }

    fn mode(p: &Path) -> u32 {
        std::fs::metadata(p).unwrap().permissions().mode() & 0o777
    }

    fn abrir(ds: &Dirs, snaps: &Path) -> HubSecrets {
        let hub = SharedHub::new(Hub::open(&ds.hub_db()).unwrap());
        HubSecrets::open(ds, hub)
            .unwrap()
            .with_snapshot_dir(snaps.to_path_buf())
    }

    const TOKEN: &str = "token-de-api-secreto-123";

    /// Hub com recuperação configurada, um secret global, um de projeto e
    /// uma variável. Devolve a chave de recuperação.
    fn popular(s: &HubSecrets) -> Zeroizing<String> {
        let g = s.recovery_generate().unwrap();
        s.recovery_verify(&g.private_key).unwrap();
        s.set(
            Scope::Global,
            "API_TOKEN",
            Kind::Secret,
            SecretValue::new(TOKEN),
        )
        .unwrap();
        s.set(
            Scope::parse_project("deep-ion-ai/demo").unwrap(),
            "D",
            Kind::Secret,
            SecretValue::new("d-secreto-456"),
        )
        .unwrap();
        s.set(
            Scope::Global,
            "RUST_LOG",
            Kind::Variable,
            SecretValue::new("info"),
        )
        .unwrap();
        Zeroizing::new(g.private_key)
    }

    fn valor(path: &Path, id: Identity, scope: &Scope, name: &str) -> String {
        let s = SecretsStore::open(path, id, &[]).unwrap();
        s.get(scope, name).unwrap().value.expose().to_string()
    }

    #[test]
    fn restaura_com_a_recuperacao_depois_de_perder_o_config() {
        let d = tempfile::tempdir().unwrap();
        let ds = dirs(d.path());
        let snaps = d.path().join("OneDrive/cthub");
        let s = abrir(&ds, &snaps);
        let rec = popular(&s);
        let antiga = ct_secrets::read_identity(&ds.secrets_key_file()).unwrap();
        let store_rev = s.status().store_rev;
        drop(s);
        let snap = snapshots::list(&snaps).unwrap()[0].path.clone();

        // Perdeu o config_dir inteiro (o hub.db fica, como num restore dele).
        std::fs::remove_dir_all(&ds.config).unwrap();
        ds.ensure().unwrap();
        let rev_antes = Hub::open(&ds.hub_db()).unwrap().config_rev().unwrap();

        // Chave errada: erro, nada muda.
        let outra = ct_secrets::identity_text(&Identity::generate());
        let e = open_snapshot(&ds, &snap, || Ok(outra.clone())).unwrap_err();
        assert!(e.contains("não abre"), "{e}");
        assert!(!e.contains(outra.as_str()));

        let opened = open_snapshot(&ds, &snap, || Ok(rec.clone())).unwrap();
        assert_eq!(opened.opened_with(), OpenedWith::RecoveryKey);
        assert_eq!(opened.store_rev(), store_rev);
        let nomes: Vec<_> = opened.items().iter().map(|i| i.name.as_str()).collect();
        assert_eq!(nomes, ["API_TOKEN", "RUST_LOG", "D"]);
        assert!(!format!("{opened:?}").contains(TOKEN));
        // Abrir não grava nada.
        assert_eq!(std::fs::read_dir(&ds.config).unwrap().count(), 0);

        let r = apply_restore(&ds, opened, &snaps).unwrap();
        assert_eq!(r.with, OpenedWith::RecoveryKey);
        assert_eq!(r.entries, 3);
        assert!(r.previous_store.is_none() && r.previous_key.is_none());
        assert!(!r.recovery_configured);
        // Chave nova, diferente da perdida.
        let nova = ct_secrets::read_identity(&ds.secrets_key_file()).unwrap();
        assert_ne!(nova.to_public(), antiga.to_public());
        assert_eq!(r.new_hub_public, Some(nova.to_public().to_string()));
        assert_eq!(mode(&ds.secrets_key_file()), 0o600);
        assert_eq!(mode(&ds.secrets_file()), 0o600);
        // O store abre com a nova (e não com a de recuperação exposta).
        assert_eq!(
            valor(&ds.secrets_file(), nova, &Scope::Global, "API_TOKEN"),
            TOKEN
        );
        let rec_id = || ct_secrets::parse_identity(&rec).ok().unwrap();
        assert!(SecretsStore::open(ds.secrets_file(), rec_id(), &[]).is_err());
        assert!(!ds.secrets_recovery_file().exists());
        assert!(!ds.secrets_recovery_pending_file().exists());
        assert!(!ds.secrets_recovery_verified_file().exists());
        // config_rev acima do rev do store (reentrega).
        assert!(r.config_rev > store_rev && r.config_rev > rev_antes);
        assert_eq!(
            Hub::open(&ds.hub_db()).unwrap().config_rev().unwrap(),
            r.config_rev
        );
        // Lista a rotacionar: só os secrets, sem valores.
        let names: Vec<_> = r.rotate_pending.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, ["API_TOKEN", "D"]);
        let bruto = std::fs::read_to_string(ds.secrets_rotate_pending_file()).unwrap();
        assert!(!bruto.contains(TOKEN) && !bruto.contains("info"), "{bruto}");
        assert!(!bruto.contains("RUST_LOG"));
        assert_eq!(mode(&ds.secrets_rotate_pending_file()), 0o600);
        // Snapshot novo do store restaurado, para a chave nova.
        let novo = r.snapshot.unwrap().unwrap();
        assert_eq!(
            std::fs::read(&novo).unwrap(),
            std::fs::read(ds.secrets_file()).unwrap()
        );

        // O hub reaberto: recuperação não configurada e a lista a rotacionar.
        let s = abrir(&ds, &snaps);
        assert!(!s.recovery_status().configured);
        let kinds: Vec<_> = s.recovery_warnings().iter().map(|w| w.kind).collect();
        assert_eq!(kinds, ["recovery_not_configured"]);
        assert_eq!(
            s.rotate_pending_warnings()[0].kind,
            "secrets_rotate_pending"
        );
        assert_eq!(s.list().unwrap().rotate_pending.unwrap().items.len(), 2);
        s.rotate_pending_dismiss().unwrap();
        assert!(s.rotate_pending().is_none() && s.rotate_pending_warnings().is_empty());
        assert!(!ds.secrets_rotate_pending_file().exists());
    }

    #[test]
    fn restaura_com_a_chave_do_hub_e_preserva_o_atual() {
        let d = tempfile::tempdir().unwrap();
        let ds = dirs(d.path());
        let snaps = d.path().join("snaps");
        let s = abrir(&ds, &snaps);
        let rec = popular(&s);
        let antigo = snapshots::list(&snaps).unwrap()[0].path.clone();
        s.set(
            Scope::Global,
            "API_TOKEN",
            Kind::Secret,
            SecretValue::new("depois-1234"),
        )
        .unwrap();
        let rev = s.status().store_rev;
        let atual = std::fs::read(ds.secrets_file()).unwrap();
        let chave = std::fs::read(ds.secrets_key_file()).unwrap();
        drop(s);

        let opened = open_snapshot(&ds, &antigo, || panic!("não pede a de recuperação")).unwrap();
        assert_eq!(opened.opened_with(), OpenedWith::HubKey);
        let r = apply_restore(&ds, opened, &snaps).unwrap();
        assert!(r.recovery_configured && r.rotate_pending.is_empty());
        assert!(r.new_hub_public.is_none() && r.previous_key.is_none());
        let prev = r.previous_store.unwrap();
        assert_eq!(std::fs::read(&prev).unwrap(), atual);
        assert_eq!(mode(&prev), 0o600);
        assert!(prev
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("secrets.age.pre-restore-"));
        assert_eq!(std::fs::read(ds.secrets_key_file()).unwrap(), chave);
        assert!(r.config_rev > rev);
        // Volta o valor do snapshot; abre com o hub e com a recuperação.
        let hub = ct_secrets::read_identity(&ds.secrets_key_file()).unwrap();
        assert_eq!(
            valor(&ds.secrets_file(), hub, &Scope::Global, "API_TOKEN"),
            TOKEN
        );
        let rec_id = ct_secrets::parse_identity(&rec).ok().unwrap();
        assert_eq!(
            valor(&ds.secrets_file(), rec_id, &Scope::Global, "API_TOKEN"),
            TOKEN
        );
        assert!(ds.secrets_recovery_file().exists());
        assert!(!ds.secrets_rotate_pending_file().exists());
    }

    #[test]
    fn com_secrets_key_que_nao_abre_usa_a_recuperacao_e_guarda_a_chave() {
        let d = tempfile::tempdir().unwrap();
        let ds = dirs(d.path());
        let snaps = d.path().join("snaps");
        let s = abrir(&ds, &snaps);
        let rec = popular(&s);
        drop(s);
        let snap = snapshots::list(&snaps).unwrap()[0].path.clone();
        // Máquina nova onde o `serve` já subiu uma vez (gerou outra chave).
        std::fs::remove_dir_all(&ds.config).unwrap();
        let gerada = ct_secrets::ensure_identity(&ds.secrets_key_file()).unwrap();
        let opened = open_snapshot(&ds, &snap, || Ok(rec.clone())).unwrap();
        assert_eq!(opened.opened_with(), OpenedWith::RecoveryKey);
        let r = apply_restore(&ds, opened, &snaps).unwrap();
        let prev = r.previous_key.unwrap();
        assert_eq!(
            ct_secrets::read_identity(&prev).unwrap().to_public(),
            gerada.to_public()
        );
        assert_ne!(
            ct_secrets::read_identity(&ds.secrets_key_file())
                .unwrap()
                .to_public(),
            gerada.to_public()
        );
    }
}
