//! Conexões externas do hub (épico #224, issue #235): o cadastro dos
//! serviços (GitHub, GitLab, Jira) que os projetos referenciam pelo `id`
//! (`tracker.connection` e `forge.connection`).
//!
//! A validação é do descritor do tipo ([`provider::descriptor`]). `delete`
//! recusa conexão em uso por projeto vivo. Editar ou apagar conexão em uso
//! sobe o `config_rev`, para os ambientes receberem o cadastro novo em
//! `GET /sync/config`.

use super::{Hub, ProjectProvider};
use crate::{json_col, LedgerError, Result};
use ct_core::now_ms;
use ct_core::provider::{self, Connection, CredentialSource, ProviderKind, Readiness, Role};
use rusqlite::{params, OptionalExtension, Row};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

const CONNECTION_COLS: &str =
    "id, name, kind, endpoint, credential, options, created_at, updated_at";

/// Parâmetros de `hub.connections.add`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NewConnection {
    pub name: String,
    pub kind: ProviderKind,
    #[serde(default)]
    pub endpoint: String,
    pub credential: CredentialSource,
    #[serde(default = "empty_options")]
    pub options: Value,
}

fn empty_options() -> Value {
    json!({})
}

/// Campos de `hub.connections.update`; `None` = não muda. O tipo não muda:
/// os projetos que a usam dependem dele.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ConnectionPatch {
    pub name: Option<String>,
    pub endpoint: Option<String>,
    pub credential: Option<CredentialSource>,
    pub options: Option<Value>,
}

/// Resultado de `hub.connections.test`: a prontidão do cadastro, sem rede.
/// O "quem sou eu" na rede fica com o ambiente, que tem a credencial.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConnectionTest {
    pub id: i64,
    pub name: String,
    pub readiness: Readiness,
}

/// Nome da conexão: minúsculas, dígitos e `-`, `_` ou `.`, até 64 caracteres
/// (`jira-empresa`).
fn check_name(name: &str) -> Result<()> {
    let ok = !name.is_empty()
        && name.len() <= 64
        && name.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"-_.".contains(&b));
    if ok {
        Ok(())
    } else {
        Err(LedgerError::Invalid(format!(
            "nome da conexão inválido: {name:?} (minúsculas, dígitos, `-`, `_` ou `.`; ex.: jira-empresa)"
        )))
    }
}

/// Confere o cadastro: nome, o que o descritor do tipo exige e a forma de
/// credencial que o ambiente sabe entregar. O ambiente só materializa o
/// comando de credencial do Jira e do GitLab (`materialize_connections`); o
/// secret, o arquivo e a CLI ainda não chegam à sessão, então o hub recusa
/// (senão o cadastro salvaria uma conexão que sobe sem token).
fn validate(c: &Connection) -> Result<()> {
    check_name(&c.name)?;
    if matches!(c.kind, ProviderKind::Jira | ProviderKind::Gitlab)
        && !matches!(c.credential, CredentialSource::Command { .. })
    {
        return Err(LedgerError::Invalid(format!(
            "{}: a credencial precisa ser um comando que imprime o token; secret, arquivo e CLI \
             ainda não chegam ao ambiente",
            provider::descriptor(c.kind).label()
        )));
    }
    provider::descriptor(c.kind)
        .validate(c)
        .map_err(LedgerError::Invalid)
}

fn row_to_connection(r: &Row) -> rusqlite::Result<Connection> {
    let kind: String = r.get(2)?;
    let kind = kind.parse::<ProviderKind>().map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(2, rusqlite::types::Type::Text, Box::new(e))
    })?;
    Ok(Connection {
        id: r.get(0)?,
        name: r.get(1)?,
        kind,
        endpoint: r.get(3)?,
        credential: json_col(r, 4)?,
        options: json_col(r, 5)?,
        created_at: r.get(6)?,
        updated_at: r.get(7)?,
    })
}

