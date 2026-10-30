//! O store: entradas em memória, gravadas cifradas num único arquivo.
//!
//! O plaintext é um JSON com versão de formato:
//! `{"format": 1, "store_rev": N, "entries": [...]}`. Cada gravação cifra o
//! conjunto inteiro de novo e troca o arquivo atomicamente; o estado em
//! memória só muda depois que a troca deu certo.

use crate::{keys, Result, SecretValue, SecretsError};
use age::x25519::{Identity, Recipient};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use zeroize::Zeroizing;

/// Versão do formato do plaintext do store.
pub const FORMAT_VERSION: u32 = 1;

/// Escopo de uma entrada. O projeto é `owner/repo` em minúsculas.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(rename = "SecretScope"))]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    Global,
    Project(String),
}

impl Scope {
    /// Escopo do projeto `owner/repo` (normalizado para minúsculas). No
    /// GitLab, `owner` é o caminho do grupo e pode ter barras
    /// (`grupo/projeto`).
    pub fn project(owner: &str, repo: &str) -> Result<Self> {
        let ok = |s: &str| {
            !s.is_empty()
                && s.chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        };
        let (owner, repo) = (owner.trim(), repo.trim());
        if !owner.split('/').all(ok) || !ok(repo) {
            return Err(SecretsError::InvalidProject(format!("{owner}/{repo}")));
        }
        Ok(Scope::Project(format!(
            "{}/{}",
            owner.to_ascii_lowercase(),
            repo.to_ascii_lowercase()
        )))
    }

    /// Escopo de um projeto escrito como `owner/repo`, dividido na última
    /// barra (`grupo/projeto/app` → `grupo/projeto` e `app`).
    pub fn parse_project(key: &str) -> Result<Self> {
        match key.trim().rsplit_once('/') {
            Some((owner, repo)) => Scope::project(owner, repo),
            None => Err(SecretsError::InvalidProject(key.trim().to_string())),
        }
    }
}

/// Tipo da entrada: secret (nunca sai do hub em claro) ou variável (pode ser
/// lida de volta).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(rename = "SecretKind"))]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Secret,
    Variable,
}

/// Uma entrada do store. O `Debug` mostra o valor como `***`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub scope: Scope,
    pub name: String,
    pub kind: Kind,
    pub value: SecretValue,
    /// `rev` da última mudança desta entrada (no hub, o `config_rev`).
    pub rev: i64,
    /// Quando a entrada mudou pela última vez (ms desde a época Unix); 0 em
    /// entradas gravadas antes deste campo existir.
    #[serde(default)]
    pub updated_at: i64,
}

impl Entry {
    pub fn is_secret(&self) -> bool {
        self.kind == Kind::Secret
    }
}

/// Entradas de `entries` que valem para `project` (`owner/repo`, sem
/// diferenciar maiúsculas) ou, com `None`, só as globais. Por nome, a do
/// projeto sobrepõe a global.
pub fn resolve_entries<'a>(
    entries: &'a [Entry],
    project: Option<&str>,
) -> BTreeMap<String, &'a Entry> {
    let project = project.map(|p| p.trim().to_ascii_lowercase());
    let mut out = BTreeMap::new();
    for e in entries.iter().filter(|e| e.scope == Scope::Global) {
        out.insert(e.name.clone(), e);
    }
    if let Some(p) = project {
        for e in entries {
            if matches!(&e.scope, Scope::Project(k) if *k == p) {
                out.insert(e.name.clone(), e);
            }
        }
    }
    out
}

/// Recusa nomes que não valem como variável de ambiente
/// (`[A-Z_][A-Z0-9_]*`).
pub fn validate_name(name: &str) -> Result<()> {
    let mut chars = name.chars();
    let first_ok = chars
        .next()
        .is_some_and(|c| c.is_ascii_uppercase() || c == '_');
    if first_ok && chars.all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_') {
        Ok(())
    } else {
        Err(SecretsError::InvalidName(name.to_string()))
    }
}

