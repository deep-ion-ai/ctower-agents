//! `env.config.get|set` (#328): as chaves do `config.toml` que a UI edita.
//! Só uma lista fechada; `set` grava no arquivo (preservando comentários e
//! as outras chaves) e aplica na hora, sem restart:
//!
//! - `update_ref`: o ref seguido pelas atualizações ([`Updates::set_ref`]);
//! - `gh_sync_interval_secs`: o intervalo do sync com o GitHub (`0`
//!   desliga), lido pelo laço do `ctowerd` a cada volta.
//!
//! `env.connections.get|set` (#276) leem e editam, do mesmo jeito, a
//! sobrescrita de credencial por conexão deste ambiente
//! (`[connections."nome"] credential_cmd`), que vale no próximo uso pelo
//! [`ct_core::SharedCredentials`]. Substituem `env.credentials.get|set`
//! (#143), que gravavam `[credentials."owner/repo"]`: o que sobrou dele no
//! arquivo ainda vale e o pull o converte ([`convert_credentials_file`]). A
//! migração do `github.token` do ambiente ([`migrate_github_token`]) cria a
//! sobrescrita da conexão GitHub.

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

/// Uma linha de `env.connections.get` (#276): uma conexão do hub que o
/// ambiente conhece, a fonte dela no hub (sem valor) e a sobrescrita deste
/// ambiente. Também lista a sobrescrita cujo nome não existe no hub (órfã),
/// para poder apagá-la.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct EnvConnection {
    pub name: String,
    /// Tipo; `None` = a conexão não veio no pull (nenhum projeto deste
    /// ambiente a usa) ou a sobrescrita é órfã.
    pub kind: Option<ct_core::provider::ProviderKind>,
    /// URL, host ou API da conexão; vazio quando não veio no pull.
    pub endpoint: String,
    /// Fonte da credencial no hub; `None` quando a conexão não veio no pull.
    pub hub_source: Option<HubSource>,
    /// Comando deste ambiente (`[connections."nome"] credential_cmd`); vazio
    /// = vale a fonte do hub.
    pub credential_cmd: String,
    /// O nome existe no hub. `false` = sobrescrita órfã, que não vale.
    pub in_hub: bool,
    /// Projetos deste ambiente que usam a conexão (nome do projeto).
    pub projects: Vec<String>,
    /// A fonte do hub é um arquivo que não existe nesta máquina e não há
    /// sobrescrita: o uso da conexão aqui falha.
    pub file_missing: bool,
}

/// Fonte de credencial de uma conexão no hub, sem o que poderia ser o
/// token: o tipo e, no secret e no arquivo, o nome e o caminho. O comando do
/// hub não vem (pode levar o token escrito).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct HubSource {
    #[cfg_attr(
        feature = "ts",
        ts(type = "\"secret\" | \"command\" | \"cli\" | \"file\" | \"none\"")
    )]
    pub kind: String,
    /// Nome do secret ou caminho do arquivo; `None` nas outras fontes.
    pub detail: Option<String>,
}

impl HubSource {
    pub fn of(source: &ct_core::provider::CredentialSource) -> Self {
        use ct_core::provider::CredentialSource as S;
        let (kind, detail) = match source {
            S::Secret { name } => ("secret", Some(name.clone())),
            S::Command { .. } => ("command", None),
            S::Cli => ("cli", None),
            S::File { path } => ("file", Some(path.clone())),
            S::None => ("none", None),
        };
        HubSource {
            kind: kind.into(),
            detail,
        }
    }
}