impl Hub {
    /// Todas as conexões, por nome.
    pub fn list_connections(&self) -> Result<Vec<Connection>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {CONNECTION_COLS} FROM connection ORDER BY name"
        ))?;
        let rows = stmt.query_map([], row_to_connection)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Uma conexão pelo `id`.
    pub fn connection(&self, id: i64) -> Result<Connection> {
        self.conn
            .query_row(
                &format!("SELECT {CONNECTION_COLS} FROM connection WHERE id = ?1"),
                [id],
                row_to_connection,
            )
            .optional()?
            .ok_or_else(|| LedgerError::NotFound("conexão", format!("id {id}")))
    }

    /// Cadastra uma conexão (`hub.connections.add`). Nada referencia uma
    /// conexão nova, então o `config_rev` não sobe.
    pub fn add_connection(&self, new: &NewConnection) -> Result<Connection> {
        let now = now_ms();
        let c = Connection {
            id: 0,
            name: new.name.trim().to_string(),
            kind: new.kind,
            endpoint: new.endpoint.trim().to_string(),
            credential: new.credential.clone(),
            options: new.options.clone(),
            created_at: now,
            updated_at: now,
        };
        validate(&c)?;
        self.check_unique_name(&c.name, None)?;
        self.conn.execute(
            "INSERT INTO connection (name, kind, endpoint, credential, options, created_at,
                                     updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)",
            params![
                c.name,
                c.kind.as_str(),
                c.endpoint,
                serde_json::to_string(&c.credential).expect("credencial serializa"),
                serde_json::to_string(&c.options).expect("opções serializam"),
                now
            ],
        )?;
        let id = self.conn.last_insert_rowid();
        self.connection(id)
    }

    /// Edita a conexão (`hub.connections.update`). Em uso, sobe o
    /// `config_rev`: o ambiente recebe o cadastro novo no próximo pull.
    pub fn update_connection(&self, id: i64, patch: &ConnectionPatch) -> Result<Connection> {
        let cur = self.connection(id)?;
        let next = Connection {
            name: patch
                .name
                .as_deref()
                .map(|n| n.trim().to_string())
                .unwrap_or_else(|| cur.name.clone()),
            endpoint: patch
                .endpoint
                .as_deref()
                .map(|e| e.trim().to_string())
                .unwrap_or_else(|| cur.endpoint.clone()),
            credential: patch
                .credential
                .clone()
                .unwrap_or_else(|| cur.credential.clone()),
            options: patch.options.clone().unwrap_or_else(|| cur.options.clone()),
            updated_at: now_ms(),
            ..cur.clone()
        };
        validate(&next)?;
        if next.name != cur.name {
            self.check_unique_name(&next.name, Some(id))?;
        }
        let in_use = !self.connection_users(id)?.is_empty();
        let tx = self.conn.unchecked_transaction()?;
        self.conn.execute(
            "UPDATE connection SET name = ?2, endpoint = ?3, credential = ?4, options = ?5,
                    updated_at = ?6
             WHERE id = ?1",
            params![
                id,
                next.name,
                next.endpoint,
                serde_json::to_string(&next.credential).expect("credencial serializa"),
                serde_json::to_string(&next.options).expect("opções serializam"),
                next.updated_at
            ],
        )?;
        if in_use {
            self.bump_config_rev()?;
        }
        tx.commit()?;
        self.connection(id)
    }

    /// Apaga a conexão (`hub.connections.delete`). Recusa se algum projeto
    /// vivo a referencia, e diz quais.
    pub fn delete_connection(&self, id: i64) -> Result<Connection> {
        let cur = self.connection(id)?;
        let users = self.connection_users(id)?;
        if !users.is_empty() {
            return Err(LedgerError::Conflict(format!(
                "a conexão {} está em uso pelos projetos {}: troque a conexão deles antes de apagar",
                cur.name,
                users.join(", ")
            )));
        }
        self.conn
            .execute("DELETE FROM connection WHERE id = ?1", [id])?;
        Ok(cur)
    }

    /// Prontidão do cadastro da conexão, sem rede (`hub.connections.test`).
    pub fn test_connection(&self, id: i64) -> Result<ConnectionTest> {
        let c = self.connection(id)?;
        let readiness = provider::descriptor(c.kind).readiness(&c);
        Ok(ConnectionTest {
            id,
            name: c.name,
            readiness,
        })
    }

    /// Projetos vivos (`owner/repo`) que referenciam a conexão, como
    /// `tracker.connection` ou `forge.connection`.
    pub fn connection_users(&self, id: i64) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT key FROM project
             WHERE deleted_at IS NULL
               AND (json_extract(tracker_config, '$.connection') = ?1
                    OR json_extract(forge_config, '$.connection') = ?1)
             ORDER BY key",
        )?;
        let rows = stmt.query_map([id], |r| r.get::<_, String>(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Conexões que os projetos atribuídos ao ambiente referenciam: o que
    /// `GET /sync/config` leva junto dos projetos.
    pub(crate) fn env_connections(&self, env_id: &str) -> Result<Vec<Connection>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {CONNECTION_COLS} FROM connection WHERE id IN (
               SELECT json_extract(p.tracker_config, '$.connection')
               FROM assignment a JOIN project p ON p.id = a.project_id
               WHERE a.env_id = ?1 AND a.removed_at IS NULL AND p.deleted_at IS NULL
               UNION
               SELECT json_extract(p.forge_config, '$.connection')
               FROM assignment a JOIN project p ON p.id = a.project_id
               WHERE a.env_id = ?1 AND a.removed_at IS NULL AND p.deleted_at IS NULL
             ) ORDER BY name"
        ))?;
        let rows = stmt.query_map([env_id], row_to_connection)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Os projetos referenciam conexão do tipo do papel: o Jira pela
    /// `tracker.connection` e o GitLab pela `forge.connection`. Conexão
    /// inexistente, de outro tipo ou que o descritor recusa: erro.
    pub(crate) fn check_project_connections(&self, p: &ProjectProvider) -> Result<()> {
        if let Some(id) = p.tracker.jira().and_then(|j| j.connection) {
            self.check_ref(id, ProviderKind::Jira, Role::Tracker)?;
        }
        if let Some(id) = p.forge.gitlab().and_then(|g| g.connection) {
            self.check_ref(id, ProviderKind::Gitlab, Role::Forge)?;
        }
        Ok(())
    }

    fn check_ref(&self, id: i64, want: ProviderKind, role: Role) -> Result<()> {
        let c = match self.connection(id) {
            Ok(c) => c,
            Err(LedgerError::NotFound(..)) => {
                return Err(LedgerError::Conflict(format!(
                    "a conexão {id} não existe no hub"
                )))
            }
            Err(e) => return Err(e),
        };
        if c.kind != want {
            return Err(LedgerError::Conflict(format!(
                "o {} do projeto é {want}, e a conexão {} é do tipo {}",
                role.label(),
                c.name,
                c.kind
            )));
        }
        provider::check_role(&c, role).map_err(LedgerError::Conflict)?;
        provider::descriptor(c.kind)
            .validate(&c)
            .map_err(LedgerError::Conflict)
    }

    fn check_unique_name(&self, name: &str, except: Option<i64>) -> Result<()> {
        let taken: Option<i64> = self
            .conn
            .query_row("SELECT id FROM connection WHERE name = ?1", [name], |r| {
                r.get(0)
            })
            .optional()?;
        match taken {
            Some(id) if Some(id) != except => {
                Err(LedgerError::Conflict(format!("já existe a conexão {name}")))
            }
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{HubProjectPatch, NewHubProject, ProjectProvider};
    use ct_core::provider::Readiness;
    use ct_core::{ForgeConfig, GitlabConfig, JiraConfig, TrackerConfig};

    fn jira(name: &str, endpoint: &str) -> NewConnection {
        NewConnection {
            name: name.into(),
            kind: ProviderKind::Jira,
            endpoint: endpoint.into(),
            credential: CredentialSource::Command {
                cmd: "pass show jira | head -1".into(),
            },
            options: json!({ "config_file": "" }),
        }
    }

    fn gitlab(name: &str) -> NewConnection {
        NewConnection {
            name: name.into(),
            kind: ProviderKind::Gitlab,
            endpoint: "gitlab.example.com".into(),
            credential: CredentialSource::Command {
                cmd: "pass show gitlab/token | head -1".into(),
            },
            options: json!({}),
        }
    }

    fn github(name: &str) -> NewConnection {
        NewConnection {
            name: name.into(),
            kind: ProviderKind::Github,
            endpoint: String::new(),
            credential: CredentialSource::Cli,
            options: json!({}),
        }
    }

    /// Projeto Jira + GitLab que referencia as duas conexões.
    fn projeto(hub: &Hub, jira_id: i64, gitlab_id: i64) -> Result<()> {
        let provider = ProjectProvider {
            tracker: TrackerConfig::Jira(JiraConfig {
                project_key: "ACME".into(),
                jql: "labels = acme-web".into(),
                connection: Some(jira_id),
                ..Default::default()
            }),
            forge: ForgeConfig::Gitlab(GitlabConfig {
                path: "acme/web".into(),
                connection: Some(gitlab_id),
                ..Default::default()
            }),
            ..Default::default()
        };
        hub.add_project_with(
            &NewHubProject {
                owner: "acme".into(),
                repo: "acme-web".into(),
                name: "Acme Web".into(),
                base_branch: "main".into(),
                protected_branches: vec!["main".into()],
            },
            &provider,
        )
        .map(|_| ())
    }

    #[test]
    fn cadastro_cria_edita_testa_e_apaga() {
        let hub = Hub::open_in_memory().unwrap();
        let c = hub
            .add_connection(&jira("jira-acme", "https://jira.example.com"))
            .unwrap();
        assert_eq!(c.kind, ProviderKind::Jira);
        assert_eq!(hub.list_connections().unwrap(), vec![c.clone()]);

        let ed = hub
            .update_connection(
                c.id,
                &ConnectionPatch {
                    endpoint: Some("https://jira2.example.com".into()),
                    credential: Some(CredentialSource::Command {
                        cmd: "pass show jira/token | head -1".into(),
                    }),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(ed.endpoint, "https://jira2.example.com");
        assert_eq!(ed.name, "jira-acme");
        assert_eq!(
            hub.test_connection(c.id).unwrap().readiness,
            Readiness::Ready
        );

        // Edição que quebra o descritor (URL sem esquema) é recusada e não muda nada.
        let err = hub
            .update_connection(
                c.id,
                &ConnectionPatch {
                    endpoint: Some("jira.example.com".into()),
                    ..Default::default()
                },
            )
            .unwrap_err();
        assert!(matches!(err, LedgerError::Invalid(_)), "{err}");
        assert_eq!(
            hub.connection(c.id).unwrap().endpoint,
            "https://jira2.example.com"
        );

        // Credencial de comando vazia não passa na validação.
        let sem_cmd = hub
            .update_connection(
                c.id,
                &ConnectionPatch {
                    credential: Some(CredentialSource::Command { cmd: String::new() }),
                    ..Default::default()
                },
            )
            .unwrap_err();
        assert!(matches!(sem_cmd, LedgerError::Invalid(_)), "{sem_cmd}");

        hub.delete_connection(c.id).unwrap();
        assert!(hub.list_connections().unwrap().is_empty());
        assert!(matches!(
            hub.connection(c.id),
            Err(LedgerError::NotFound(..))
        ));
    }

    /// Jira e GitLab só aceitam comando de credencial: secret, arquivo e CLI
    /// são recusados no cadastro e na edição, com mensagem clara. O GitHub
    /// segue com a CLI, o arquivo e o secret.
    #[test]
    fn jira_e_gitlab_so_aceitam_comando_de_credencial() {
        let hub = Hub::open_in_memory().unwrap();
        for cred in [
            CredentialSource::Secret {
                name: "jira-acme".into(),
            },
            CredentialSource::File {
                path: "/etc/acme/token".into(),
            },
            CredentialSource::Cli,
            CredentialSource::None,
        ] {
            let mut j = jira("jira-acme", "https://jira.example.com");
            j.credential = cred.clone();
            let err = hub.add_connection(&j).unwrap_err();
            assert!(matches!(err, LedgerError::Invalid(_)), "{cred:?}: {err}");
            assert!(
                err.to_string().contains("comando que imprime o token"),
                "{err}"
            );

            let mut g = gitlab("gitlab-acme");
            g.credential = cred.clone();
            assert!(matches!(
                hub.add_connection(&g),
                Err(LedgerError::Invalid(_))
            ));
        }
        assert!(hub.list_connections().unwrap().is_empty());

        let c = hub
            .add_connection(&jira("jira-acme", "https://jira.example.com"))
            .unwrap();
        let err = hub
            .update_connection(
                c.id,
                &ConnectionPatch {
                    credential: Some(CredentialSource::Secret {
                        name: "jira-acme".into(),
                    }),
                    ..Default::default()
                },
            )
            .unwrap_err();
        assert!(matches!(err, LedgerError::Invalid(_)), "{err}");
        assert!(matches!(
            hub.connection(c.id).unwrap().credential,
            CredentialSource::Command { .. }
        ));

        let gh = hub
            .add_connection(&NewConnection {
                credential: CredentialSource::Secret {
                    name: "github-acme".into(),
                },
                ..github("github-acme")
            })
            .unwrap();
        assert_eq!(gh.kind, ProviderKind::Github);
    }

    #[test]
    fn tipo_e_opcoes_seguem_o_descritor() {
        let hub = Hub::open_in_memory().unwrap();
        let gh = hub.add_connection(&github("github-acme")).unwrap();
        assert_eq!(
            hub.test_connection(gh.id).unwrap().readiness,
            Readiness::Ready
        );
        let opcoes_ruins = NewConnection {
            options: json!("x"),
            ..jira("jira-acme", "https://jira.example.com")
        };
        assert!(matches!(
            hub.add_connection(&opcoes_ruins),
            Err(LedgerError::Invalid(_))
        ));
        let sem_host = NewConnection {
            endpoint: String::new(),
            ..gitlab("gitlab-acme")
        };
        assert!(matches!(
            hub.add_connection(&sem_host),
            Err(LedgerError::Invalid(_))
        ));
    }

    #[test]
    fn nome_invalido_e_repetido_sao_recusados() {
        let hub = Hub::open_in_memory().unwrap();
        for nome in ["", "Jira Acme", "-jira", "jira/acme"] {
            let err = hub
                .add_connection(&jira(nome, "https://jira.example.com"))
                .unwrap_err();
            assert!(matches!(err, LedgerError::Invalid(_)), "{nome}: {err}");
        }
        hub.add_connection(&jira("jira-acme", "https://jira.example.com"))
            .unwrap();
        let err = hub
            .add_connection(&jira("jira-acme", "https://jira.example.com"))
            .unwrap_err();
        assert!(matches!(err, LedgerError::Conflict(_)), "{err}");
    }

    #[test]
    fn delete_recusa_conexao_em_uso_e_edicao_sobe_a_config() {
        let hub = Hub::open_in_memory().unwrap();
        let j = hub
            .add_connection(&jira("jira-acme", "https://jira.example.com"))
            .unwrap();
        let g = hub.add_connection(&gitlab("gitlab-acme")).unwrap();
        projeto(&hub, j.id, g.id).unwrap();
        assert_eq!(hub.connection_users(j.id).unwrap(), vec!["acme/acme-web"]);

        let err = hub.delete_connection(j.id).unwrap_err();
        assert!(matches!(err, LedgerError::Conflict(_)), "{err}");
        assert!(err.to_string().contains("acme/acme-web"), "{err}");

        // Editar a conexão em uso sobe o config_rev (os ambientes releem).
        let rev = hub.config_rev().unwrap();
        hub.update_connection(
            g.id,
            &ConnectionPatch {
                endpoint: Some("gitlab2.example.com".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(hub.config_rev().unwrap() > rev);

        // Tirar a referência do projeto libera a exclusão.
        let cfg = JiraConfig {
            url: "https://jira.example.com".into(),
            project_key: "ACME".into(),
            jql: "labels = acme-web".into(),
            credential_cmd: "pass show jira | head -1".into(),
            ..Default::default()
        };
        hub.update_project(
            "acme",
            "acme-web",
            &HubProjectPatch {
                tracker: Some(TrackerConfig::Jira(cfg)),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(hub.connection_users(j.id).unwrap().is_empty());
        hub.delete_connection(j.id).unwrap();
        assert_eq!(hub.list_connections().unwrap().len(), 1);
    }

    #[test]
    fn projeto_recusa_conexao_de_tipo_incompativel() {
        let hub = Hub::open_in_memory().unwrap();
        let j = hub
            .add_connection(&jira("jira-acme", "https://jira.example.com"))
            .unwrap();
        let g = hub.add_connection(&gitlab("gitlab-acme")).unwrap();
        let gh = hub.add_connection(&github("github-acme")).unwrap();

        // Rastreador Jira com conexão GitLab: o papel não serve.
        let err = projeto(&hub, g.id, g.id).unwrap_err();
        assert!(matches!(err, LedgerError::Conflict(_)), "{err}");
        assert!(err.to_string().contains("é do tipo"), "{err}");
        // Forja GitLab com conexão Jira.
        let err = projeto(&hub, j.id, j.id).unwrap_err();
        assert!(err.to_string().contains("é do tipo"), "{err}");
        // Forja GitLab com conexão GitHub: tipo diferente do projeto.
        let err = projeto(&hub, j.id, gh.id).unwrap_err();
        assert!(err.to_string().contains("é do tipo"), "{err}");
        // Conexão que não existe.
        let err = projeto(&hub, j.id, 9999).unwrap_err();
        assert!(err.to_string().contains("não existe"), "{err}");
        assert!(hub.find_project("acme", "acme-web").unwrap().is_none());

        projeto(&hub, j.id, g.id).unwrap();
        assert!(hub.find_project("acme", "acme-web").unwrap().is_some());
    }

    #[test]
    fn sync_config_leva_as_conexoes_dos_projetos_do_ambiente() {
        let hub = Hub::open_in_memory().unwrap();
        let j = hub
            .add_connection(&jira("jira-acme", "https://jira.example.com"))
            .unwrap();
        let g = hub.add_connection(&gitlab("gitlab-acme")).unwrap();
        hub.add_connection(&jira("jira-outra", "https://jira.example.com"))
            .unwrap();
        projeto(&hub, j.id, g.id).unwrap();
        hub.ingest("env-a", "a", &[]).unwrap();

        // Sem atribuição, o ambiente não recebe conexão nenhuma.
        let cfg = hub.env_config("env-a", None, None).unwrap();
        assert_eq!(cfg.connections, Some(vec![]));

        hub.add_assignment("acme", "acme-web", "env-a", None)
            .unwrap();
        let cfg = hub.env_config("env-a", None, None).unwrap();
        let nomes: Vec<String> = cfg
            .connections
            .unwrap()
            .into_iter()
            .map(|c| c.name)
            .collect();
        assert_eq!(nomes, vec!["gitlab-acme", "jira-acme"]);
    }
}
