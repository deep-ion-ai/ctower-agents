//! Chave de recuperação do store de secrets (issue #169, épico #171).
//!
//! A chave privada fica fora da máquina, no gerenciador de senhas do
//! usuário; o hub guarda só a pública. O fluxo tem duas etapas:
//!
//! 1. [`HubSecrets::recovery_generate`] gera uma identidade age, devolve a
//!    chave privada **uma vez** e grava só a pública, como pendente, em
//!    `<config>/secrets.recovery.pending.pub` (0600). Gerar de novo troca a
//!    pendente. Com uma chave já configurada, gerar inicia uma troca: a
//!    atual continua valendo até a nova ser verificada.
//! 2. [`HubSecrets::recovery_verify`] recebe a chave colada de volta. Se a
//!    pública dela é a pendente, o hub prova a ida e volta (cifra o
//!    conteúdo atual para os destinatários e decifra em memória com a
//!    chave colada), regrava o store para hub + recuperação, confere que o
//!    arquivo gravado abre com a chave colada e só então promove a pendente
//!    a `<config>/secrets.recovery.pub`. Se a pública é a já configurada, é
//!    a reverificação periódica: confere que o store gravado abre com ela.
//!
//! Nos dois casos, a data da verificação bem-sucedida vai para
//! `<config>/secrets.recovery.verified` (JSON `{verified_at, recipient}`,
//! 0600), que alimenta o lembrete (`recovery_reverify_days`, padrão 90) em
//! `hub.status`. A chave colada só vive em memória ([`Zeroizing`] em quem
//! chama e a identidade do age, que se zera no `drop`): nunca vai para
//! disco, log nem mensagem de erro.
//!
//! Com os snapshots cifrados de secrets (#168), a verificação abre o
//! snapshot mais recente ([`HubSecrets::recovery_artifact`]): é ele que a
//! chave precisa abrir se a máquina for perdida. Promover uma chave recifra
//! os snapshots existentes para ela ([`HubSecrets::reencrypt_snapshots`]).

use super::{HubSecrets, HubSecretsError};
use crate::backup::Warning;
use crate::config::Dirs;
use ct_secrets::SecretsStore;
use ct_secrets::{Identity, Recipient};
use serde::{Deserialize, Serialize};
use std::io;
use std::path::PathBuf;

/// Padrão de `recovery_reverify_days`.
pub const DEFAULT_REVERIFY_DAYS: u64 = 90;

const DAY_MS: i64 = 24 * 60 * 60 * 1000;

/// Estado da chave de recuperação (`hub.recovery.status`). Só chaves
/// públicas: a privada nunca fica no hub.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct RecoveryStatus {
    /// Há chave verificada: o store é cifrado também para ela.
    pub configured: bool,
    /// Pública da chave configurada (`age1...`).
    pub public_key: Option<String>,
    /// Há chave gerada esperando a verificação.
    pub pending: bool,
    pub pending_public_key: Option<String>,
    /// Última verificação bem-sucedida da chave configurada (ms).
    pub last_verified_at: Option<i64>,
    /// Prazo do lembrete; `0` = lembrete desligado.
    pub reverify_days: u64,
    /// Quando a próxima verificação vence (ms); `null` sem chave
    /// configurada, sem verificação ou com o lembrete desligado.
    pub next_verify_at: Option<i64>,
    /// A verificação periódica venceu (ou a chave configurada nunca foi
    /// verificada).
    pub reverify_due: bool,
}

/// Resposta de `hub.recovery.generate`.
#[derive(Clone, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct RecoveryGenerated {
    /// A chave privada (`AGE-SECRET-KEY-1...`). Só nesta resposta: o hub
    /// não a guarda e não há como pedi-la de novo.
    pub private_key: String,
    /// A pública correspondente, agora pendente.
    pub public_key: String,
    pub status: RecoveryStatus,
}

impl std::fmt::Debug for RecoveryGenerated {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RecoveryGenerated")
            .field("private_key", &"***")
            .field("public_key", &self.public_key)
            .finish_non_exhaustive()
    }
}

