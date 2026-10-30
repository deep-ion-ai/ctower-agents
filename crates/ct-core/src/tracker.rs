//! Rastreador (onde estão as issues) e forja (onde está o repositório) de um
//! projeto, neutros de provedor (spec do Jira e GitLab §3, issue #347).
//!
//! No banco, cada um vira duas colunas: o tipo (`tracker_kind`,
//! `forge_kind`) e a configuração em JSON (`tracker_config`,
//! `forge_config`; `{}` no GitHub). No protocolo, o par vira um objeto
//! marcado por `kind` (`{ "kind": "github" }`, `{ "kind": "jira", "url":
//! ... }`).
//!
//! O work item guarda só o número (`gh_number`). No Jira, é o número da
//! chave (`PROJ-5450` → `5450`), e a chave em texto é sempre derivada do
//! projeto (`{project_key}-{n}`), nunca gravada. As referências saem todas
//! de [`item_ref`], [`item_short`], [`item_key`] e [`item_url`].

use crate::ParseEnumError;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

str_enum!(TrackerKind, "tracker_kind", { Github => "github", Jira => "jira" });
str_enum!(ForgeKind, "forge_kind", { Github => "github", Gitlab => "gitlab" });

// Papel de um tipo de issue do Jira no Control Tower (spec §6.3): `epic`
// vira épico, `bug` e `chore` valem como as labels do GitHub no prefixo da
// branch, e `ignore` tira a issue do espelho.
str_enum!(JiraTypeRole, "jira_type_role", {
    Epic => "epic",
    Issue => "issue",
    Bug => "bug",
    Chore => "chore",
    Ignore => "ignore",
});

// Estado de uma sprint do Jira Software: `active` (em andamento), `closed`
// (concluída) ou `future` (planejada).
str_enum!(SprintState, "sprint_state", {
    Active => "active",
    Closed => "closed",
    Future => "future",
});

/// Sprint do Jira Software em que uma issue está ou esteve. Uma issue pode
/// ter várias: as fechadas por onde passou e a atual (ativa ou futura).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Sprint {
    pub id: i64,
    pub name: String,
    pub state: SprintState,
    /// Quadro de origem (`rapidViewId`/`boardId`).
    #[serde(default)]
    pub board_id: Option<i64>,
    /// Início, fim previsto e conclusão (ms); `None` = não definido.
    #[serde(default)]
    pub start_date: Option<i64>,
    #[serde(default)]
    pub end_date: Option<i64>,
    #[serde(default)]
    pub complete_date: Option<i64>,
}

/// JQL do modo "somente a sprint ativa".
pub const ACTIVE_SPRINT_JQL: &str = "sprint in openSprints()";
/// Limite padrão de issues sincronizadas fora do modo sprint ativa.
pub const DEFAULT_MAX_ISSUES: u32 = 100;
/// Teto do limite de issues sincronizadas.
pub const MAX_ISSUES_CEILING: u32 = 5000;

fn default_max_issues() -> u32 {
    DEFAULT_MAX_ISSUES
}

/// `tracker_config` do projeto Jira.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(default)]
pub struct JiraConfig {
    /// URL do servidor, sem barra final (`https://jira.example.com`).
    pub url: String,
    /// Chave do projeto Jira (`PROJ`).
    pub project_key: String,
    /// Filtro JQL do sync (obrigatório sem `active_sprint_only`); ignorado
    /// com `active_sprint_only`.
    pub jql: String,
    /// Sincroniza só a sprint ativa (`sprint in openSprints()`), sem limite
    /// de quantidade. Ausente (cadastro anterior) = `false`: o sync usa o
    /// `jql` com o `max_issues`, e a próxima reconciliação poda o espelho
    /// para as `max_issues` mais recentes (mais as que têm work item).
    #[cfg_attr(feature = "ts", ts(as = "Option<bool>", optional))]
    pub active_sprint_only: bool,
    /// Fora do modo sprint ativa, quantas issues do `jql` ficam no espelho:
    /// as mais recentes por `updated` (mais as com work item e os pais
    /// delas). Ausente = 100. Ignorado com `active_sprint_only`.
    #[serde(default = "default_max_issues")]
    #[cfg_attr(feature = "ts", ts(as = "Option<u32>", optional))]
    pub max_issues: u32,
    /// Comando que imprime a senha (`pass show jira | head -1`). Texto do
    /// cadastro, não secret: roda no ambiente.
    pub credential_cmd: String,
    /// Configuração do jira-cli; vazio = `~/.config/.jira/.config.yml`.
    pub config_file: String,
    /// Campo do Epic Link (`customfield_10601`).
    pub epic_link_field: String,
    /// Campo do Epic Name (`customfield_10604`).
    pub epic_name_field: String,
    /// Campo da sprint (`customfield_10600`); vazio = detectar em cada
    /// issue (o `customfield_*` com o formato de sprint do Jira Software).
    /// Opcional no TypeScript: o hub anterior não manda.
    #[cfg_attr(feature = "ts", ts(as = "Option<String>", optional))]
    pub sprint_field: String,
    /// Nome do tipo de issue → papel.
    pub type_map: BTreeMap<String, JiraTypeRole>,
}

