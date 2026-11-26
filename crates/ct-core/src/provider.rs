//! Conexões externas (épico #224, análise de providers §3): o tipo de
//! provider, a fonte de credencial e um descritor por tipo, que leva a
//! validação, o prefixo das instruções da sessão e o nome da variável do
//! token. Puro, sem I/O: a checagem de "quem sou eu" na rede fica com o
//! ambiente.
//!
//! Ainda não há conexão gravada. O cadastro atual (`JiraConfig`,
//! `GitlabConfig`) entra por [`Connection::from_jira`] e
//! [`Connection::from_gitlab`] até a migração do épico.

use crate::model::ParseEnumError;
use crate::tracker::{
    ForgeConfig, GitlabConfig, JiraConfig, TrackerConfig, MAX_ISSUES_CEILING, MAX_JIRA_EXTRA_FIELDS,
};
use crate::JiraExtraField;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fmt;
use std::str::FromStr;

str_enum!(ProviderKind, "provider_kind", {
    Github => "github",
    Gitlab => "gitlab",
    Jira => "jira",
});

/// Conexão com um serviço externo (GitHub, GitLab, Jira).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Connection {
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub id: i64,
    /// Único no hub, ex.: `jira-empresa`.
    pub name: String,
    pub kind: ProviderKind,
    /// URL (Jira), host (GitLab) ou API (GitHub; vazio = padrão do tipo).
    pub endpoint: String,
    pub credential: CredentialSource,
    /// Específico do tipo, validado pelo descritor (ex.: Jira: `config_file`).
    #[cfg_attr(feature = "ts", ts(type = "Record<string, unknown>"))]
    pub options: Value,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub created_at: i64,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub updated_at: i64,
}

impl Connection {
    /// Conexão equivalente ao cadastro atual do Jira (ponte até a migração).
    pub fn from_jira(j: &JiraConfig) -> Self {
        Connection {
            id: 0,
            name: String::new(),
            kind: ProviderKind::Jira,
            endpoint: j.url.clone(),
            credential: CredentialSource::Command {
                cmd: j.credential_cmd.clone(),
            },
            options: json!({ "config_file": j.config_file }),
            created_at: 0,
            updated_at: 0,
        }
    }

    /// Conexão equivalente ao cadastro atual do GitLab (ponte até a migração).
    pub fn from_gitlab(g: &GitlabConfig) -> Self {
        Connection {
            id: 0,
            name: String::new(),
            kind: ProviderKind::Gitlab,
            endpoint: g.host.clone(),
            credential: CredentialSource::Command {
                cmd: g.credential_cmd.clone(),
            },
            options: json!({}),
            created_at: 0,
            updated_at: 0,
        }
    }
}

/// De onde vem o token de uma conexão.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CredentialSource {
    /// Secret do store do hub, pelo nome.
    Secret { name: String },
    /// Comando de shell; a primeira linha da saída é o token.
    Command { cmd: String },
    /// Token da CLI do serviço (`gh auth token`; só GitHub por ora).
    Cli,
    /// Arquivo com o token (compatível com `github.token`).
    File { path: String },
    /// Sem autenticação.
    None,
}

/// O que o tipo sabe fazer: rastrear issues e/ou operar a forja (PRs, MRs).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Capabilities {
    pub tracker: bool,
    pub forge: bool,
}

/// Prontidão de uma conexão, sem rede: se o cadastro basta para tentar.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Readiness {
    Ready,
    NeedsSetup { reason: String },
}

/// Descritor de um tipo de provider: o que os `match` por tipo faziam
/// espalhados pelo código.
pub trait ProviderDescriptor: Sync {
    fn kind(&self) -> ProviderKind;
    fn label(&self) -> &'static str;
    fn capabilities(&self) -> Capabilities;
    /// Endpoint quando a conexão não diz outro.
    fn default_endpoint(&self) -> Option<&'static str> {
        None
    }
    /// Variável de ambiente que leva o token (`GH_TOKEN`...).
    fn token_env(&self) -> &'static str;
    /// Confere a conexão sem rede: tipo, endpoint, credencial e opções.
    fn validate(&self, c: &Connection) -> std::result::Result<(), String>;
    /// Prefixo das instruções da sessão (`JIRA_API_TOKEN="$(...)" jira`).
    fn session_prefix(&self, c: &Connection) -> Option<String>;
    /// Prontidão estática; o "quem sou eu" na rede é do ambiente.
    fn readiness(&self, c: &Connection) -> Readiness {
        match self.validate(c) {
            Ok(()) => Readiness::Ready,
            Err(reason) => Readiness::NeedsSetup { reason },
        }
    }
}

pub struct GithubDescriptor;
pub struct GitlabDescriptor;
pub struct JiraDescriptor;

/// Descritor do GitHub.
pub static GITHUB: GithubDescriptor = GithubDescriptor;
/// Descritor do GitLab.
pub static GITLAB: GitlabDescriptor = GitlabDescriptor;
/// Descritor do Jira.
pub static JIRA: JiraDescriptor = JiraDescriptor;

/// Registro estático: um descritor por [`ProviderKind`].
pub static PROVIDERS: &[&dyn ProviderDescriptor] = &[&GITHUB, &GITLAB, &JIRA];

/// Descritor do tipo. Panics só se um tipo novo ficar sem descritor, o que
/// o teste `registro_tem_um_descritor_por_tipo` pega.
pub fn descriptor(kind: ProviderKind) -> &'static dyn ProviderDescriptor {
    PROVIDERS
        .iter()
        .copied()
        .find(|d| d.kind() == kind)
        .expect("todo ProviderKind tem descritor no registro")
}