/// Nomes exatos que o `ctowerd` não deixa um secret ou variável
/// sobrescrever no processo do agente (issue #166): o ambiente básico do
/// processo e o que o PTY (`portable-pty` define `SHELL`; `TERM`,
/// `COLUMNS` e `LINES` descrevem o terminal) e o Claude Code usam.
pub const RESERVED_NAMES: &[&str] = &[
    "PATH",
    "HOME",
    "USER",
    "LOGNAME",
    "SHELL",
    "TERM",
    "COLORTERM",
    "COLUMNS",
    "LINES",
    "PWD",
    "OLDPWD",
    "LANG",
    "CLAUDE_CONFIG_DIR",
    "CLAUDECODE",
];

/// Prefixos reservados: `CTOWER_` (o que o `ctowerd` exporta para a sessão:
/// `CTOWER_SESSION`, `CTOWER_URL`, `CTOWER_TOKEN`, `CTOWER_PROTECTED`,
/// `CTOWER_SPOOL_DIR` e os que vierem), `LC_` (locale) e `LD_`/`DYLD_`
/// (carregador dinâmico: `LD_PRELOAD` e afins).
pub const RESERVED_PREFIXES: &[&str] = &["CTOWER_", "CTHUB_", "LC_", "LD_", "DYLD_"];

/// Prefixo das entradas só do hub (#263: credenciais do destino de backup).
/// Ficam no mesmo `secrets.age`, mas nunca vão para um ambiente: o sync as
/// filtra e `hub.secrets.*` não as lista nem grava.
pub const HUB_ONLY_PREFIX: &str = "CTHUB_";

/// A entrada é só do hub ([`HUB_ONLY_PREFIX`]).
pub fn is_hub_only(name: &str) -> bool {
    name.starts_with(HUB_ONLY_PREFIX)
}

/// O nome é reservado ([`RESERVED_NAMES`] ou um dos
/// [`RESERVED_PREFIXES`]): o hub recusa gravar (`hub.secrets.set`) e o
/// `ctowerd` ignora no spawn, com aviso, o que já estiver no cache.
pub fn is_reserved_name(name: &str) -> bool {
    RESERVED_NAMES.contains(&name) || RESERVED_PREFIXES.iter().any(|p| name.starts_with(p))
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Document {
    format: u32,
    /// Maior `rev` já aplicado, inclusive o de remoções: quem entrega o
    /// store (#184) compara este número para saber se algo mudou.
    store_rev: i64,
    entries: Vec<Entry>,
    /// Maior `rev` de remoção por escopo, para quem precisa saber se uma
    /// remoção atingiu um escopo (estado de entrega por ambiente, #165).
    /// Vazio em stores gravados antes deste campo existir.
    #[serde(default)]
    removed: Vec<Removal>,
}

/// Última remoção num escopo.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Removal {
    pub scope: Scope,
    /// `rev` da remoção mais recente no escopo.
    pub rev: i64,
}

/// Store aberto: o conteúdo decifrado em memória e as chaves para gravar.
pub struct SecretsStore {
    path: PathBuf,
    identity: Identity,
    /// Destinatários de cada gravação: a própria identidade primeiro, depois
    /// os extras (a chave de recuperação).
    recipients: Vec<Recipient>,
    doc: Document,
    #[cfg(test)]
    fail_before_rename: bool,
}

impl std::fmt::Debug for SecretsStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SecretsStore")
            .field("path", &self.path)
            .field("entries", &self.doc.entries.len())
            .field("store_rev", &self.doc.store_rev)
            .field("recovery_configured", &self.recovery_configured())
            .finish_non_exhaustive()
    }
}

