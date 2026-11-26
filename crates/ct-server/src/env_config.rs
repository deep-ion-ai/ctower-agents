//! `env.config.get|set` (#328): as chaves do `config.toml` que a UI edita.
//! Só uma lista fechada; `set` grava no arquivo (preservando comentários e
//! as outras chaves) e aplica na hora, sem restart:
//!
//! - `update_ref`: o ref seguido pelas atualizações ([`Updates::set_ref`]);
//! - `gh_sync_interval_secs`: o intervalo do sync com o GitHub (`0`
//!   desliga), lido pelo laço do `ctowerd` a cada volta.
//!
//! `env.credentials.get|set` (#143) editam, do mesmo jeito, os comandos de
//! credencial do Jira e do GitLab deste ambiente (`[credentials."owner/repo"]`),
//! que valem no próximo uso pelo [`ct_core::SharedCredentials`].

use crate::update::Updates;
use serde::Serialize;
use serde_json::{json, Value};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::watch;

/// Padrões de `DaemonConfig` das chaves editáveis.
const DEFAULT_UPDATE_REF: &str = "main";
const DEFAULT_GH_SYNC: u64 = 180;

/// Menor intervalo de sync aceito (além de `0`, desligado).
pub const MIN_GH_SYNC_SECS: u64 = 30;

pub const KEYS: [&str; 2] = ["update_ref", "gh_sync_interval_secs"];

/// Uma chave editável e o valor em uso.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct ConfigItem {
    pub key: String,
    /// Valor em uso neste daemon.
    #[cfg_attr(feature = "ts", ts(type = "string | number"))]
    pub value: Value,
    #[cfg_attr(feature = "ts", ts(type = "string | number"))]
    pub default: Value,
    /// A chave está escrita no `config.toml` (senão vale o padrão).
    pub configured: bool,
}

/// Uma linha de `env.credentials.get` (#143): projeto Jira ou GitLab
/// atribuído, os comandos do cadastro e os deste ambiente, e como foi a
/// última leitura com eles aqui.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct EnvCredential {
    pub owner: String,
    pub repo: String,
    pub name: String,
    pub tracker: ct_core::TrackerKind,
    pub forge: ct_core::ForgeKind,
    /// Comando do cadastro (hub), o que vale sem sobrescrita.
    pub project_tracker_cmd: String,
    pub project_forge_cmd: String,
    /// Comando deste ambiente; vazio = o do cadastro.
    pub tracker_cmd: String,
    pub forge_cmd: String,
    /// Prontidão do último ciclo do sync do Jira neste ambiente.
    pub readiness: Option<ct_core::TrackerReadiness>,
    /// Último sync neste ambiente (erro e falhas seguidas).
    pub sync: Option<ct_ledger::SyncState>,
}

pub struct EnvConfig {
    /// `<config>/config.toml`.
    path: PathBuf,
    gh_sync: watch::Sender<u64>,
    updates: Option<Arc<Updates>>,
    /// `[credentials]` em uso no daemon; `None` = edição desligada.
    credentials: Option<ct_core::SharedCredentials>,
}

/// Limite de cada comando de credencial editado pela UI.
pub const MAX_CREDENTIAL_CMD: usize = 1000;

/// Uma escrita do `config.toml` por vez no processo. `env.config.set`,
/// `env.credentials.set` e a conversão do `[credentials]` (#237) leem o
/// arquivo e o regravam: as três passam por aqui.
static CONFIG_WRITE: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn config_write() -> std::sync::MutexGuard<'static, ()> {
    CONFIG_WRITE.lock().unwrap_or_else(|e| e.into_inner())
}

impl EnvConfig {
    pub fn new(path: PathBuf, gh_sync_secs: u64, updates: Option<Arc<Updates>>) -> Self {
        EnvConfig {
            path,
            gh_sync: watch::channel(gh_sync_secs).0,
            updates,
            credentials: None,
        }
    }