impl Default for JiraConfig {
    fn default() -> Self {
        JiraConfig {
            url: String::new(),
            project_key: String::new(),
            jql: String::new(),
            active_sprint_only: false,
            max_issues: DEFAULT_MAX_ISSUES,
            credential_cmd: String::new(),
            config_file: String::new(),
            epic_link_field: String::new(),
            epic_name_field: String::new(),
            sprint_field: String::new(),
            type_map: BTreeMap::new(),
        }
    }
}

impl JiraConfig {
    /// O JQL que o sync usa: o da sprint ativa no modo sprint ativa, senão
    /// o `jql` do cadastro (vazio = o projeto todo, só em cadastro
    /// anterior).
    pub fn effective_jql(&self) -> String {
        if self.active_sprint_only {
            ACTIVE_SPRINT_JQL.to_string()
        } else {
            self.jql.trim().to_string()
        }
    }

    /// Quantas issues o sync espelha (as mais recentes); `None` = sem
    /// limite (modo sprint ativa).
    pub fn issue_limit(&self) -> Option<usize> {
        (!self.active_sprint_only).then(|| self.max_issues.clamp(1, MAX_ISSUES_CEILING) as usize)
    }

    /// Número da chave do projeto (`PROJ-5450` → `5450`); `None` se a
    /// chave é de outro projeto ou não tem número.
    pub fn number_from_key(&self, key: &str) -> Option<i64> {
        let n = key
            .strip_prefix(self.project_key.as_str())?
            .strip_prefix('-')?;
        if n.is_empty() || !n.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        n.parse().ok().filter(|n| *n > 0)
    }

    fn key(&self, n: i64) -> String {
        format!("{}-{n}", self.project_key)
    }
}

/// `forge_config` do projeto GitLab.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(default)]
pub struct GitlabConfig {
    /// Host do GitLab (`gitlab.example.com`).
    pub host: String,
    /// Caminho do projeto (`grupo/projeto/app`).
    pub path: String,
    /// URL de clone como o usuário colou (HTTPS ou SSH).
    pub clone_url: String,
    /// Comando que imprime o token (`pass show gitlab | head -1`).
    pub credential_cmd: String,
}

/// Rastreador do projeto com a configuração dele.
// Um por projeto, lido raramente: não vale o `Box` no Jira.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TrackerConfig {
    #[default]
    Github,
    Jira(JiraConfig),
}

/// Forja do projeto com a configuração dela.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ForgeConfig {
    #[default]
    Github,
    Gitlab(GitlabConfig),
}

/// Par de colunas (tipo, JSON) que não forma uma configuração válida.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("configuração inválida de {column}: {message}")]
pub struct ConfigColumnError {
    pub column: &'static str,
    pub message: String,
}

fn config_json<T: Serialize>(cfg: Option<&T>) -> String {
    cfg.map(|c| serde_json::to_string(c).expect("configuração serializa"))
        .unwrap_or_else(|| "{}".to_string())
}

fn parse_config<T: serde::de::DeserializeOwned>(
    column: &'static str,
    json: &str,
) -> Result<T, ConfigColumnError> {
    serde_json::from_str(json).map_err(|e| ConfigColumnError {
        column,
        message: e.to_string(),
    })
}

impl TrackerConfig {
    pub fn kind(&self) -> TrackerKind {
        match self {
            TrackerConfig::Github => TrackerKind::Github,
            TrackerConfig::Jira(_) => TrackerKind::Jira,
        }
    }