/// Nome da conexão GitHub padrão (#275): a que a migração cria (#237) e
/// que vale para o projeto GitHub sem conexão.
pub const DEFAULT_GITHUB_CONNECTION: &str = "github";

/// Conexão GitHub de um projeto (#275): a referenciada (`id`) ou, sem
/// referência, a padrão ([`DEFAULT_GITHUB_CONNECTION`]). Conexão de outro
/// tipo não serve. `None` = nenhuma: quem usa cai no `gh auth token`.
pub fn github_connection(conns: &[Connection], id: Option<i64>) -> Option<&Connection> {
    let github = conns.iter().filter(|c| c.kind == ProviderKind::Github);
    match id {
        Some(id) => github.into_iter().find(|c| c.id == id),
        None => github
            .into_iter()
            .find(|c| c.name == DEFAULT_GITHUB_CONNECTION),
    }
}

/// Papel de uma conexão num projeto: o rastreador (issues) ou a forja
/// (repositório, PRs e MRs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Tracker,
    Forge,
}

impl Role {
    pub fn label(self) -> &'static str {
        match self {
            Role::Tracker => "rastreador",
            Role::Forge => "forja",
        }
    }
}

/// A conexão serve ao papel, pelo que o descritor do tipo diz de si
/// ([`ProviderDescriptor::capabilities`]).
pub fn check_role(c: &Connection, role: Role) -> std::result::Result<(), String> {
    let caps = descriptor(c.kind).capabilities();
    let serve = match role {
        Role::Tracker => caps.tracker,
        Role::Forge => caps.forge,
    };
    if serve {
        Ok(())
    } else {
        Err(format!(
            "conexão {} é do tipo {}, que não serve de {}",
            c.name,
            c.kind,
            role.label()
        ))
    }
}

/// Comando de shell que imprime o token da conexão: o que o cadastro antigo
/// guardava em `credential_cmd` e o que o prefixo da sessão expande
/// ([`source_cmd`]).
pub fn credential_cmd(c: &Connection) -> Option<String> {
    source_cmd(&c.credential)
}

/// Comando de shell que imprime o token da fonte, para o prefixo das
/// instruções da sessão (`JIRA_API_TOKEN="$(<comando>)"`, #274):
///
/// - `Command`: o próprio comando;
/// - `Secret`: `ctower secret get NOME`, que lê o valor atual no projeto da
///   sessão (a entrada do projeto sobrepõe a global, #279);
/// - `File`: a primeira linha não vazia do arquivo, sem espaços nas pontas,
///   com `~` expandido, como o resolvedor do ambiente lê ([`file_cmd`]).
///
/// `Cli` e `None` não têm comando (o `Cli` é só do GitHub, que não leva
/// prefixo). Fonte sem nome, comando ou caminho também não.
pub fn source_cmd(source: &CredentialSource) -> Option<String> {
    match source {
        CredentialSource::Command { cmd } if !cmd.trim().is_empty() => Some(cmd.trim().to_string()),
        CredentialSource::Secret { name } if !name.trim().is_empty() => {
            Some(format!("ctower secret get {}", shell_word(name.trim())))
        }
        CredentialSource::File { path } if !path.trim().is_empty() => Some(file_cmd(path.trim())),
        _ => None,
    }
}

/// Comando que imprime a primeira linha não vazia de `path`, sem espaços nas
/// pontas: o mesmo token que o resolvedor do ambiente lê da fonte `File`.
/// `~` e `~/...` viram `"$HOME"` (o resto vai entre aspas simples quando
/// precisa, então espaço e aspas no caminho não quebram o comando). O
/// arquivo entra pelo `<`, e não como argumento do `awk`, que trataria
/// `a=b` como atribuição.
pub fn file_cmd(path: &str) -> String {
    const AWK: &str =
        r#"awk '{ sub(/^[[:space:]]+/, ""); sub(/[[:space:]]+$/, "") } $0 != "" { print; exit }'"#;
    format!("{AWK} < {}", file_word(path))
}

/// Caminho como palavra do shell, com `~` trocado por `"$HOME"` só nas
/// formas `~` e `~/...` (as que o resolvedor expande). Outro `~` no começo
/// (`~fulano/...`) vai entre aspas: o resolvedor não o expande, e o shell
/// também não pode expandir.
fn file_word(path: &str) -> String {
    if path == "~" {
        return r#""$HOME""#.into();
    }
    if let Some(rest) = path.strip_prefix("~/") {
        return match rest {
            "" => r#""$HOME"/"#.into(),
            r => format!(r#""$HOME"/{}"#, shell_word(r)),
        };
    }
    if path.starts_with('~') {
        return format!("'{}'", path.replace('\'', r"'\''"));
    }
    shell_word(path)
}

/// O que faltou ao resolver as conexões de um projeto
/// ([`resolve_connections`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionGap {
    /// O projeto referencia a conexão `id`, mas não há conexão desse tipo
    /// com esse `id`. O lado fica como está no cadastro.
    Missing { kind: ProviderKind, id: i64 },
    /// A conexão existe, mas não deu comando de credencial.
    NoCredential { kind: ProviderKind, name: String },
}