    /// Liga `env.credentials.get|set` sobre as sobrescritas em uso.
    pub fn with_credentials(mut self, credentials: ct_core::SharedCredentials) -> Self {
        self.credentials = Some(credentials);
        self
    }

    /// Sobrescritas em uso; `None` = edição desligada neste daemon.
    pub fn credentials(&self) -> Option<ct_core::CredentialOverrides> {
        self.credentials.as_ref().map(|c| c.snapshot())
    }

    /// Grava os comandos de credencial deste ambiente para `owner/repo` em
    /// `[credentials."owner/repo"]` e aplica. Os dois vazios removem a
    /// entrada (volta o comando do projeto).
    pub fn set_credentials(
        &self,
        owner: &str,
        repo: &str,
        tracker_cmd: &str,
        forge_cmd: &str,
    ) -> Result<ct_core::CredentialOverrides, (i64, String)> {
        let invalid = |m: String| (-32602, m);
        let io = |e: io::Error| (-32000, format!("{}: {e}", self.path.display()));
        let shared = self.credentials.as_ref().ok_or_else(|| {
            (
                -32000,
                "edição das credenciais desligada neste ambiente".into(),
            )
        })?;
        let (owner, repo) = (owner.trim(), repo.trim());
        if owner.is_empty() || repo.is_empty() {
            return Err(invalid("owner e repo são obrigatórios".into()));
        }
        let (tracker_cmd, forge_cmd) = (tracker_cmd.trim(), forge_cmd.trim());
        for (name, cmd) in [("tracker_cmd", tracker_cmd), ("forge_cmd", forge_cmd)] {
            if cmd.contains(['\n', '\r']) {
                return Err(invalid(format!("{name}: o comando deve ter uma linha só")));
            }
            if cmd.len() > MAX_CREDENTIAL_CMD {
                return Err(invalid(format!(
                    "{name}: comando acima de {MAX_CREDENTIAL_CMD} caracteres"
                )));
            }
        }
        let key = format!("{owner}/{repo}");
        let _guard = config_write();
        let mut doc = self.read().map_err(io)?;
        if !doc.contains_key("credentials") {
            let mut t = toml_edit::Table::new();
            t.set_implicit(true);
            doc["credentials"] = toml_edit::Item::Table(t);
        }
        let creds = doc["credentials"].as_table_mut().ok_or_else(|| {
            (
                -32000,
                "[credentials] do config.toml não é uma tabela".into(),
            )
        })?;
        // A chave já gravada, com outra caixa, é a mesma.
        let existing: Vec<String> = creds
            .iter()
            .map(|(k, _)| k.to_string())
            .filter(|k| k.trim().eq_ignore_ascii_case(&key))
            .collect();
        for k in existing {
            creds.remove(&k);
        }
        if !tracker_cmd.is_empty() || !forge_cmd.is_empty() {
            let mut t = toml_edit::Table::new();
            if !tracker_cmd.is_empty() {
                t["tracker_cmd"] = toml_edit::value(tracker_cmd);
            }
            if !forge_cmd.is_empty() {
                t["forge_cmd"] = toml_edit::value(forge_cmd);
            }
            creds.insert(&key, toml_edit::Item::Table(t));
        }
        // O que o `DaemonConfig` vai ler no próximo início (o resto do
        // arquivo não interessa aqui).
        let parsed = toml::from_str::<CredentialsOnly>(&doc.to_string())
            .map(|c| c.credentials)
            .map_err(|e| (-32000, format!("[credentials] inválido: {e}")))?;
        self.store(&doc).map_err(io)?;
        shared.replace(parsed.clone());
        Ok(parsed)
    }

    /// Intervalo do sync com o GitHub em segundos (`0` = desligado); o laço
    /// do `ctowerd` acorda quando muda.
    pub fn gh_sync(&self) -> watch::Receiver<u64> {
        self.gh_sync.subscribe()
    }

    pub fn get(&self) -> io::Result<Vec<ConfigItem>> {
        let doc = self.read()?;
        Ok(KEYS.iter().map(|k| self.item(&doc, k)).collect())
    }