    pub fn jira(&self) -> Option<&JiraConfig> {
        match self {
            TrackerConfig::Jira(c) => Some(c),
            TrackerConfig::Github => None,
        }
    }

    /// Valor da coluna `tracker_config` (`{}` no GitHub).
    pub fn config_json(&self) -> String {
        config_json(self.jira())
    }

    /// Monta a partir das colunas `tracker_kind` e `tracker_config`.
    pub fn from_columns(kind: &str, json: &str) -> Result<Self, ConfigColumnError> {
        let kind = kind.parse::<TrackerKind>().map_err(|e| ConfigColumnError {
            column: "tracker_kind",
            message: e.to_string(),
        })?;
        Ok(match kind {
            TrackerKind::Github => TrackerConfig::Github,
            TrackerKind::Jira => TrackerConfig::Jira(parse_config("tracker_config", json)?),
        })
    }
}

impl ForgeConfig {
    pub fn kind(&self) -> ForgeKind {
        match self {
            ForgeConfig::Github => ForgeKind::Github,
            ForgeConfig::Gitlab(_) => ForgeKind::Gitlab,
        }
    }

    pub fn gitlab(&self) -> Option<&GitlabConfig> {
        match self {
            ForgeConfig::Gitlab(c) => Some(c),
            ForgeConfig::Github => None,
        }
    }

    /// Valor da coluna `forge_config` (`{}` no GitHub).
    pub fn config_json(&self) -> String {
        config_json(self.gitlab())
    }

    /// Monta a partir das colunas `forge_kind` e `forge_config`.
    pub fn from_columns(kind: &str, json: &str) -> Result<Self, ConfigColumnError> {
        let kind = kind.parse::<ForgeKind>().map_err(|e| ConfigColumnError {
            column: "forge_kind",
            message: e.to_string(),
        })?;
        Ok(match kind {
            ForgeKind::Github => ForgeConfig::Github,
            ForgeKind::Gitlab => ForgeConfig::Gitlab(parse_config("forge_config", json)?),
        })
    }
}

/// Mapa de tipos padrão do cadastro Jira (spec §6.3). Tipos fora dele caem
/// no reconhecimento pelo Epic Name.
pub fn default_type_map() -> BTreeMap<String, JiraTypeRole> {
    [
        ("Epic", JiraTypeRole::Epic),
        ("Épico", JiraTypeRole::Epic),
        ("Bug", JiraTypeRole::Bug),
        ("Erro", JiraTypeRole::Bug),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect()
}

/// Par rastreador + forja que o cadastro aceita (spec §2): GitHub + GitHub
/// e Jira + GitLab. O modelo comporta os outros, recusados até haver um
/// projeto real.
pub fn supported_combination(tracker: TrackerKind, forge: ForgeKind) -> bool {
    matches!(
        (tracker, forge),
        (TrackerKind::Github, ForgeKind::Github) | (TrackerKind::Jira, ForgeKind::Gitlab)
    )
}

/// Prontidão de um ambiente para um projeto Jira + GitLab (spec §8.4):
/// calculada pelo ambiente no boot e a cada ciclo de sync, sem rodar nada na
/// hora da leitura. Vai no `env.info` (`trackers`) e, do ambiente de sync,
/// no `tracker_status` do outbox.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(default)]
pub struct TrackerReadiness {
    pub owner: String,
    pub repo: String,
    /// Quando o ambiente conferiu (ms).
    pub checked_at: i64,
    pub jira: JiraReadiness,
    pub gitlab: GitlabReadiness,
    /// Leitura do Jira em andamento no ciclo (o primeiro lê issue por
    /// issue e pode levar minutos); `None` = nenhum ciclo lendo agora.
    /// Ausente = ambiente anterior.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub progress: Option<TrackerProgress>,
}

/// Quantas issues do ciclo em andamento já foram lidas (`issue view`). Com
/// `total` 0, o ciclo ainda está listando as chaves (`issue list`) e `done`
/// é quantas já foram listadas.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(default)]
pub struct TrackerProgress {
    /// Lidas até agora (com ou sem sucesso); na listagem, as chaves listadas.
    pub done: i64,
    /// A ler no ciclo; 0 = ainda listando.
    pub total: i64,
    /// Início do ciclo (ms).
    pub started_at: i64,
}

/// O que o ambiente conferiu do jira-cli.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(default)]
pub struct JiraReadiness {
    /// Versão do jira-cli (`jira version`); `None` = não encontrado.
    pub cli: Option<String>,
    /// `.config.yml` do jira-cli presente (`jira init`).
    pub config_present: bool,
    /// O `server` do `.config.yml` é a `url` do cadastro.
    pub server_matches: bool,
    /// O comando de credencial respondeu com uma linha não vazia.
    pub credential_ok: bool,
    /// `jira me` respondeu com a senha.
    pub me_ok: bool,
    /// Versão do Jira (`jira serverinfo`).
    pub server_version: Option<String>,
    /// Motivo da primeira falha, sem a senha.
    pub error: Option<String>,
}

/// O que o ambiente conferiu do glab.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(default)]
pub struct GitlabReadiness {
    /// Versão do glab; `None` = não encontrado.
    pub cli: Option<String>,
    pub credential_ok: bool,
    /// `glab api version` respondeu com o token.
    pub api_ok: bool,
    pub error: Option<String>,
}

static GITHUB: TrackerConfig = TrackerConfig::Github;

/// O par (`owner/repo`, rastreador) de que as referências de work item
/// precisam. Todo `Project`/`HubProject` sabe montar o dele.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrackerRef<'a> {
    /// `owner` da forja (no GitLab, o caminho sem o último segmento).
    pub owner: &'a str,
    pub repo: &'a str,
    pub tracker: &'a TrackerConfig,
}