/// Arquivos da chave de recuperação em `<config>`.
pub(super) struct RecoveryPaths {
    /// O store (`secrets.age`), que a verificação abre na falta de
    /// snapshot.
    store: PathBuf,
    /// Chave do hub (`secrets.key`), que abre os snapshots para recifrar.
    key: PathBuf,
    configured: PathBuf,
    pending: PathBuf,
    verified: PathBuf,
}

impl RecoveryPaths {
    pub(super) fn new(dirs: &Dirs) -> Self {
        RecoveryPaths {
            store: dirs.secrets_file(),
            key: dirs.secrets_key_file(),
            configured: dirs.secrets_recovery_file(),
            pending: dirs.secrets_recovery_pending_file(),
            verified: dirs.secrets_recovery_verified_file(),
        }
    }
}

/// Conteúdo de `secrets.recovery.verified`. A pública amarra a data à
/// chave: depois de uma troca, a data antiga não vale para a nova.
#[derive(Debug, Serialize, Deserialize)]
struct VerifiedFile {
    verified_at: i64,
    recipient: String,
}

/// O que o hub sabe da chave de recuperação, em memória.
#[derive(Debug, Clone, Default)]
pub(super) struct RecoveryState {
    pub(super) configured: Option<Recipient>,
    pub(super) pending: Option<Recipient>,
    /// Só vale para a `configured` atual.
    pub(super) verified_at: Option<i64>,
}

impl RecoveryState {
    /// Lê os arquivos. Uma data de verificação ilegível ou de outra chave
    /// conta como "nunca verificada" (só lembra antes da hora).
    pub(super) fn load(paths: &RecoveryPaths) -> Result<Self, HubSecretsError> {
        let configured = ct_secrets::read_recipient(&paths.configured)?;
        let pending = ct_secrets::read_recipient(&paths.pending)?;
        let verified_at = match (&configured, std::fs::read(&paths.verified)) {
            (Some(c), Ok(bytes)) => match serde_json::from_slice::<VerifiedFile>(&bytes) {
                Ok(v) if v.recipient == c.to_string() => Some(v.verified_at),
                Ok(_) => None,
                Err(e) => {
                    tracing::warn!("{}: ilegível ({e})", paths.verified.display());
                    None
                }
            },
            (_, Err(e)) if e.kind() != io::ErrorKind::NotFound => {
                return Err(io_err(&paths.verified, e));
            }
            _ => None,
        };
        Ok(RecoveryState {
            configured,
            pending,
            verified_at,
        })
    }

    fn status_at(&self, reverify_days: u64, now: i64) -> RecoveryStatus {
        let configured = self.configured.is_some();
        let period = (reverify_days as i64).saturating_mul(DAY_MS);
        let next_verify_at = match (configured, self.verified_at) {
            (true, Some(at)) if reverify_days > 0 => Some(at.saturating_add(period)),
            _ => None,
        };
        let reverify_due =
            configured && reverify_days > 0 && next_verify_at.is_none_or(|next| now >= next);
        RecoveryStatus {
            configured,
            public_key: self.configured.as_ref().map(ToString::to_string),
            pending: self.pending.is_some(),
            pending_public_key: self.pending.as_ref().map(ToString::to_string),
            last_verified_at: self.verified_at.filter(|_| configured),
            reverify_days,
            next_verify_at,
            reverify_due,
        }
    }
}

fn io_err(path: &std::path::Path, e: io::Error) -> HubSecretsError {
    HubSecretsError::Secrets(ct_secrets::SecretsError::Io {
        path: path.to_path_buf(),
        source: e,
    })
}

pub(super) fn write_public(
    path: &std::path::Path,
    title: &str,
    r: &Recipient,
) -> Result<(), HubSecretsError> {
    let text = format!("# {title} (age X25519)\n{r}\n");
    ct_core::files::write_private_atomic(path, text.as_bytes()).map_err(|e| io_err(path, e))
}