    /// Valida, grava no `config.toml` e aplica. Devolve o item atualizado.
    pub fn set(&self, key: &str, value: &Value) -> Result<ConfigItem, (i64, String)> {
        let invalid = |m: String| (-32602, m);
        let io = |e: io::Error| (-32000, format!("{}: {e}", self.path.display()));
        let parsed = match key {
            "update_ref" => {
                let r = value
                    .as_str()
                    .map(str::trim)
                    .filter(|r| valid_ref(r))
                    .ok_or_else(|| {
                        invalid("update_ref deve ser um nome de branch ou tag".into())
                    })?;
                toml_edit::value(r)
            }
            "gh_sync_interval_secs" => {
                let n = value
                    .as_u64()
                    .filter(|n| *n == 0 || *n >= MIN_GH_SYNC_SECS)
                    .ok_or_else(|| {
                        invalid(format!(
                            "gh_sync_interval_secs deve ser 0 (desligado) ou pelo menos \
                             {MIN_GH_SYNC_SECS}"
                        ))
                    })?;
                toml_edit::value(
                    i64::try_from(n).map_err(|_| invalid("valor grande demais".into()))?,
                )
            }
            _ => {
                return Err(invalid(format!(
                    "chave não editável: {key} (aceitas: {})",
                    KEYS.join(", ")
                )))
            }
        };
        let _guard = config_write();
        let mut doc = self.read().map_err(io)?;
        doc[key] = parsed;
        self.store(&doc).map_err(io)?;
        match key {
            "update_ref" => {
                if let (Some(u), Some(r)) = (&self.updates, doc[key].as_str()) {
                    u.set_ref(r);
                }
            }
            _ => {
                let n = doc[key].as_integer().unwrap_or_default() as u64;
                self.gh_sync.send_replace(n);
            }
        }
        Ok(self.item(&doc, key))
    }

    fn item(&self, doc: &toml_edit::DocumentMut, key: &str) -> ConfigItem {
        let configured = doc.contains_key(key);
        let (value, default) = match key {
            "update_ref" => (
                json!(self
                    .updates
                    .as_ref()
                    .map(|u| u.source().git_ref)
                    .or_else(|| doc.get(key).and_then(|v| v.as_str()).map(String::from))
                    .unwrap_or_else(|| DEFAULT_UPDATE_REF.into())),
                json!(DEFAULT_UPDATE_REF),
            ),
            _ => (json!(*self.gh_sync.borrow()), json!(DEFAULT_GH_SYNC)),
        };
        ConfigItem {
            key: key.into(),
            value,
            default,
            configured,
        }
    }

    fn read(&self) -> io::Result<toml_edit::DocumentMut> {
        match std::fs::read_to_string(&self.path) {
            Ok(s) => s
                .parse()
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Default::default()),
            Err(e) => Err(e),
        }
    }

    fn store(&self, doc: &toml_edit::DocumentMut) -> io::Result<()> {
        write_atomic(&self.path, &doc.to_string())
    }
}

#[derive(serde::Deserialize)]
struct CredentialsOnly {
    #[serde(default)]
    credentials: ct_core::CredentialOverrides,
}

#[derive(serde::Deserialize)]
struct ConnectionsOnly {
    #[serde(default)]
    connections: ct_core::ConnectionOverrides,
}

/// `[connections]` do `config.toml` (#237). Sem arquivo, ou sem a tabela,
/// vazio. Erro de leitura não cita o conteúdo do arquivo.
pub fn read_connection_overrides(path: &Path) -> ct_core::ConnectionOverrides {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Default::default();
    };
    match toml::from_str::<ConnectionsOnly>(&text) {
        Ok(c) => c.connections,
        Err(_) => {
            tracing::warn!("config.toml: [connections] não lê; sem sobrescrita de conexão");
            Default::default()
        }
    }
}

