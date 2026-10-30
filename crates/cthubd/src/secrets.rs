//! Store de secrets e variáveis do hub (issue #164, épico #171), sobre o
//! `ct-secrets`: `<config>/secrets.age` cifrado para a chave do hub
//! (`<config>/secrets.key`, gerada no `serve`) e, se houver, para a chave
//! pública de recuperação (`<config>/secrets.recovery.pub`, #169).
//!
//! Toda mudança (inclusive remoção) primeiro incrementa o `config_rev` do
//! `hub.db` (#175), usa o valor como `rev` da entrada e só depois grava o
//! store. Se a gravação falhar, sobra um `config_rev` sem mudança, o que é
//! inofensivo. A entrega aos ambientes é da #184; a listagem para a UI e o
//! estado de entrega por ambiente ([`HubSecrets::list`]), da #165. A chave
//! de recuperação (gerar, verificar, lembrete), da #169: veja [`recovery`].
//! Os snapshots cifrados do store, da #168: veja [`snapshots`]. Restaurar
//! um snapshot e rotacionar as chaves (`cthubd secrets restore` e
//! `cthubd secrets rotate-keys`, com o serviço parado), da #170: veja
//! [`restore`] e [`rotate`].

mod recovery;
pub mod restore;
pub mod rotate;
mod snapshots;

pub use recovery::{RecoveryGenerated, RecoveryStatus, DEFAULT_REVERIFY_DAYS};
pub use restore::{RotateItem, RotatePending};
pub use snapshots::CHECK_INTERVAL as SNAPSHOT_CHECK_INTERVAL;

use crate::backup::Warning;
use crate::config::Dirs;
use ct_ledger::{EnvSecretsInput, SharedHub};
use ct_secrets::{Entry, Kind, Scope, SecretValue, SecretsStore};
use serde::Serialize;
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

#[derive(Debug, thiserror::Error)]
pub enum HubSecretsError {
    #[error(transparent)]
    Secrets(#[from] ct_secrets::SecretsError),
    #[error("hub.db: {0}")]
    Ledger(#[from] ct_ledger::LedgerError),
    /// Texto colado não é uma identidade age. Só o motivo, nunca o texto.
    #[error("chave de recuperação inválida: {0} (esperado AGE-SECRET-KEY-1...)")]
    InvalidRecoveryKey(&'static str),
    #[error("nenhuma chave de recuperação gerada: gere uma antes de verificar")]
    RecoveryNotStarted,
    /// A chave colada não é a gerada (nem a configurada) ou não abre o store.
    #[error("a chave não confere com a chave de recuperação {0}")]
    RecoveryKeyMismatch(&'static str),
}

/// Resumo do store, sem nomes nem valores.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct SecretsStatus {
    pub entries: usize,
    pub store_rev: i64,
    /// `false` = cifrado só para a chave do hub: chave de recuperação
    /// ainda não verificada (`hub.recovery.*`, #169).
    pub recovery_configured: bool,
}

/// Uma entrada em `hub.secrets.list`. O valor só vem para variáveis: o de
/// um secret nunca sai do hub pelo `/ws`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct SecretItem {
    pub scope: Scope,
    pub name: String,
    pub kind: Kind,
    /// `config_rev` da última mudança.
    pub rev: i64,
    /// Quando mudou pela última vez (ms); `null` em entradas gravadas antes
    /// do campo existir.
    pub updated_at: Option<i64>,
    /// Só para `variable`; ausente para `secret`.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub value: Option<String>,
}

impl SecretItem {
    fn from_entry(e: &Entry) -> Self {
        SecretItem {
            scope: e.scope.clone(),
            name: e.name.clone(),
            kind: e.kind,
            rev: e.rev,
            updated_at: Some(e.updated_at).filter(|t| *t > 0),
            value: match e.kind {
                Kind::Variable => Some(e.value.expose().to_string()),
                Kind::Secret => None,
            },
        }
    }
}

/// Os secrets da última mudança relevante já chegaram ao ambiente?
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum DeliveryState {
    /// O ambiente confirmou uma revisão igual ou acima da necessária, para
    /// a chave atual dele.
    Delivered,
    /// Há mudança ainda não confirmada, ou a chave mudou desde a última
    /// entrega.
    Pending,
    /// O ambiente nunca mandou a chave pública (ambiente antigo ou sem
    /// `env.key`): não recebe secrets.
    NoKey,
}

/// Estado de entrega de secrets de um ambiente.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct EnvDelivery {
    pub env_id: String,
    pub env_name: String,
    pub state: DeliveryState,
    /// `config_rev` que o ambiente confirmou ter aplicado (época atual).
    pub applied_rev: Option<i64>,
    /// `config_rev` da última mudança que afeta os secrets do ambiente; 0 =
    /// nenhuma.
    pub required_rev: i64,
    /// Projetos atribuídos (`owner/repo` em minúsculas), para a tela do
    /// projeto filtrar.
    pub projects: Vec<String>,
}

/// Resposta de `hub.secrets.list`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct SecretsList {
    pub status: SecretsStatus,
    /// Todas as entradas, por escopo e nome.
    pub items: Vec<SecretItem>,
    /// Um por ambiente conhecido do hub, por nome.
    pub delivery: Vec<EnvDelivery>,
    /// Secrets a rotacionar na origem depois de uma restauração com a chave
    /// de recuperação (#170); `null` = nenhum.
    pub rotate_pending: Option<RotatePending>,
}