/// Visão efetiva do rastreador e da forja (#237, #264): nos lados que
/// referenciam uma conexão, preenche os campos do cadastro antigo a partir
/// dela (Jira: `url`, `config_file` e `credential_cmd`; GitLab: `host` e
/// `credential_cmd`). Só em memória: o cadastro do projeto não volta a
/// guardar esses campos. `credential_cmd` diz o comando de credencial de
/// cada conexão (o ambiente aplica as sobrescritas dele; o hub usa
/// [`credential_cmd`]). Ponto único do ambiente (`materialize_connections`)
/// e do hub. Devolve o que faltou.
pub fn resolve_connections(
    tracker: &mut TrackerConfig,
    forge: &mut ForgeConfig,
    conns: &[Connection],
    credential_cmd: impl Fn(&Connection) -> String,
) -> Vec<ConnectionGap> {
    let find = |kind: ProviderKind, id: i64| conns.iter().find(|c| c.id == id && c.kind == kind);
    let mut gaps = Vec::new();
    if let TrackerConfig::Jira(j) = tracker {
        if let Some(id) = j.connection {
            match find(ProviderKind::Jira, id) {
                Some(c) => {
                    j.url = c.endpoint.trim().trim_end_matches('/').to_string();
                    j.config_file = options_str(c, "config_file").to_string();
                    j.credential_cmd = credential_cmd(c);
                    if j.credential_cmd.is_empty() {
                        gaps.push(ConnectionGap::NoCredential {
                            kind: c.kind,
                            name: c.name.clone(),
                        });
                    }
                }
                None => gaps.push(ConnectionGap::Missing {
                    kind: ProviderKind::Jira,
                    id,
                }),
            }
        }
    }
    if let ForgeConfig::Gitlab(g) = forge {
        if let Some(id) = g.connection {
            match find(ProviderKind::Gitlab, id) {
                Some(c) => {
                    g.host = c.endpoint.trim().to_string();
                    g.credential_cmd = credential_cmd(c);
                    if g.credential_cmd.is_empty() {
                        gaps.push(ConnectionGap::NoCredential {
                            kind: c.kind,
                            name: c.name.clone(),
                        });
                    }
                }
                None => gaps.push(ConnectionGap::Missing {
                    kind: ProviderKind::Gitlab,
                    id,
                }),
            }
        }
    }
    gaps
}

impl ProviderDescriptor for GithubDescriptor {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Github
    }

    fn label(&self) -> &'static str {
        "GitHub"
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            tracker: true,
            forge: true,
        }
    }

    fn default_endpoint(&self) -> Option<&'static str> {
        Some("https://api.github.com")
    }

    fn token_env(&self) -> &'static str {
        "GH_TOKEN"
    }

    fn validate(&self, c: &Connection) -> std::result::Result<(), String> {
        check_kind(ProviderKind::Github, c)?;
        check_options(c)
    }

    /// O GitHub não leva prefixo: o `gh` lê o token do ambiente da sessão.
    fn session_prefix(&self, _c: &Connection) -> Option<String> {
        None
    }
}

impl ProviderDescriptor for GitlabDescriptor {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Gitlab
    }

    fn label(&self) -> &'static str {
        "GitLab"
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            tracker: false,
            forge: true,
        }
    }

    fn token_env(&self) -> &'static str {
        "GITLAB_TOKEN"
    }

    fn validate(&self, c: &Connection) -> std::result::Result<(), String> {
        check_kind(ProviderKind::Gitlab, c)?;
        check_options(c)?;
        if c.endpoint.is_empty() {
            return Err("informe o host do GitLab".into());
        }
        check_token_source("GitLab", "gitlab", c)
    }

    /// `GITLAB_HOST=<host> GITLAB_TOKEN="$(<cmd>)" glab`.
    fn session_prefix(&self, c: &Connection) -> Option<String> {
        let mut parts = Vec::new();
        if !c.endpoint.trim().is_empty() {
            parts.push(format!("GITLAB_HOST={}", shell_word(c.endpoint.trim())));
        }
        if let Some(expr) = token_expr(&c.credential) {
            parts.push(format!("GITLAB_TOKEN=\"{expr}\""));
        }
        parts.push("glab".into());
        Some(parts.join(" "))
    }
}

impl GitlabDescriptor {
    /// Repositório do GitLab: host e caminho (`grupo/projeto`). Fica no
    /// cadastro do projeto, não na conexão.
    pub fn validate_project(&self, g: &GitlabConfig) -> std::result::Result<(), String> {
        // Com conexão referenciada, o host vem dela.
        let host_ok = g.connection.is_some() || !g.host.is_empty();
        if !host_ok || !g.path.contains('/') {
            return Err(format!(
                "repositório do GitLab inválido: host {:?}, caminho {:?}",
                g.host, g.path
            ));
        }
        Ok(())
    }
}

impl ProviderDescriptor for JiraDescriptor {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Jira
    }

    fn label(&self) -> &'static str {
        "Jira"
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            tracker: true,
            forge: false,
        }
    }

    fn token_env(&self) -> &'static str {
        "JIRA_API_TOKEN"
    }

    fn validate(&self, c: &Connection) -> std::result::Result<(), String> {
        check_kind(ProviderKind::Jira, c)?;
        check_options(c)?;
        check_jira_url(c)?;
        check_token_source("Jira", "jira", c)
    }

    /// `JIRA_CONFIG_FILE=<arquivo> JIRA_API_TOKEN="$(<cmd>)" jira`. O
    /// `$(...)` é expandido pelo shell do agente, só no processo do `jira`:
    /// o prefixo nunca leva a senha.
    fn session_prefix(&self, c: &Connection) -> Option<String> {
        let mut parts = Vec::new();
        let config_file = options_str(c, "config_file").trim();
        if !config_file.is_empty() {
            parts.push(format!("JIRA_CONFIG_FILE={}", shell_word(config_file)));
        }
        if let Some(expr) = token_expr(&c.credential) {
            parts.push(format!("JIRA_API_TOKEN=\"{expr}\""));
        }
        parts.push("jira".into());
        Some(parts.join(" "))
    }
}