/// Primeiro pull com conexões (#237): o `[credentials."owner/repo"]` do
/// arquivo vai para `[connections."nome"]`, pela conexão de cada projeto.
/// O que não converte (projeto sem conexão aqui, ou comando que brigaria
/// com outro) fica em `[credentials]`. Sem `[credentials]`, ou sem nada que
/// converta, o arquivo não muda; senão as tabelas são editadas no lugar.
/// Devolve as conexões em uso, o que ficou em `[credentials]` e os avisos.
pub fn convert_credentials_file(
    path: &Path,
    projects: &[ct_core::ProjectConnections],
) -> io::Result<(
    ct_core::ConnectionOverrides,
    ct_core::CredentialOverrides,
    Vec<String>,
)> {
    let _guard = config_write();
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Default::default()),
        Err(e) => return Err(e),
    };
    let invalid = || io::Error::new(io::ErrorKind::InvalidData, "config.toml não lê");
    let creds = toml::from_str::<CredentialsOnly>(&text).map_err(|_| invalid())?;
    // `[connections]` tem de ler. Se não lê, a conversão espera: a tabela seria
    // regravada do zero e as entradas do usuário se perderiam.
    let existing = toml::from_str::<ConnectionsOnly>(&text)
        .map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "[connections] do config.toml não lê; a conversão do [credentials] fica para depois",
            )
        })?
        .connections;
    if creds.credentials.is_empty() {
        return Ok((existing, Default::default(), Vec::new()));
    }
    let (conns, left, warnings) =
        ct_core::convert_credentials(&creds.credentials, projects, &existing);
    // Sem mudança (o que sobra não converte, #269), o arquivo fica intacto.
    if conns == existing && left == creds.credentials {
        return Ok((conns, left, warnings));
    }
    // As tabelas são editadas no lugar, não recriadas: ficam na mesma
    // posição e com os comentários do usuário.
    let mut doc: toml_edit::DocumentMut = text.parse().map_err(|_| invalid())?;
    if left.is_empty() {
        doc.remove("credentials");
    } else if let Some(t) = doc
        .get_mut("credentials")
        .and_then(|i| i.as_table_like_mut())
    {
        let keys: Vec<String> = t.iter().map(|(k, _)| k.to_string()).collect();
        for key in keys {
            let Some(o) = left.0.get(&key) else {
                t.remove(&key);
                continue;
            };
            if let Some(e) = t.get_mut(&key).and_then(|i| i.as_table_like_mut()) {
                set_cmd(e, "tracker_cmd", &o.tracker_cmd);
                set_cmd(e, "forge_cmd", &o.forge_cmd);
            }
        }
    }
    if conns != existing {
        if !doc.contains_key("connections") {
            let mut t = toml_edit::Table::new();
            t.set_implicit(true);
            doc.insert("connections", toml_edit::Item::Table(t));
        }
        let t = doc["connections"].as_table_like_mut().ok_or_else(invalid)?;
        for (name, o) in &conns.0 {
            if existing.0.get(name) == Some(o) {
                continue;
            }
            match t.get_mut(name).and_then(|i| i.as_table_like_mut()) {
                Some(e) => set_cmd(e, "credential_cmd", &o.credential_cmd),
                None => {
                    let mut e = toml_edit::Table::new();
                    e["credential_cmd"] = toml_edit::value(o.credential_cmd.clone());
                    t.insert(name, toml_edit::Item::Table(e));
                }
            }
        }
    }
    write_atomic(path, &doc.to_string())?;
    Ok((conns, left, warnings))
}

/// Grava `cmd` em `field` (vazio remove). Valor igual não é tocado; valor
/// trocado mantém o comentário da linha.
fn set_cmd(t: &mut dyn toml_edit::TableLike, field: &str, cmd: &str) {
    if cmd.is_empty() {
        t.remove(field);
        return;
    }
    match t.get_mut(field).and_then(|i| i.as_value_mut()) {
        Some(v) if v.as_str() == Some(cmd) => {}
        Some(v) => {
            let decor = v.decor().clone();
            *v = cmd.into();
            *v.decor_mut() = decor;
        }
        None => {
            t.insert(field, toml_edit::value(cmd));
        }
    }
}

