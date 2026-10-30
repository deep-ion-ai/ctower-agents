//! Entrega dos secrets do hub aos ambientes e o cache cifrado no ambiente
//! (issue #184, épico #171).
//!
//! - O `ctowerd` tem uma identidade age própria (`<config>/env.key`, 0600) e
//!   manda a chave pública ao hub em todo `GET /sync/config`.
//! - Quando a configuração muda (ou a chave pública muda), o hub responde com
//!   o conjunto **completo** das entradas globais e dos projetos atribuídos
//!   ao ambiente, cifrado só para essa chave ([`seal_delivery`], em base64 no
//!   campo `secrets`). O plaintext é um JSON versionado:
//!   `{"format": 1, "config_rev": N, "entries": [...]}`, com o escopo de cada
//!   entrada para o ambiente resolver por projeto.
//! - O ambiente grava o arquivo age recebido como está em
//!   `<data>/secrets-cache.age` (0600, escrita atômica) com
//!   [`EnvCache::store`]. Como o conjunto é completo, desatribuir um projeto
//!   tira os secrets dele do cache no pull seguinte. Hub fora do ar: vale o
//!   último cache ([`EnvCache::load`]).
//! - [`EnvCache::rotate_key`] troca a identidade do ambiente sem perder o
//!   cache (recifrado para a chave nova); a chave pública nova faz o hub
//!   reentregar no pull seguinte.

use crate::store::resolve_entries;
use crate::{keys, Entry, Result, SecretsError};
use age::x25519::{Identity, Recipient};
use base64::Engine;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use zeroize::Zeroizing;

/// Versão do formato do plaintext entregue ao ambiente.
pub const DELIVERY_FORMAT: u32 = 1;

#[derive(Serialize, Deserialize)]
struct Delivery {
    format: u32,
    config_rev: i64,
    entries: Vec<Entry>,
}

/// No hub: cifra `entries` (o conjunto completo do ambiente) para `to` e
/// devolve o arquivo age em base64, pronto para o campo `secrets` de
/// `GET /sync/config`.
pub fn seal_delivery(config_rev: i64, entries: Vec<Entry>, to: &Recipient) -> Result<String> {
    let doc = Delivery {
        format: DELIVERY_FORMAT,
        config_rev,
        entries,
    };
    let plain = Zeroizing::new(serde_json::to_vec(&doc).expect("Delivery serializa"));
    let sealed = keys::encrypt(&plain, std::slice::from_ref(to))?;
    Ok(base64::engine::general_purpose::STANDARD.encode(sealed))
}

/// No ambiente: o arquivo age do campo `secrets` (base64), ainda cifrado.
pub fn decode_delivery(b64: &str) -> Result<Vec<u8>> {
    base64::engine::general_purpose::STANDARD
        .decode(b64.trim())
        .map_err(|_| SecretsError::Decrypt("entrega de secrets fora do base64".into()))
}

/// Secrets e variáveis entregues ao ambiente, decifrados. O `Debug` não
/// mostra valores.
pub struct EnvSecrets {
    config_rev: i64,
    entries: Vec<Entry>,
}

impl std::fmt::Debug for EnvSecrets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EnvSecrets")
            .field("config_rev", &self.config_rev)
            .field("entries", &self.entries.len())
            .finish()
    }
}

impl EnvSecrets {
    /// Decifra uma entrega (o arquivo age, não o base64) com `identity`.
    pub fn open(sealed: &[u8], identity: &Identity) -> Result<Self> {
        let plain = keys::decrypt(sealed, identity)?;
        parse(&plain)
    }

    /// `config_rev` do hub na entrega.
    pub fn config_rev(&self) -> i64 {
        self.config_rev
    }

    /// Todas as entradas entregues (globais e dos projetos atribuídos).
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// Entradas que valem para `project` (`owner/repo`, sem diferenciar
    /// maiúsculas) ou, com `None`, só as globais. Por nome, a do projeto
    /// sobrepõe a global.
    pub fn resolve(&self, project: Option<&str>) -> BTreeMap<String, &Entry> {
        resolve_entries(&self.entries, project)
    }
}

fn parse(plain: &[u8]) -> Result<EnvSecrets> {
    #[derive(Deserialize)]
    struct Head {
        format: u32,
    }
    // Só a posição: a mensagem do serde pode citar o trecho (e o valor).
    let fmt_err = |e: serde_json::Error| SecretsError::Format {
        line: e.line(),
        column: e.column(),
    };
    let head: Head = serde_json::from_slice(plain).map_err(fmt_err)?;
    if head.format != DELIVERY_FORMAT {
        return Err(SecretsError::UnknownFormat(head.format));
    }
    let d: Delivery = serde_json::from_slice(plain).map_err(fmt_err)?;
    Ok(EnvSecrets {
        config_rev: d.config_rev,
        entries: d.entries,
    })
}

