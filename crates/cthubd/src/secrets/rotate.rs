//! Rotação das chaves do store de secrets (issue #170, épico #171):
//! `cthubd secrets rotate-keys [--hub] [--recovery]` (sem flags = as duas),
//! com o serviço parado.
//!
//! - `--hub`: gera uma `secrets.key` nova, recifra o `secrets.age` para ela
//!   e para a recuperação **configurada**, recifra cada snapshot (abrindo
//!   com a chave antiga ou a nova) e **apaga** os que nenhuma das duas abre
//!   ou que não deu para recifrar. A chave antiga só sai do disco depois
//!   que tudo isso deu certo; numa falha no meio ela fica em
//!   `secrets.key.old` e o erro diz o que faltou. O `config_rev` não muda:
//!   os valores são os mesmos e a entrega aos ambientes usa a chave de cada
//!   ambiente, não a do hub.
//! - `--recovery`: gera uma identidade de recuperação nova; a privada sai
//!   **uma vez** no terminal e a pública vira a pendente
//!   (`secrets.recovery.pending.pub`). A atual continua valendo até a nova
//!   ser verificada na tela Hub (#169), que então recifra o store e os
//!   snapshots para ela. Por isso o `--hub` desta mesma execução recifra
//!   para a recuperação atual, não para a pendente.
//!
//! Ordem do `--hub`, segura contra queda no meio: (1) grava
//! `secrets.key.new`; (2) recifra o store para a antiga + a nova (+
//! recuperação), então qualquer das duas abre; (3) copia a antiga para
//! `secrets.key.old` e troca `secrets.key.new` por `secrets.key`; (4)
//! recifra o store só para a nova (+ recuperação); (5) os snapshots; (6)
//! snapshot do store novo; (7) apaga `secrets.key.old`.

use super::{snapshots, HubSecrets};
use crate::config::Dirs;
use ct_ledger::{Hub, SharedHub};
use ct_secrets::{Identity, Zeroizing};
use std::io;
use std::path::{Path, PathBuf};

/// O que rotacionar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RotateOpts {
    pub hub: bool,
    pub recovery: bool,
}

/// Resultado do `--hub`.
#[derive(Debug)]
pub struct HubRotated {
    pub public: String,
    pub snapshots_reencrypted: usize,
    /// Snapshots apagados (nenhuma chave abre ou não deu para recifrar).
    pub snapshots_removed: Vec<PathBuf>,
    /// Snapshot do store recifrado; `Err` = falhou (a rotação vale).
    pub snapshot: Result<Option<PathBuf>, String>,
}

/// Resultado do `--recovery`.
pub struct RecoveryRotated {
    /// A chave privada nova: mostrar uma vez e esquecer.
    pub private_key: Zeroizing<String>,
    pub public: String,
}

impl std::fmt::Debug for RecoveryRotated {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RecoveryRotated")
            .field("private_key", &"***")
            .field("public", &self.public)
            .finish()
    }
}

#[derive(Debug, Default)]
pub struct RotateReport {
    pub hub: Option<HubRotated>,
    pub recovery: Option<RecoveryRotated>,
}

/// `cthubd secrets rotate-keys` (veja o módulo). Chame com o serviço
/// parado. `snapshot_dir` = destino dos snapshots.
pub fn rotate_keys(
    dirs: &Dirs,
    opts: RotateOpts,
    snapshot_dir: &Path,
) -> Result<RotateReport, String> {
    let mut report = RotateReport::default();
    if opts.hub {
        report.hub = Some(rotate_hub(dirs, snapshot_dir)?);
    }
    if opts.recovery {
        let identity = Identity::generate();
        let public = identity.to_public();
        super::recovery::write_public(
            &dirs.secrets_recovery_pending_file(),
            "chave pública de recuperação pendente do Control Tower: vira \
             secrets.recovery.pub quando a verificação passar",
            &public,
        )
        .map_err(|e| e.to_string())?;
        report.recovery = Some(RecoveryRotated {
            private_key: ct_secrets::identity_text(&identity),
            public: public.to_string(),
        });
    }
    Ok(report)
}