/// As linhas de `env.connections.get`: as conexões do pull (`conns`), os
/// nomes de todas as do hub (`names`) e as sobrescritas, com os projetos
/// atribuídos que usam cada conexão. Em ordem de nome.
pub fn connection_rows(
    conns: &[ct_core::provider::Connection],
    names: &[String],
    overrides: &ct_core::ConnectionOverrides,
    projects: &[ct_ledger::Project],
) -> Vec<EnvConnection> {
    use ct_core::provider::{self, CredentialSource, ProviderKind};
    let uses = |p: &ct_ledger::Project, c: &provider::Connection| match c.kind {
        ProviderKind::Jira => p.tracker.jira().and_then(|j| j.connection) == Some(c.id),
        ProviderKind::Gitlab => p.forge.gitlab().and_then(|g| g.connection) == Some(c.id),
        ProviderKind::Github => {
            let github = p.tracker.kind() == ct_core::TrackerKind::Github
                || p.forge.kind() == ct_core::ForgeKind::Github;
            github
                && provider::github_connection(
                    conns,
                    ct_core::github_connection(&p.tracker, &p.forge),
                )
                .is_some_and(|g| g.id == c.id)
        }
    };
    let mut all: std::collections::BTreeSet<&str> = names.iter().map(String::as_str).collect();
    all.extend(conns.iter().map(|c| c.name.as_str()));
    all.extend(overrides.0.keys().map(String::as_str));
    all.into_iter()
        .map(|name| {
            let conn = conns.iter().find(|c| c.name == name);
            let credential_cmd = overrides.credential_cmd(name).unwrap_or("").to_string();
            let file_missing = conn.is_some_and(|c| match &c.credential {
                CredentialSource::File { path } => {
                    credential_cmd.is_empty() && !provider::local_file(path).exists()
                }
                _ => false,
            });
            EnvConnection {
                name: name.to_string(),
                kind: conn.map(|c| c.kind),
                endpoint: conn.map(|c| c.endpoint.clone()).unwrap_or_default(),
                hub_source: conn.map(|c| HubSource::of(&c.credential)),
                in_hub: conn.is_some() || names.iter().any(|n| n == name),
                projects: conn
                    .map(|c| {
                        projects
                            .iter()
                            .filter(|p| uses(p, c))
                            .map(|p| p.name.clone())
                            .collect()
                    })
                    .unwrap_or_default(),
                file_missing,
                credential_cmd,
            }
        })
        .collect()
}

pub struct EnvConfig {
    /// `<config>/config.toml`.
    path: PathBuf,
    gh_sync: watch::Sender<u64>,
    updates: Option<Arc<Updates>>,
    /// Credenciais em uso no daemon; `None` = edição desligada.
    credentials: Option<ct_core::SharedCredentials>,
}

/// Limite do comando de credencial editado pela UI.
pub const MAX_CREDENTIAL_CMD: usize = 1000;

