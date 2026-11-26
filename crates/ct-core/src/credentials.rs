//! Comandos de credencial do rastreador e da forja por ambiente (issue
//! #143): `[credentials."owner/repo"]` no `config.toml` do ambiente.
//!
//! O `credential_cmd` do Jira e do GitLab é do cadastro do projeto no hub e
//! chega igual a todos os ambientes. Caminho de binário, gerenciador de
//! senhas e sistema operacional mudam de um ambiente para outro, então um
//! ambiente pode trocar o comando só para si:
//!
//! ```toml
//! [credentials."acme/acme-web"]
//! tracker_cmd = "/usr/bin/pass show acme/jira | head -1"
//! forge_cmd = "/usr/bin/pass show acme/gitlab | head -1"
//! ```
//!
//! Campo vazio ou ausente = o comando do projeto. A sobrescrita vale só onde
//! o comando roda neste ambiente (sync, leitura de issue, glab, prompt da
//! sessão): o cadastro do hub não muda e ela nunca sai do ambiente.
//!
//! Com as conexões (#237), a forma atual é por conexão,
//! `[connections."nome"] credential_cmd` ([`ConnectionOverrides`]), que a UI
//! edita (`env.connections.set`, #276). O `[credentials]` ficou só como
//! legado: o pull converte o que dá, e o que sobra continua valendo.

use crate::provider::{self, Connection, CredentialSource, ProviderKind};
use crate::{ForgeConfig, GitlabConfig, JiraConfig, TrackerConfig};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

/// Comandos de credencial de um projeto neste ambiente.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CredentialOverride {
    /// Comando que imprime a senha do rastreador (Jira); vazio = o do projeto.
    pub tracker_cmd: String,
    /// Comando que imprime o token da forja (GitLab); vazio = o do projeto.
    pub forge_cmd: String,
}

/// `[credentials]` do `config.toml`: sobrescritas por projeto, pela chave
/// `owner/repo` (sem diferença de maiúsculas).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CredentialOverrides(pub BTreeMap<String, CredentialOverride>);

/// O que a sobrescrita trocou num projeto.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CredentialApplied {
    pub tracker: bool,
    pub forge: bool,
}

impl CredentialOverrides {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Sobrescrita do projeto `owner/repo`, se houver.
    pub fn get(&self, owner: &str, repo: &str) -> Option<&CredentialOverride> {
        let key = format!("{owner}/{repo}");
        self.0
            .iter()
            .find(|(k, _)| k.trim().eq_ignore_ascii_case(&key))
            .map(|(_, v)| v)
    }

    /// Troca, em `tracker` e `forge`, os comandos de credencial pelos deste
    /// ambiente. Só mexe no Jira e no GitLab (o GitHub usa o `gh` do
    /// ambiente) e só com o comando preenchido.
    pub fn apply(
        &self,
        owner: &str,
        repo: &str,
        tracker: &mut TrackerConfig,
        forge: &mut ForgeConfig,
    ) -> CredentialApplied {
        let mut out = CredentialApplied::default();
        let Some(o) = self.get(owner, repo) else {
            return out;
        };
        if let (TrackerConfig::Jira(j), cmd) = (tracker, o.tracker_cmd.trim()) {
            if !cmd.is_empty() {
                j.credential_cmd = cmd.to_string();
                out.tracker = true;
            }
        }
        if let (ForgeConfig::Gitlab(g), cmd) = (forge, o.forge_cmd.trim()) {
            if !cmd.is_empty() {
                g.credential_cmd = cmd.to_string();
                out.forge = true;
            }
        }
        out
    }

    /// Prompt da sessão com os comandos do projeto trocados pelos deste
    /// ambiente. Os templates levam o comando como `"$(<comando>)"`
    /// ([`crate::jira_cli_prefix`], [`crate::glab_cli_prefix`]); só essa
    /// forma é trocada, para não mexer em outro texto do prompt.
    pub fn rewrite_prompt(
        &self,
        owner: &str,
        repo: &str,
        tracker: &TrackerConfig,
        forge: &ForgeConfig,
        prompt: &str,
    ) -> String {
        let (mut t, mut f) = (tracker.clone(), forge.clone());
        let applied = self.apply(owner, repo, &mut t, &mut f);
        let mut out = prompt.to_string();
        if applied.tracker {
            if let (Some(from), Some(to)) = (tracker.jira(), t.jira()) {
                out = replace_cmd(&out, &from.credential_cmd, &to.credential_cmd);
            }
        }
        if applied.forge {
            if let (Some(from), Some(to)) = (forge.gitlab(), f.gitlab()) {
                out = replace_cmd(&out, &from.credential_cmd, &to.credential_cmd);
            }
        }
        out
    }
}