/// Estado de entrega de um ambiente a partir do que o hub sabe dele e das
/// revisões do store.
///
/// A revisão necessária (`required_rev`) é o maior `rev` entre: as entradas
/// e a última remoção globais; para cada projeto atribuído, as entradas e a
/// última remoção dele; e, para cada atribuição (ativa ou não) de um
/// projeto que tem ou já teve entradas, o `rev` da atribuição ou do
/// projeto, porque atribuir, tirar ou excluir o projeto muda o conjunto
/// entregue. É conservador: uma mudança de projeto sem efeito nos secrets
/// também deixa o ambiente "pendente" até ele aplicar a revisão, o que ele
/// faz no próximo pull.
fn env_delivery(
    env: EnvSecretsInput,
    entry_revs: &HashMap<&Scope, i64>,
    removal_revs: &HashMap<&Scope, i64>,
) -> EnvDelivery {
    let rev_of = |scope: &Scope| {
        entry_revs
            .get(scope)
            .copied()
            .unwrap_or(0)
            .max(removal_revs.get(scope).copied().unwrap_or(0))
    };
    let touched =
        |scope: &Scope| entry_revs.contains_key(scope) || removal_revs.contains_key(scope);
    let mut required = rev_of(&Scope::Global);
    let mut projects = Vec::new();
    for a in &env.assignments {
        let scope = Scope::Project(a.project_key.clone());
        if a.active {
            projects.push(a.project_key.clone());
            required = required.max(rev_of(&scope));
        }
        if touched(&scope) {
            required = required.max(a.rev);
        }
    }
    let state = match &env.key {
        None => DeliveryState::NoKey,
        Some(k) if k.needs_delivery() => DeliveryState::Pending,
        Some(_) if env.applied_rev.unwrap_or(0) >= required => DeliveryState::Delivered,
        Some(_) => DeliveryState::Pending,
    };
    EnvDelivery {
        env_id: env.env_id,
        env_name: env.env_name,
        state,
        applied_rev: env.applied_rev,
        required_rev: required,
        projects,
    }
}

/// O store aberto e o `hub.db` de onde vem o `rev` de cada mudança.
///
/// Ordem dos locks: `store` antes do hub e `store` antes de `recovery`;
/// `recovery`, `rotate_pending` e o estado dos snapshots nunca pegam outro
/// lock (o `hub.status` os lê segurando o hub).
pub struct HubSecrets {
    hub: SharedHub,
    store: Mutex<SecretsStore>,
    recovery: Mutex<recovery::RecoveryState>,
    paths: recovery::RecoveryPaths,
    /// Snapshots cifrados do store (#168); desligados até
    /// [`HubSecrets::with_snapshot_dir`].
    snapshots: snapshots::Snapshots,
    /// O destino dos snapshots segue o `backup_dir` (#168: vazio no config).
    snapshot_follows_backup: bool,
    /// Dias até o lembrete de reverificação; `0` desliga.
    reverify_days: u64,
    /// `secrets.rotate-pending.json` (#170) e a lista lida dele.
    rotate_pending_path: PathBuf,
    rotate_pending: Mutex<Option<RotatePending>>,
}