/// Chave e cache de secrets do ambiente: `<config>/env.key` e
/// `<data>/secrets-cache.age`, ambos 0600.
///
/// A rotação grava a chave nova primeiro em `<key>.new`, recifra o cache
/// para ela e só então troca `<key>`. Se o processo cair no meio,
/// [`EnvCache::ensure_key`] termina ou desfaz a troca conforme a chave que
/// abre o cache.
#[derive(Debug, Clone)]
pub struct EnvCache {
    key: PathBuf,
    cache: PathBuf,
}

/// Resultado de [`EnvCache::rotate_key`].
#[derive(Debug, Clone)]
pub struct Rotated {
    /// Chave pública nova (vai ao hub no próximo pull).
    pub public: Recipient,
    /// O cache existia e foi recifrado para a chave nova. `false` = não
    /// havia cache, ou ele não abria com a chave antiga e foi descartado (o
    /// hub reentrega no próximo pull).
    pub cache_kept: bool,
}

impl EnvCache {
    pub fn new(key: impl Into<PathBuf>, cache: impl Into<PathBuf>) -> Self {
        EnvCache {
            key: key.into(),
            cache: cache.into(),
        }
    }

    pub fn key_path(&self) -> &Path {
        &self.key
    }

    pub fn cache_path(&self) -> &Path {
        &self.cache
    }

    fn pending_key(&self) -> PathBuf {
        let mut s: OsString = self.key.as_os_str().to_owned();
        s.push(".new");
        PathBuf::from(s)
    }

    /// Identidade do ambiente; gera na primeira vez (0600). Termina (ou
    /// desfaz) uma rotação interrompida.
    pub fn ensure_key(&self) -> Result<Identity> {
        let pending = self.pending_key();
        if pending.exists() {
            self.finish_rotation(&pending)?;
        }
        if !self.key.exists() {
            let id = Identity::generate();
            keys::write_identity(&self.key, &id, "chave do ambiente do Control Tower")?;
            return Ok(id);
        }
        keys::ensure_identity(&self.key)
    }

    /// Rotação interrompida: se o cache já abre com a chave nova, ela vale;
    /// senão (ou sem cache), fica a antiga.
    fn finish_rotation(&self, pending: &Path) -> Result<()> {
        let adopt = match (keys::read_identity(pending), fs::read(&self.cache)) {
            (Ok(new), Ok(bytes)) => keys::decrypt(&bytes, &new).is_ok(),
            _ => false,
        };
        if adopt {
            rename_synced(pending, &self.key)
        } else {
            fs::remove_file(pending).map_err(|e| SecretsError::io(pending, e))
        }
    }

    /// Chave pública do ambiente (gera a identidade se faltar).
    pub fn public_key(&self) -> Result<Recipient> {
        Ok(self.ensure_key()?.to_public())
    }

    /// Grava uma entrega (o arquivo age, não o base64) como cache, depois de
    /// conferir que ela abre com a chave do ambiente: uma entrega ruim nunca
    /// troca um cache bom.
    pub fn store(&self, sealed: &[u8]) -> Result<EnvSecrets> {
        let identity = self.ensure_key()?;
        let secrets = EnvSecrets::open(sealed, &identity)?;
        if let Some(dir) = self.cache.parent().filter(|d| !d.as_os_str().is_empty()) {
            ct_core::files::private_dir(dir).map_err(|e| SecretsError::io(dir, e))?;
        }
        ct_core::files::write_private_atomic(&self.cache, sealed)
            .map_err(|e| SecretsError::io(&self.cache, e))?;
        Ok(secrets)
    }

    /// Lê o cache (hub no ar ou não). `None` = nenhuma entrega ainda. Só
    /// lê: não gera chave nem mexe numa rotação em andamento (se o cache já
    /// estiver cifrado para a chave nova, tenta `<key>.new`).
    pub fn load(&self) -> Result<Option<EnvSecrets>> {
        let bytes = match fs::read(&self.cache) {
            Ok(b) => b,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(SecretsError::io(&self.cache, e)),
        };
        let identity = keys::read_identity(&self.key)?;
        match EnvSecrets::open(&bytes, &identity) {
            Ok(s) => Ok(Some(s)),
            Err(e @ SecretsError::Decrypt(_)) => match keys::read_identity(&self.pending_key()) {
                Ok(new) => EnvSecrets::open(&bytes, &new).map(Some),
                Err(_) => Err(e),
            },
            Err(e) => Err(e),
        }
    }