impl JiraDescriptor {
    /// Projeto Jira: chave e campos próprios da tela da issue. Fica no
    /// cadastro do projeto, não na conexão.
    pub fn validate_project(&self, j: &JiraConfig) -> std::result::Result<(), String> {
        check_project_key(j)?;
        validate_extra_fields(&j.extra_fields)
    }

    /// Cadastro do Jira sem conexão referenciada: a conexão embutida (URL e
    /// credencial) e o projeto, na ordem de antes da #234 (URL, chave do
    /// projeto, credencial, campos da tela). Com mais de uma falha, o erro
    /// da chave vem antes do da credencial (#272).
    pub fn validate_inline(&self, j: &JiraConfig) -> std::result::Result<(), String> {
        let c = Connection::from_jira(j);
        check_jira_url(&c)?;
        check_project_key(j)?;
        check_token_source("Jira", "jira", &c)?;
        validate_extra_fields(&j.extra_fields)
    }

    /// Filtro do sync: no modo sprint ativa, nada a conferir; fora dele,
    /// JQL obrigatório e limite de issues entre 1 e o teto.
    pub fn validate_sync_filter(&self, j: &JiraConfig) -> std::result::Result<(), String> {
        if j.active_sprint_only {
            return Ok(());
        }
        if j.jql.trim().is_empty() {
            return Err(
                "informe o JQL do sync (ex.: labels = minha-equipe AND updated >= -90d) \
                        ou marque \"sincronizar somente a sprint ativa\""
                    .into(),
            );
        }
        if j.max_issues == 0 || j.max_issues > MAX_ISSUES_CEILING {
            return Err(format!(
                "limite de issues sincronizadas inválido: {} (de 1 a {})",
                j.max_issues, MAX_ISSUES_CEILING
            ));
        }
        Ok(())
    }
}

fn check_jira_url(c: &Connection) -> std::result::Result<(), String> {
    let url = c.endpoint.to_ascii_lowercase();
    if !(url.starts_with("https://") || url.starts_with("http://")) || url.len() < 10 {
        return Err(format!("URL do Jira inválida: {:?}", c.endpoint));
    }
    Ok(())
}

/// Chave do projeto Jira: maiúsculas, dígitos e `_`, começando por letra.
fn check_project_key(j: &JiraConfig) -> std::result::Result<(), String> {
    let key_ok = j.project_key.starts_with(|c: char| c.is_ascii_uppercase())
        && j.project_key
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
    if !key_ok {
        return Err(format!(
            "chave do projeto Jira inválida: {:?} (maiúsculas, como PROJ)",
            j.project_key
        ));
    }
    Ok(())
}

/// Campos do projeto na tela da issue: ID e rótulo preenchidos, sem ID
/// repetido e no máximo [`MAX_JIRA_EXTRA_FIELDS`]. O ID não precisa estar
/// no catálogo do jira-cli: pode ser digitado à mão.
fn validate_extra_fields(fields: &[JiraExtraField]) -> std::result::Result<(), String> {
    let max = MAX_JIRA_EXTRA_FIELDS;
    if fields.len() > max {
        return Err(format!(
            "campos do projeto: no máximo {max} campos na tela da issue ({} informados)",
            fields.len()
        ));
    }
    let mut seen = std::collections::HashSet::new();
    for (i, f) in fields.iter().enumerate() {
        let (id, label) = (f.id.trim(), f.label.trim());
        let n = i + 1;
        if id.is_empty() {
            return Err(format!(
                "campos do projeto: informe o campo do Jira na linha {n} (ex.: customfield_10099)"
            ));
        }
        if label.is_empty() {
            return Err(format!(
                "campos do projeto: informe o rótulo na tela do campo {id} (linha {n})"
            ));
        }
        if !seen.insert(id.to_ascii_lowercase()) {
            return Err(format!(
                "campos do projeto: o campo {id} aparece mais de uma vez (linha {n})"
            ));
        }
    }
    Ok(())
}

fn check_kind(expected: ProviderKind, c: &Connection) -> std::result::Result<(), String> {
    if c.kind == expected {
        Ok(())
    } else {
        Err(format!("conexão do tipo {} não é {expected}", c.kind))
    }
}

fn check_options(c: &Connection) -> std::result::Result<(), String> {
    match &c.options {
        Value::Object(_) | Value::Null => Ok(()),
        _ => Err("opções da conexão devem ser um objeto".into()),
    }
}

/// Jira e GitLab aceitam comando de credencial, secret do hub ou arquivo
/// (#279). O `Cli` é o token da CLI do próprio tipo e só existe no GitHub
/// (`gh auth token`).
fn check_token_source(label: &str, slug: &str, c: &Connection) -> std::result::Result<(), String> {
    match &c.credential {
        CredentialSource::Command { cmd } if cmd.trim().is_empty() => Err(format!(
            "informe o comando de credencial do {label} (ex.: pass show {slug} | head -1)"
        )),
        CredentialSource::Secret { name } if name.trim().is_empty() => {
            Err(format!("informe o nome do secret da credencial do {label}"))
        }
        CredentialSource::File { path } if path.trim().is_empty() => Err(format!(
            "informe o caminho do arquivo com o token do {label}"
        )),
        CredentialSource::Command { .. }
        | CredentialSource::Secret { .. }
        | CredentialSource::File { .. } => Ok(()),
        CredentialSource::Cli | CredentialSource::None => Err(format!(
            "credencial do {label} precisa de comando de credencial, secret ou arquivo"
        )),
    }
}