impl<'a> TrackerRef<'a> {
    pub fn new(owner: &'a str, repo: &'a str, tracker: &'a TrackerConfig) -> Self {
        TrackerRef {
            owner,
            repo,
            tracker,
        }
    }

    /// Projeto GitHub (issues no próprio repositório).
    pub fn github(owner: &'a str, repo: &'a str) -> Self {
        TrackerRef::new(owner, repo, &GITHUB)
    }
}

/// Referência completa: `owner/repo#12` no GitHub, `PROJ-5450` no Jira.
pub fn item_ref(t: &TrackerRef, n: i64) -> String {
    match t.tracker {
        TrackerConfig::Github => format!("{}/{}#{n}", t.owner, t.repo),
        TrackerConfig::Jira(c) => c.key(n),
    }
}

/// Referência curta, dentro do projeto: `#12` no GitHub, `PROJ-5450` no
/// Jira.
pub fn item_short(t: &TrackerRef, n: i64) -> String {
    match t.tracker {
        TrackerConfig::Github => format!("#{n}"),
        TrackerConfig::Jira(c) => c.key(n),
    }
}

/// O `{key}` do padrão de branch: `12` no GitHub, `PROJ-5450` no Jira.
pub fn item_key(t: &TrackerRef, n: i64) -> String {
    match t.tracker {
        TrackerConfig::Github => n.to_string(),
        TrackerConfig::Jira(c) => c.key(n),
    }
}

/// Link da issue: `https://github.com/owner/repo/issues/12` ou
/// `<jira>/browse/PROJ-5450`.
pub fn item_url(t: &TrackerRef, n: i64) -> String {
    match t.tracker {
        TrackerConfig::Github => format!("https://github.com/{}/{}/issues/{n}", t.owner, t.repo),
        TrackerConfig::Jira(c) => {
            format!("{}/browse/{}", c.url.trim_end_matches('/'), c.key(n))
        }
    }
}

impl ForgeKind {
    /// Nome da forja nas mensagens: `GitHub` ou `GitLab`.
    pub fn label(self) -> &'static str {
        match self {
            ForgeKind::Github => "GitHub",
            ForgeKind::Gitlab => "GitLab",
        }
    }
}

/// Nome da mudança na forja: `PR` no GitHub, `MR` no GitLab.
pub fn change_label(forge: ForgeKind) -> &'static str {
    match forge {
        ForgeKind::Github => "PR",
        ForgeKind::Gitlab => "MR",
    }
}

/// Referência curta da mudança: `#30` no GitHub, `!30` (o `iid`) no GitLab.
pub fn change_short(forge: ForgeKind, n: i64) -> String {
    match forge {
        ForgeKind::Github => format!("#{n}"),
        ForgeKind::Gitlab => format!("!{n}"),
    }
}

