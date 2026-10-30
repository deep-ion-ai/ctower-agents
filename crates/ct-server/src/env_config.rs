//! `env.config.get|set` (#328): as chaves do `config.toml` que a UI edita.
//! Só uma lista fechada; `set` grava no arquivo (preservando comentários e
//! as outras chaves) e aplica na hora, sem restart:
//!
//! - `update_ref`: o ref seguido pelas atualizações ([`Updates::set_ref`]);
//! - `gh_sync_interval_secs`: o intervalo do sync com o GitHub (`0`
//!   desliga), lido pelo laço do `ctowerd` a cada volta.

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

pub struct EnvConfig {
    /// `<config>/config.toml`.
    path: PathBuf,
    gh_sync: watch::Sender<u64>,
    updates: Option<Arc<Updates>>,
    /// Uma escrita por vez.
    write: std::sync::Mutex<()>,
}

impl EnvConfig {
    pub fn new(path: PathBuf, gh_sync_secs: u64, updates: Option<Arc<Updates>>) -> Self {
        EnvConfig {
            path,
            gh_sync: watch::channel(gh_sync_secs).0,
            updates,
            write: std::sync::Mutex::new(()),
        }
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
