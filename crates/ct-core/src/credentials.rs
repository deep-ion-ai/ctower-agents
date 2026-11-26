//! Credencial das conexões neste ambiente: as conexões do hub que o pull
//! traz e as sobrescritas do `config.toml`.
//!
//! Caminho de binário, gerenciador de senhas e sistema operacional mudam de
//! um ambiente para outro, então um ambiente pode trocar a fonte da
//! credencial de uma conexão por um comando só seu, pelo nome dela no hub
//! (#237, [`ConnectionOverrides`]), que a UI edita (`env.connections.set`,
//! #276):
//!
//! ```toml
//! [connections."jira-acme"]
//! credential_cmd = "/usr/bin/pass show acme/jira | head -1"
//! ```
//!
//! Vazio ou ausente = a fonte da conexão. A sobrescrita vale só onde o
//! token é usado neste ambiente (sync, leitura de issue, glab, `ct-gh`,
//! prompt da sessão): o cadastro do hub não muda e ela nunca sai do
//! ambiente.
//!
//! Legado (#143): `[credentials."owner/repo"]` (`tracker_cmd`,
//! `forge_cmd`, só Jira e GitLab) trocava o comando por projeto. Ele não tem
//! mais tela: o pull o converte em `[connections]` ([`convert_credentials`])
//! e o que não converte continua valendo, na frente da sobrescrita da
//! conexão.
//!
//! O ambiente lê as conexões do projeto por [`SharedCredentials::resolve`]
//! (#277) e a fonte do token por [`SharedCredentials::source`]: as conexões
//! do último pull com as duas sobrescritas.

use crate::provider::{
    self, Connection, ConnectionGap, CredentialSource, ProviderKind, ResolvedConnections,
};
use crate::{ForgeConfig, TrackerConfig};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

/// Comandos de credencial de um projeto neste ambiente.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CredentialOverride {
    /// Comando que imprime a senha do rastreador (Jira); vazio = a fonte da
    /// conexão.
    pub tracker_cmd: String,
    /// Comando que imprime o token da forja (GitLab); vazio = a fonte da
    /// conexão.
    pub forge_cmd: String,
}

/// `[credentials]` do `config.toml`: sobrescritas por projeto, pela chave
/// `owner/repo` (sem diferença de maiúsculas).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CredentialOverrides(pub BTreeMap<String, CredentialOverride>);

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
}

