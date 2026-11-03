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
use std::path::PathBuf;
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
    /// Uma escrita por vez.
    write: std::sync::Mutex<()>,
    /// `[credentials]` em uso no daemon; `None` = edição desligada.
    credentials: Option<ct_core::SharedCredentials>,
}

/// Limite de cada comando de credencial editado pela UI.
pub const MAX_CREDENTIAL_CMD: usize = 1000;

impl EnvConfig {
    pub fn new(path: PathBuf, gh_sync_secs: u64, updates: Option<Arc<Updates>>) -> Self {
        EnvConfig {
            path,
            gh_sync: watch::channel(gh_sync_secs).0,
            updates,
            write: std::sync::Mutex::new(()),
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
        let _guard = self.write.lock().unwrap();
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
        let _guard = self.write.lock().unwrap();
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

    /// Temporário + rename, com a permissão do arquivo atual.
    fn store(&self, doc: &toml_edit::DocumentMut) -> io::Result<()> {
        let tmp = self.path.with_extension("toml.tmp");
        std::fs::write(&tmp, doc.to_string())?;
        if let Ok(m) = std::fs::metadata(&self.path) {
            std::fs::set_permissions(&tmp, m.permissions())?;
        }
        std::fs::rename(&tmp, &self.path)
    }
}

#[derive(serde::Deserialize)]
struct CredentialsOnly {
    #[serde(default)]
    credentials: ct_core::CredentialOverrides,
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