/// Uma escrita do `config.toml` por vez no processo. `env.config.set`,
/// `env.connections.set`, a conversão do `[credentials]` (#237) e a
/// migração do `github.token` (#276) leem o arquivo e o regravam: todas
/// passam por aqui.
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

    /// Liga `env.connections.set` sobre as sobrescritas em uso.
    pub fn with_credentials(mut self, credentials: ct_core::SharedCredentials) -> Self {
        self.credentials = Some(credentials);
        self
    }

    /// Grava o comando deste ambiente para a conexão `name` em
    /// `[connections."name"] credential_cmd` e aplica no
    /// [`ct_core::SharedCredentials`] (#276). Vazio remove a entrada (volta a
    /// fonte do hub). `known` são os nomes das conexões do hub: comando para
    /// um nome fora dele é recusado, mas a sobrescrita órfã pode ser apagada.
    /// A tabela é editada no lugar e o arquivo só é regravado com mudança.
    pub fn set_connection(
        &self,
        name: &str,
        cmd: &str,
        known: &[String],
    ) -> Result<ct_core::ConnectionOverrides, (i64, String)> {
        let invalid = |m: String| (-32602, m);
        let io = |e: io::Error| (-32000, format!("{}: {e}", self.path.display()));
        let shared = self.credentials.as_ref().ok_or_else(|| {
            (
                -32000,
                "edição das conexões desligada neste ambiente".into(),
            )
        })?;
        let (name, cmd) = (name.trim(), cmd.trim());
        if name.is_empty() {
            return Err(invalid("name é obrigatório".into()));
        }
        if cmd.contains(['\n', '\r']) {
            return Err(invalid("o comando deve ter uma linha só".into()));
        }
        if cmd.len() > MAX_CREDENTIAL_CMD {
            return Err(invalid(format!(
                "comando acima de {MAX_CREDENTIAL_CMD} caracteres"
            )));
        }
        let _guard = config_write();
        let text = self.read_text().map_err(io)?;
        // `[connections]` tem de ler: senão a edição apagaria o que o
        // usuário escreveu à mão.
        let current = toml::from_str::<ConnectionsOnly>(&text)
            .map_err(|_| {
                (
                    -32000,
                    "[connections] do config.toml não lê; corrija o arquivo à mão".to_string(),
                )
            })?
            .connections;
        let exists = current.0.contains_key(name);
        if !known.iter().any(|k| k == name) && !(cmd.is_empty() && exists) {
            return Err(invalid(format!("conexão {name:?} não existe no hub")));
        }
        let mut doc: toml_edit::DocumentMut = text
            .parse()
            .map_err(|_| (-32000, "config.toml não lê".to_string()))?;
        let before = doc.to_string();
        if cmd.is_empty() {
            if let Some(t) = doc
                .get_mut("connections")
                .and_then(|i| i.as_table_like_mut())
            {
                t.remove(name);
            }
        } else {
            set_connection_cmd(&mut doc, name, cmd).map_err(io)?;
        }
        let after = doc.to_string();
        let parsed = toml::from_str::<ConnectionsOnly>(&after)
            .map(|c| c.connections)
            .map_err(|e| (-32000, format!("[connections] inválido: {e}")))?;
        if after != before {
            write_atomic(&self.path, &after).map_err(io)?;
        }
        shared.set_overrides(parsed.clone());
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
        self.read_text()?
            .parse()
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }

    /// O texto do `config.toml`; sem arquivo, vazio.
    fn read_text(&self) -> io::Result<String> {
        match std::fs::read_to_string(&self.path) {
            Ok(s) => Ok(s),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(String::new()),
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

/// Grava `cmd` em `[connections."name"] credential_cmd`, editando a entrada
/// que já existe no lugar (com os comentários) ou criando a tabela.
fn set_connection_cmd(doc: &mut toml_edit::DocumentMut, name: &str, cmd: &str) -> io::Result<()> {
    if !doc.contains_key("connections") {
        let mut t = toml_edit::Table::new();
        t.set_implicit(true);
        doc.insert("connections", toml_edit::Item::Table(t));
    }
    let t = doc["connections"].as_table_like_mut().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "[connections] do config.toml não é uma tabela",
        )
    })?;
    match t.get_mut(name).and_then(|i| i.as_table_like_mut()) {
        Some(e) => set_cmd(e, "credential_cmd", cmd),
        None => {
            let mut e = toml_edit::Table::new();
            e["credential_cmd"] = toml_edit::value(cmd);
            t.insert(name, toml_edit::Item::Table(e));
        }
    }
    Ok(())
}

/// Migração do token do GitHub do ambiente (#276). O `github.token`
/// (`token_file`), gravado pela tela antiga do ambiente, valia no lugar da
/// conexão GitHub. Agora o token vem da conexão, com a sobrescrita
/// `[connections."nome"]`: cada conexão GitHub em uso aqui (`names`) sem
/// sobrescrita ganha uma que lê o arquivo, com o mesmo comando que
/// [`ct_core::provider::source_cmd`] gera para a fonte `File`.
///
/// O arquivo vai para `dest`, e a sobrescrita lê `dest`: sem o
/// `github.token`, a migração não roda de novo, e apagar a sobrescrita pela
/// UI não a faz voltar no pull seguinte. Sem conexão GitHub em uso, ou sem
/// token, nada muda (o arquivo espera um projeto GitHub). Devolve as
/// sobrescritas depois da migração; `None` = nada mudou.
pub fn migrate_github_token(
    config: &Path,
    token_file: &Path,
    dest: &Path,
    names: &[String],
) -> io::Result<Option<ct_core::ConnectionOverrides>> {
    if names.is_empty() {
        return Ok(None);
    }
    match std::fs::read_to_string(token_file) {
        Ok(t) if !t.trim().is_empty() => {}
        Ok(_) => return Ok(None),
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    }
    let _guard = config_write();
    let text = match std::fs::read_to_string(config) {
        Ok(t) => t,
        Err(e) if e.kind() == io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e),
    };
    let invalid = || io::Error::new(io::ErrorKind::InvalidData, "config.toml não lê");
    let existing = toml::from_str::<ConnectionsOnly>(&text)
        .map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "[connections] do config.toml não lê; a migração do github.token fica para depois",
            )
        })?
        .connections;
    let cmd = ct_core::provider::source_cmd(&ct_core::provider::CredentialSource::File {
        path: home_relative(dest),
    })
    .ok_or_else(invalid)?;
    let missing: Vec<&String> = names
        .iter()
        .filter(|n| existing.credential_cmd(n).is_none())
        .collect();
    let mut doc: toml_edit::DocumentMut = text.parse().map_err(|_| invalid())?;
    for name in &missing {
        set_connection_cmd(&mut doc, name, &cmd)?;
    }
    let after = doc.to_string();
    let parsed = toml::from_str::<ConnectionsOnly>(&after)
        .map_err(|_| invalid())?
        .connections;
    std::fs::rename(token_file, dest)?;
    if !missing.is_empty() {
        if let Err(e) = write_atomic(config, &after) {
            // Sem a sobrescrita, o token volta para onde estava.
            let _ = std::fs::rename(dest, token_file);
            return Err(e);
        }
    }
    Ok(Some(parsed))
}