impl HubSecrets {
    fn recovery_state(&self) -> std::sync::MutexGuard<'_, RecoveryState> {
        self.recovery.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// `hub.recovery.status`.
    pub fn recovery_status(&self) -> RecoveryStatus {
        self.recovery_status_at(ct_core::now_ms())
    }

    fn recovery_status_at(&self, now: i64) -> RecoveryStatus {
        self.recovery_state().status_at(self.reverify_days, now)
    }

    /// Avisos de `hub.status`: `recovery_not_configured` enquanto não há
    /// chave verificada e `recovery_reverify_due` quando a verificação
    /// periódica venceu. Só pega o lock de `recovery`.
    pub fn recovery_warnings(&self) -> Vec<Warning> {
        self.recovery_warnings_at(ct_core::now_ms())
    }

    fn recovery_warnings_at(&self, now: i64) -> Vec<Warning> {
        let st = self.recovery_status_at(now);
        let mut out = Vec::new();
        if !st.configured {
            out.push(Warning {
                kind: "recovery_not_configured",
                message: if st.pending {
                    "chave de recuperação gerada, mas ainda não verificada: cole-a de volta na \
                     tela Hub para concluir. Até lá, perder a máquina do hub perde os secrets"
                        .into()
                } else {
                    "chave de recuperação não configurada: se a máquina do hub se perder, os \
                     secrets não voltam. Gere a chave na tela Hub e guarde no seu gerenciador \
                     de senhas"
                        .into()
                },
            });
        } else if st.reverify_due {
            out.push(Warning {
                kind: "recovery_reverify_due",
                message: match st.last_verified_at {
                    Some(_) => format!(
                        "a chave de recuperação não é verificada há mais de {} dias: cole-a de \
                         novo na tela Hub para conferir que ela ainda está no seu gerenciador \
                         de senhas",
                        st.reverify_days
                    ),
                    None => "a chave de recuperação nunca foi verificada: cole-a na tela Hub \
                             para conferir que ela abre o store"
                        .into(),
                },
            });
        }
        out
    }

    /// `hub.recovery.generate`: identidade nova; a pública vira a pendente
    /// e a privada só sai na resposta.
    pub fn recovery_generate(&self) -> Result<RecoveryGenerated, HubSecretsError> {
        // O lock do store serializa com `recovery_verify`.
        let _store = self.store();
        let identity = Identity::generate();
        let public = identity.to_public();
        write_public(
            &self.paths.pending,
            "chave pública de recuperação pendente do Control Tower: vira \
             secrets.recovery.pub quando a verificação passar",
            &public,
        )?;
        self.recovery_state().pending = Some(public.clone());
        tracing::info!("chave de recuperação gerada, esperando verificação: {public}");
        let private = ct_secrets::identity_text(&identity);
        Ok(RecoveryGenerated {
            private_key: private.to_string(),
            public_key: public.to_string(),
            status: self.recovery_status(),
        })
    }