fn rotate_hub(dirs: &Dirs, snapshot_dir: &Path) -> Result<HubRotated, String> {
    let s = |e: ct_secrets::SecretsError| e.to_string();
    let key = dirs.secrets_key_file();
    let key_new = dirs.config.join("secrets.key.new");
    let key_old = dirs.config.join("secrets.key.old");
    let store_file = dirs.secrets_file();
    if !key.exists() {
        return Err(format!(
            "{} não existe: não há chave do hub para rotacionar",
            key.display()
        ));
    }
    let old = ct_secrets::read_identity(&key).map_err(s)?;
    let recovery = ct_secrets::read_recipient(&dirs.secrets_recovery_file()).map_err(s)?;
    let plain = match std::fs::read(&store_file) {
        Ok(bytes) => Some(ct_secrets::decrypt(&bytes, &old).map_err(|e| {
            format!(
                "{} não abre com a chave atual do hub ({e}): nada foi alterado",
                store_file.display()
            )
        })?),
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(e) => return Err(format!("{}: {e}", store_file.display())),
    };
    let new = Identity::generate();
    let mut final_recipients = vec![new.to_public()];
    final_recipients.extend(recovery.iter().cloned());

    // (1) e (2): a antiga e a nova abrem o store durante a troca.
    ct_secrets::write_identity(&key_new, &new, "chave do store de secrets do Control Tower")
        .map_err(s)?;
    let seal = |recipients: &[ct_secrets::Recipient]| -> Result<(), String> {
        let Some(plain) = &plain else {
            return Ok(());
        };
        let sealed = ct_secrets::encrypt(plain, recipients).map_err(s)?;
        ct_core::files::write_private_atomic(&store_file, &sealed)
            .map_err(|e| format!("{}: {e}", store_file.display()))
    };
    let mut transition = vec![old.to_public()];
    transition.extend(final_recipients.iter().cloned());
    if let Err(e) = seal(&transition) {
        let _ = std::fs::remove_file(&key_new);
        return Err(format!("{e}: nada foi alterado"));
    }
    // (3)
    std::fs::copy(&key, &key_old)
        .and_then(|_| {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&key_old, std::fs::Permissions::from_mode(0o600))
        })
        .map_err(|e| {
            format!(
                "{}: {e} (o store abre com a chave atual)",
                key_old.display()
            )
        })?;
    std::fs::rename(&key_new, &key)
        .map_err(|e| format!("{}: {e} (o store abre com a chave atual)", key.display()))?;
    let kept = |what: String| {
        format!(
            "{what}. A chave antiga ficou em {}: apague-a depois de resolver e rode de novo",
            key_old.display()
        )
    };
    // (4)
    seal(&final_recipients).map_err(kept)?;
    // (5)
    let rekeyed = snapshots::rekey(snapshot_dir, &[&old, &new], &final_recipients)
        .map_err(|e| kept(format!("snapshots em {}: {e}", snapshot_dir.display())))?;
    // (6) confere que o store abre com a nova e tira o snapshot dele.
    let hub = Hub::open(&dirs.hub_db()).map_err(|e| kept(format!("hub.db: {e}")))?;
    let secrets = HubSecrets::open(dirs, SharedHub::new(hub))
        .map_err(|e| kept(format!("o store não abre com a chave nova: {e}")))?
        .with_snapshot_dir(snapshot_dir.to_path_buf());
    let snapshot = secrets.snapshot_now();
    // (7)
    std::fs::remove_file(&key_old).map_err(|e| format!("{}: {e}", key_old.display()))?;
    Ok(HubRotated {
        public: new.to_public().to_string(),
        snapshots_reencrypted: rekeyed.reencrypted,
        snapshots_removed: rekeyed.removed,
        snapshot,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ct_secrets::{Kind, Scope, SecretValue, SecretsStore};

    fn dirs(root: &Path) -> Dirs {
        let d = Dirs {
            config: root.join("config"),
            data: root.join("data"),
        };
        d.ensure().unwrap();
        d
    }

    fn abrir(ds: &Dirs, snaps: &Path) -> HubSecrets {
        let hub = SharedHub::new(Hub::open(&ds.hub_db()).unwrap());
        HubSecrets::open(ds, hub)
            .unwrap()
            .with_snapshot_dir(snaps.to_path_buf())
    }

    fn abre(path: &Path, id: &Identity) -> bool {
        ct_secrets::decrypt(&std::fs::read(path).unwrap(), id).is_ok()
    }

    #[test]
    fn rotacao_da_chave_do_hub() {
        let d = tempfile::tempdir().unwrap();
        let ds = dirs(d.path());
        let snaps = d.path().join("snaps");
        let s = abrir(&ds, &snaps);
        let g = s.recovery_generate().unwrap();
        s.recovery_verify(&g.private_key).unwrap();
        s.set(
            Scope::Global,
            "T",
            Kind::Secret,
            SecretValue::new("valor-1234"),
        )
        .unwrap();
        s.set(
            Scope::Global,
            "T",
            Kind::Secret,
            SecretValue::new("valor-5678"),
        )
        .unwrap();
        let rev = Hub::open(&ds.hub_db()).unwrap().config_rev().unwrap();
        drop(s);
        let antiga = ct_secrets::read_identity(&ds.secrets_key_file()).unwrap();
        let rec = ct_secrets::parse_identity(&g.private_key).ok().unwrap();
        // Uma cópia antiga fora da pasta (histórico do OneDrive) e um
        // snapshot de uma chave que ninguém mais tem.
        let antes = snapshots::list(&snaps).unwrap();
        let copia = d.path().join("copia.age");
        std::fs::copy(&antes[0].path, &copia).unwrap();
        let alheio = snaps.join("secrets-20200101T000000Z.age");
        let ninguem = Identity::generate();
        std::fs::write(
            &alheio,
            ct_secrets::encrypt(b"x", &[ninguem.to_public()]).unwrap(),
        )
        .unwrap();

        let r = rotate_keys(
            &ds,
            RotateOpts {
                hub: true,
                recovery: false,
            },
            &snaps,
        )
        .unwrap();
        assert!(r.recovery.is_none());
        let h = r.hub.unwrap();
        let nova = ct_secrets::read_identity(&ds.secrets_key_file()).unwrap();
        assert_ne!(nova.to_public(), antiga.to_public());
        assert_eq!(h.public, nova.to_public().to_string());
        assert_eq!(h.snapshots_removed, vec![alheio.clone()]);
        assert_eq!(h.snapshots_reencrypted, antes.len());
        assert!(!alheio.exists());
        // A antiga e a temporária não ficam em disco.
        assert!(!ds.config.join("secrets.key.old").exists());
        assert!(!ds.config.join("secrets.key.new").exists());
        // O snapshot antigo não abre com a chave nova; na pasta, nenhum
        // abre com a antiga e todos com a nova e a recuperação.
        assert!(!abre(&copia, &nova));
        let depois = snapshots::list(&snaps).unwrap();
        assert_eq!(depois.len(), antes.len() + 1, "+ o do store recifrado");
        for sn in &depois {
            assert!(!abre(&sn.path, &antiga), "{}", sn.path.display());
            assert!(abre(&sn.path, &nova) && abre(&sn.path, &rec));
        }
        assert_eq!(
            std::fs::read(&depois[0].path).unwrap(),
            std::fs::read(ds.secrets_file()).unwrap()
        );
        // O store abre com a nova e a recuperação; não com a antiga.
        let st = SecretsStore::open(ds.secrets_file(), nova, &[]).unwrap();
        assert_eq!(
            st.get(&Scope::Global, "T").unwrap().value.expose(),
            "valor-5678"
        );
        assert!(SecretsStore::open(ds.secrets_file(), antiga, &[]).is_err());
        assert!(SecretsStore::open(ds.secrets_file(), rec, &[]).is_ok());
        // Valores iguais: o config_rev não muda.
        assert_eq!(Hub::open(&ds.hub_db()).unwrap().config_rev().unwrap(), rev);
        // O serviço sobe normalmente com a chave nova.
        assert_eq!(abrir(&ds, &snaps).status().entries, 1);
    }

    #[test]
    fn rotacao_da_recuperacao_fica_pendente() {
        let d = tempfile::tempdir().unwrap();
        let ds = dirs(d.path());
        let snaps = d.path().join("snaps");
        let s = abrir(&ds, &snaps);
        let g = s.recovery_generate().unwrap();
        s.recovery_verify(&g.private_key).unwrap();
        s.set(
            Scope::Global,
            "T",
            Kind::Secret,
            SecretValue::new("valor-1234"),
        )
        .unwrap();
        drop(s);
        let store = std::fs::read(ds.secrets_file()).unwrap();
        let chave = std::fs::read(ds.secrets_key_file()).unwrap();

        let r = rotate_keys(
            &ds,
            RotateOpts {
                hub: false,
                recovery: true,
            },
            &snaps,
        )
        .unwrap();
        assert!(r.hub.is_none());
        let rec = r.recovery.unwrap();
        assert!(rec.private_key.starts_with("AGE-SECRET-KEY-1"));
        assert!(!format!("{rec:?}").contains(rec.private_key.as_str()));
        // Store e chave do hub intactos; a nova fica pendente.
        assert_eq!(std::fs::read(ds.secrets_file()).unwrap(), store);
        assert_eq!(std::fs::read(ds.secrets_key_file()).unwrap(), chave);
        let s = abrir(&ds, &snaps);
        let st = s.recovery_status();
        assert!(st.configured && st.pending);
        assert_eq!(st.public_key.as_deref(), Some(&*g.public_key));
        assert_eq!(st.pending_public_key.as_deref(), Some(&*rec.public));
        // A verificação na UI (#169) conclui a troca.
        s.recovery_verify(&rec.private_key).unwrap();
        let nova = ct_secrets::parse_identity(&rec.private_key).ok().unwrap();
        assert!(SecretsStore::open(ds.secrets_file(), nova, &[]).is_ok());
    }

    #[test]
    fn rotacao_sem_chave_do_hub_falha() {
        let d = tempfile::tempdir().unwrap();
        let ds = dirs(d.path());
        let e = rotate_keys(
            &ds,
            RotateOpts {
                hub: true,
                recovery: true,
            },
            &d.path().join("snaps"),
        )
        .unwrap_err();
        assert!(e.contains("não há chave do hub"), "{e}");
        assert!(!ds.secrets_recovery_pending_file().exists());
    }
}