impl SecretsStore {
    /// Abre o store em `path`, decifrando com `identity`. Arquivo ausente =
    /// store vazio (o arquivo só nasce na primeira gravação). As gravações
    /// cifram para a própria `identity` e para cada um de `recipients` (ex.:
    /// a chave pública de recuperação, #169); sem extras, o store fica com
    /// [`SecretsStore::recovery_configured`] falso.
    pub fn open(
        path: impl Into<PathBuf>,
        identity: Identity,
        recipients: &[Recipient],
    ) -> Result<Self> {
        let path = path.into();
        let doc = match std::fs::read(&path) {
            Ok(bytes) => parse(&keys::decrypt(&bytes, &identity)?)?,
            Err(e) if e.kind() == io::ErrorKind::NotFound => Document {
                format: FORMAT_VERSION,
                ..Document::default()
            },
            Err(e) => return Err(SecretsError::io(path, e)),
        };
        let own = identity.to_public();
        let mut all = vec![own];
        for r in recipients {
            if !all.contains(r) {
                all.push(r.clone());
            }
        }
        Ok(SecretsStore {
            path,
            identity,
            recipients: all,
            doc,
            #[cfg(test)]
            fail_before_rename: false,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Há um destinatário além da chave do hub (a chave de recuperação)?
    pub fn recovery_configured(&self) -> bool {
        self.recipients.len() > 1
    }

    /// Destinatários extras das gravações (hoje, só a chave de recuperação).
    pub fn extra_recipients(&self) -> &[Recipient] {
        &self.recipients[1..]
    }

    /// Troca os destinatários extras (a chave de recuperação, #169) e
    /// regrava o store cifrado para a própria identidade e para eles, mesmo
    /// vazio (o arquivo nasce aqui). Se a gravação falhar, os destinatários
    /// e o arquivo ficam como estavam.
    pub fn set_recipients(&mut self, recipients: &[Recipient]) -> Result<()> {
        let mut all = vec![self.identity.to_public()];
        for r in recipients {
            if !all.contains(r) {
                all.push(r.clone());
            }
        }
        let old = std::mem::replace(&mut self.recipients, all);
        let doc = self.doc.clone();
        if let Err(e) = self.commit(doc) {
            self.recipients = old;
            return Err(e);
        }
        Ok(())
    }

    /// Prova de ida e volta, só em memória: cifra o conteúdo atual para os
    /// destinatários das gravações mais `extra` e decifra com `identity`.
    /// `Err(Decrypt)` se `identity` não abre o resultado. Não grava nada.
    pub fn round_trip(&self, extra: &[Recipient], identity: &Identity) -> Result<()> {
        let mut all = self.recipients.clone();
        all.extend(extra.iter().cloned());
        let plain = Zeroizing::new(serde_json::to_vec(&self.doc).expect("Document serializa"));
        let sealed = keys::encrypt(&plain, &all)?;
        let back = keys::decrypt(&sealed, identity)?;
        if back.as_slice() != plain.as_slice() {
            return Err(SecretsError::Decrypt("conteúdo diferente na volta".into()));
        }
        Ok(())
    }

    /// Chave pública desta identidade (a do hub).
    pub fn public_key(&self) -> Recipient {
        self.identity.to_public()
    }

    /// Maior `rev` já aplicado (inclusive remoções); 0 = nunca mudou.
    pub fn store_rev(&self) -> i64 {
        self.doc.store_rev
    }

    /// Todas as entradas, ordenadas por escopo e nome.
    pub fn entries(&self) -> &[Entry] {
        &self.doc.entries
    }

    /// Última remoção de cada escopo que já teve alguma.
    pub fn removals(&self) -> &[Removal] {
        &self.doc.removed
    }

    pub fn get(&self, scope: &Scope, name: &str) -> Option<&Entry> {
        self.doc
            .entries
            .iter()
            .find(|e| &e.scope == scope && e.name == name)
    }

    /// Entradas que valem para `project` (`owner/repo`, sem diferenciar
    /// maiúsculas) ou, com `None`, só as globais. Por nome, a do projeto
    /// sobrepõe a global.
    pub fn resolve(&self, project: Option<&str>) -> BTreeMap<String, &Entry> {
        resolve_entries(&self.doc.entries, project)
    }

    /// Cria ou troca a entrada `(scope, name)` com o `rev` dado e grava.
    pub fn set(
        &mut self,
        scope: Scope,
        name: &str,
        kind: Kind,
        value: SecretValue,
        rev: i64,
    ) -> Result<()> {
        validate_name(name)?;
        let mut doc = self.doc.clone();
        doc.entries
            .retain(|e| !(e.scope == scope && e.name == name));
        doc.entries.push(Entry {
            scope,
            name: name.to_string(),
            kind,
            value,
            rev,
            updated_at: ct_core::now_ms(),
        });
        doc.entries
            .sort_by(|a, b| (&a.scope, &a.name).cmp(&(&b.scope, &b.name)));
        doc.store_rev = doc.store_rev.max(rev);
        self.commit(doc)
    }

    /// Remove a entrada `(scope, name)` e grava, levando o `store_rev` a
    /// `rev`. `false` (sem gravar) se ela não existe.
    pub fn delete(&mut self, scope: &Scope, name: &str, rev: i64) -> Result<bool> {
        if self.get(scope, name).is_none() {
            return Ok(false);
        }
        let mut doc = self.doc.clone();
        doc.entries
            .retain(|e| !(&e.scope == scope && e.name == name));
        doc.store_rev = doc.store_rev.max(rev);
        match doc.removed.iter_mut().find(|r| &r.scope == scope) {
            Some(r) => r.rev = r.rev.max(rev),
            None => doc.removed.push(Removal {
                scope: scope.clone(),
                rev,
            }),
        }
        self.commit(doc)?;
        Ok(true)
    }

    /// Cifra `doc`, troca o arquivo e, só então, adota `doc` em memória.
    fn commit(&mut self, doc: Document) -> Result<()> {
        let plain = Zeroizing::new(serde_json::to_vec(&doc).expect("Document serializa"));
        let sealed = keys::encrypt(&plain, &self.recipients)?;
        #[cfg(test)]
        let fail = self.fail_before_rename;
        #[cfg(not(test))]
        let fail = false;
        ct_core::files::write_private_atomic_with(&self.path, &sealed, |_| {
            if fail {
                Err(io::Error::other("falha simulada antes do rename"))
            } else {
                Ok(())
            }
        })
        .map_err(|e| SecretsError::io(&self.path, e))?;
        self.doc = doc;
        Ok(())
    }
}

fn parse(plain: &[u8]) -> Result<Document> {
    // Primeiro só a versão, para recusar um formato futuro com a mensagem
    // certa em vez de um erro de campo.
    #[derive(Deserialize)]
    struct Head {
        format: u32,
    }
    let fmt_err = |e: serde_json::Error| SecretsError::Format {
        line: e.line(),
        column: e.column(),
    };
    let head: Head = serde_json::from_slice(plain).map_err(fmt_err)?;
    if head.format != FORMAT_VERSION {
        return Err(SecretsError::UnknownFormat(head.format));
    }
    serde_json::from_slice(plain).map_err(fmt_err)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    const VALOR: &str = "valor-super-secreto-123";

    fn v(s: &str) -> SecretValue {
        SecretValue::new(s)
    }

    fn proj() -> Scope {
        Scope::project("Deep-Ion-AI", "Ctower-Agents").unwrap()
    }

    #[test]
    fn nomes_reservados() {
        for n in [
            "CTOWER_TOKEN",
            "CTOWER_X",
            "PATH",
            "HOME",
            "TERM",
            "COLUMNS",
            "LC_ALL",
            "LD_PRELOAD",
            "DYLD_INSERT_LIBRARIES",
            "CLAUDE_CONFIG_DIR",
            "CLAUDECODE",
        ] {
            assert!(is_reserved_name(n), "{n}");
        }
        for n in [
            "API_TOKEN",
            "CTOWER",
            "PATHS",
            "MY_PATH",
            "CLAUDE_X",
            "RUST_LOG",
        ] {
            assert!(!is_reserved_name(n), "{n}");
        }
    }

    #[test]
    fn ida_e_volta() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("secrets.age");
        let id = Identity::generate();
        let mut s = SecretsStore::open(&p, id.clone(), &[]).unwrap();
        assert!(s.entries().is_empty());
        assert!(!p.exists(), "store vazio não cria o arquivo");
        s.set(Scope::Global, "API_TOKEN", Kind::Secret, v(VALOR), 1)
            .unwrap();
        s.set(proj(), "RUST_LOG", Kind::Variable, v("debug"), 2)
            .unwrap();

        let bruto = fs::read(&p).unwrap();
        assert!(bruto.starts_with(b"age-encryption.org/v1"));
        assert!(!String::from_utf8_lossy(&bruto).contains(VALOR));

        let s2 = SecretsStore::open(&p, id, &[]).unwrap();
        assert_eq!(s2.entries(), s.entries());
        assert_eq!(s2.store_rev(), 2);
        let e = s2.get(&Scope::Global, "API_TOKEN").unwrap();
        assert_eq!((e.kind, e.rev, e.value.expose()), (Kind::Secret, 1, VALOR));
        let e = s2
            .get(
                &Scope::Project("deep-ion-ai/ctower-agents".into()),
                "RUST_LOG",
            )
            .unwrap();
        assert_eq!((e.kind, e.value.expose()), (Kind::Variable, "debug"));

        // Outra chave não abre.
        assert!(matches!(
            SecretsStore::open(&p, Identity::generate(), &[]),
            Err(SecretsError::Decrypt(_))
        ));
    }

    #[test]
    fn troca_e_remocao_sobem_o_store_rev() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("secrets.age");
        let id = Identity::generate();
        let mut s = SecretsStore::open(&p, id.clone(), &[]).unwrap();
        s.set(Scope::Global, "A", Kind::Variable, v("1"), 3)
            .unwrap();
        s.set(Scope::Global, "A", Kind::Secret, v("2"), 4).unwrap();
        assert_eq!(s.entries().len(), 1);
        assert_eq!(s.get(&Scope::Global, "A").unwrap().kind, Kind::Secret);
        assert!(!s.delete(&Scope::Global, "B", 5).unwrap());
        assert_eq!(s.store_rev(), 4, "remoção de entrada inexistente não grava");
        assert!(s.removals().is_empty());
        assert!(s.get(&Scope::Global, "A").unwrap().updated_at > 0);
        assert!(s.delete(&Scope::Global, "A", 6).unwrap());
        let s2 = SecretsStore::open(&p, id, &[]).unwrap();
        assert!(s2.entries().is_empty());
        assert_eq!(s2.store_rev(), 6);
        assert_eq!(
            s2.removals(),
            [Removal {
                scope: Scope::Global,
                rev: 6
            }]
        );
    }

    #[test]
    fn projeto_sobrepoe_global() {
        let d = tempfile::tempdir().unwrap();
        let mut s = SecretsStore::open(d.path().join("s.age"), Identity::generate(), &[]).unwrap();
        s.set(Scope::Global, "A", Kind::Variable, v("global"), 1)
            .unwrap();
        s.set(Scope::Global, "B", Kind::Secret, v("b"), 2).unwrap();
        s.set(proj(), "A", Kind::Secret, v("projeto"), 3).unwrap();
        s.set(
            Scope::parse_project("outro/repo").unwrap(),
            "C",
            Kind::Variable,
            v("c"),
            4,
        )
        .unwrap();

        let r = s.resolve(Some("deep-ion-ai/Ctower-Agents"));
        let vals: Vec<_> = r
            .iter()
            .map(|(k, e)| (k.as_str(), e.value.expose()))
            .collect();
        assert_eq!(vals, [("A", "projeto"), ("B", "b")]);
        let r = s.resolve(None);
        let vals: Vec<_> = r
            .iter()
            .map(|(k, e)| (k.as_str(), e.value.expose()))
            .collect();
        assert_eq!(vals, [("A", "global"), ("B", "b")]);
        let r = s.resolve(Some("sem/entradas"));
        assert_eq!(r.len(), 2);
    }

    #[test]
    fn nome_invalido_e_recusado() {
        for ok in ["A", "_", "API_TOKEN", "_X1", "A9_B"] {
            validate_name(ok).unwrap();
        }
        for ruim in ["", "a", "1A", "A-B", "A B", "ÁGUA", "api_token", "A.B"] {
            assert!(validate_name(ruim).is_err(), "{ruim:?}");
        }
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("s.age");
        let mut s = SecretsStore::open(&p, Identity::generate(), &[]).unwrap();
        let e = s
            .set(Scope::Global, "minusculo", Kind::Secret, v(VALOR), 1)
            .unwrap_err();
        assert!(matches!(e, SecretsError::InvalidName(_)));
        assert!(!p.exists());
        assert!(Scope::parse_project("sem-barra").is_err());
        // GitLab: o `owner` é o caminho do grupo (#347).
        assert_eq!(
            Scope::parse_project("GRUPO/projeto/app").unwrap(),
            Scope::Project("grupo/projeto/app".into())
        );
        assert_eq!(
            Scope::project("grupo/projeto", "app").unwrap(),
            Scope::parse_project("grupo/projeto/app").unwrap()
        );
        for ruim in ["a//c", "/c", "a/", "a/b c/d"] {
            assert!(Scope::parse_project(ruim).is_err(), "{ruim:?}");
        }
        assert!(Scope::project("", "x").is_err());
    }

    #[test]
    fn falha_no_meio_da_escrita_preserva_o_arquivo() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("secrets.age");
        let id = Identity::generate();
        let mut s = SecretsStore::open(&p, id.clone(), &[]).unwrap();
        s.set(Scope::Global, "A", Kind::Secret, v("antigo"), 1)
            .unwrap();
        let antes = fs::read(&p).unwrap();

        s.fail_before_rename = true;
        let e = s
            .set(Scope::Global, "A", Kind::Secret, v(VALOR), 2)
            .unwrap_err();
        assert!(matches!(e, SecretsError::Io { .. }), "{e}");
        assert!(!e.to_string().contains(VALOR));
        // Arquivo e memória como antes; nenhum temporário sobrou.
        assert_eq!(fs::read(&p).unwrap(), antes);
        assert_eq!(s.get(&Scope::Global, "A").unwrap().value.expose(), "antigo");
        assert_eq!(s.store_rev(), 1);
        assert_eq!(fs::read_dir(d.path()).unwrap().count(), 1);
        let s2 = SecretsStore::open(&p, id, &[]).unwrap();
        assert_eq!(
            s2.get(&Scope::Global, "A").unwrap().value.expose(),
            "antigo"
        );

        s.fail_before_rename = false;
        s.set(Scope::Global, "A", Kind::Secret, v("novo"), 3)
            .unwrap();
        assert_eq!(s.store_rev(), 3);
    }