    /// Apaga o cache (ex.: o ambiente deixou de estar ligado a um hub).
    pub fn clear(&self) -> Result<bool> {
        match fs::remove_file(&self.cache) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(SecretsError::io(&self.cache, e)),
        }
    }

    /// Gera uma identidade nova, recifra o cache para ela (continua usável
    /// offline) e troca `<key>` atomicamente. Quem chama manda a chave
    /// pública nova ao hub (pull), que reentrega.
    pub fn rotate_key(&self) -> Result<Rotated> {
        let old = self.ensure_key()?;
        let new = Identity::generate();
        let pending = self.pending_key();
        keys::write_identity(&pending, &new, "chave do ambiente do Control Tower")?;
        let r = self.reseal_cache(&old, &new);
        let cache_kept = match r {
            Ok(kept) => kept,
            Err(e) => {
                let _ = fs::remove_file(&pending);
                return Err(e);
            }
        };
        rename_synced(&pending, &self.key)?;
        Ok(Rotated {
            public: new.to_public(),
            cache_kept,
        })
    }

    /// Recifra o cache de `old` para `new`. `false` se não havia cache ou se
    /// ele não abria com `old` (descartado: o hub reentrega).
    fn reseal_cache(&self, old: &Identity, new: &Identity) -> Result<bool> {
        let bytes = match fs::read(&self.cache) {
            Ok(b) => b,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(e) => return Err(SecretsError::io(&self.cache, e)),
        };
        let plain = match keys::decrypt(&bytes, old) {
            Ok(p) => p,
            Err(_) => {
                self.clear()?;
                return Ok(false);
            }
        };
        let sealed = keys::encrypt(&plain, &[new.to_public()])?;
        ct_core::files::write_private_atomic(&self.cache, &sealed)
            .map_err(|e| SecretsError::io(&self.cache, e))?;
        Ok(true)
    }
}