impl HubSecrets {
    /// Carrega (ou gera, na primeira vez) a chave do hub e abre o store.
    pub fn open(dirs: &Dirs, hub: SharedHub) -> Result<Self, HubSecretsError> {
        let identity = ct_secrets::ensure_identity(&dirs.secrets_key_file())?;
        let paths = recovery::RecoveryPaths::new(dirs);
        let state = recovery::RecoveryState::load(&paths)?;
        let store = SecretsStore::open(dirs.secrets_file(), identity, state.configured.as_slice())?;
        // Um hub.db restaurado de snapshot pode ter `config_rev` abaixo do
        // store; a época nova do restore já força o conjunto completo.
        let config_rev = hub.lock().config_rev()?;
        if store.store_rev() > config_rev {
            tracing::warn!(
                "store de secrets em rev {} acima do config_rev {config_rev} do hub.db \
                 (hub.db restaurado?)",
                store.store_rev()
            );
        }
        Ok(HubSecrets {
            hub,
            store: Mutex::new(store),
            recovery: Mutex::new(state),
            paths,
            snapshots: snapshots::Snapshots::disabled(),
            snapshot_follows_backup: false,
            reverify_days: DEFAULT_REVERIFY_DAYS,
            rotate_pending: Mutex::new(restore::read_rotate_pending(
                &dirs.secrets_rotate_pending_file(),
            )),
            rotate_pending_path: dirs.secrets_rotate_pending_file(),
        })
    }

    /// Liga os snapshots cifrados do store em `dir` (`secrets_snapshot_dir`
    /// do config, #168). A primeira conferência (que tira um snapshot se o
    /// mais recente não bate) é a de [`spawn_snapshot_check`].
    pub fn with_snapshot_dir(mut self, dir: PathBuf) -> Self {
        let store = self.store().path().to_path_buf();
        self.snapshots = snapshots::Snapshots::new(dir, &store);
        self
    }

    /// O destino dos snapshots acompanha o `backup_dir` mudado pela UI
    /// (`secrets_snapshot_dir` vazio no config). Ver
    /// [`HubSecrets::follow_backup_dir`].
    pub fn following_backup_dir(mut self) -> Self {
        self.snapshot_follows_backup = true;
        self
    }

    /// Destino dos snapshots; `None` = desligados.
    pub fn snapshot_dir(&self) -> Option<PathBuf> {
        self.snapshots.dir()
    }

    /// O `backup_dir` passou a ser `dir`: se o destino dos snapshots o
    /// acompanha, muda para lá e confere na hora (tira o primeiro snapshot
    /// no destino novo). Síncrona; não chame com o hub pego.
    pub fn follow_backup_dir(&self, dir: &std::path::Path) {
        if !self.snapshot_follows_backup {
            return;
        }
        let store = self.store();
        if self.snapshots.retarget(dir, store.path()) {
            tracing::info!("snapshots de secrets em {}", dir.display());
            self.snapshots.check(store.path());
        }
    }

    /// Avisos de `hub.status` dos snapshots (`secrets_snapshot_*`). Só pega
    /// o lock do estado deles: pode com o hub pego.
    pub fn snapshot_warnings(&self) -> Vec<Warning> {
        self.snapshots.warnings()
    }

    /// Conferência periódica dos snapshots: destino acessível e mais recente
    /// igual ao store; tira um se não bater. Síncrona: chame fora do
    /// runtime.
    pub fn check_snapshots(&self) {
        let store = self.store();
        self.snapshots.check(store.path());
    }

    /// Snapshot logo depois de uma alteração gravada, com o lock do store.
    /// Uma falha só vira aviso: a alteração local já está feita.
    fn snapshot_after_change(&self, store: &SecretsStore) {
        match self.snapshots.take(store.path()) {
            Ok(Some(p)) => tracing::debug!("snapshot de secrets em {}", p.display()),
            Ok(None) => {}
            Err(e) => tracing::warn!(
                "snapshot de secrets falhou (a alteração continua gravada no hub): {e}"
            ),
        }
    }

    /// Prazo do lembrete de reverificação da chave de recuperação
    /// (`recovery_reverify_days` do config; `0` desliga).
    pub fn with_reverify_days(mut self, days: u64) -> Self {
        self.reverify_days = days;
        self
    }

    fn store(&self) -> std::sync::MutexGuard<'_, SecretsStore> {
        self.store.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn status(&self) -> SecretsStatus {
        let s = self.store();
        SecretsStatus {
            entries: s.entries().len(),
            store_rev: s.store_rev(),
            recovery_configured: s.recovery_configured(),
        }
    }