/// As sobrescritas em uso no daemon, compartilhadas entre o sync, o glab,
/// as sessões e a edição pela UI (`env.connections.set`, #276): a mudança vale no
/// próximo uso, sem restart.
///
/// Guarda também as conexões do hub deste ambiente e o `[connections]` do
/// `config.toml` (#274), que o pull atualiza: é daqui que o ambiente tira a
/// fonte de credencial efetiva de cada projeto ([`Self::source`]).
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

    /// Comando de `[credentials."owner/repo"]` deste ambiente para o lado
    /// do projeto (o `tracker_cmd` no Jira, o `forge_cmd` no GitLab), se
    /// houver. Legado (#143): só o Jira e o GitLab tinham `[credentials]`, e
    /// tipo novo nenhum entra aqui.
    fn legacy_cmd(&self, owner: &str, repo: &str, kind: ProviderKind) -> Option<String> {
        let legacy = self.legacy.read().unwrap_or_else(|e| e.into_inner());
        let o = legacy.get(owner, repo)?;
        let cmd = match kind {
            ProviderKind::Jira => o.tracker_cmd.trim(),
            ProviderKind::Gitlab => o.forge_cmd.trim(),
            _ => "",
        };
        (!cmd.is_empty()).then(|| cmd.to_string())
    }

    /// As conexões do projeto `owner/repo` neste ambiente (#277): as do
    /// último pull que o rastreador e a forja referenciam, cada uma com a
    /// fonte efetiva daqui (o `[credentials]` do projeto, depois a
    /// sobrescrita `[connections."nome"]`, depois a da conexão). Ponto único
    /// de leitura do endereço, do `config_file` e do prefixo da sessão no
    /// ambiente ([`provider::resolve_connections`]). Devolve também o que
    /// faltou.
    pub fn resolve(
        &self,
        owner: &str,
        repo: &str,
        tracker: &TrackerConfig,
        forge: &ForgeConfig,
    ) -> (ResolvedConnections, Vec<ConnectionGap>) {
        let conns = self.connections.read().unwrap_or_else(|e| e.into_inner());
        provider::resolve_connections(tracker, forge, &conns.hub, |c| {
            match self.legacy_cmd(owner, repo, c.kind) {
                Some(cmd) => CredentialSource::Command { cmd },
                None => conns.overrides.source(c),
            }
        })
    }

    /// Prompt da sessão com os comandos de credencial da conexão trocados
    /// pelos deste ambiente. Quem renderiza o template (a UI) pode ter usado
    /// os prefixos do hub, com a fonte da conexão
    /// ([`ResolvedConnections::jira_prefix`]); aqui o `$(<comando>)` da
    /// conexão vira o da fonte efetiva ([`Self::resolve`]). Só essa forma é
    /// trocada, para não mexer em outro texto do prompt.
    pub fn rewrite_prompt(
        &self,
        owner: &str,
        repo: &str,
        tracker: &TrackerConfig,
        forge: &ForgeConfig,
        prompt: &str,
    ) -> String {
        let (effective, _) = self.resolve(owner, repo, tracker, forge);
        let (hub, _) = {
            let conns = self.connections.read().unwrap_or_else(|e| e.into_inner());
            provider::resolve_connections(tracker, forge, &conns.hub, |c| c.credential.clone())
        };
        let mut out = prompt.to_string();
        for (from, to) in [
            (&hub.tracker, &effective.tracker),
            (&hub.forge, &effective.forge),
        ] {
            let cmd = |c: &Option<Connection>| {
                c.as_ref().and_then(|c| provider::source_cmd(&c.credential))
            };
            if let (Some(from), Some(to)) = (cmd(from), cmd(to)) {
                out = replace_cmd(&out, &from, &to);
            }
        }
        out
    }

    /// Fonte do token do tipo `kind` do projeto `owner/repo` neste ambiente
    /// (#274, #275, #278). `None` = nenhum lado do projeto é do tipo.
    ///
    /// Ordem: o comando de `[credentials."owner/repo"]` (por projeto, o que
    /// sobrou da conversão, #267); a conexão do projeto
    /// ([`provider::project_connection`]: a referenciada ou, sem referência,
    /// a padrão do tipo, como a `github`), com a sobrescrita
    /// `[connections."nome"]` ([`ConnectionOverrides::source`]). Sem conexão
    /// (cadastro antigo) ou com ela ausente do pull, a fonte de reserva do
    /// descritor ([`provider::ProviderDescriptor::fallback_credential`]):
    /// `None` no Jira e no GitLab, e o resolvedor recusa com o motivo; o
    /// `gh auth token` no GitHub, como antes das conexões.
    pub fn source(
        &self,
        owner: &str,
        repo: &str,
        tracker: &TrackerConfig,
        forge: &ForgeConfig,
        kind: ProviderKind,
    ) -> Option<EnvSource> {
        let id = provider::project_ref(tracker, forge, kind)?;
        if let Some(cmd) = self.legacy_cmd(owner, repo, kind) {
            return Some(EnvSource {
                source: CredentialSource::Command { cmd },
                from_env: true,
            });
        }
        let conns = self.connections.read().unwrap_or_else(|e| e.into_inner());
        let source = provider::connection_of(&conns.hub, kind, id).map_or_else(
            || provider::descriptor(kind).fallback_credential(),
            |c| conns.overrides.source(c),
        );
        Some(EnvSource {
            source,
            from_env: false,
        })
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
    /// Ponto único do resolvedor e do prefixo da sessão no ambiente (#274,
    /// #277, [`SharedCredentials::resolve`]).
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

    fn overrides(toml_text: &str) -> CredentialOverrides {
        #[derive(Deserialize)]
        struct Cfg {
            credentials: CredentialOverrides,
        }
        toml::from_str::<Cfg>(toml_text).unwrap().credentials
    }

    fn conexao(id: i64, name: &str, kind: ProviderKind, endpoint: &str, cmd: &str) -> Connection {
        Connection {
            id,
            name: name.into(),
            kind,
            endpoint: endpoint.into(),
            credential: CredentialSource::Command { cmd: cmd.into() },
            options: serde_json::json!({}),
            created_at: 0,
            updated_at: 0,
        }
    }

    /// Projeto Jira + GitLab com as conexões 7 e 8, e o ambiente com elas.
    fn projeto() -> (TrackerConfig, ForgeConfig, SharedCredentials) {
        let shared = SharedCredentials::default();
        shared.set_connections(
            vec![
                conexao(
                    7,
                    "jira-acme",
                    ProviderKind::Jira,
                    "https://jira.example.com",
                    "/opt/homebrew/bin/pass show jira | head -1",
                ),
                conexao(
                    8,
                    "gitlab-acme",
                    ProviderKind::Gitlab,
                    "gitlab.example.com",
                    "/opt/homebrew/bin/pass show gitlab | head -1",
                ),
            ],
            ConnectionOverrides::default(),
        );
        (
            TrackerConfig::Jira(JiraConfig {
                project_key: "ACME".into(),
                connection: Some(7),
                ..JiraConfig::default()
            }),
            ForgeConfig::Gitlab(GitlabConfig {
                path: "acme/acme-web".into(),
                connection: Some(8),
                ..GitlabConfig::default()
            }),
            shared,
        )
    }

    #[test]
    fn le_do_toml_e_vale_so_para_o_projeto_da_chave() {
        let o = overrides(
            r#"
            [credentials."Acme/acme-web"]
            tracker_cmd = "/usr/bin/pass show acme/jira | head -1"
            "#,
        );
        assert_eq!(
            o.get("acme", "ACME-WEB").unwrap().tracker_cmd,
            "/usr/bin/pass show acme/jira | head -1"
        );
        assert!(o.get("acme", "outro").is_none());
        let (t, f, shared) = projeto();
        shared.replace(o);
        let (r, gaps) = shared.resolve("acme", "acme-web", &t, &f);
        assert!(gaps.is_empty(), "{gaps:?}");
        assert_eq!(
            r.tracker.unwrap().credential,
            CredentialSource::Command {
                cmd: "/usr/bin/pass show acme/jira | head -1".into()
            }
        );
        // O GitLab segue com a fonte da conexão; outro projeto também.
        assert_eq!(
            r.forge.unwrap().credential,
            CredentialSource::Command {
                cmd: "/opt/homebrew/bin/pass show gitlab | head -1".into()
            }
        );
        let (r, _) = shared.resolve("acme", "outro", &t, &f);
        assert_eq!(
            r.tracker.unwrap().credential,
            CredentialSource::Command {
                cmd: "/opt/homebrew/bin/pass show jira | head -1".into()
            }
        );
    }

    #[test]
    fn campo_vazio_fica_com_a_fonte_da_conexao() {
        let (t, f, shared) = projeto();
        shared.replace(overrides(
            r#"
            [credentials."acme/acme-web"]
            tracker_cmd = "  "
            forge_cmd = "printf tok"
            "#,
        ));
        let j = shared
            .source("acme", "acme-web", &t, &f, ProviderKind::Jira)
            .unwrap();
        assert!(!j.from_env);
        let g = shared
            .source("acme", "acme-web", &t, &f, ProviderKind::Gitlab)
            .unwrap();
        assert!(g.from_env);
        assert_eq!(
            g.source,
            CredentialSource::Command {
                cmd: "printf tok".into()
            }
        );
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

    /// O prompt renderizado com os prefixos do hub (a fonte da conexão)
    /// ganha os comandos deste ambiente: o do `[credentials]` e o da
    /// sobrescrita `[connections]`.
    #[test]
    fn prompt_troca_so_o_comando_dentro_do_cifrao() {
        let (t, f, shared) = projeto();
        let (hub, _) = {
            let (conns, _) = shared.connections();
            provider::resolve_connections(&t, &f, &conns, |c| c.credential.clone())
        };
        let prompt = format!(
            "Leia com `{} issue view ACME-123`. MR com `{} mr create`. \
             (o comando /opt/homebrew/bin/pass show jira | head -1 citado fica)",
            hub.jira_prefix(),
            hub.glab_prefix(),
        );
        // Sem sobrescrita, o prompt fica igual.
        assert_eq!(
            shared.rewrite_prompt("acme", "acme-web", &t, &f, &prompt),
            prompt
        );
        shared.replace(overrides(
            r#"
            [credentials."acme/acme-web"]
            tracker_cmd = "/usr/bin/pass show jira | head -1"
            "#,
        ));
        let (conns, _) = shared.connections();
        shared.set_connections(
            conns,
            ConnectionOverrides(
                [(
                    "gitlab-acme".to_string(),
                    ConnectionOverride {
                        credential_cmd: "/usr/bin/pass show gitlab | head -1".into(),
                    },
                )]
                .into(),
            ),
        );
        let out = shared.rewrite_prompt("acme", "acme-web", &t, &f, &prompt);
        assert!(
            out.contains(r#"JIRA_API_TOKEN="$(/usr/bin/pass show jira | head -1)" jira"#),
            "{out}"
        );
        assert!(
            out.contains(r#"GITLAB_TOKEN="$(/usr/bin/pass show gitlab | head -1)" glab"#),
            "{out}"
        );
        assert!(out.contains("o comando /opt/homebrew/bin/pass show jira | head -1 citado"));
        // O prefixo efetivo do ambiente já sai com os comandos daqui.
        let (r, _) = shared.resolve("acme", "acme-web", &t, &f);
        assert!(out.contains(&r.jira_prefix()), "{out}");
        assert!(out.contains(&r.glab_prefix()), "{out}");
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
    /// com a sobrescrita `[connections]`; sem conexão, nenhuma (#277).
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
        let projeto = |jira: Option<i64>, gitlab: Option<i64>| {
            (
                TrackerConfig::Jira(crate::JiraConfig {
                    connection: jira,
                    ..Default::default()
                }),
                ForgeConfig::Gitlab(crate::GitlabConfig {
                    connection: gitlab,
                    ..Default::default()
                }),
            )
        };
        let (t, f) = projeto(Some(7), Some(8));
        let fonte = |repo: &str, t: &TrackerConfig, f: &ForgeConfig, kind: ProviderKind| {
            let e = shared.source("acme", repo, t, f, kind).unwrap();
            (e.source, e.from_env)
        };
        assert_eq!(
            fonte("acme-web", &t, &f, ProviderKind::Jira),
            (secret.clone(), false)
        );
        assert_eq!(
            fonte("acme-web", &t, &f, ProviderKind::Gitlab),
            (arquivo, false)
        );
        // Tipo que o projeto não usa: nenhuma fonte.
        assert_eq!(
            shared.source("acme", "acme-web", &t, &f, ProviderKind::Github),
            None
        );
        // Conexão de outro tipo com o mesmo id não serve.
        let (te, fe) = projeto(Some(7), Some(7));
        assert_eq!(
            fonte("acme-web", &te, &fe, ProviderKind::Gitlab).0,
            CredentialSource::None
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
            fonte("acme-web", &t, &f, ProviderKind::Jira),
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
            fonte("acme-web", &t, &f, ProviderKind::Jira),
            (
                CredentialSource::Command {
                    cmd: "pass show web".into()
                },
                true
            )
        );
        // Projeto sem conexão: nenhuma fonte (#277).
        let (ts, fs) = projeto(None, None);
        assert_eq!(
            fonte("outro", &ts, &fs, ProviderKind::Jira).0,
            CredentialSource::None
        );
    }

    /// GitHub (#275): a conexão referenciada, senão a `github` padrão, com a
    /// sobrescrita `[connections]`; sem nenhuma, o `gh auth token`.
    #[test]
    fn fonte_do_github_no_ambiente() {
        let shared = SharedCredentials::default();
        let gh = |c: Option<i64>| {
            (
                TrackerConfig::Github(crate::GithubConfig { connection: c }),
                ForgeConfig::Github(crate::GithubConfig { connection: c }),
            )
        };
        let fonte = |c: Option<i64>| {
            let (t, f) = gh(c);
            shared
                .source("acme", "acme-web", &t, &f, ProviderKind::Github)
                .unwrap()
        };
        let src = |c: Option<i64>| fonte(c).source;
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
        assert!(!fonte(None).from_env);
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