/// `rename` seguido de `fsync` do diretório.
fn rename_synced(from: &Path, to: &Path) -> Result<()> {
    fs::rename(from, to).map_err(|e| SecretsError::io(to, e))?;
    if let Some(dir) = to.parent().filter(|d| !d.as_os_str().is_empty()) {
        if let Ok(d) = fs::File::open(dir) {
            let _ = d.sync_all();
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Kind, Scope, SecretValue};
    use std::os::unix::fs::PermissionsExt;

    const VALOR: &str = "valor-entregue-456";

    fn entry(scope: Scope, name: &str, value: &str) -> Entry {
        Entry {
            scope,
            name: name.into(),
            kind: Kind::Secret,
            value: SecretValue::new(value),
            rev: 1,
            updated_at: 0,
        }
    }

    fn cache(d: &Path) -> EnvCache {
        EnvCache::new(d.join("cfg/env.key"), d.join("data/secrets-cache.age"))
    }

    fn mode(p: &Path) -> u32 {
        fs::metadata(p).unwrap().permissions().mode() & 0o777
    }

    fn entrega(c: &EnvCache, rev: i64, entries: Vec<Entry>) -> Vec<u8> {
        let b64 = seal_delivery(rev, entries, &c.public_key().unwrap()).unwrap();
        decode_delivery(&b64).unwrap()
    }

    #[test]
    fn entrega_vira_cache_que_so_abre_com_a_chave_do_ambiente() {
        let d = tempfile::tempdir().unwrap();
        let c = cache(d.path());
        assert!(c.load().unwrap().is_none(), "sem entrega ainda");
        let p = Scope::parse_project("deep-ion-ai/demo").unwrap();
        let blob = entrega(
            &c,
            7,
            vec![
                entry(Scope::Global, "A", "global"),
                entry(p.clone(), "A", VALOR),
                entry(Scope::Global, "B", "b"),
            ],
        );
        let s = c.store(&blob).unwrap();
        assert_eq!(s.config_rev(), 7);
        assert_eq!(mode(c.key_path()), 0o600);
        assert_eq!(mode(c.cache_path()), 0o600);
        assert_eq!(fs::read(c.cache_path()).unwrap(), blob, "grava como veio");
        assert!(!String::from_utf8_lossy(&blob).contains(VALOR));

        let s = c.load().unwrap().unwrap();
        let r = s.resolve(Some("Deep-Ion-AI/Demo"));
        assert_eq!(r["A"].value.expose(), VALOR);
        assert_eq!(r["B"].value.expose(), "b");
        assert_eq!(s.resolve(None)["A"].value.expose(), "global");
        assert!(!format!("{s:?}").contains(VALOR));

        // Outra chave não abre o cache.
        let outra = Identity::generate();
        assert!(matches!(
            EnvSecrets::open(&fs::read(c.cache_path()).unwrap(), &outra),
            Err(SecretsError::Decrypt(_))
        ));
        let alheio = EnvCache::new(d.path().join("outro.key"), c.cache_path());
        alheio.ensure_key().unwrap();
        assert!(alheio.load().is_err());
    }

    #[test]
    fn entrega_para_outra_chave_nao_troca_o_cache() {
        let d = tempfile::tempdir().unwrap();
        let c = cache(d.path());
        c.store(&entrega(&c, 1, vec![entry(Scope::Global, "A", "1")]))
            .unwrap();
        let b64 = seal_delivery(
            2,
            vec![entry(Scope::Global, "A", "2")],
            &Identity::generate().to_public(),
        )
        .unwrap();
        assert!(c.store(&decode_delivery(&b64).unwrap()).is_err());
        assert!(decode_delivery("não é base64!").is_err());
        let s = c.load().unwrap().unwrap();
        assert_eq!(s.config_rev(), 1);
        assert_eq!(s.resolve(None)["A"].value.expose(), "1");
    }

    #[test]
    fn rotacao_mantem_o_cache_legivel_com_a_chave_nova() {
        let d = tempfile::tempdir().unwrap();
        let c = cache(d.path());
        let antiga = c.ensure_key().unwrap();
        c.store(&entrega(&c, 3, vec![entry(Scope::Global, "A", VALOR)]))
            .unwrap();
        let r = c.rotate_key().unwrap();
        assert!(r.cache_kept);
        assert_ne!(r.public, antiga.to_public());
        assert_eq!(c.public_key().unwrap(), r.public);
        assert_eq!(mode(c.key_path()), 0o600);
        assert!(!c.pending_key().exists());
        let s = c.load().unwrap().unwrap();
        assert_eq!(s.resolve(None)["A"].value.expose(), VALOR);
        let bytes = fs::read(c.cache_path()).unwrap();
        assert!(
            EnvSecrets::open(&bytes, &antiga).is_err(),
            "a antiga não abre mais"
        );

        // Sem cache: só troca a chave.
        c.clear().unwrap();
        let r2 = c.rotate_key().unwrap();
        assert!(!r2.cache_kept);
        assert_eq!(c.public_key().unwrap(), r2.public);
    }

    #[test]
    fn rotacao_interrompida_e_retomada() {
        let d = tempfile::tempdir().unwrap();
        let c = cache(d.path());
        let antiga = c.ensure_key().unwrap();
        c.store(&entrega(&c, 1, vec![entry(Scope::Global, "A", "x")]))
            .unwrap();

        // Caiu antes de recifrar o cache: fica a chave antiga.
        let nova = Identity::generate();
        keys::write_identity(&c.pending_key(), &nova, "t").unwrap();
        assert_eq!(c.public_key().unwrap(), antiga.to_public());
        assert!(!c.pending_key().exists());

        // Caiu depois de recifrar, antes do rename: o cache abre (load tenta
        // a nova) e o ensure adota a nova.
        keys::write_identity(&c.pending_key(), &nova, "t").unwrap();
        let plain = keys::decrypt(&fs::read(c.cache_path()).unwrap(), &antiga).unwrap();
        fs::write(
            c.cache_path(),
            keys::encrypt(&plain, &[nova.to_public()]).unwrap(),
        )
        .unwrap();
        assert_eq!(c.load().unwrap().unwrap().resolve(None).len(), 1);
        assert_eq!(c.public_key().unwrap(), nova.to_public());
        assert!(!c.pending_key().exists());
        assert_eq!(c.load().unwrap().unwrap().resolve(None).len(), 1);
    }

    #[test]
    fn formato_futuro_e_recusado() {
        let d = tempfile::tempdir().unwrap();
        let c = cache(d.path());
        let id = c.ensure_key().unwrap();
        let futuro = br#"{"format":2,"config_rev":1,"entries":[]}"#;
        let sealed = keys::encrypt(futuro, &[id.to_public()]).unwrap();
        assert!(matches!(
            c.store(&sealed),
            Err(SecretsError::UnknownFormat(2))
        ));
        assert!(!c.cache_path().exists());
    }
}