/// `PR #30` ou `MR !30`, para as mensagens.
pub fn change_ref(forge: ForgeKind, n: i64) -> String {
    format!("{} {}", change_label(forge), change_short(forge, n))
}

/// Link da mudança quando a forja não deu um (o sync guarda o `html_url`
/// do PR ou o `web_url` do MR): `https://github.com/<owner>/<repo>/pull/<n>`
/// ou `https://<host>/<path>/-/merge_requests/<iid>`.
pub fn change_url(owner: &str, repo: &str, forge: &ForgeConfig, n: i64) -> String {
    match forge {
        ForgeConfig::Github => format!("https://github.com/{owner}/{repo}/pull/{n}"),
        ForgeConfig::Gitlab(g) => {
            let path = match g.path.trim_matches('/') {
                "" => format!("{owner}/{repo}"),
                p => p.to_string(),
            };
            format!("https://{}/{path}/-/merge_requests/{n}", g.host)
        }
    }
}

/// Palavra do shell com aspas simples só quando precisa (caminho com espaço
/// ou caractere especial).
fn shell_word(s: &str) -> String {
    let safe = !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"/._-~+=:,@%".contains(&b));
    if safe {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', r"'\''"))
    }
}

/// Prefixo do jira-cli nas sessões (`{{jira}}` dos templates, spec §8.2 e
/// §9.2): `JIRA_API_TOKEN="$(<credential_cmd>)" jira`, com
/// `JIRA_CONFIG_FILE=<arquivo>` antes quando configurado. Leva o comando de
/// credencial do cadastro, nunca a senha: o `$(...)` é expandido pelo shell
/// do agente, só no processo do `jira`.
pub fn jira_cli_prefix(j: &JiraConfig) -> String {
    let mut parts = Vec::new();
    if !j.config_file.trim().is_empty() {
        parts.push(format!(
            "JIRA_CONFIG_FILE={}",
            shell_word(j.config_file.trim())
        ));
    }
    if !j.credential_cmd.trim().is_empty() {
        parts.push(format!("JIRA_API_TOKEN=\"$({})\"", j.credential_cmd.trim()));
    }
    parts.push("jira".into());
    parts.join(" ")
}

/// Prefixo do glab nas sessões (`{{glab}}`):
/// `GITLAB_HOST=<host> GITLAB_TOKEN="$(<credential_cmd>)" glab`, no mesmo
/// modelo do [`jira_cli_prefix`]. O `GITLAB_HOST` também vem no ambiente da
/// sessão; no prefixo, o comando funciona fora dela.
pub fn glab_cli_prefix(g: &GitlabConfig) -> String {
    let mut parts = Vec::new();
    if !g.host.trim().is_empty() {
        parts.push(format!("GITLAB_HOST={}", shell_word(g.host.trim())));
    }
    if !g.credential_cmd.trim().is_empty() {
        parts.push(format!("GITLAB_TOKEN=\"$({})\"", g.credential_cmd.trim()));
    }
    parts.push("glab".into());
    parts.join(" ")
}