/// Expressão do token no prefixo: `$(<comando>)`, com o comando da fonte
/// ([`source_cmd`]).
fn token_expr(credential: &CredentialSource) -> Option<String> {
    source_cmd(credential).map(|cmd| format!("$({cmd})"))
}

fn options_str<'a>(c: &'a Connection, key: &str) -> &'a str {
    c.options.get(key).and_then(Value::as_str).unwrap_or("")
}

/// Palavra do shell com aspas simples só quando precisa (caminho com espaço
/// ou caractere especial).
pub fn shell_word(s: &str) -> String {
    let safe = !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"/._-~+=:,@%".contains(&b));
    if safe {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', r"'\''"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jira_cfg() -> JiraConfig {
        JiraConfig {
            url: "https://jira.example.com".into(),
            project_key: "ACME".into(),
            jql: "labels = acme-web".into(),
            max_issues: 100,
            credential_cmd: "pass show jira | head -1".into(),
            ..Default::default()
        }
    }

    fn conn(kind: ProviderKind, endpoint: &str, credential: CredentialSource) -> Connection {
        Connection {
            id: 1,
            name: "acme".into(),
            kind,
            endpoint: endpoint.into(),
            credential,
            options: json!({}),
            created_at: 0,
            updated_at: 0,
        }
    }

    #[test]
    fn registro_tem_um_descritor_por_tipo() {
        assert_eq!(PROVIDERS.len(), ProviderKind::ALL.len());
        for kind in ProviderKind::ALL {
            assert_eq!(descriptor(*kind).kind(), *kind);
        }
        assert_eq!(
            GITHUB.capabilities(),
            Capabilities {
                tracker: true,
                forge: true
            }
        );
        assert_eq!(
            GITLAB.capabilities(),
            Capabilities {
                tracker: false,
                forge: true
            }
        );
        assert_eq!(
            JIRA.capabilities(),
            Capabilities {
                tracker: true,
                forge: false
            }
        );
        assert_eq!(GITHUB.default_endpoint(), Some("https://api.github.com"));
        assert_eq!(GITLAB.default_endpoint(), None);
        assert_eq!(JIRA.default_endpoint(), None);
        assert_eq!(GITHUB.token_env(), "GH_TOKEN");
        assert_eq!(GITLAB.token_env(), "GITLAB_TOKEN");
        assert_eq!(JIRA.token_env(), "JIRA_API_TOKEN");
    }

    #[test]
    fn tipo_e_fonte_de_credencial_serializam_como_no_contrato() {
        assert_eq!("jira".parse::<ProviderKind>(), Ok(ProviderKind::Jira));
        assert!("bitbucket".parse::<ProviderKind>().is_err());
        assert_eq!(
            serde_json::to_value(CredentialSource::Command {
                cmd: "pass show jira | head -1".into()
            })
            .unwrap(),
            json!({"kind": "command", "cmd": "pass show jira | head -1"})
        );
        assert_eq!(
            serde_json::to_value(CredentialSource::Secret {
                name: "jira-acme".into()
            })
            .unwrap(),
            json!({"kind": "secret", "name": "jira-acme"})
        );
        assert_eq!(
            serde_json::to_value(CredentialSource::Cli).unwrap(),
            json!({"kind": "cli"})
        );
        assert_eq!(
            serde_json::to_value(CredentialSource::File {
                path: "/etc/acme/token".into()
            })
            .unwrap(),
            json!({"kind": "file", "path": "/etc/acme/token"})
        );
        assert_eq!(
            serde_json::to_value(CredentialSource::None).unwrap(),
            json!({"kind": "none"})
        );
    }

    #[test]
    fn jira_valida_tipo_url_credencial_e_opcoes() {
        assert!(JIRA.validate(&Connection::from_jira(&jira_cfg())).is_ok());
        let url_ruim = JiraConfig {
            url: "jira.example.com".into(),
            ..jira_cfg()
        };
        let err = JIRA
            .validate(&Connection::from_jira(&url_ruim))
            .unwrap_err();
        assert!(err.contains("URL do Jira inválida"), "{err}");
        let sem_cmd = JiraConfig {
            credential_cmd: "  ".into(),
            ..jira_cfg()
        };
        let err = JIRA.validate(&Connection::from_jira(&sem_cmd)).unwrap_err();
        assert!(
            err.contains("informe o comando de credencial do Jira"),
            "{err}"
        );
        let base = conn(
            ProviderKind::Jira,
            "https://jira.example.com",
            CredentialSource::None,
        );
        assert!(JIRA
            .validate(&base)
            .unwrap_err()
            .contains("precisa de comando"));
        let secret = Connection {
            credential: CredentialSource::Secret {
                name: "jira-acme".into(),
            },
            ..base.clone()
        };
        assert!(JIRA.validate(&secret).is_ok());
        let secret_vazio = Connection {
            credential: CredentialSource::Secret { name: " ".into() },
            ..base.clone()
        };
        assert!(JIRA
            .validate(&secret_vazio)
            .unwrap_err()
            .contains("nome do secret"));
        // Arquivo vale (#279); o `Cli` é só do GitHub.
        let arquivo = Connection {
            credential: CredentialSource::File {
                path: "~/.config/acme/jira.token".into(),
            },
            ..base.clone()
        };
        assert!(JIRA.validate(&arquivo).is_ok());
        let arquivo_vazio = Connection {
            credential: CredentialSource::File { path: " ".into() },
            ..base.clone()
        };
        assert!(JIRA
            .validate(&arquivo_vazio)
            .unwrap_err()
            .contains("caminho do arquivo"));
        let cli = Connection {
            credential: CredentialSource::Cli,
            ..base.clone()
        };
        assert!(JIRA
            .validate(&cli)
            .unwrap_err()
            .contains("comando de credencial, secret ou arquivo"));
        let gl = |credential| conn(ProviderKind::Gitlab, "gitlab.example.com", credential);
        assert!(GITLAB
            .validate(&gl(CredentialSource::Secret {
                name: "GITLAB_TOKEN_ACME".into()
            }))
            .is_ok());
        assert!(GITLAB
            .validate(&gl(CredentialSource::File {
                path: "/etc/acme/gitlab.token".into()
            }))
            .is_ok());
        assert!(GITLAB.validate(&gl(CredentialSource::Cli)).is_err());
        let de_outro_tipo = Connection {
            kind: ProviderKind::Github,
            ..secret.clone()
        };
        assert!(JIRA
            .validate(&de_outro_tipo)
            .unwrap_err()
            .contains("não é jira"));
        let opcoes_ruins = Connection {
            options: json!("x"),
            ..secret
        };
        assert!(JIRA
            .validate(&opcoes_ruins)
            .unwrap_err()
            .contains("opções da conexão"));
    }

    #[test]
    fn jira_sem_conexao_mantem_a_ordem_dos_erros_de_antes_da_234() {
        assert!(JIRA.validate_inline(&jira_cfg()).is_ok());
        // Chave e credencial inválidas ao mesmo tempo: vale o erro da chave.
        let chave_e_credencial = JiraConfig {
            project_key: "acme".into(),
            credential_cmd: " ".into(),
            ..jira_cfg()
        };
        let err = JIRA.validate_inline(&chave_e_credencial).unwrap_err();
        assert!(err.contains("chave do projeto Jira inválida"), "{err}");
        // URL e chave inválidas: vale o erro da URL.
        let url_e_chave = JiraConfig {
            url: "jira.example.com".into(),
            ..chave_e_credencial.clone()
        };
        let err = JIRA.validate_inline(&url_e_chave).unwrap_err();
        assert!(err.contains("URL do Jira inválida"), "{err}");
        // Credencial e campo da tela inválidos: vale o erro da credencial.
        let credencial_e_campo = JiraConfig {
            credential_cmd: " ".into(),
            extra_fields: vec![JiraExtraField {
                id: " ".into(),
                label: "Equipe".into(),
            }],
            ..jira_cfg()
        };
        let err = JIRA.validate_inline(&credencial_e_campo).unwrap_err();
        assert!(
            err.contains("informe o comando de credencial do Jira"),
            "{err}"
        );
    }

    #[test]
    fn jira_valida_chave_campos_e_filtro_do_projeto() {
        assert!(JIRA.validate_project(&jira_cfg()).is_ok());
        let chave = JiraConfig {
            project_key: "acme".into(),
            ..jira_cfg()
        };
        assert!(JIRA
            .validate_project(&chave)
            .unwrap_err()
            .contains("chave do projeto Jira inválida"));
        let sem_rotulo = JiraConfig {
            extra_fields: vec![JiraExtraField {
                id: "customfield_10099".into(),
                label: " ".into(),
            }],
            ..jira_cfg()
        };
        assert!(JIRA
            .validate_project(&sem_rotulo)
            .unwrap_err()
            .contains("informe o rótulo na tela do campo customfield_10099"));
        let repetido = JiraConfig {
            extra_fields: vec![
                JiraExtraField {
                    id: "customfield_1".into(),
                    label: "A".into(),
                },
                JiraExtraField {
                    id: "CUSTOMFIELD_1".into(),
                    label: "B".into(),
                },
            ],
            ..jira_cfg()
        };
        assert!(JIRA
            .validate_project(&repetido)
            .unwrap_err()
            .contains("aparece mais de uma vez (linha 2)"));
        // Filtro: só fora do modo sprint ativa.
        let sem_jql = JiraConfig {
            jql: " ".into(),
            ..jira_cfg()
        };
        assert!(JIRA
            .validate_sync_filter(&sem_jql)
            .unwrap_err()
            .contains("informe o JQL do sync"));
        let sem_limite = JiraConfig {
            max_issues: MAX_ISSUES_CEILING + 1,
            ..jira_cfg()
        };
        assert!(JIRA
            .validate_sync_filter(&sem_limite)
            .unwrap_err()
            .contains("limite de issues sincronizadas inválido"));
        let sprint = JiraConfig {
            active_sprint_only: true,
            jql: String::new(),
            max_issues: 0,
            ..jira_cfg()
        };
        assert!(JIRA.validate_sync_filter(&sprint).is_ok());
    }

    #[test]
    fn jira_prefixo_leva_o_comando_nunca_a_senha() {
        let prefixo = |c: &Connection| JIRA.session_prefix(c).unwrap();
        assert_eq!(
            prefixo(&Connection::from_jira(&jira_cfg())),
            r#"JIRA_API_TOKEN="$(pass show jira | head -1)" jira"#
        );
        let com_arquivo = JiraConfig {
            config_file: "acme/jira config.yml".into(),
            ..jira_cfg()
        };
        assert_eq!(
            prefixo(&Connection::from_jira(&com_arquivo)),
            r#"JIRA_CONFIG_FILE='acme/jira config.yml' JIRA_API_TOKEN="$(pass show jira | head -1)" jira"#
        );
        let sem_cmd = JiraConfig {
            credential_cmd: "   ".into(),
            ..jira_cfg()
        };
        assert_eq!(prefixo(&Connection::from_jira(&sem_cmd)), "jira");
        let secret = conn(
            ProviderKind::Jira,
            "https://jira.example.com",
            CredentialSource::Secret {
                name: "jira-acme".into(),
            },
        );
        assert_eq!(
            prefixo(&secret),
            r#"JIRA_API_TOKEN="$(ctower secret get jira-acme)" jira"#
        );
    }

    #[test]
    fn gitlab_valida_repositorio_host_e_credencial() {
        let g = GitlabConfig {
            host: "gitlab.example.com".into(),
            path: "acme/web".into(),
            credential_cmd: "pass show gitlab | head -1".into(),
            ..Default::default()
        };
        assert!(GITLAB.validate_project(&g).is_ok());
        assert!(GITLAB.validate(&Connection::from_gitlab(&g)).is_ok());
        let sem_host = GitlabConfig {
            host: String::new(),
            ..g.clone()
        };
        assert!(GITLAB
            .validate_project(&sem_host)
            .unwrap_err()
            .contains("repositório do GitLab inválido"));
        let sem_barra = GitlabConfig {
            path: "acme".into(),
            ..g.clone()
        };
        assert!(GITLAB
            .validate_project(&sem_barra)
            .unwrap_err()
            .contains("caminho \"acme\""));
        let sem_cmd = GitlabConfig {
            credential_cmd: String::new(),
            ..g.clone()
        };
        assert!(GITLAB
            .validate(&Connection::from_gitlab(&sem_cmd))
            .unwrap_err()
            .contains("informe o comando de credencial do GitLab"));
        let sem_endpoint = conn(ProviderKind::Gitlab, "", CredentialSource::Cli);
        assert!(GITLAB
            .validate(&sem_endpoint)
            .unwrap_err()
            .contains("informe o host do GitLab"));
    }

    #[test]
    fn gitlab_prefixo_leva_host_e_comando() {
        let g = GitlabConfig {
            host: "gitlab.example.com".into(),
            path: "acme/web".into(),
            credential_cmd: "pass show gitlab | head -1".into(),
            ..Default::default()
        };
        assert_eq!(
            GITLAB.session_prefix(&Connection::from_gitlab(&g)).unwrap(),
            r#"GITLAB_HOST=gitlab.example.com GITLAB_TOKEN="$(pass show gitlab | head -1)" glab"#
        );
        let sem_cmd = GitlabConfig {
            credential_cmd: String::new(),
            ..g
        };
        assert_eq!(
            GITLAB
                .session_prefix(&Connection::from_gitlab(&sem_cmd))
                .unwrap(),
            "GITLAB_HOST=gitlab.example.com glab"
        );
        let secret = conn(
            ProviderKind::Gitlab,
            "gitlab.example.com",
            CredentialSource::Secret {
                name: "GITLAB_TOKEN_ACME".into(),
            },
        );
        assert_eq!(
            GITLAB.session_prefix(&secret).unwrap(),
            r#"GITLAB_HOST=gitlab.example.com GITLAB_TOKEN="$(ctower secret get GITLAB_TOKEN_ACME)" glab"#
        );
        let arquivo = conn(
            ProviderKind::Gitlab,
            "gitlab.example.com",
            CredentialSource::File {
                path: "~/.acme/gitlab".into(),
            },
        );
        assert_eq!(
            GITLAB.session_prefix(&arquivo).unwrap(),
            format!(
                r#"GITLAB_HOST=gitlab.example.com GITLAB_TOKEN="$({})" glab"#,
                file_cmd("~/.acme/gitlab")
            )
        );
    }

    #[test]
    fn github_aceita_as_fontes_e_nao_tem_prefixo() {
        for credential in [
            CredentialSource::Cli,
            CredentialSource::None,
            CredentialSource::File {
                path: "/etc/acme/token".into(),
            },
        ] {
            let c = conn(ProviderKind::Github, "", credential);
            assert!(GITHUB.validate(&c).is_ok());
            assert_eq!(GITHUB.readiness(&c), Readiness::Ready);
            assert_eq!(GITHUB.session_prefix(&c), None);
        }
        let de_outro = conn(ProviderKind::Jira, "", CredentialSource::Cli);
        assert!(GITHUB
            .validate(&de_outro)
            .unwrap_err()
            .contains("não é github"));
    }

    #[test]
    fn papel_da_conexao_segue_o_descritor() {
        let jira_c = conn(
            ProviderKind::Jira,
            "https://jira.example.com",
            CredentialSource::None,
        );
        let gitlab_c = conn(
            ProviderKind::Gitlab,
            "gitlab.example.com",
            CredentialSource::None,
        );
        let github_c = conn(ProviderKind::Github, "", CredentialSource::Cli);
        assert!(check_role(&jira_c, Role::Tracker).is_ok());
        assert!(check_role(&jira_c, Role::Forge)
            .unwrap_err()
            .contains("não serve de forja"));
        assert!(check_role(&gitlab_c, Role::Tracker)
            .unwrap_err()
            .contains("não serve de rastreador"));
        assert!(check_role(&gitlab_c, Role::Forge).is_ok());
        assert!(check_role(&github_c, Role::Tracker).is_ok());
        assert!(check_role(&github_c, Role::Forge).is_ok());
    }

    #[test]
    fn prontidao_reflete_a_validacao() {
        let ok = Connection::from_jira(&jira_cfg());
        assert_eq!(JIRA.readiness(&ok), Readiness::Ready);
        let sem_cmd = Connection {
            credential: CredentialSource::Command { cmd: String::new() },
            ..ok
        };
        match JIRA.readiness(&sem_cmd) {
            Readiness::NeedsSetup { reason } => {
                assert!(reason.contains("comando de credencial do Jira"), "{reason}")
            }
            outro => panic!("esperado NeedsSetup, veio {outro:?}"),
        }
    }

    #[test]
    fn resolve_connections_preenche_jira_e_gitlab_em_memoria() {
        let mut jira = conn(
            ProviderKind::Jira,
            "https://jira.example.com/",
            CredentialSource::Command {
                cmd: " pass show jira | head -1 ".into(),
            },
        );
        jira.id = 7;
        jira.options = json!({ "config_file": "/etc/jira/acme.yml" });
        let mut gitlab = conn(
            ProviderKind::Gitlab,
            "gitlab.example.com",
            CredentialSource::Command {
                cmd: "pass show gitlab | head -1".into(),
            },
        );
        gitlab.id = 8;
        let conns = [jira, gitlab];
        let mut tracker = TrackerConfig::Jira(JiraConfig {
            project_key: "ACME".into(),
            connection: Some(7),
            ..Default::default()
        });
        let mut forge = ForgeConfig::Gitlab(GitlabConfig {
            path: "grupo/app".into(),
            connection: Some(8),
            ..Default::default()
        });
        let gaps = resolve_connections(&mut tracker, &mut forge, &conns, |c| {
            credential_cmd(c).unwrap_or_default()
        });
        assert!(gaps.is_empty(), "{gaps:?}");
        let j = tracker.jira().unwrap();
        assert_eq!(j.url, "https://jira.example.com");
        assert_eq!(j.config_file, "/etc/jira/acme.yml");
        assert_eq!(j.credential_cmd, "pass show jira | head -1");
        let g = forge.gitlab().unwrap();
        assert_eq!(g.host, "gitlab.example.com");
        assert_eq!(g.credential_cmd, "pass show gitlab | head -1");

        // Conexão ausente ou de outro tipo: o lado fica como está.
        let mut tracker = TrackerConfig::Jira(JiraConfig {
            connection: Some(8),
            ..Default::default()
        });
        let mut forge = ForgeConfig::Github;
        let gaps = resolve_connections(&mut tracker, &mut forge, &conns, |_| "x".into());
        assert_eq!(
            gaps,
            vec![ConnectionGap::Missing {
                kind: ProviderKind::Jira,
                id: 8
            }]
        );
        assert_eq!(tracker.jira().unwrap().url, "");
    }

    #[test]
    fn credential_cmd_de_comando_secret_e_arquivo() {
        let c = |credential| conn(ProviderKind::Jira, "https://jira.example.com", credential);
        assert_eq!(
            credential_cmd(&c(CredentialSource::Command {
                cmd: "pass show jira".into()
            })),
            Some("pass show jira".into())
        );
        assert_eq!(
            credential_cmd(&c(CredentialSource::Command { cmd: " ".into() })),
            None
        );
        assert_eq!(
            credential_cmd(&c(CredentialSource::Secret {
                name: " JIRA_TOKEN_ACME ".into()
            })),
            Some("ctower secret get JIRA_TOKEN_ACME".into())
        );
        assert_eq!(
            credential_cmd(&c(CredentialSource::Secret { name: " ".into() })),
            None
        );
        let arquivo = credential_cmd(&c(CredentialSource::File {
            path: "~/.config/acme/jira token".into(),
        }))
        .unwrap();
        assert!(
            arquivo.ends_with(r#" < "$HOME"/'.config/acme/jira token'"#),
            "{arquivo}"
        );
        assert_eq!(
            credential_cmd(&c(CredentialSource::File { path: "".into() })),
            None
        );
        assert_eq!(credential_cmd(&c(CredentialSource::Cli)), None);
        assert_eq!(credential_cmd(&c(CredentialSource::None)), None);
    }

    #[test]
    fn caminho_do_arquivo_vira_palavra_do_shell() {
        assert_eq!(file_word("~"), r#""$HOME""#);
        assert_eq!(file_word("~/"), r#""$HOME"/"#);
        assert_eq!(file_word("~/.acme/tok"), r#""$HOME"/.acme/tok"#);
        assert_eq!(file_word("~/a b/it's"), r#""$HOME"/'a b/it'\''s'"#);
        assert_eq!(file_word("~fulano/tok"), "'~fulano/tok'");
        assert_eq!(file_word("/etc/acme/tok"), "/etc/acme/tok");
        assert_eq!(file_word("a=b"), "a=b");
        assert_eq!(file_word("/x/$(rm -rf y)"), r"'/x/$(rm -rf y)'");
    }

    /// O prefixo com `File` lê o mesmo token que o resolvedor do ambiente:
    /// a primeira linha não vazia, sem espaços nas pontas, com `~`
    /// expandido, mesmo com espaço e aspas no caminho.
    #[test]
    fn prefixo_com_arquivo_le_o_mesmo_token_no_shell() {
        let home = tempfile::tempdir().unwrap();
        let dir = home.path().join("acme cfg");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("it's token"),
            "\r\n   \n\t tok-acme-arquivo \r\nsegunda\n",
        )
        .unwrap();
        let c = conn(
            ProviderKind::Jira,
            "https://jira.example.com",
            CredentialSource::File {
                path: "~/acme cfg/it's token".into(),
            },
        );
        let prefixo = JIRA.session_prefix(&c).unwrap();
        // No lugar do `jira`, um `sh` que imprime o token que recebeu.
        let atribuicao = prefixo.strip_suffix(" jira").unwrap();
        let script = format!(r#"{atribuicao} sh -c 'printf "[%s]" "$JIRA_API_TOKEN"'"#);
        let out = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(&script)
            .env("HOME", home.path())
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&out.stdout),
            "[tok-acme-arquivo]",
            "{script}\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}