    /// Acesso de leitura ao store (ex.: listar nomes, ler variáveis).
    pub fn with_store<R>(&self, f: impl FnOnce(&SecretsStore) -> R) -> R {
        f(&self.store())
    }

    /// Entradas que valem para o projeto `owner/repo` (ou só as globais),
    /// com o projeto sobrepondo o global.
    pub fn resolve(&self, project: Option<&str>) -> BTreeMap<String, Entry> {
        self.store()
            .resolve(project)
            .into_iter()
            .map(|(k, e)| (k, e.clone()))
            .collect()
    }

    /// `hub.secrets.list`: entradas (valor só das variáveis), status e o
    /// estado de entrega por ambiente. Lê o store e depois o `hub.db`, sem
    /// segurar os dois locks juntos.
    pub fn list(&self) -> Result<SecretsList, HubSecretsError> {
        let (status, items, entry_revs, removal_revs) = {
            let store = self.store();
            let mut entry_revs: HashMap<Scope, i64> = HashMap::new();
            for e in store
                .entries()
                .iter()
                .filter(|e| !ct_secrets::is_hub_only(&e.name))
            {
                let r = entry_revs.entry(e.scope.clone()).or_default();
                *r = (*r).max(e.rev);
            }
            let removal_revs: HashMap<Scope, i64> = store
                .removals()
                .iter()
                .map(|r| (r.scope.clone(), r.rev))
                .collect();
            let status = SecretsStatus {
                entries: store.entries().len(),
                store_rev: store.store_rev(),
                recovery_configured: store.recovery_configured(),
            };
            let items = store
                .entries()
                .iter()
                .filter(|e| !ct_secrets::is_hub_only(&e.name))
                .map(SecretItem::from_entry)
                .collect();
            (status, items, entry_revs, removal_revs)
        };
        let inputs = self.hub.lock().secrets_delivery_inputs()?;
        let entry_revs: HashMap<&Scope, i64> = entry_revs.iter().map(|(k, v)| (k, *v)).collect();
        let removal_revs: HashMap<&Scope, i64> =
            removal_revs.iter().map(|(k, v)| (k, *v)).collect();
        let delivery = inputs
            .into_iter()
            .map(|env| env_delivery(env, &entry_revs, &removal_revs))
            .collect();
        Ok(SecretsList {
            status,
            items,
            delivery,
            rotate_pending: self.rotate_pending(),
        })
    }

    /// Cria ou troca uma entrada. Devolve o `config_rev` da mudança. Um
    /// nome inválido é recusado antes de mexer no `config_rev`.
    pub fn set(
        &self,
        scope: Scope,
        name: &str,
        kind: Kind,
        value: SecretValue,
    ) -> Result<i64, HubSecretsError> {
        ct_secrets::validate_name(name)?;
        // O lock do store fica do bump até a gravação: as mudanças entram no
        // store na ordem dos `rev`.
        let mut store = self.store();
        let rev = self.hub.lock().bump_config_rev_for_secrets()?;
        store.set(scope, name, kind, value, rev)?;
        self.snapshot_after_change(&store);
        Ok(rev)
    }

    /// Grava uma credencial só do hub (`CTHUB_*`, #263). Não passa pela
    /// validação de `hub.secrets.set`, que recusa esse prefixo.
    pub fn set_hub_only(&self, name: &str, value: &str) -> Result<(), HubSecretsError> {
        debug_assert!(ct_secrets::is_hub_only(name));
        self.set(Scope::Global, name, Kind::Secret, SecretValue::new(value))
            .map(|_| ())
    }

    /// Lê uma credencial só do hub.
    pub fn get_hub_only(&self, name: &str) -> Option<String> {
        self.store()
            .get(&Scope::Global, name)
            .map(|e| e.value.expose().to_string())
    }

    /// Remove uma credencial só do hub; `true` se existia.
    pub fn delete_hub_only(&self, name: &str) -> Result<bool, HubSecretsError> {
        Ok(self.delete(&Scope::Global, name)?.is_some())
    }

    /// Recipients para cifrar um backup: a chave do hub e a de recuperação.
    pub fn backup_recipients(&self) -> Vec<ct_secrets::Recipient> {
        let s = self.store();
        let mut r = vec![s.public_key()];
        r.extend(s.extra_recipients().iter().cloned());
        r
    }

    /// Caminho do `secrets.age` (cifrado), para subir junto do backup.
    pub fn store_path(&self) -> PathBuf {
        self.store().path().to_path_buf()
    }