/// `path` com o `HOME` trocado por `~` (o comando vale mesmo que o `HOME`
/// mude); fora do `HOME`, como está.
fn home_relative(path: &Path) -> String {
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        if let Ok(rest) = path.strip_prefix(&home) {
            if !rest.as_os_str().is_empty() {
                return format!("~/{}", rest.display());
            }
        }
    }
    path.display().to_string()
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

#[cfg(test)]
mod sobrescrita_tests {
    use super::*;
    use ct_core::provider::{CredentialSource, ProviderKind};

    fn nomes(n: &[&str]) -> Vec<String> {
        n.iter().map(|s| s.to_string()).collect()
    }

    fn inode(p: &Path) -> u64 {
        use std::os::unix::fs::MetadataExt;
        std::fs::metadata(p).unwrap().ino()
    }

    /// #276: `env.connections.set` grava no lugar, com os comentários, vale
    /// na hora no `SharedCredentials` e só regrava com mudança.
    #[test]
    fn sobrescrita_grava_no_lugar_e_aplica() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("config.toml");
        std::fs::write(
            &path,
            "# meu comentário\nport = 7878\n\n\
             [connections.\"gitlab-acme\"] # do GitLab\n\
             credential_cmd = \"pass show gl\" # token\n",
        )
        .unwrap();
        let shared = ct_core::SharedCredentials::default();
        let c = EnvConfig::new(path.clone(), 180, None).with_credentials(shared.clone());
        let known = nomes(&["jira-acme", "gitlab-acme"]);