    #[test]
    fn store_fica_com_0600() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("secrets.age");
        let mut s = SecretsStore::open(&p, Identity::generate(), &[]).unwrap();
        s.set(Scope::Global, "A", Kind::Secret, v("x"), 1).unwrap();
        assert_eq!(
            fs::metadata(&p).unwrap().permissions().mode() & 0o777,
            0o600
        );
        fs::set_permissions(&p, fs::Permissions::from_mode(0o644)).unwrap();
        s.set(Scope::Global, "A", Kind::Secret, v("y"), 2).unwrap();
        assert_eq!(
            fs::metadata(&p).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn dois_destinatarios() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("secrets.age");
        let hub = Identity::generate();
        let rec = Identity::generate();
        let s = SecretsStore::open(&p, hub.clone(), &[]).unwrap();
        assert!(!s.recovery_configured());
        // A própria chave repetida não conta como recuperação.
        let s = SecretsStore::open(&p, hub.clone(), &[hub.to_public()]).unwrap();
        assert!(!s.recovery_configured());

        let mut s = SecretsStore::open(&p, hub.clone(), &[rec.to_public()]).unwrap();
        assert!(s.recovery_configured());
        s.set(Scope::Global, "A", Kind::Secret, v(VALOR), 1)
            .unwrap();
        let pelo_hub = SecretsStore::open(&p, hub, &[]).unwrap();
        let pela_recuperacao = SecretsStore::open(&p, rec, &[]).unwrap();
        assert_eq!(pelo_hub.entries(), pela_recuperacao.entries());
        assert_eq!(
            pela_recuperacao
                .get(&Scope::Global, "A")
                .unwrap()
                .value
                .expose(),
            VALOR
        );
    }