    /// Remove uma entrada. `None` (sem mexer no `config_rev`) se ela não
    /// existe; senão, o `config_rev` da remoção.
    pub fn delete(&self, scope: &Scope, name: &str) -> Result<Option<i64>, HubSecretsError> {
        let mut store = self.store();
        if store.get(scope, name).is_none() {
            return Ok(None);
        }
        let rev = self.hub.lock().bump_config_rev_for_secrets()?;
        store.delete(scope, name, rev)?;
        self.snapshot_after_change(&store);
        Ok(Some(rev))
    }
}

/// Conferência dos snapshots de secrets: a primeira já no start, depois a
/// cada [`SNAPSHOT_CHECK_INTERVAL`]. Nada se os snapshots estão desligados.
pub fn spawn_snapshot_check(secrets: Arc<HubSecrets>) {
    if secrets.snapshot_dir().is_none() {
        return;
    }
    tokio::spawn(async move {
        loop {
            let s = Arc::clone(&secrets);
            if let Err(e) = tokio::task::spawn_blocking(move || s.check_snapshots()).await {
                tracing::warn!("conferência dos snapshots de secrets falhou: {e}");
            }
            tokio::time::sleep(SNAPSHOT_CHECK_INTERVAL).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use ct_ledger::Hub;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;

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

    #[test]
    fn gera_a_chave_e_muda_o_config_rev() {
        let d = tempfile::tempdir().unwrap();
        let ds = dirs(d.path());
        let hub = SharedHub::new(Hub::open(&ds.hub_db()).unwrap());
        let s = HubSecrets::open(&ds, hub.clone()).unwrap();
        assert_eq!(mode(&ds.secrets_key_file()), 0o600);
        assert!(!ds.secrets_file().exists());
        assert_eq!(
            s.status(),
            SecretsStatus {
                entries: 0,
                store_rev: 0,
                recovery_configured: false
            }
        );

        let r1 = s
            .set(Scope::Global, "A", Kind::Variable, SecretValue::new("g"))
            .unwrap();
        let p = Scope::parse_project("deep-ion-ai/demo").unwrap();
        let r2 = s
            .set(p.clone(), "A", Kind::Secret, SecretValue::new("p"))
            .unwrap();
        assert_eq!((r1, r2), (1, 2));
        assert_eq!(hub.lock().config_rev().unwrap(), 2);
        assert_eq!(mode(&ds.secrets_file()), 0o600);

        // Nome inválido e remoção de inexistente não mexem no config_rev.
        assert!(matches!(
            s.set(Scope::Global, "a", Kind::Secret, SecretValue::new("x")),
            Err(HubSecretsError::Secrets(
                ct_secrets::SecretsError::InvalidName(_)
            ))
        ));
        assert_eq!(s.delete(&Scope::Global, "NADA").unwrap(), None);
        assert_eq!(hub.lock().config_rev().unwrap(), 2);

        let r = s.resolve(Some("deep-ion-ai/demo"));
        assert_eq!(r["A"].value.expose(), "p");
        assert_eq!(r["A"].rev, 2);
        assert_eq!(s.delete(&p, "A").unwrap(), Some(3));
        assert_eq!(s.resolve(Some("deep-ion-ai/demo"))["A"].value.expose(), "g");

        // Reabrir usa a mesma chave e vê o mesmo conteúdo.
        drop(s);
        let s = HubSecrets::open(&ds, hub).unwrap();
        assert_eq!(s.status().store_rev, 3);
        assert_eq!(s.status().entries, 1);
    }

    /// Avisos dos snapshots, sem o de mesmo disco (o tempdir dos testes
    /// está sempre no disco do store).
    fn snap_kinds(s: &HubSecrets) -> Vec<&'static str> {
        s.snapshot_warnings()
            .iter()
            .map(|w| w.kind)
            .filter(|k| *k != "secrets_snapshot_same_device")
            .collect()
    }

    #[test]
    fn snapshot_a_cada_alteracao_e_destino_inacessivel_vira_aviso() {
        let d = tempfile::tempdir().unwrap();
        let ds = dirs(d.path());
        let hub = SharedHub::new(Hub::open(&ds.hub_db()).unwrap());
        // Destino sob um arquivo comum: não dá para criar.
        let bloqueio = d.path().join("bloqueio");
        std::fs::write(&bloqueio, "x").unwrap();
        let dest = bloqueio.join("snaps");
        let s = HubSecrets::open(&ds, hub.clone())
            .unwrap()
            .with_snapshot_dir(dest.clone());
        assert_eq!(s.snapshot_dir(), Some(dest.clone()));
        assert!(s
            .snapshot_warnings()
            .iter()
            .any(|w| w.kind == "secrets_snapshot_same_device"));
        // Store ainda vazio: nada a proteger, mas o destino não responde.
        s.check_snapshots();
        assert_eq!(snap_kinds(&s), ["secrets_snapshot_unreachable"]);

        // A alteração passa e fica no store local; o snapshot vira aviso.
        let rev = s
            .set(Scope::Global, "A", Kind::Secret, SecretValue::new("1"))
            .unwrap();
        assert_eq!(rev, 1);
        assert_eq!(s.resolve(None)["A"].value.expose(), "1");
        assert_eq!(
            HubSecrets::open(&ds, hub.clone()).unwrap().resolve(None)["A"]
                .value
                .expose(),
            "1"
        );
        assert_eq!(
            snap_kinds(&s),
            ["secrets_snapshot_unreachable", "secrets_snapshot_stale"]
        );
        let w = s.snapshot_warnings();
        let msg = &w
            .iter()
            .find(|w| w.kind == "secrets_snapshot_unreachable")
            .unwrap()
            .message;
        assert!(msg.contains("bloqueio/snaps"), "{msg}");
        assert!(s.snapshots.last_ok_at().is_none());

        // O destino volta: a conferência periódica tira o snapshot.
        std::fs::remove_file(&bloqueio).unwrap();
        s.check_snapshots();
        assert!(snap_kinds(&s).is_empty(), "{:?}", snap_kinds(&s));
        assert!(s.snapshots.last_ok_at().is_some());
        let l = snapshots::list(&dest).unwrap();
        assert_eq!(l.len(), 1);
        let store_bytes = || std::fs::read(ds.secrets_file()).unwrap();
        assert_eq!(std::fs::read(&l[0].path).unwrap(), store_bytes());
        assert_eq!(mode(&l[0].path), 0o600);
        assert_eq!(mode(&dest), 0o700);

        // Cada set/delete tira um; conferir sem mudança não tira outro.
        s.set(Scope::Global, "B", Kind::Variable, SecretValue::new("2"))
            .unwrap();
        s.delete(&Scope::Global, "A").unwrap();
        s.check_snapshots();
        let l = snapshots::list(&dest).unwrap();
        assert_eq!(l.len(), 3);
        assert_eq!(std::fs::read(&l[0].path).unwrap(), store_bytes());
        assert_eq!(s.snapshots.newest(), Some(l[0].path.clone()));

        // Some de novo no meio: o próximo set normaliza sozinho.
        std::fs::remove_dir_all(&dest).unwrap();
        std::fs::write(&dest, "x").unwrap();
        s.set(Scope::Global, "C", Kind::Secret, SecretValue::new("3"))
            .unwrap();
        assert_eq!(
            snap_kinds(&s),
            ["secrets_snapshot_unreachable", "secrets_snapshot_stale"]
        );
        std::fs::remove_file(&dest).unwrap();
        s.set(Scope::Global, "C", Kind::Secret, SecretValue::new("4"))
            .unwrap();
        assert!(snap_kinds(&s).is_empty());
        assert_eq!(
            std::fs::read(&snapshots::list(&dest).unwrap()[0].path).unwrap(),
            store_bytes()
        );
        assert_eq!(s.status().store_rev, 5);
    }

    #[test]
    fn com_chave_de_recuperacao_cifra_para_ela_tambem() {
        let d = tempfile::tempdir().unwrap();
        let ds = dirs(d.path());
        let rec = ct_secrets::Identity::generate();
        std::fs::write(ds.secrets_recovery_file(), format!("{}\n", rec.to_public())).unwrap();
        let hub = SharedHub::new(Hub::open_in_memory().unwrap());
        let s = HubSecrets::open(&ds, hub).unwrap();
        assert!(s.status().recovery_configured);
        s.set(Scope::Global, "T", Kind::Secret, SecretValue::new("v"))
            .unwrap();
        let pela_recuperacao = SecretsStore::open(ds.secrets_file(), rec, &[]).unwrap();
        assert_eq!(
            pela_recuperacao
                .get(&Scope::Global, "T")
                .unwrap()
                .value
                .expose(),
            "v"
        );
    }
}