    /// `hub.recovery.verify`: confere a chave colada (`key`, texto de uma
    /// identidade age) contra a pendente ou, sem pendente que bata, contra
    /// a configurada. A chave colada nunca é gravada; com chave errada,
    /// nada muda.
    pub fn recovery_verify(&self, key: &str) -> Result<RecoveryStatus, HubSecretsError> {
        let identity =
            ct_secrets::parse_identity(key).map_err(HubSecretsError::InvalidRecoveryKey)?;
        let public = identity.to_public();
        let mut store = self.store();
        let state = self.recovery_state().clone();
        let mismatch = |what| HubSecretsError::RecoveryKeyMismatch(what);

        if state.pending.as_ref() == Some(&public) {
            // Promoção: prova em memória, regrava para hub + recuperação,
            // confere o arquivo gravado e só então troca a pública.
            store
                .round_trip(std::slice::from_ref(&public), &identity)
                .map_err(|_| mismatch("gerada"))?;
            let old = store.extra_recipients().to_vec();
            store.set_recipients(std::slice::from_ref(&public))?;
            // O store regravado vira o snapshot mais recente (#168). Uma
            // falha do destino só vira aviso: a prova obrigatória é o
            // store gravado.
            self.snapshot_after_change(&store);
            let promote = || -> Result<(), HubSecretsError> {
                open_file(&self.paths.store, &identity).map_err(|_| mismatch("gerada"))?;
                write_public(
                    &self.paths.configured,
                    "chave pública de recuperação do Control Tower: o store de secrets \
                     também é cifrado para ela",
                    &public,
                )
            };
            if let Err(e) = promote() {
                if let Err(e2) = store.set_recipients(&old) {
                    tracing::warn!("falha ao voltar os destinatários do store: {e2}");
                }
                self.snapshot_after_change(&store);
                return Err(e);
            }
            match std::fs::remove_file(&self.paths.pending) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => tracing::warn!("{}: {e}", self.paths.pending.display()),
            }
            {
                let mut st = self.recovery_state();
                st.configured = Some(public.clone());
                st.pending = None;
                st.verified_at = None;
            }
            self.reencrypt_snapshots(&store);
            if let Err(e) = self.open_artifact(&identity) {
                // Só com o destino dos snapshots com problema (a cópia ou a
                // recifragem falhou): o aviso `secrets_snapshot_*` mostra.
                tracing::warn!(
                    "o snapshot mais recente dos secrets ainda não abre com a chave de \
                     recuperação: {e}"
                );
            }
            tracing::info!("chave de recuperação verificada e configurada: {public}");
        } else if state.configured.as_ref() == Some(&public) {
            // Reverificação periódica.
            if self.open_artifact(&identity).is_err() {
                // Store ou snapshots gravados antes desta chave valer (ex.:
                // pública posta à mão): regrava o store para os
                // destinatários atuais, tira um snapshot dele, recifra os
                // antigos e confere de novo.
                store.set_recipients(std::slice::from_ref(&public))?;
                self.snapshot_after_change(&store);
                self.reencrypt_snapshots(&store);
                self.open_artifact(&identity)
                    .map_err(|_| mismatch("configurada"))?;
            }
            tracing::info!("chave de recuperação reverificada: {public}");
        } else {
            return Err(match (&state.pending, &state.configured) {
                (None, None) => HubSecretsError::RecoveryNotStarted,
                (Some(_), _) => mismatch("gerada"),
                (None, Some(_)) => mismatch("configurada"),
            });
        }
        self.record_verified(&public)?;
        Ok(self.recovery_status())
    }

    /// Grava e adota a data da verificação de `public`.
    fn record_verified(&self, public: &Recipient) -> Result<(), HubSecretsError> {
        let now = ct_core::now_ms();
        let v = VerifiedFile {
            verified_at: now,
            recipient: public.to_string(),
        };
        let bytes = serde_json::to_vec(&v).expect("VerifiedFile serializa");
        ct_core::files::write_private_atomic(&self.paths.verified, &bytes)
            .map_err(|e| io_err(&self.paths.verified, e))?;
        self.recovery_state().verified_at = Some(now);
        Ok(())
    }

    /// O arquivo que a verificação abre com a chave colada: o snapshot
    /// cifrado mais recente (#168), que é o que a chave precisa abrir se a
    /// máquina for perdida; sem snapshot (desligados, ainda nenhum ou
    /// destino ilegível), o `secrets.age` atual.
    fn recovery_artifact(&self) -> PathBuf {
        self.snapshots
            .newest()
            .unwrap_or_else(|| self.paths.store.clone())
    }

    /// Decifra [`HubSecrets::recovery_artifact`] com `identity`, só em
    /// memória.
    fn open_artifact(&self, identity: &Identity) -> Result<(), HubSecretsError> {
        open_file(&self.recovery_artifact(), identity)
    }

    /// Recifra os snapshots existentes para os destinatários atuais do
    /// store (hub + recuperação) depois de configurar (ou trocar) a chave de
    /// recuperação: decifra cada um com a chave do hub, cifra de novo e
    /// grava no lugar, com o mesmo nome e mtime (a rotação continua igual).
    /// Chame com o lock do store. Uma falha só vira aviso
    /// (`secrets_snapshot_unreachable`).
    fn reencrypt_snapshots(&self, store: &SecretsStore) {
        if self.snapshots.dir().is_none() {
            return;
        }
        let hub = match ct_secrets::read_identity(&self.paths.key) {
            Ok(i) => i,
            Err(e) => {
                tracing::warn!("snapshots de secrets não recifrados: {e}");
                return;
            }
        };
        let mut recipients = vec![store.public_key()];
        recipients.extend(store.extra_recipients().iter().cloned());
        match self.snapshots.reencrypt(&hub, &recipients, store.path()) {
            Ok(0) => {}
            Ok(n) => tracing::info!("{n} snapshot(s) de secrets recifrados para a recuperação"),
            Err(e) => tracing::warn!("falha ao recifrar os snapshots de secrets: {e}"),
        }
    }
}

