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

use crate::{ForgeConfig, TrackerConfig};
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
/// as sessões e a edição pela UI (`env.credentials.set`): a mudança vale no
/// próximo uso, sem restart.
#[derive(Debug, Clone, Default)]
pub struct SharedCredentials(Arc<RwLock<CredentialOverrides>>);

impl SharedCredentials {
    pub fn new(o: CredentialOverrides) -> Self {
        SharedCredentials(Arc::new(RwLock::new(o)))
    }

    /// Cópia das sobrescritas de agora.
    pub fn snapshot(&self) -> CredentialOverrides {
        self.0.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn replace(&self, o: CredentialOverrides) {
        *self.0.write().unwrap_or_else(|e| e.into_inner()) = o;
    }

    /// [`CredentialOverrides::apply`] com as sobrescritas de agora.
    pub fn apply(
        &self,
        owner: &str,
        repo: &str,
        tracker: &mut TrackerConfig,
        forge: &mut ForgeConfig,
    ) -> CredentialApplied {
        self.0
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .apply(owner, repo, tracker, forge)
    }
}

impl PartialEq for SharedCredentials {
    fn eq(&self, other: &Self) -> bool {
        self.snapshot() == other.snapshot()
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
/// credencial que este ambiente usa para uma conexão do hub. Vale sobre o da
/// conexão; vazio = o da conexão.
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
        let (mut t, mut f) = (jira("a"), ForgeConfig::Github);
        assert_eq!(
            o.apply("acme", "web", &mut t, &mut f),
            CredentialApplied::default()
        );
        assert_eq!(t.jira().unwrap().credential_cmd, "a");
        let (mut t, mut f) = (TrackerConfig::Github, gitlab("b"));
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
}