/// As sobrescritas em uso no daemon, compartilhadas entre o sync, o glab,
/// as sessões e a edição pela UI (`env.connections.set`, #276): a mudança vale no
/// próximo uso, sem restart.
///
/// Guarda também as conexões do hub deste ambiente e o `[connections]` do
/// `config.toml` (#274), que o pull atualiza: é daqui que o ambiente tira a
/// fonte de credencial efetiva de cada projeto ([`Self::jira_source`],
/// [`Self::gitlab_source`]).
#[derive(Debug, Clone, Default)]
pub struct SharedCredentials {
    legacy: Arc<RwLock<CredentialOverrides>>,
    connections: Arc<RwLock<EnvConnections>>,
}

/// Conexões do hub que o último pull com mudança trouxe e as sobrescritas
/// `[connections."nome"]` deste ambiente.
#[derive(Debug, Clone, Default, PartialEq)]
struct EnvConnections {
    hub: Vec<Connection>,
    overrides: ConnectionOverrides,
}

/// Fonte de credencial de um lado do projeto neste ambiente.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvSource {
    pub source: CredentialSource,
    /// Vem de `[credentials."owner/repo"]` deste ambiente (#143), não do
    /// cadastro nem da conexão.
    pub from_env: bool,
}

impl SharedCredentials {
    pub fn new(o: CredentialOverrides) -> Self {
        SharedCredentials {
            legacy: Arc::new(RwLock::new(o)),
            connections: Default::default(),
        }
    }