/// Temporário + rename, com a permissão do arquivo atual. O temporário leva
/// o pid e um contador, para não colidir com outra escrita.
fn write_atomic(path: &Path, text: &str) -> io::Result<()> {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = path.with_extension(format!("toml.{}.{seq}.tmp", std::process::id()));
    std::fs::write(&tmp, text)?;
    if let Ok(m) = std::fs::metadata(path) {
        std::fs::set_permissions(&tmp, m.permissions())?;
    }
    std::fs::rename(&tmp, path)
}

/// Nome de branch ou tag razoável (subconjunto de `git check-ref-format`).
fn valid_ref(r: &str) -> bool {
    !r.is_empty()
        && r.len() <= 200
        && !r.starts_with(['-', '/', '.'])
        && !r.ends_with(['/', '.'])
        && !r.ends_with(".lock")
        && !r.contains("..")
        && !r.contains("//")
        && !r.contains("@{")
        && r.chars()
            .all(|c| !c.is_whitespace() && !c.is_control() && !"~^:?*[\\".contains(c))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grava_preservando_o_resto_e_aplica() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("config.toml");
        std::fs::write(&path, "# meu comentário\nport = 7878\n").unwrap();
        let c = EnvConfig::new(path.clone(), 180, None);
        let rx = c.gh_sync();
        let items = c.get().unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[1].value, json!(180));
        assert!(!items[1].configured);

        let it = c.set("gh_sync_interval_secs", &json!(600)).unwrap();
        assert_eq!((it.value.clone(), it.configured), (json!(600), true));
        assert_eq!(*rx.borrow(), 600);
        let it = c.set("update_ref", &json!(" develop ")).unwrap();
        assert_eq!(it.value, json!("develop"));
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            text.starts_with("# meu comentário\nport = 7878\n"),
            "{text}"
        );
        assert!(text.contains("gh_sync_interval_secs = 600"), "{text}");
        assert!(text.contains("update_ref = \"develop\""), "{text}");

        for (k, v) in [
            ("gh_sync_interval_secs", json!(5)),
            ("gh_sync_interval_secs", json!("60")),
            ("update_ref", json!("a b")),
            ("update_ref", json!("--force")),
            ("update_ref", json!("")),
            ("port", json!(1)),
        ] {
            assert_eq!(c.set(k, &v).unwrap_err().0, -32602, "{k}={v}");
        }
        assert_eq!(
            c.set("gh_sync_interval_secs", &json!(0)).unwrap().value,
            json!(0)
        );
    }

    /// #143: grava `[credentials."owner/repo"]` preservando o resto, aplica
    /// na hora e remove a entrada com os dois comandos vazios.
    #[test]
    fn credenciais_do_ambiente_gravam_e_aplicam() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("config.toml");
        std::fs::write(&path, "# meu comentário\nport = 7878\n").unwrap();
        let shared = ct_core::SharedCredentials::default();
        let c = EnvConfig::new(path.clone(), 180, None).with_credentials(shared.clone());

        let got = c
            .set_credentials("acme", "web", " /usr/bin/pass show jira | head -1 ", "")
            .unwrap();
        let want = ct_core::CredentialOverride {
            tracker_cmd: "/usr/bin/pass show jira | head -1".into(),
            forge_cmd: String::new(),
        };
        assert_eq!(got.get("acme", "web"), Some(&want));
        assert_eq!(shared.snapshot(), got);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            text.starts_with("# meu comentário\nport = 7878\n"),
            "{text}"
        );
        assert!(text.contains("[credentials.\"acme/web\"]"), "{text}");
        // O arquivo continua legível pelo `DaemonConfig`.
        let back: CredentialsOnly = toml::from_str(&text).unwrap();
        assert_eq!(back.credentials, got);

        // Outra caixa = a mesma chave; os dois vazios removem.
        let got = c.set_credentials("ACME", "Web", "", "printf tok").unwrap();
        assert_eq!(got.0.len(), 1);
        assert_eq!(got.get("acme", "web").unwrap().forge_cmd, "printf tok");
        let got = c.set_credentials("acme", "web", " ", "").unwrap();
        assert!(got.is_empty());
        assert!(shared.snapshot().is_empty());

        for (o, r, t) in [("", "web", "x"), ("acme", "web", "a\nb")] {
            assert_eq!(c.set_credentials(o, r, t, "").unwrap_err().0, -32602);
        }
        let off = EnvConfig::new(path, 180, None);
        assert!(off.credentials().is_none());
        assert_eq!(
            off.set_credentials("a", "b", "x", "").unwrap_err().0,
            -32000
        );
    }

    #[test]
    fn refs_validos() {
        for r in ["main", "develop", "v1.2.0", "release/2.0"] {
            assert!(valid_ref(r), "{r}");
        }
        for r in ["", "-x", "a..b", "a b", "x.lock", "a~1", "/x", "x/"] {
            assert!(!valid_ref(r), "{r}");
        }
    }
}