        let got = c
            .set_connection("jira-acme", " pass show acme/jira ", &known)
            .unwrap();
        assert_eq!(got.credential_cmd("jira-acme"), Some("pass show acme/jira"));
        assert_eq!(got.credential_cmd("gitlab-acme"), Some("pass show gl"));
        assert_eq!(shared.connections().1, got);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            text.starts_with(
                "# meu comentário\nport = 7878\n\n[connections.\"gitlab-acme\"] # do GitLab\n\
                 credential_cmd = \"pass show gl\" # token\n"
            ),
            "{text}"
        );
        assert_eq!(read_connection_overrides(&path), got);

        // Troca no lugar: o comentário da linha fica.
        c.set_connection("gitlab-acme", "pass show acme/gl", &known)
            .unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            text.contains("credential_cmd = \"pass show acme/gl\" # token\n"),
            "{text}"
        );

        // O mesmo valor não regrava o arquivo.
        let antes = inode(&path);
        c.set_connection("jira-acme", "pass show acme/jira", &known)
            .unwrap();
        assert_eq!(inode(&path), antes);

        // Vazio remove: volta a fonte do hub.
        let got = c.set_connection("jira-acme", "  ", &known).unwrap();
        assert_eq!(got.credential_cmd("jira-acme"), None);
        assert_eq!(shared.connections().1, got);
        assert!(!std::fs::read_to_string(&path)
            .unwrap()
            .contains("jira-acme"));
    }

    #[test]
    fn sobrescrita_recusa_nome_fora_do_hub_mas_apaga_a_orfa() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("config.toml");
        std::fs::write(
            &path,
            "[connections.\"jira-antigo\"]\ncredential_cmd = \"pass show x\"\n",
        )
        .unwrap();
        let shared = ct_core::SharedCredentials::default();
        let c = EnvConfig::new(path.clone(), 180, None).with_credentials(shared.clone());
        let known = nomes(&["jira-acme"]);
        for (n, cmd) in [
            ("jira-outro", "pass show y"),
            ("jira-outro", ""),
            ("", "pass show y"),
            ("jira-acme", "a\nb"),
            ("jira-antigo", "pass show z"),
        ] {
            assert_eq!(
                c.set_connection(n, cmd, &known).unwrap_err().0,
                -32602,
                "{n}={cmd}"
            );
        }
        let got = c.set_connection("jira-antigo", "", &known).unwrap();
        assert!(got.0.is_empty(), "{got:?}");

        // `[connections]` que não lê: nada é regravado.
        let ruim = "[connections.\"jira-acme\"]\ncredencial = \"x\"\n";
        std::fs::write(&path, ruim).unwrap();
        assert_eq!(
            c.set_connection("jira-acme", "pass show acme/jira", &known)
                .unwrap_err()
                .0,
            -32000
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), ruim);

        let off = EnvConfig::new(path, 180, None);
        assert_eq!(
            off.set_connection("jira-acme", "x", &known).unwrap_err().0,
            -32000
        );
    }

    fn ledger_com_conexoes(arquivo: &str) -> ct_ledger::Ledger {
        let l = ct_ledger::Ledger::open_in_memory().unwrap();
        let cfg: ct_ledger::ConfigResponse = serde_json::from_value(json!({
            "config_rev": 7,
            "changed": true,
            "projects": [
                { "owner": "acme", "repo": "acme-web", "name": "acme-web",
                  "base_branch": "main", "protected_branches": [],
                  "repo_path": "/x", "rev": 7,
                  "tracker": { "kind": "jira", "project_key": "ACME",
                               "jql": "labels = acme-web", "connection": 1 } },
                { "owner": "acme", "repo": "acme-api", "name": "acme-api",
                  "base_branch": "main", "protected_branches": [],
                  "repo_path": "/y", "rev": 7 }
            ],
            "connections": [
                { "id": 1, "name": "jira-acme", "kind": "jira",
                  "endpoint": "https://jira.example.com",
                  "credential": { "kind": "command", "cmd": "printf 'tok-ficticio'" },
                  "options": {}, "created_at": 0, "updated_at": 0 },
                { "id": 2, "name": "github", "kind": "github", "endpoint": "",
                  "credential": { "kind": "file", "path": arquivo },
                  "options": {}, "created_at": 0, "updated_at": 0 }
            ],
            "connection_names": ["github", "jira-acme", "jira-livre"]
        }))
        .unwrap();
        l.apply_hub_config(&cfg, |p| p.repo_path.clone()).unwrap();
        l
    }

    /// `env.connections.get`: as conexões do pull, os nomes do hub e a
    /// sobrescrita órfã; a fonte sem o comando do hub; quem usa cada uma; e
    /// o arquivo do hub que não existe aqui.
    #[test]
    fn linhas_das_conexoes_sem_token_e_com_o_arquivo_ausente() {
        let d = tempfile::tempdir().unwrap();
        let ausente = d.path().join("ausente.token").display().to_string();
        let l = ledger_com_conexoes(&ausente);
        let (conns, names) = l.hub_connections().unwrap();
        let projetos = l.list_projects().unwrap();
        let overrides: ct_core::ConnectionOverrides = toml::from_str(
            "[\"jira-acme\"]\ncredential_cmd = \"pass show acme/jira\"\n\
             [\"jira-antigo\"]\ncredential_cmd = \"pass show x\"\n",
        )
        .unwrap();
        let rows = connection_rows(&conns, &names, &overrides, &projetos);
        let nomes: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(nomes, ["github", "jira-acme", "jira-antigo", "jira-livre"]);

        let gh = &rows[0];
        assert_eq!(gh.kind, Some(ProviderKind::Github));
        assert_eq!(
            gh.hub_source,
            Some(HubSource {
                kind: "file".into(),
                detail: Some(ausente.clone())
            })
        );
        // O acme-web tem o Jira e o repositório no GitHub (a forja padrão).
        assert_eq!(gh.projects, ["acme-api", "acme-web"]);
        assert!(gh.file_missing && gh.in_hub);

        let jira = &rows[1];
        assert_eq!(jira.endpoint, "https://jira.example.com");
        assert_eq!(jira.credential_cmd, "pass show acme/jira");
        assert_eq!(jira.projects, ["acme-web"]);
        assert_eq!(jira.hub_source.as_ref().unwrap().kind, "command");
        assert_eq!(jira.hub_source.as_ref().unwrap().detail, None);
        let json = serde_json::to_string(&rows).unwrap();
        assert!(!json.contains("tok-ficticio"), "{json}");

        let orfa = &rows[2];
        assert!(!orfa.in_hub && orfa.kind.is_none());
        assert_eq!(orfa.credential_cmd, "pass show x");
        let livre = &rows[3];
        assert!(livre.in_hub && livre.hub_source.is_none() && livre.projects.is_empty());

        // Com a sobrescrita, o arquivo ausente não importa.
        let com: ct_core::ConnectionOverrides =
            toml::from_str("[github]\ncredential_cmd = \"gh auth token\"\n").unwrap();
        assert!(!connection_rows(&conns, &names, &com, &projetos)[0].file_missing);
        // Arquivo que existe também não.
        std::fs::write(&ausente, "x").unwrap();
        assert!(!connection_rows(&conns, &names, &overrides, &projetos)[0].file_missing);
    }

    /// Migração do `github.token` do ambiente (#276): cada conexão GitHub
    /// sem sobrescrita ganha uma que lê o arquivo (o comando da fonte
    /// `File`); o arquivo muda de nome e a migração não roda de novo.
    #[test]
    fn github_token_vira_sobrescrita_uma_vez() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("config.toml");
        let token = d.path().join("github.token");
        let dest = d.path().join("github-env.token");
        std::fs::write(
            &path,
            "# topo\n[connections.\"github-acme\"]\ncredential_cmd = \"pass show acme/gh\"\n",
        )
        .unwrap();
        let names = nomes(&["github", "github-acme"]);

        // Sem token, ou sem conexão GitHub em uso: nada muda.
        assert_eq!(
            migrate_github_token(&path, &token, &dest, &names).unwrap(),
            None
        );
        std::fs::write(&token, "ghp_ficticio_123\n").unwrap();
        assert_eq!(
            migrate_github_token(&path, &token, &dest, &[]).unwrap(),
            None
        );
        assert!(token.exists());

        let got = migrate_github_token(&path, &token, &dest, &names)
            .unwrap()
            .unwrap();
        let esperado = ct_core::provider::source_cmd(&CredentialSource::File {
            path: home_relative(&dest),
        })
        .unwrap();
        assert_eq!(got.credential_cmd("github"), Some(esperado.as_str()));
        // A sobrescrita que já havia fica.
        assert_eq!(got.credential_cmd("github-acme"), Some("pass show acme/gh"));
        assert!(!token.exists());
        assert_eq!(
            std::fs::read_to_string(&dest).unwrap(),
            "ghp_ficticio_123\n"
        );
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("# topo\n"), "{text}");
        assert!(!text.contains("ghp_ficticio"), "{text}");
        assert_eq!(read_connection_overrides(&path), got);

        // O comando lê o token do arquivo novo.
        let out = std::process::Command::new("sh")
            .args(["-c", &esperado])
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout), "ghp_ficticio_123\n");

        // Apagar a sobrescrita não a traz de volta.
        let c = EnvConfig::new(path.clone(), 180, None)
            .with_credentials(ct_core::SharedCredentials::default());
        c.set_connection("github", "", &names).unwrap();
        assert_eq!(
            migrate_github_token(&path, &token, &dest, &names).unwrap(),
            None
        );
        assert_eq!(
            read_connection_overrides(&path).credential_cmd("github"),
            None
        );
    }

    /// `[connections]` que não lê: a migração espera e o token fica.
    #[test]
    fn migracao_do_github_token_espera_config_ilegivel() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("config.toml");
        let token = d.path().join("github.token");
        let dest = d.path().join("github-env.token");
        let ruim = "[connections.github]\ncredencial = \"x\"\n";
        std::fs::write(&path, ruim).unwrap();
        std::fs::write(&token, "ghp_ficticio_123\n").unwrap();
        assert!(migrate_github_token(&path, &token, &dest, &nomes(&["github"])).is_err());
        assert!(token.exists() && !dest.exists());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), ruim);
    }
}