/// Decifra `path` com `identity`, só em memória.
fn open_file(path: &std::path::Path, identity: &Identity) -> Result<(), HubSecretsError> {
    let bytes = std::fs::read(path).map_err(|e| io_err(path, e))?;
    ct_secrets::decrypt(&bytes, identity)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ct_ledger::{Hub, SharedHub};
    use ct_secrets::{Kind, Scope, SecretValue, SecretsStore};
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

    fn abrir(ds: &Dirs) -> HubSecrets {
        let hub = SharedHub::new(Hub::open(&ds.hub_db()).unwrap());
        HubSecrets::open(ds, hub).unwrap()
    }

    fn mode(p: &Path) -> u32 {
        std::fs::metadata(p).unwrap().permissions().mode() & 0o777
    }

    /// Arquivos sob `dir` (recursivo) cujo conteúdo contém `needle`.
    fn achar(dir: &Path, needle: &str) -> Vec<PathBuf> {
        let mut out = Vec::new();
        for e in std::fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                out.extend(achar(&p, needle));
            } else if String::from_utf8_lossy(&std::fs::read(&p).unwrap()).contains(needle) {
                out.push(p);
            }
        }
        out
    }

    fn kinds(s: &HubSecrets, now: i64) -> Vec<&'static str> {
        s.recovery_warnings_at(now).iter().map(|w| w.kind).collect()
    }

    #[test]
    fn chave_privada_nunca_vai_para_o_disco() {
        let d = tempfile::tempdir().unwrap();
        let ds = dirs(d.path());
        let s = abrir(&ds);
        s.set(Scope::Global, "T", Kind::Secret, SecretValue::new("v"))
            .unwrap();
        assert_eq!(kinds(&s, ct_core::now_ms()), ["recovery_not_configured"]);

        let g = s.recovery_generate().unwrap();
        assert!(g.private_key.starts_with("AGE-SECRET-KEY-1"));
        assert!(!format!("{g:?}").contains(&g.private_key));
        assert!(g.status.pending && !g.status.configured);
        assert_eq!(g.status.pending_public_key.as_deref(), Some(&*g.public_key));
        assert_eq!(mode(&ds.secrets_recovery_pending_file()), 0o600);
        assert!(!ds.secrets_recovery_file().exists());
        assert!(!s.status().recovery_configured);
        let w = s.recovery_warnings_at(ct_core::now_ms());
        assert_eq!(w[0].kind, "recovery_not_configured");
        assert!(w[0].message.contains("não verificada"), "{}", w[0].message);

        let st = s.recovery_verify(&g.private_key).unwrap();
        assert!(st.configured && !st.pending && !st.reverify_due);
        assert_eq!(st.public_key.as_deref(), Some(&*g.public_key));
        assert!(st.last_verified_at.is_some());
        assert!(s.status().recovery_configured);
        assert!(!ds.secrets_recovery_pending_file().exists());
        assert_eq!(mode(&ds.secrets_recovery_file()), 0o600);
        assert_eq!(mode(&ds.secrets_recovery_verified_file()), 0o600);
        assert!(kinds(&s, ct_core::now_ms()).is_empty());

        // O store gravado abre com a chave de recuperação.
        let rec = ct_secrets::parse_identity(&g.private_key).ok().unwrap();
        let pela_recuperacao = SecretsStore::open(ds.secrets_file(), rec, &[]).unwrap();
        assert_eq!(
            pela_recuperacao
                .get(&Scope::Global, "T")
                .unwrap()
                .value
                .expose(),
            "v"
        );

        // A reverificação também não grava a chave.
        s.recovery_verify(&g.private_key).unwrap();
        for dir in [&ds.config, &ds.data] {
            assert_eq!(achar(dir, &g.private_key), Vec::<PathBuf>::new());
            assert_eq!(achar(dir, &g.private_key[15..]), Vec::<PathBuf>::new());
        }

        // Reabrir lembra a chave e a data da verificação.
        let last = s.recovery_status().last_verified_at;
        drop(s);
        let s = abrir(&ds);
        let st = s.recovery_status();
        assert!(st.configured && !st.pending);
        assert_eq!(st.last_verified_at, last);
        assert!(s.status().recovery_configured);
    }

    #[test]
    fn chave_errada_falha_e_nao_muda_nada() {
        let d = tempfile::tempdir().unwrap();
        let ds = dirs(d.path());
        let s = abrir(&ds);
        let outra = ct_secrets::identity_text(&Identity::generate());
        assert!(matches!(
            s.recovery_verify(&outra),
            Err(HubSecretsError::RecoveryNotStarted)
        ));

        let g = s.recovery_generate().unwrap();
        let antes = s.recovery_status();
        let e = s.recovery_verify(&outra).unwrap_err();
        assert!(matches!(e, HubSecretsError::RecoveryKeyMismatch(_)), "{e}");
        assert!(e.to_string().contains("não confere"), "{e}");
        assert!(!e.to_string().contains(outra.as_str()));
        let e = s.recovery_verify("AGE-SECRET-KEY-1LIXOLIXO").unwrap_err();
        assert!(matches!(e, HubSecretsError::InvalidRecoveryKey(_)), "{e}");
        assert!(!e.to_string().contains("LIXOLIXO"), "{e}");
        assert_eq!(s.recovery_status(), antes);
        assert!(!ds.secrets_recovery_file().exists());
        assert!(!ds.secrets_file().exists());
        assert!(!ds.secrets_recovery_verified_file().exists());

        // Colar com espaços e quebra de linha funciona.
        s.recovery_verify(&format!("\n  {}  \n", g.private_key))
            .unwrap();
        assert!(s.recovery_status().configured);
    }

    #[test]
    fn troca_de_chave_so_vale_depois_de_verificar() {
        let d = tempfile::tempdir().unwrap();
        let ds = dirs(d.path());
        let s = abrir(&ds);
        let a = s.recovery_generate().unwrap();
        s.recovery_verify(&a.private_key).unwrap();
        s.set(Scope::Global, "T", Kind::Secret, SecretValue::new("v"))
            .unwrap();

        let b = s.recovery_generate().unwrap();
        let st = s.recovery_status();
        assert!(st.configured && st.pending);
        assert_eq!(st.public_key.as_deref(), Some(&*a.public_key));
        // Gerar de novo troca a pendente.
        let c = s.recovery_generate().unwrap();
        assert_ne!(b.public_key, c.public_key);
        assert!(matches!(
            s.recovery_verify(&b.private_key),
            Err(HubSecretsError::RecoveryKeyMismatch(_))
        ));
        // A atual continua valendo (e reverifica) durante a troca.
        s.recovery_verify(&a.private_key).unwrap();
        let ida = |k: &str| ct_secrets::parse_identity(k).ok().unwrap();
        assert!(SecretsStore::open(ds.secrets_file(), ida(&a.private_key), &[]).is_ok());

        let st = s.recovery_verify(&c.private_key).unwrap();
        assert_eq!(st.public_key.as_deref(), Some(&*c.public_key));
        assert!(!st.pending);
        assert!(SecretsStore::open(ds.secrets_file(), ida(&c.private_key), &[]).is_ok());
        assert!(SecretsStore::open(ds.secrets_file(), ida(&a.private_key), &[]).is_err());
        assert!(matches!(
            s.recovery_verify(&a.private_key),
            Err(HubSecretsError::RecoveryKeyMismatch("configurada"))
        ));
    }

    fn snapshots(dir: &Path) -> Vec<PathBuf> {
        super::super::snapshots::list(dir)
            .unwrap()
            .into_iter()
            .map(|s| s.path)
            .collect()
    }

    fn valor(path: &Path, id: Identity) -> String {
        let s = SecretsStore::open(path, id, &[]).unwrap();
        s.get(&Scope::Global, "T")
            .unwrap()
            .value
            .expose()
            .to_string()
    }

    #[test]
    fn snapshots_recifrados_e_copia_abre_com_a_recuperacao() {
        let d = tempfile::tempdir().unwrap();
        let ds = dirs(d.path());
        let dest = d.path().join("OneDrive/cthub");
        let s = abrir(&ds).with_snapshot_dir(dest.clone());
        s.set(Scope::Global, "T", Kind::Secret, SecretValue::new("antes"))
            .unwrap();
        let antigo = snapshots(&dest)[0].clone();
        let mtime = std::fs::metadata(&antigo).unwrap().modified().unwrap();

        let g = s.recovery_generate().unwrap();
        let rec = || ct_secrets::parse_identity(&g.private_key).ok().unwrap();
        // Antes da verificação, o snapshot é só do hub.
        assert!(SecretsStore::open(&antigo, rec(), &[]).is_err());
        s.recovery_verify(&g.private_key).unwrap();

        // O antigo foi recifrado no lugar: mesmo nome e mtime, abre com a
        // recuperação.
        assert_eq!(valor(&antigo, rec()), "antes");
        assert_eq!(
            std::fs::metadata(&antigo).unwrap().modified().unwrap(),
            mtime
        );
        assert_eq!(mode(&antigo), 0o600);
        // O store regravado para hub + recuperação é o mais recente.
        let l = snapshots(&dest);
        assert_eq!(l.len(), 2);
        assert_eq!(
            std::fs::read(&l[0]).unwrap(),
            std::fs::read(ds.secrets_file()).unwrap()
        );
        assert_eq!(s.recovery_artifact(), l[0]);

        // Um snapshot copiado para outra máquina abre só com a chave de
        // recuperação (sem `secrets.key`).
        s.set(Scope::Global, "T", Kind::Secret, SecretValue::new("depois"))
            .unwrap();
        let outra = tempfile::tempdir().unwrap();
        let copia = outra.path().join("secrets-copia.age");
        std::fs::copy(&snapshots(&dest)[0], &copia).unwrap();
        assert_eq!(valor(&copia, rec()), "depois");
        assert!(SecretsStore::open(&copia, Identity::generate(), &[]).is_err());

        s.check_snapshots();
        assert!(s
            .snapshot_warnings()
            .iter()
            .all(|w| w.kind == "secrets_snapshot_same_device"));
        // Nenhum arquivo dos snapshots contém a chave privada.
        assert_eq!(achar(&dest, &g.private_key), Vec::<PathBuf>::new());
    }

    #[test]
    fn verificacao_usa_o_snapshot_mais_recente() {
        let d = tempfile::tempdir().unwrap();
        let ds = dirs(d.path());
        // Sem snapshots, a verificação abre o secrets.age.
        let s = abrir(&ds);
        assert_eq!(s.recovery_artifact(), ds.secrets_file());
        drop(s);

        let dest = d.path().join("snaps");
        let s = abrir(&ds).with_snapshot_dir(dest.clone());
        // Ligados, mas ainda sem snapshot: também o secrets.age.
        assert_eq!(s.recovery_artifact(), ds.secrets_file());
        s.set(Scope::Global, "T", Kind::Secret, SecretValue::new("v"))
            .unwrap();
        let g = s.recovery_generate().unwrap();
        s.recovery_verify(&g.private_key).unwrap();
        let rec = || ct_secrets::parse_identity(&g.private_key).ok().unwrap();
        assert_eq!(s.recovery_artifact(), snapshots(&dest)[0]);

        // Um snapshot mais recente cifrado só para o hub: a reverificação
        // percebe, recifra e passa.
        let hub_key = ct_secrets::read_identity(&ds.secrets_key_file()).unwrap();
        let futuro = dest.join("secrets-29991231T000000Z.age");
        let so_hub = ct_secrets::encrypt(b"conteudo", &[hub_key.to_public()]).unwrap();
        std::fs::write(&futuro, so_hub).unwrap();
        assert_eq!(s.recovery_artifact(), futuro);
        s.recovery_verify(&g.private_key).unwrap();
        let aberto = ct_secrets::decrypt(&std::fs::read(&futuro).unwrap(), &rec()).unwrap();
        assert_eq!(aberto.as_slice(), b"conteudo");

        // O mais recente ilegível: a verificação falha, porque é ele que a
        // chave precisa abrir se a máquina se perder.
        std::fs::write(&futuro, "lixo").unwrap();
        assert!(matches!(
            s.recovery_verify(&g.private_key),
            Err(HubSecretsError::RecoveryKeyMismatch("configurada"))
        ));
        std::fs::remove_file(&futuro).unwrap();
        s.recovery_verify(&g.private_key).unwrap();
    }

    #[test]
    fn lembrete_de_reverificacao() {
        let d = tempfile::tempdir().unwrap();
        let ds = dirs(d.path());
        let s = abrir(&ds);
        let g = s.recovery_generate().unwrap();
        s.recovery_verify(&g.private_key).unwrap();
        let at = s.recovery_status().last_verified_at.unwrap();
        assert_eq!(s.recovery_status().next_verify_at, Some(at + 90 * DAY_MS));
        assert!(kinds(&s, at + 89 * DAY_MS).is_empty());
        let w = s.recovery_warnings_at(at + 90 * DAY_MS);
        assert_eq!(w[0].kind, "recovery_reverify_due");
        assert!(w[0].message.contains("90 dias"), "{}", w[0].message);
        assert!(s.recovery_status_at(at + 91 * DAY_MS).reverify_due);

        // Prazo do config; 0 desliga.
        drop(s);
        let s = abrir(&ds).with_reverify_days(30);
        assert_eq!(kinds(&s, at + 31 * DAY_MS), ["recovery_reverify_due"]);
        let s = s.with_reverify_days(0);
        assert!(kinds(&s, at + 1000 * DAY_MS).is_empty());
        assert_eq!(s.recovery_status().next_verify_at, None);

        // Pública posta à mão, nunca verificada: lembra já; a verificação
        // regrava o store para ela.
        drop(s);
        let manual = Identity::generate();
        std::fs::write(
            ds.secrets_recovery_file(),
            format!("{}\n", manual.to_public()),
        )
        .unwrap();
        let s = abrir(&ds);
        let st = s.recovery_status();
        assert!(st.configured && st.reverify_due && st.last_verified_at.is_none());
        let w = s.recovery_warnings_at(ct_core::now_ms());
        assert!(w[0].message.contains("nunca"), "{}", w[0].message);
        s.recovery_verify(&ct_secrets::identity_text(&manual))
            .unwrap();
        assert!(kinds(&s, ct_core::now_ms()).is_empty());
        assert!(SecretsStore::open(ds.secrets_file(), manual, &[]).is_ok());
    }
}