    /// Cópia das sobrescritas de agora.
    pub fn snapshot(&self) -> CredentialOverrides {
        self.legacy
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn replace(&self, o: CredentialOverrides) {
        *self.legacy.write().unwrap_or_else(|e| e.into_inner()) = o;
    }

    /// Troca as conexões do hub e as sobrescritas `[connections]` em uso
    /// (no boot e a cada pull).
    pub fn set_connections(&self, hub: Vec<Connection>, overrides: ConnectionOverrides) {
        *self.connections.write().unwrap_or_else(|e| e.into_inner()) =
            EnvConnections { hub, overrides };
    }

    /// Conexões do hub e sobrescritas `[connections]` em uso agora.
    pub fn connections(&self) -> (Vec<Connection>, ConnectionOverrides) {
        let c = self.connections.read().unwrap_or_else(|e| e.into_inner());
        (c.hub.clone(), c.overrides.clone())
    }

    /// Troca só as sobrescritas `[connections]`, mantendo as conexões do
    /// hub: a edição pela UI (`env.connections.set`, #276) vale no próximo
    /// uso, sem esperar o pull.
    pub fn set_overrides(&self, overrides: ConnectionOverrides) {
        self.connections
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .overrides = overrides;
    }

    /// Nome da conexão GitHub que vale para `connection` (a referenciada ou
    /// a `github` padrão), se ela veio no pull. Para as mensagens de erro.
    pub fn github_connection_name(&self, connection: Option<i64>) -> Option<String> {
        let conns = self.connections.read().unwrap_or_else(|e| e.into_inner());
        provider::github_connection(&conns.hub, connection).map(|c| c.name.clone())
    }

    /// [`CredentialOverrides::apply`] com as sobrescritas de agora.
    pub fn apply(
        &self,
        owner: &str,
        repo: &str,
        tracker: &mut TrackerConfig,
        forge: &mut ForgeConfig,
    ) -> CredentialApplied {
        self.legacy
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .apply(owner, repo, tracker, forge)
    }

    /// Fonte do token do Jira do projeto `owner/repo` neste ambiente.
    pub fn jira_source(&self, owner: &str, repo: &str, j: &JiraConfig) -> EnvSource {
        let legacy = self.snapshot();
        let cmd = legacy.get(owner, repo).map(|o| o.tracker_cmd.as_str());
        self.source_of(cmd, ProviderKind::Jira, j.connection, &j.credential_cmd)
    }

    /// Fonte do token do GitLab do projeto `owner/repo` neste ambiente.
    pub fn gitlab_source(&self, owner: &str, repo: &str, g: &GitlabConfig) -> EnvSource {
        let legacy = self.snapshot();
        let cmd = legacy.get(owner, repo).map(|o| o.forge_cmd.as_str());
        self.source_of(cmd, ProviderKind::Gitlab, g.connection, &g.credential_cmd)
    }

    /// Fonte do token do GitHub de um projeto neste ambiente (#275): a da
    /// conexão `connection` ou, sem ela, a da conexão `github` padrão
    /// ([`provider::github_connection`]), com a sobrescrita
    /// `[connections."nome"]`. Sem conexão no pull, o `gh auth token`
    /// (`Cli`), como antes das conexões. O `[credentials]` não tem lado
    /// GitHub.
    pub fn github_source(&self, connection: Option<i64>) -> EnvSource {
        let conns = self.connections.read().unwrap_or_else(|e| e.into_inner());
        let source = provider::github_connection(&conns.hub, connection)
            .map_or(CredentialSource::Cli, |c| conns.overrides.source(c));
        EnvSource {
            source,
            from_env: false,
        }
    }

    /// Ordem: o comando de `[credentials."owner/repo"]` (por projeto, o que
    /// sobrou da conversão, #267); a conexão referenciada, com a sobrescrita
    /// `[connections."nome"]` ([`ConnectionOverrides::source`]); e, sem
    /// conexão (cadastro antigo) ou com ela ausente do pull, o comando do
    /// cadastro do projeto.
    fn source_of(
        &self,
        legacy_cmd: Option<&str>,
        kind: ProviderKind,
        connection: Option<i64>,
        credential_cmd: &str,
    ) -> EnvSource {
        if let Some(cmd) = legacy_cmd.map(str::trim).filter(|c| !c.is_empty()) {
            return EnvSource {
                source: CredentialSource::Command { cmd: cmd.into() },
                from_env: true,
            };
        }
        let conns = self.connections.read().unwrap_or_else(|e| e.into_inner());
        let source = match connection
            .and_then(|id| conns.hub.iter().find(|c| c.id == id && c.kind == kind))
        {
            Some(c) => conns.overrides.source(c),
            None => CredentialSource::Command {
                cmd: credential_cmd.trim().to_string(),
            },
        };
        EnvSource {
            source,
            from_env: false,
        }
    }
}

impl PartialEq for SharedCredentials {
    fn eq(&self, other: &Self) -> bool {
        self.snapshot() == other.snapshot()
            && *self.connections.read().unwrap_or_else(|e| e.into_inner())
                == *other.connections.read().unwrap_or_else(|e| e.into_inner())
    }
}

fn replace_cmd(text: &str, from: &str, to: &str) -> String {
    let from = from.trim();
    if from.is_empty() {
        return text.to_string();
    }
    text.replace(&format!("$({from})"), &format!("$({})", to.trim()))
}

/// `[connections."nome"]` do `config.toml` do ambiente (#237): o comando de
/// credencial que este ambiente usa para uma conexão do hub. Vale sobre a
/// fonte da conexão, qualquer que seja (comando, secret ou arquivo); vazio =
/// a da conexão. Só comando: ele cobre os outros casos de uma máquina (o
/// arquivo em outro caminho vira `cat`, `pass`...), e um secret é do hub,
/// igual em todos os ambientes.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ConnectionOverride {
    pub credential_cmd: String,
}

/// `[connections]` do `config.toml`, pelo nome da conexão no hub.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ConnectionOverrides(pub BTreeMap<String, ConnectionOverride>);

impl ConnectionOverrides {
    /// Comando de credencial deste ambiente para a conexão, se houver.
    pub fn credential_cmd(&self, connection: &str) -> Option<&str> {
        self.0
            .get(connection)
            .map(|o| o.credential_cmd.trim())
            .filter(|c| !c.is_empty())
    }