    #[test]
    fn troca_de_destinatarios_regrava_o_store() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("secrets.age");
        let hub = Identity::generate();
        let rec = Identity::generate();
        let mut s = SecretsStore::open(&p, hub.clone(), &[]).unwrap();
        // Ida e volta: só a identidade certa abre, e nada é gravado.
        s.round_trip(&[rec.to_public()], &rec).unwrap();
        assert!(matches!(
            s.round_trip(&[rec.to_public()], &Identity::generate()),
            Err(SecretsError::Decrypt(_))
        ));
        assert!(!p.exists());

        // Store vazio: o arquivo nasce já para os dois.
        s.set_recipients(&[rec.to_public()]).unwrap();
        assert!(s.recovery_configured());
        assert_eq!(s.extra_recipients(), [rec.to_public()]);
        SecretsStore::open(&p, rec.clone(), &[]).unwrap();
        s.set(Scope::Global, "A", Kind::Secret, v(VALOR), 1)
            .unwrap();
        let pela_recuperacao = SecretsStore::open(&p, rec.clone(), &[]).unwrap();
        assert_eq!(pela_recuperacao.entries(), s.entries());

        // Falha na regravação: destinatários e arquivo como antes.
        let antes = fs::read(&p).unwrap();
        s.fail_before_rename = true;
        assert!(s.set_recipients(&[]).is_err());
        assert!(s.recovery_configured());
        assert_eq!(fs::read(&p).unwrap(), antes);
        s.fail_before_rename = false;