/// Como ler uma issue do projeto, para o gestor (spec §13): `gh issue view
/// <n> -R owner/repo` no GitHub; no Jira, o [`jira_cli_prefix`] com
/// `issue view <KEY>-<n> --plain`. `<n>` fica para o agente trocar.
pub fn issue_view_cmd(owner: &str, repo: &str, tracker: &TrackerConfig) -> String {
    match tracker {
        TrackerConfig::Github => format!("gh issue view <n> -R {owner}/{repo}"),
        TrackerConfig::Jira(j) => format!(
            "{} issue view {}-<n> --plain",
            jira_cli_prefix(j),
            j.project_key
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn jira() -> TrackerConfig {
        TrackerConfig::Jira(JiraConfig {
            url: "https://jira.example.com/".into(),
            project_key: "PROJ".into(),
            epic_link_field: "customfield_10601".into(),
            type_map: [("Épico".to_string(), JiraTypeRole::Epic)].into(),
            ..Default::default()
        })
    }

    #[test]
    fn mudanca_na_forja_pr_ou_mr() {
        let gl = ForgeConfig::Gitlab(GitlabConfig {
            host: "gitlab.example.com".into(),
            path: "grupo/projeto/app".into(),
            ..Default::default()
        });
        assert_eq!(change_ref(ForgeKind::Github, 30), "PR #30");
        assert_eq!(change_ref(ForgeKind::Gitlab, 30), "MR !30");
        assert_eq!(ForgeKind::Gitlab.label(), "GitLab");
        assert_eq!(
            change_url("o", "r", &ForgeConfig::Github, 30),
            "https://github.com/o/r/pull/30"
        );
        assert_eq!(
            change_url("grupo/projeto", "app", &gl, 7),
            "https://gitlab.example.com/grupo/projeto/app/-/merge_requests/7"
        );
    }

    #[test]
    fn referencias_do_github_sao_as_de_hoje() {
        let t = TrackerRef::github("deep-ion-ai", "ctower-agents");
        assert_eq!(item_ref(&t, 12), "deep-ion-ai/ctower-agents#12");
        assert_eq!(item_short(&t, 12), "#12");
        assert_eq!(item_key(&t, 12), "12");
        assert_eq!(
            item_url(&t, 12),
            "https://github.com/deep-ion-ai/ctower-agents/issues/12"
        );
    }

    #[test]
    fn referencias_do_jira_usam_a_chave_derivada() {
        let cfg = jira();
        let t = TrackerRef::new("grupo/projeto", "app", &cfg);
        assert_eq!(item_ref(&t, 5450), "PROJ-5450");
        assert_eq!(item_short(&t, 5450), "PROJ-5450");
        assert_eq!(item_key(&t, 5450), "PROJ-5450");
        assert_eq!(
            item_url(&t, 5450),
            "https://jira.example.com/browse/PROJ-5450"
        );
    }

    #[test]
    fn numero_a_partir_da_chave() {
        let cfg = jira();
        let c = cfg.jira().unwrap();
        assert_eq!(c.number_from_key("PROJ-5450"), Some(5450));
        for k in [
            "OUTRO-5450",
            "PROJ5450",
            "PROJ-",
            "PROJ-12a",
            "PROJ-0",
            "X-1",
        ] {
            assert_eq!(c.number_from_key(k), None, "{k}");
        }
    }

    #[test]
    fn protocolo_marca_o_tipo_e_aceita_ausencia_como_github() {
        assert_eq!(
            serde_json::to_value(TrackerConfig::Github).unwrap(),
            json!({ "kind": "github" })
        );
        assert_eq!(
            serde_json::to_value(ForgeConfig::Github).unwrap(),
            json!({ "kind": "github" })
        );
        let v = serde_json::to_value(jira()).unwrap();
        assert_eq!(v["kind"], "jira");
        assert_eq!(v["project_key"], "PROJ");
        assert_eq!(v["type_map"], json!({ "Épico": "epic" }));
        let volta: TrackerConfig = serde_json::from_value(v).unwrap();
        assert_eq!(volta, jira());
        // Campos ausentes ficam vazios.
        let f: ForgeConfig =
            serde_json::from_value(json!({ "kind": "gitlab", "host": "gitlab.example.com" }))
                .unwrap();
        assert_eq!(f.gitlab().unwrap().host, "gitlab.example.com");
        assert_eq!(f.gitlab().unwrap().path, "");
    }

    #[test]
    fn so_github_github_e_jira_gitlab_sao_suportados() {
        use ForgeKind as F;
        use TrackerKind as T;
        assert!(supported_combination(T::Github, F::Github));
        assert!(supported_combination(T::Jira, F::Gitlab));
        assert!(!supported_combination(T::Github, F::Gitlab));
        assert!(!supported_combination(T::Jira, F::Github));
        let m = default_type_map();
        assert_eq!(m.get("Épico"), Some(&JiraTypeRole::Epic));
        assert_eq!(m.get("Erro"), Some(&JiraTypeRole::Bug));
    }

    #[test]
    fn prontidao_com_campos_ausentes_fica_vazia() {
        let r: TrackerReadiness = serde_json::from_value(json!({
            "owner": "grupo/projeto", "repo": "app", "checked_at": 1,
            "jira": { "cli": "1.7.0", "me_ok": true }
        }))
        .unwrap();
        assert_eq!(r.jira.cli.as_deref(), Some("1.7.0"));
        assert!(r.jira.me_ok && !r.jira.credential_ok);
        assert_eq!(r.gitlab, GitlabReadiness::default());
    }

    #[test]
    fn colunas_ida_e_volta() {
        assert_eq!(TrackerConfig::Github.config_json(), "{}");
        assert_eq!(
            TrackerConfig::from_columns("github", "{}").unwrap(),
            TrackerConfig::Github
        );
        let cfg = jira();
        let (k, j) = (cfg.kind().as_str(), cfg.config_json());
        assert_eq!(k, "jira");
        assert!(!j.contains("\"kind\""), "{j}");
        assert_eq!(TrackerConfig::from_columns(k, &j).unwrap(), cfg);

        let forge = ForgeConfig::Gitlab(GitlabConfig {
            host: "gitlab.example.com".into(),
            path: "grupo/projeto/app".into(),
            ..Default::default()
        });
        assert_eq!(
            ForgeConfig::from_columns(forge.kind().as_str(), &forge.config_json()).unwrap(),
            forge
        );
        assert_eq!(
            ForgeConfig::from_columns("github", "lixo").unwrap(),
            ForgeConfig::Github
        );

        let err = TrackerConfig::from_columns("linear", "{}").unwrap_err();
        assert_eq!(err.column, "tracker_kind");
        let err = TrackerConfig::from_columns("jira", "[").unwrap_err();
        assert_eq!(err.column, "tracker_config");
    }

    #[test]
    fn modo_sprint_ativa_e_limite() {
        // Cadastro anterior, sem os campos: JQL do cadastro com limite 100.
        let j: JiraConfig =
            serde_json::from_value(json!({ "project_key": "P", "jql": " labels = web " })).unwrap();
        assert!(!j.active_sprint_only);
        assert_eq!(j.max_issues, 100);
        assert_eq!(j.effective_jql(), "labels = web");
        assert_eq!(j.issue_limit(), Some(100));
        let s = JiraConfig {
            active_sprint_only: true,
            max_issues: 7,
            ..j.clone()
        };
        assert_eq!(s.effective_jql(), "sprint in openSprints()");
        assert_eq!(s.issue_limit(), None);
        let grande = JiraConfig {
            max_issues: 1_000_000,
            ..j
        };
        assert_eq!(grande.issue_limit(), Some(MAX_ISSUES_CEILING as usize));
    }

    #[test]
    fn prefixos_do_jira_e_do_glab_levam_o_comando_nunca_a_senha() {
        let j = JiraConfig {
            project_key: "PROJ".into(),
            credential_cmd: "pass show jira | head -1".into(),
            ..Default::default()
        };
        assert_eq!(
            jira_cli_prefix(&j),
            r#"JIRA_API_TOKEN="$(pass show jira | head -1)" jira"#
        );
        let com_config = JiraConfig {
            config_file: "/Users/a/.config/.jira/jira.yml".into(),
            ..j.clone()
        };
        assert_eq!(
            jira_cli_prefix(&com_config),
            r#"JIRA_CONFIG_FILE=/Users/a/.config/.jira/jira.yml JIRA_API_TOKEN="$(pass show jira | head -1)" jira"#
        );
        let espaco = JiraConfig {
            config_file: "/tmp/meu jira.yml".into(),
            credential_cmd: String::new(),
            ..j.clone()
        };
        assert_eq!(
            jira_cli_prefix(&espaco),
            "JIRA_CONFIG_FILE='/tmp/meu jira.yml' jira"
        );
        let g = GitlabConfig {
            host: "gitlab.example.com".into(),
            path: "grupo/projeto/app".into(),
            credential_cmd: "pass show gitlab | head -1".into(),
            ..Default::default()
        };
        assert_eq!(
            glab_cli_prefix(&g),
            r#"GITLAB_HOST=gitlab.example.com GITLAB_TOKEN="$(pass show gitlab | head -1)" glab"#
        );
        assert_eq!(
            issue_view_cmd("grupo/projeto", "app", &TrackerConfig::Jira(j)),
            r#"JIRA_API_TOKEN="$(pass show jira | head -1)" jira issue view PROJ-<n> --plain"#
        );
        assert_eq!(
            issue_view_cmd("o", "r", &TrackerConfig::Github),
            "gh issue view <n> -R o/r"
        );
    }
}