    /// Fonte efetiva da conexão neste ambiente: o comando de
    /// `[connections."nome"]`, se houver, senão a fonte da própria conexão.
    /// Ponto único da materialização e do resolvedor do ambiente (#274).
    pub fn source(&self, c: &Connection) -> CredentialSource {
        match self.credential_cmd(&c.name) {
            Some(cmd) => CredentialSource::Command { cmd: cmd.into() },
            None => c.credential.clone(),
        }
    }
}

/// Os nomes das conexões que o projeto `owner/repo` usa, do lado do
/// rastreador e da forja. `None` = o projeto não tem conexão ali.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProjectConnections {
    pub owner: String,
    pub repo: String,
    pub tracker: Option<String>,
    pub forge: Option<String>,
}

/// Conversão do `[credentials]` para `[connections]` (#237), no primeiro
/// pull com a configuração nova. Cada comando do projeto vai para a conexão
/// dele (`tracker_cmd` para a do rastreador, `forge_cmd` para a da forja).
///
/// Devolve as conexões em uso (as já existentes e as novas), as sobrescritas
/// que ficaram em `[credentials]` e os avisos. Ficam em `[credentials]`: o
/// projeto sem conexão neste ambiente, e o comando que brigaria com outro
/// já convertido para a mesma conexão (o aviso diz qual). Esses continuam
/// valendo pelo caminho antigo, por projeto.
pub fn convert_credentials(
    creds: &CredentialOverrides,
    projects: &[ProjectConnections],
    existing: &ConnectionOverrides,
) -> (ConnectionOverrides, CredentialOverrides, Vec<String>) {
    let mut conns = existing.clone();
    let mut left = CredentialOverrides::default();
    let mut warnings = Vec::new();
    for (key, ov) in &creds.0 {
        let Some(p) = projects
            .iter()
            .find(|p| format!("{}/{}", p.owner, p.repo).eq_ignore_ascii_case(key.trim()))
        else {
            warnings.push(format!(
                "[credentials.\"{key}\"] não é de projeto atribuído a este ambiente: fica no config.toml"
            ));
            left.0.insert(key.clone(), ov.clone());
            continue;
        };
        let mut keep = CredentialOverride::default();
        for (cmd, conn, role) in [
            (ov.tracker_cmd.trim(), p.tracker.as_deref(), "rastreador"),
            (ov.forge_cmd.trim(), p.forge.as_deref(), "forja"),
        ] {
            if cmd.is_empty() {
                continue;
            }
            let Some(name) = conn else {
                warnings.push(format!(
                    "[credentials.\"{key}\"]: o projeto não tem conexão de {role}; o comando fica no config.toml"
                ));
                if role == "rastreador" {
                    keep.tracker_cmd = cmd.to_string();
                } else {
                    keep.forge_cmd = cmd.to_string();
                }
                continue;
            };
            match conns.credential_cmd(name) {
                Some(cur) if cur != cmd => {
                    warnings.push(format!(
                        "[credentials.\"{key}\"]: a conexão {name} já tem outro comando neste ambiente; o do projeto fica no config.toml"
                    ));
                    if role == "rastreador" {
                        keep.tracker_cmd = cmd.to_string();
                    } else {
                        keep.forge_cmd = cmd.to_string();
                    }
                }
                _ => {
                    conns.0.insert(
                        name.to_string(),
                        ConnectionOverride {
                            credential_cmd: cmd.to_string(),
                        },
                    );
                }
            }
        }
        if keep != CredentialOverride::default() {
            left.0.insert(key.clone(), keep);
        }
    }
    (conns, left, warnings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{GitlabConfig, JiraConfig};

    fn jira(cmd: &str) -> TrackerConfig {
        TrackerConfig::Jira(JiraConfig {
            credential_cmd: cmd.into(),
            ..JiraConfig::default()
        })
    }

    fn gitlab(cmd: &str) -> ForgeConfig {
        ForgeConfig::Gitlab(GitlabConfig {
            credential_cmd: cmd.into(),
            ..GitlabConfig::default()
        })
    }

    fn overrides(toml_text: &str) -> CredentialOverrides {
        #[derive(Deserialize)]
        struct Cfg {
            credentials: CredentialOverrides,
        }
        toml::from_str::<Cfg>(toml_text).unwrap().credentials
    }

    #[test]
    fn le_do_toml_e_troca_so_o_projeto_da_chave() {
        let o = overrides(
            r#"
            [credentials."Acme/acme-web"]
            tracker_cmd = "/usr/bin/pass show acme/jira | head -1"
            "#,
        );
        let (mut t, mut f) = (jira("pass show jira"), gitlab("pass show gitlab"));
        let a = o.apply("acme", "ACME-WEB", &mut t, &mut f);
        assert_eq!(
            a,
            CredentialApplied {
                tracker: true,
                forge: false
            }
        );
        assert_eq!(
            t.jira().unwrap().credential_cmd,
            "/usr/bin/pass show acme/jira | head -1"
        );
        assert_eq!(f.gitlab().unwrap().credential_cmd, "pass show gitlab");

        let (mut t, mut f) = (jira("pass show jira"), gitlab("x"));
        assert_eq!(
            o.apply("acme", "outro", &mut t, &mut f),
            CredentialApplied::default()
        );
        assert_eq!(t.jira().unwrap().credential_cmd, "pass show jira");
    }

    #[test]
    fn campo_vazio_e_github_ficam_como_estao() {
        let o = overrides(
            r#"
            [credentials."acme/web"]
            tracker_cmd = "  "
            forge_cmd = "printf tok"
            "#,
        );
        let (mut t, mut f) = (jira("a"), ForgeConfig::Github(Default::default()));
        assert_eq!(
            o.apply("acme", "web", &mut t, &mut f),
            CredentialApplied::default()
        );
        assert_eq!(t.jira().unwrap().credential_cmd, "a");
        let (mut t, mut f) = (TrackerConfig::Github(Default::default()), gitlab("b"));
        let a = o.apply("acme", "web", &mut t, &mut f);
        assert!(a.forge && !a.tracker);
        assert_eq!(f.gitlab().unwrap().credential_cmd, "printf tok");
    }

    #[test]
    fn campo_desconhecido_e_recusado() {
        #[derive(Deserialize, Debug)]
        #[allow(dead_code)]
        struct Cfg {
            credentials: CredentialOverrides,
        }
        let r = toml::from_str::<Cfg>("[credentials.\"a/b\"]\ncredential_cmd = \"x\"\n");
        assert!(r.is_err());
    }

    #[test]
    fn prompt_troca_so_o_comando_dentro_do_cifrao() {
        let o = overrides(
            r#"
            [credentials."acme/web"]
            tracker_cmd = "/usr/bin/pass show jira | head -1"
            forge_cmd = "/usr/bin/pass show gitlab | head -1"
            "#,
        );
        let (t, f) = (
            jira("/opt/homebrew/bin/pass show jira | head -1"),
            gitlab("/opt/homebrew/bin/pass show gitlab | head -1"),
        );
        let prompt = format!(
            "Leia com `{} issue view ACME-123`. MR com `{} mr create`. \
             (o comando /opt/homebrew/bin/pass show jira | head -1 citado fica)",
            crate::jira_cli_prefix(t.jira().unwrap()),
            crate::glab_cli_prefix(f.gitlab().unwrap()),
        );
        let out = o.rewrite_prompt("acme", "web", &t, &f, &prompt);
        assert!(
            out.contains(r#"JIRA_API_TOKEN="$(/usr/bin/pass show jira | head -1)" jira"#),
            "{out}"
        );
        assert!(
            out.contains(r#"GITLAB_TOKEN="$(/usr/bin/pass show gitlab | head -1)" glab"#),
            "{out}"
        );
        assert!(out.contains("citado"));
        assert!(out.contains("o comando /opt/homebrew/bin/pass show jira | head -1 citado"));
        // Sem sobrescrita, o prompt fica igual.
        assert_eq!(
            CredentialOverrides::default().rewrite_prompt("acme", "web", &t, &f, &prompt),
            prompt
        );
    }
}

#[cfg(test)]
mod conexoes_tests {
    use super::*;

    fn cred(tracker: &str, forge: &str) -> CredentialOverride {
        CredentialOverride {
            tracker_cmd: tracker.into(),
            forge_cmd: forge.into(),
        }
    }

    fn proj(repo: &str, tracker: Option<&str>, forge: Option<&str>) -> ProjectConnections {
        ProjectConnections {
            owner: "acme".into(),
            repo: repo.into(),
            tracker: tracker.map(String::from),
            forge: forge.map(String::from),
        }
    }

    #[test]
    fn credentials_viram_connections_pela_conexao_do_projeto() {
        let creds = CredentialOverrides(
            [
                (
                    "acme/acme-web".to_string(),
                    cred("pass show a/jira", "pass show a/gl"),
                ),
                ("acme/acme-api".to_string(), cred("pass show a/jira", "")),
            ]
            .into(),
        );
        let projetos = [
            proj("acme-web", Some("jira-acme"), Some("gitlab-acme")),
            proj("acme-api", Some("jira-acme"), None),
        ];
        let (conns, left, warnings) =
            convert_credentials(&creds, &projetos, &ConnectionOverrides::default());
        // Os dois projetos dividem a conexão do Jira com o mesmo comando.
        assert_eq!(conns.credential_cmd("jira-acme"), Some("pass show a/jira"));
        assert_eq!(conns.credential_cmd("gitlab-acme"), Some("pass show a/gl"));
        assert!(left.is_empty(), "{left:?}");
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    #[test]
    fn conflito_e_projeto_sem_conexao_ficam_com_aviso_e_no_credentials() {
        let creds = CredentialOverrides(
            [
                ("acme/acme-api".to_string(), cred("pass show outro", "")),
                ("acme/acme-mobile".to_string(), cred("pass show x", "")),
                ("acme/fora".to_string(), cred("pass show y", "")),
            ]
            .into(),
        );
        let projetos = [
            proj("acme-web", Some("jira-acme"), None),
            proj("acme-api", Some("jira-acme"), None),
            proj("acme-mobile", None, None),
        ];
        let existing = ConnectionOverrides(
            [(
                "jira-acme".to_string(),
                ConnectionOverride {
                    credential_cmd: "pass show a/jira".into(),
                },
            )]
            .into(),
        );
        let (conns, left, warnings) = convert_credentials(&creds, &projetos, &existing);
        // A conexão já tem outro comando: o do projeto fica no credentials.
        assert_eq!(conns.credential_cmd("jira-acme"), Some("pass show a/jira"));
        assert_eq!(
            left.get("acme", "acme-api").unwrap().tracker_cmd,
            "pass show outro"
        );
        // Sem conexão de rastreador aqui, e projeto não atribuído.
        assert!(left.get("acme", "acme-mobile").is_some());
        assert!(left.get("acme", "fora").is_some());
        assert_eq!(warnings.len(), 3, "{warnings:?}");
    }

    #[test]
    fn comando_vazio_nao_sobrescreve() {
        let o = ConnectionOverrides(
            [(
                "jira-acme".to_string(),
                ConnectionOverride {
                    credential_cmd: "  ".into(),
                },
            )]
            .into(),
        );
        assert_eq!(o.credential_cmd("jira-acme"), None);
    }

    fn conexao(
        id: i64,
        name: &str,
        kind: ProviderKind,
        credential: CredentialSource,
    ) -> Connection {
        Connection {
            id,
            name: name.into(),
            kind,
            endpoint: "https://jira.example.com".into(),
            credential,
            options: serde_json::json!({}),
            created_at: 0,
            updated_at: 0,
        }
    }

    /// A fonte efetiva (#274): `[credentials]` do projeto, depois a conexão
    /// com a sobrescrita `[connections]`, depois o cadastro do projeto.
    #[test]
    fn fonte_efetiva_da_conexao_no_ambiente() {
        let secret = CredentialSource::Secret {
            name: "JIRA_TOKEN_ACME".into(),
        };
        let arquivo = CredentialSource::File {
            path: "~/.acme/gitlab".into(),
        };
        let shared = SharedCredentials::default();
        shared.set_connections(
            vec![
                conexao(7, "jira-acme", ProviderKind::Jira, secret.clone()),
                conexao(8, "gitlab-acme", ProviderKind::Gitlab, arquivo.clone()),
            ],
            ConnectionOverrides::default(),
        );
        let j = JiraConfig {
            connection: Some(7),
            credential_cmd: "ctower secret get JIRA_TOKEN_ACME".into(),
            ..JiraConfig::default()
        };
        let g = GitlabConfig {
            connection: Some(8),
            ..GitlabConfig::default()
        };
        let fonte = |e: EnvSource| (e.source, e.from_env);
        assert_eq!(
            fonte(shared.jira_source("acme", "acme-web", &j)),
            (secret.clone(), false)
        );
        assert_eq!(
            fonte(shared.gitlab_source("acme", "acme-web", &g)),
            (arquivo, false)
        );
        // Conexão de outro tipo com o mesmo id não serve.
        let errado = GitlabConfig {
            connection: Some(7),
            credential_cmd: "pass show gl".into(),
            ..GitlabConfig::default()
        };
        assert_eq!(
            shared.gitlab_source("acme", "acme-web", &errado).source,
            CredentialSource::Command {
                cmd: "pass show gl".into()
            }
        );
        // A sobrescrita da conexão vale sobre o secret.
        shared.set_connections(
            vec![conexao(7, "jira-acme", ProviderKind::Jira, secret)],
            ConnectionOverrides(
                [(
                    "jira-acme".to_string(),
                    ConnectionOverride {
                        credential_cmd: " pass show acme/jira ".into(),
                    },
                )]
                .into(),
            ),
        );
        assert_eq!(
            fonte(shared.jira_source("acme", "acme-web", &j)),
            (
                CredentialSource::Command {
                    cmd: "pass show acme/jira".into()
                },
                false
            )
        );
        // E o `[credentials]` do projeto vale sobre as duas.
        shared.replace(CredentialOverrides(
            [("acme/acme-web".to_string(), cred("pass show web", ""))].into(),
        ));
        assert_eq!(
            fonte(shared.jira_source("acme", "acme-web", &j)),
            (
                CredentialSource::Command {
                    cmd: "pass show web".into()
                },
                true
            )
        );
        // Projeto sem conexão: o comando do cadastro.
        let antigo = JiraConfig {
            credential_cmd: "pass show antigo".into(),
            ..JiraConfig::default()
        };
        assert_eq!(
            shared.jira_source("acme", "outro", &antigo).source,
            CredentialSource::Command {
                cmd: "pass show antigo".into()
            }
        );
    }

    /// GitHub (#275): a conexão referenciada, senão a `github` padrão, com a
    /// sobrescrita `[connections]`; sem nenhuma, o `gh auth token`.
    #[test]
    fn fonte_do_github_no_ambiente() {
        let shared = SharedCredentials::default();
        let src = |c: Option<i64>| shared.github_source(c).source;
        assert_eq!(src(None), CredentialSource::Cli);
        let arquivo = CredentialSource::File {
            path: "~/.config/acme/gh.token".into(),
        };
        let secret = CredentialSource::Secret {
            name: "GH_TOKEN_ACME".into(),
        };
        shared.set_connections(
            vec![
                conexao(1, "github", ProviderKind::Github, arquivo.clone()),
                conexao(2, "github-acme", ProviderKind::Github, secret.clone()),
                conexao(3, "jira-acme", ProviderKind::Jira, CredentialSource::None),
            ],
            ConnectionOverrides(
                [(
                    "github-acme".to_string(),
                    ConnectionOverride {
                        credential_cmd: "pass show acme/gh".into(),
                    },
                )]
                .into(),
            ),
        );
        assert_eq!(src(None), arquivo, "a padrão");
        assert_eq!(
            src(Some(2)),
            CredentialSource::Command {
                cmd: "pass show acme/gh".into()
            }
        );
        assert_eq!(src(Some(3)), CredentialSource::Cli, "Jira não serve");
        assert_eq!(src(Some(9)), CredentialSource::Cli);
        assert!(!shared.github_source(None).from_env);
        assert_eq!(
            shared.github_connection_name(None).as_deref(),
            Some("github")
        );
        assert_eq!(
            shared.github_connection_name(Some(2)).as_deref(),
            Some("github-acme")
        );
        assert_eq!(shared.github_connection_name(Some(3)), None);

        // Só a sobrescrita muda (edição pela UI, #276); as conexões ficam.
        shared.set_overrides(ConnectionOverrides(
            [(
                "github".to_string(),
                ConnectionOverride {
                    credential_cmd: "gh auth token".into(),
                },
            )]
            .into(),
        ));
        assert_eq!(
            src(None),
            CredentialSource::Command {
                cmd: "gh auth token".into()
            }
        );
        assert_eq!(src(Some(2)), secret, "sem a sobrescrita antiga");
        assert_eq!(shared.connections().0.len(), 3);
    }
}