#[cfg(test)]
mod conexoes_tests {
    use super::*;

    #[test]
    fn credentials_viram_connections_e_a_segunda_passada_nao_muda_nada() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("config.toml");
        std::fs::write(
            &path,
            "# meu comentário\nport = 7878\n\n[credentials.\"acme/acme-web\"]\ntracker_cmd = \"pass show a/jira\"\n",
        )
        .unwrap();
        let projetos = [ct_core::ProjectConnections {
            owner: "acme".into(),
            repo: "acme-web".into(),
            tracker: Some("jira-acme".into()),
            forge: None,
        }];
        let (conns, sobra, avisos) = convert_credentials_file(&path, &projetos).unwrap();
        assert!(sobra.is_empty(), "{sobra:?}");
        assert!(avisos.is_empty(), "{avisos:?}");
        assert_eq!(conns.credential_cmd("jira-acme"), Some("pass show a/jira"));
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("[credentials"), "{text}");
        assert!(text.contains("# meu comentário"), "{text}");
        assert!(text.contains("port = 7878"), "{text}");
        assert_eq!(read_connection_overrides(&path), conns);

        let (de_novo, _, avisos) = convert_credentials_file(&path, &projetos).unwrap();
        assert_eq!(de_novo, conns);
        assert!(avisos.is_empty());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
    }

    /// Identidade do arquivo: a gravação é temporário + rename, então
    /// qualquer escrita troca o inode.
    fn inode(path: &Path) -> u64 {
        use std::os::unix::fs::MetadataExt;
        std::fs::metadata(path).unwrap().ino()
    }

    fn projetos() -> [ct_core::ProjectConnections; 2] {
        [
            ct_core::ProjectConnections {
                owner: "acme".into(),
                repo: "acme-web".into(),
                tracker: Some("jira-acme".into()),
                forge: None,
            },
            // Sem conexão neste ambiente: o comando fica em `[credentials]`.
            ct_core::ProjectConnections {
                owner: "acme".into(),
                repo: "acme-api".into(),
                tracker: None,
                forge: None,
            },
        ]
    }

    /// #269: o que sobra em `[credentials]` não faz o pull regravar o
    /// arquivo; e a conversão que grava mantém o lugar e os comentários das
    /// tabelas.
    #[test]
    fn sobra_em_credentials_nao_regrava_e_comentarios_ficam() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("config.toml");
        let original = "# topo\nport = 7878\n\n\
                        # minhas conexões\n\
                        [connections.\"gitlab-acme\"] # do GitLab\n\
                        credential_cmd = \"pass show gl\" # token\n\n\
                        [credentials.\"acme/acme-api\"]\n\
                        tracker_cmd = \"pass show api\"\n\n\
                        # a converter\n\
                        [credentials.\"acme/acme-web\"]\n\
                        tracker_cmd = \"pass show web\"\n\n\
                        # fim\n";
        std::fs::write(&path, original).unwrap();

        let (conns, sobra, avisos) = convert_credentials_file(&path, &projetos()).unwrap();
        assert_eq!(conns.credential_cmd("jira-acme"), Some("pass show web"));
        assert_eq!(conns.credential_cmd("gitlab-acme"), Some("pass show gl"));
        assert_eq!(
            sobra.get("acme", "acme-api").unwrap().tracker_cmd,
            "pass show api"
        );
        assert!(sobra.get("acme", "acme-web").is_none());
        assert_eq!(avisos.len(), 1, "{avisos:?}");
        let text = std::fs::read_to_string(&path).unwrap();
        for trecho in [
            "# topo\nport = 7878\n",
            "# minhas conexões\n[connections.\"gitlab-acme\"] # do GitLab\n\
             credential_cmd = \"pass show gl\" # token\n",
            "[credentials.\"acme/acme-api\"]\ntracker_cmd = \"pass show api\"\n",
            "# fim\n",
        ] {
            assert!(text.contains(trecho), "{trecho:?} em:\n{text}");
        }
        assert!(!text.contains("acme/acme-web"), "{text}");
        assert!(!text.contains("pass show web\" "), "{text}");
        // A conexão nova entra junto das outras, antes do `[credentials]`.
        let nova = text.find("[connections.jira-acme]").expect(&text);
        assert!(nova < text.find("[credentials").unwrap(), "{text}");
        assert!(
            text.find("[connections.\"gitlab-acme\"]").unwrap() < nova,
            "{text}"
        );
        assert_eq!(read_connection_overrides(&path), conns);

        // Pulls seguintes, sem mudança efetiva: o arquivo não é tocado.
        let antes = inode(&path);
        for _ in 0..2 {
            let (de_novo, sobra_de_novo, _) = convert_credentials_file(&path, &projetos()).unwrap();
            assert_eq!((de_novo, sobra_de_novo), (conns.clone(), sobra.clone()));
            assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
            assert_eq!(inode(&path), antes);
        }
    }

    /// Comando trocado no lugar (o projeto ganhou conexão da forja): a linha
    /// fica onde estava e o comentário dela também.
    #[test]
    fn comando_que_fica_e_editado_no_lugar() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("config.toml");
        std::fs::write(
            &path,
            "[credentials.\"acme/acme-web\"] # web\n\
             tracker_cmd = \"pass show web\" # jira\n\
             forge_cmd = \"pass show gl\"\n",
        )
        .unwrap();
        let p = [ct_core::ProjectConnections {
            owner: "acme".into(),
            repo: "acme-web".into(),
            tracker: None,
            forge: Some("gitlab-acme".into()),
        }];
        let (conns, sobra, _) = convert_credentials_file(&path, &p).unwrap();
        assert_eq!(conns.credential_cmd("gitlab-acme"), Some("pass show gl"));
        assert_eq!(sobra.get("acme", "acme-web").unwrap().forge_cmd, "");
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            text.starts_with(
                "[credentials.\"acme/acme-web\"] # web\ntracker_cmd = \"pass show web\" # jira\n"
            ),
            "{text}"
        );
        assert!(!text.contains("forge_cmd"), "{text}");
        let antes = inode(&path);
        convert_credentials_file(&path, &p).unwrap();
        assert_eq!(inode(&path), antes);
    }
}

#[cfg(test)]
mod conexoes_ilegiveis_tests {
    use super::*;

    /// `[connections]` que não lê (chave desconhecida): a conversão aborta e
    /// o arquivo fica como estava, sem apagar as entradas do usuário.
    #[test]
    fn connections_que_nao_le_aborta_a_conversao_sem_reescrever() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("config.toml");
        let original = "[credentials.\"acme/acme-web\"]\ntracker_cmd = \"pass show a/jira\"\n\n\
                        [connections.\"jira-antigo\"]\ncredencial = \"x\"\n";
        std::fs::write(&path, original).unwrap();
        let projetos = [ct_core::ProjectConnections {
            owner: "acme".into(),
            repo: "acme-web".into(),
            tracker: Some("jira-acme".into()),
            forge: None,
        }];
        assert!(convert_credentials_file(&path, &projetos).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
    }
}