        // Trocar a chave de recuperação: a antiga deixa de abrir.
        let nova = Identity::generate();
        s.set_recipients(&[nova.to_public()]).unwrap();
        assert!(SecretsStore::open(&p, nova, &[]).is_ok());
        assert!(SecretsStore::open(&p, rec, &[]).is_err());
        assert!(SecretsStore::open(&p, hub, &[]).is_ok());
    }

    #[test]
    fn valores_nao_aparecem_em_debug_nem_em_erro() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("secrets.age");
        let id = Identity::generate();
        let mut s = SecretsStore::open(&p, id.clone(), &[]).unwrap();
        s.set(Scope::Global, "A", Kind::Secret, v(VALOR), 1)
            .unwrap();
        let dbg = format!("{s:?} {:?} {:#?}", s.entries(), s.resolve(None));
        assert!(!dbg.contains(VALOR), "{dbg}");
        assert!(dbg.contains("***"), "{dbg}");

        // Plaintext com o valor no lugar errado: o erro de formato só dá a
        // posição, não o trecho.
        let ruim = format!(
            r#"{{"format":1,"store_rev":1,"entries":[{{"scope":"global","name":"A","kind":"secret","value":1,"rev":"{VALOR}"}}]}}"#
        );
        fs::write(
            &p,
            keys::encrypt(ruim.as_bytes(), &[id.to_public()]).unwrap(),
        )
        .unwrap();
        let e = SecretsStore::open(&p, id.clone(), &[]).unwrap_err();
        assert!(matches!(e, SecretsError::Format { .. }), "{e}");
        assert!(!e.to_string().contains(VALOR), "{e}");
        assert!(!format!("{e:?}").contains(VALOR), "{e:?}");

        // Store gravado antes de `updated_at` e `removed`: abre com os padrões.
        let antigo = br#"{"format":1,"store_rev":1,"entries":[{"scope":"global","name":"A","kind":"variable","value":"x","rev":1}]}"#;
        fs::write(&p, keys::encrypt(antigo, &[id.to_public()]).unwrap()).unwrap();
        let s3 = SecretsStore::open(&p, id.clone(), &[]).unwrap();
        assert_eq!(s3.get(&Scope::Global, "A").unwrap().updated_at, 0);
        assert!(s3.removals().is_empty());

        let futuro = br#"{"format":2,"store_rev":0,"entries":[]}"#;
        fs::write(&p, keys::encrypt(futuro, &[id.to_public()]).unwrap()).unwrap();
        assert!(matches!(
            SecretsStore::open(&p, id, &[]),
            Err(SecretsError::UnknownFormat(2))
        ));
    }
}
