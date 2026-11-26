//! Migração dos cadastros para conexões (épico #224, issue #237).
//!
//! Até aqui o Jira trazia `url`, `credential_cmd` e `config_file` no cadastro
//! do projeto, e o GitLab trazia `host` e `credential_cmd`. A migração cria
//! as conexões equivalentes e aponta os projetos para elas:
//!
//! - projetos Jira com o mesmo (`url`, `credential_cmd`, `config_file`) e
//!   projetos GitLab com o mesmo (`host`, `credential_cmd`) dividem uma
//!   conexão; o nome sai do tipo e do host (`jira-jira.example.com`), com
//!   `-2`, `-3`... quando o nome já é de outra conexão;
//! - os campos antigos saem do JSON do projeto, e o `connection` entra no
//!   lugar. Projeto sem URL/host ou sem comando de credencial não migra: vira
//!   aviso e fica como está;
//! - a conexão padrão do GitHub (`github`) sai de `gh_api_url` e do
//!   `github.token`, por [`Hub::ensure_default_github_connection`].
//!
//! Idempotente: sem projeto com cadastro antigo convertível, nada muda, nada
//! é copiado e nada sobe no `config_rev`. Antes de gravar, o `hub.db` é
//! copiado com `VACUUM INTO` para `<dir>/hub-<AAAAMMDD-HHMM>-pre-connections.db`.

use crate::connection::NewConnection;
use crate::{Hub, LedgerError, Result};
use ct_core::provider::{Connection, CredentialSource, ProviderKind};
use rusqlite::{params, OptionalExtension};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// Teto do nome derivado da conexão (o mesmo de `hub.connections.add`).
const MAX_NAME: usize = 64;

/// O que a migração fez, para o log do `cthubd` e para os testes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LegacyMigration {
    /// Cópia do `hub.db` feita antes de gravar (`None` = nada a migrar).
    pub backup: Option<PathBuf>,
    /// Nomes das conexões criadas.
    pub created: Vec<String>,
    /// Projetos (`owner/repo`) que passaram a referenciar uma conexão.
    pub projects: Vec<String>,
    /// Cadastros antigos que não migraram, com o motivo. Não se citam valores
    /// de credencial.
    pub warnings: Vec<String>,
    /// `config_rev` depois da migração (`None` = nada mudou).
    pub config_rev: Option<i64>,
}

/// Grupo de cadastros antigos que viram uma conexão.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Group {
    kind: ProviderKind,
    endpoint: String,
    cmd: String,
    options: Value,
}

impl Group {
    fn credential(&self) -> CredentialSource {
        CredentialSource::Command {
            cmd: self.cmd.clone(),
        }
    }
}

/// Cadastro antigo de um lado do projeto (JSON) com o grupo que vira conexão.
type Legacy = (Group, Value);

/// Projeto com pelo menos um cadastro antigo a migrar: rastreador e forja.
type Planned = (Row, Option<Legacy>, Option<Legacy>);

/// Linha de `project` com as duas configurações em JSON.
struct Row {
    id: i64,
    key: String,
    tracker_kind: String,
    tracker: String,
    forge_kind: String,
    forge: String,
}

fn str_field(v: &Value, k: &str) -> String {
    v.get(k)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string()
}

/// Ainda sem conexão (`connection` ausente ou nulo).
fn unlinked(v: &Value) -> bool {
    matches!(v.get("connection"), None | Some(Value::Null))
}

/// Grupo do Jira, se o cadastro antigo tem URL. Sem comando de credencial,
/// o erro diz o que falta.
fn jira_group(v: &Value) -> std::result::Result<Group, String> {
    let url = str_field(v, "url");
    if url.is_empty() {
        return Err("cadastro do Jira sem URL".into());
    }
    let cmd = str_field(v, "credential_cmd");
    if cmd.is_empty() {
        return Err("cadastro do Jira sem comando de credencial".into());
    }
    Ok(Group {
        kind: ProviderKind::Jira,
        endpoint: url.trim_end_matches('/').to_string(),
        cmd,
        options: json!({ "config_file": str_field(v, "config_file") }),
    })
}

/// Grupo do GitLab, se o cadastro antigo tem host.
fn gitlab_group(v: &Value) -> std::result::Result<Group, String> {
    let host = str_field(v, "host");
    if host.is_empty() {
        return Err("cadastro do GitLab sem host".into());
    }
    let cmd = str_field(v, "credential_cmd");
    if cmd.is_empty() {
        return Err("cadastro do GitLab sem comando de credencial".into());
    }
    Ok(Group {
        kind: ProviderKind::Gitlab,
        endpoint: host,
        cmd,
        options: json!({}),
    })
}

/// Nome base da conexão: tipo e host, em minúsculas (`jira-jira.example.com`).
fn base_name(kind: ProviderKind, endpoint: &str) -> String {
    let no_scheme = endpoint.rsplit("://").next().unwrap_or(endpoint);
    let host = no_scheme.split('/').next().unwrap_or_default();
    let clean: String = host
        .to_ascii_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let name = format!("{kind}-{clean}");
    let cut: String = name.chars().take(MAX_NAME - 4).collect();
    cut.trim_end_matches(['-', '.']).to_string()
}

/// Nome do snapshot `hub-<stamp>-pre-connections[-n].db`, sem sobrescrever.
fn snapshot_path(dir: &Path, now_ms: i64) -> PathBuf {
    let stamp = ct_core::utc_stamp(now_ms);
    std::iter::once(format!("hub-{stamp}-pre-connections.db"))
        .chain((2u32..).map(|n| format!("hub-{stamp}-pre-connections-{n}.db")))
        .map(|n| dir.join(n))
        .find(|p| !p.exists())
        .expect("algum sufixo livre")
}

impl Hub {
    /// Migra os cadastros antigos para conexões (ver o módulo). `backup_dir`
    /// recebe a cópia do `hub.db` quando há o que migrar.
    pub fn migrate_legacy_connections(
        &self,
        backup_dir: &Path,
        now: i64,
    ) -> Result<LegacyMigration> {
        let mut out = LegacyMigration::default();
        let rows = self.legacy_rows()?;

        // Primeira passada, sem gravar: o que converte e o que fica.
        let mut plan: Vec<Planned> = Vec::new();
        for r in rows {
            let t = match (
                r.tracker_kind.as_str(),
                serde_json::from_str::<Value>(&r.tracker),
            ) {
                ("jira", Ok(v)) if unlinked(&v) && has_legacy_jira(&v) => match jira_group(&v) {
                    Ok(g) => Some((g, v)),
                    Err(e) => {
                        out.warnings.push(format!("{}: {e}", r.key));
                        None
                    }
                },
                _ => None,
            };
            let f = match (
                r.forge_kind.as_str(),
                serde_json::from_str::<Value>(&r.forge),
            ) {
                ("gitlab", Ok(v)) if unlinked(&v) && has_legacy_gitlab(&v) => {
                    match gitlab_group(&v) {
                        Ok(g) => Some((g, v)),
                        Err(e) => {
                            out.warnings.push(format!("{}: {e}", r.key));
                            None
                        }
                    }
                }
                _ => None,
            };
            if t.is_some() || f.is_some() {
                plan.push((r, t, f));
            }
        }
        if plan.is_empty() {
            return Ok(out);
        }

        // Cópia antes de qualquer escrita. Se a cópia falha, nada muda.
        std::fs::create_dir_all(backup_dir).map_err(|e| LedgerError::Conflict(e.to_string()))?;
        let dest = snapshot_path(backup_dir, now);
        self.conn
            .execute("VACUUM INTO ?1", [dest.to_string_lossy().as_ref()])?;
        out.backup = Some(dest);

        let tx = self.conn.unchecked_transaction()?;
        let rev = self.bump_config_rev()?;
        for (r, t, f) in plan {
            let mut tracker = r.tracker.clone();
            let mut forge = r.forge.clone();
            if let Some((g, mut v)) = t {
                let id = self.ensure_group_connection(&g, &mut out)?;
                strip_jira(&mut v, id);
                tracker = v.to_string();
            }
            if let Some((g, mut v)) = f {
                let id = self.ensure_group_connection(&g, &mut out)?;
                strip_gitlab(&mut v, id);
                forge = v.to_string();
            }
            self.conn.execute(
                "UPDATE project SET tracker_config = ?2, forge_config = ?3, rev = ?4
                 WHERE id = ?1",
                params![r.id, tracker, forge, rev],
            )?;
            out.projects.push(r.key);
        }
        tx.commit()?;
        out.config_rev = Some(rev);
        Ok(out)
    }

    /// Conexão padrão do GitHub, `github`: API de `endpoint` (vazio = a
    /// padrão do GitHub) e credencial `File` se `token_file` existe, senão
    /// `Cli` (`gh auth token`). Não mexe no que já existe com esse nome.
    pub fn ensure_default_github_connection(
        &self,
        endpoint: &str,
        token_file: Option<&Path>,
    ) -> Result<Option<Connection>> {
        if self.connection_by_name("github")?.is_some() {
            return Ok(None);
        }
        let endpoint = match endpoint.trim().trim_end_matches('/') {
            "" | "https://api.github.com" => String::new(),
            e => e.to_string(),
        };
        let credential = match token_file {
            Some(p) if p.is_file() => CredentialSource::File {
                path: p.to_string_lossy().into_owned(),
            },
            _ => CredentialSource::Cli,
        };
        self.add_connection(&NewConnection {
            name: "github".into(),
            kind: ProviderKind::Github,
            endpoint,
            credential,
            options: json!({}),
        })
        .map(Some)
    }

    /// Projetos ativos com algum cadastro antigo (Jira ou GitLab sem conexão).
    fn legacy_rows(&self) -> Result<Vec<Row>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, key, tracker_kind, tracker_config, forge_kind, forge_config
             FROM project WHERE deleted_at IS NULL ORDER BY id",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(Row {
                id: r.get(0)?,
                key: r.get(1)?,
                tracker_kind: r.get(2)?,
                tracker: r.get(3)?,
                forge_kind: r.get(4)?,
                forge: r.get(5)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    fn connection_by_name(&self, name: &str) -> Result<Option<Connection>> {
        let id: Option<i64> = self
            .conn
            .query_row("SELECT id FROM connection WHERE name = ?1", [name], |r| {
                r.get(0)
            })
            .optional()?;
        id.map(|id| self.connection(id)).transpose()
    }

    /// Conexão do grupo: reaproveita a que já tem o nome e o mesmo cadastro;
    /// nome tomado por outra conexão pede o próximo sufixo.
    fn ensure_group_connection(&self, g: &Group, out: &mut LegacyMigration) -> Result<i64> {
        let base = base_name(g.kind, &g.endpoint);
        let mut n = 1u32;
        loop {
            let name = if n == 1 {
                base.clone()
            } else {
                format!("{base}-{n}")
            };
            match self.connection_by_name(&name)? {
                Some(c)
                    if c.kind == g.kind
                        && c.endpoint == g.endpoint
                        && c.credential == g.credential()
                        && c.options == g.options =>
                {
                    return Ok(c.id)
                }
                Some(_) => {}
                None => {
                    let c = self.add_connection(&NewConnection {
                        name: name.clone(),
                        kind: g.kind,
                        endpoint: g.endpoint.clone(),
                        credential: g.credential(),
                        options: g.options.clone(),
                    })?;
                    out.created.push(name);
                    return Ok(c.id);
                }
            }
            n += 1;
        }
    }
}

fn has_legacy_jira(v: &Value) -> bool {
    !str_field(v, "url").is_empty() || !str_field(v, "credential_cmd").is_empty()
}

fn has_legacy_gitlab(v: &Value) -> bool {
    !str_field(v, "host").is_empty() || !str_field(v, "credential_cmd").is_empty()
}

/// Tira os campos antigos do Jira e aponta a conexão.
fn strip_jira(v: &mut Value, connection: i64) {
    if let Some(o) = v.as_object_mut() {
        for k in ["url", "credential_cmd", "config_file"] {
            o.remove(k);
        }
        o.insert("connection".into(), json!(connection));
    }
}

/// Tira os campos antigos do GitLab e aponta a conexão.
fn strip_gitlab(v: &mut Value, connection: i64) {
    if let Some(o) = v.as_object_mut() {
        for k in ["host", "credential_cmd"] {
            o.remove(k);
        }
        o.insert("connection".into(), json!(connection));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{NewHubProject, ProjectProvider};
    use ct_core::{ForgeConfig, GitlabConfig, JiraConfig, TrackerConfig};

    const JIRA_URL: &str = "https://jira.example.com/";
    const JIRA_CMD: &str = "pass show acme/jira | head -1";
    const GITLAB_CMD: &str = "pass show acme/gitlab | head -1";
    const MIL: i64 = 1_792_158_300_000;

    /// Projeto `acme-*` com Jira e GitLab de cadastro antigo (sem conexão).
    fn cadastra(hub: &Hub, repo: &str, url: &str, config_file: &str) {
        let provider = ProjectProvider {
            tracker: TrackerConfig::Jira(JiraConfig {
                url: url.into(),
                project_key: "ACME".into(),
                jql: format!("labels = {repo}"),
                credential_cmd: JIRA_CMD.into(),
                config_file: config_file.into(),
                ..Default::default()
            }),
            forge: ForgeConfig::Gitlab(GitlabConfig {
                host: "gitlab.example.com".into(),
                path: format!("acme/{}", repo.trim_start_matches("acme-")),
                credential_cmd: GITLAB_CMD.into(),
                ..Default::default()
            }),
            ..Default::default()
        };
        hub.add_project_with(
            &NewHubProject {
                owner: "acme".into(),
                repo: repo.into(),
                name: repo.into(),
                base_branch: "main".into(),
                protected_branches: vec!["main".into()],
            },
            &provider,
        )
        .unwrap();
    }

    fn nomes(hub: &Hub) -> Vec<String> {
        let mut v: Vec<String> = hub
            .list_connections()
            .unwrap()
            .into_iter()
            .map(|c| c.name)
            .collect();
        v.sort();
        v
    }

    /// Conexão do Jira e host do GitLab que o projeto guarda no JSON.
    fn vinculo(hub: &Hub, repo: &str) -> (Option<i64>, Option<String>, Option<String>) {
        hub.conn()
            .query_row(
                "SELECT json_extract(tracker_config, '$.connection'),
                        json_extract(tracker_config, '$.url'),
                        json_extract(forge_config, '$.host')
                 FROM project WHERE gh_owner = 'acme' AND gh_repo = ?1",
                [repo],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap()
    }

    /// Projeto migrado (#264): o cadastro fica sem URL, host e credencial,
    /// mas o hub lê a conexão. O `issue_view` do gestor leva o prefixo com
    /// a credencial, e os links de issue e de MR saem com o host da conexão.
    #[test]
    fn projeto_migrado_le_a_conexao_no_hub() {
        let hub = Hub::open_in_memory().unwrap();
        cadastra(&hub, "acme-web", JIRA_URL, "jira-acme.yml");
        let dir = tempfile::tempdir().unwrap();
        hub.migrate_legacy_connections(dir.path(), MIL).unwrap();
        assert_eq!(vinculo(&hub, "acme-web").1, None);
        assert_eq!(vinculo(&hub, "acme-web").2, None);

        let view = format!(
            "JIRA_CONFIG_FILE=jira-acme.yml JIRA_API_TOKEN=\"$({JIRA_CMD})\" jira \
             issue view ACME-<n> --plain"
        );
        let overview = hub
            .agent_overview(&crate::HealthThresholds::default(), 0, MIL)
            .unwrap();
        assert_eq!(overview.projects[0].issue_view, view);
        let project = hub
            .agent_project("acme", "acme-web", true, 10)
            .unwrap()
            .unwrap();
        assert_eq!(project.issue_view, view);

        // As três leituras que a UI usa entregam a visão efetiva.
        let links = |p: &crate::HubProject| {
            (
                ct_core::item_url(&p.tracker_ref(), 12),
                ct_core::change_url(&p.gh_owner, &p.gh_repo, &p.forge, 3),
            )
        };
        let esperado = (
            "https://jira.example.com/browse/ACME-12".to_string(),
            "https://gitlab.example.com/acme/web/-/merge_requests/3".to_string(),
        );
        assert_eq!(links(&hub.project_views().unwrap()[0].project), esperado);
        assert_eq!(
            links(&hub.dashboard().unwrap().projects[0].project),
            esperado
        );
        let tree = hub.tree("acme", "acme-web").unwrap().unwrap();
        assert_eq!(links(&tree.project), esperado);
        let forge = tree.project.forge.gitlab().unwrap();
        assert_eq!(forge.credential_cmd, GITLAB_CMD);

        // Só em memória: o cadastro continua sem os campos antigos.
        assert_eq!(vinculo(&hub, "acme-web").1, None);
        assert_eq!(vinculo(&hub, "acme-web").2, None);
    }

    /// Editar a forja de um projeto migrado (#264): a UI manda o host tirado
    /// da URL de clone, e o cadastro guardado não tem host. O repositório é
    /// o mesmo (o host da conexão), então a edição passa.
    #[test]
    fn projeto_migrado_aceita_edicao_da_forja() {
        let hub = Hub::open_in_memory().unwrap();
        cadastra(&hub, "acme-web", JIRA_URL, "");
        let dir = tempfile::tempdir().unwrap();
        hub.migrate_legacy_connections(dir.path(), MIL).unwrap();
        let conn = hub
            .find_project("acme", "acme-web")
            .unwrap()
            .unwrap()
            .forge
            .gitlab()
            .unwrap()
            .connection;
        let forge = |host: &str| {
            ForgeConfig::Gitlab(GitlabConfig {
                host: host.into(),
                path: "acme/web".into(),
                clone_url: format!("https://{host}/acme/web.git"),
                credential_cmd: String::new(),
                connection: conn,
            })
        };
        let patch = |host: &str| crate::HubProjectPatch {
            forge: Some(forge(host)),
            ..Default::default()
        };
        hub.update_project("acme", "acme-web", &patch("gitlab.example.com"))
            .unwrap();
        // Outro host continua sendo outro repositório.
        let err = hub
            .update_project("acme", "acme-web", &patch("gitlab2.example.com"))
            .unwrap_err();
        assert!(err.to_string().contains("não muda"), "{err}");
    }

    #[test]
    fn dois_projetos_na_mesma_instancia_viram_uma_conexao() {
        let hub = Hub::open_in_memory().unwrap();
        cadastra(&hub, "acme-web", JIRA_URL, "");
        cadastra(&hub, "acme-api", JIRA_URL, "");
        // Outro arquivo de config do jira-cli: outro grupo, mesmo host.
        cadastra(&hub, "acme-mobile", JIRA_URL, "jira-acme.yml");
        let rev_antes = hub.config_rev().unwrap();
        let dir = tempfile::tempdir().unwrap();

        let r = hub.migrate_legacy_connections(dir.path(), MIL).unwrap();
        let mut created = r.created.clone();
        created.sort();
        assert_eq!(
            created,
            vec![
                "gitlab-gitlab.example.com",
                "jira-jira.example.com",
                "jira-jira.example.com-2"
            ]
        );
        assert_eq!(r.projects.len(), 3);
        assert!(r.warnings.is_empty(), "{:?}", r.warnings);
        assert!(r.config_rev.unwrap() > rev_antes);
        let backup = r.backup.clone().expect("cópia antes de gravar");
        assert!(backup.exists());
        assert!(backup
            .file_name()
            .unwrap()
            .to_string_lossy()
            .ends_with("-pre-connections.db"));

        // Acme-web e acme-api dividem a conexão do Jira; acme-mobile tem a sua.
        let jira = hub
            .list_connections()
            .unwrap()
            .into_iter()
            .find(|c| c.name == "jira-jira.example.com")
            .unwrap();
        assert_eq!(jira.endpoint, "https://jira.example.com");
        assert_eq!(
            jira.credential,
            CredentialSource::Command {
                cmd: JIRA_CMD.into()
            }
        );
        assert_eq!(jira.options, json!({ "config_file": "" }));
        assert_eq!(
            hub.connection_users(jira.id).unwrap(),
            vec!["acme/acme-api", "acme/acme-web"]
        );
        let (conn, url, _) = vinculo(&hub, "acme-web");
        assert_eq!(conn, Some(jira.id));
        assert_eq!(url, None, "o campo antigo saiu do projeto");
        let (conn, _, host) = vinculo(&hub, "acme-mobile");
        assert_ne!(conn, Some(jira.id));
        assert_eq!(host, None);

        for c in hub.list_connections().unwrap() {
            assert_eq!(
                hub.test_connection(c.id).unwrap().readiness,
                ct_core::provider::Readiness::Ready
            );
        }
    }

    #[test]
    fn rodar_de_novo_nao_muda_nada() {
        let hub = Hub::open_in_memory().unwrap();
        cadastra(&hub, "acme-web", JIRA_URL, "");
        cadastra(&hub, "acme-api", JIRA_URL, "");
        let dir = tempfile::tempdir().unwrap();
        hub.migrate_legacy_connections(dir.path(), MIL).unwrap();
        let rev = hub.config_rev().unwrap();
        let antes = nomes(&hub);
        let arquivos = std::fs::read_dir(dir.path()).unwrap().count();

        let r = hub
            .migrate_legacy_connections(dir.path(), MIL + 60_000)
            .unwrap();
        assert_eq!(r, LegacyMigration::default());
        assert_eq!(hub.config_rev().unwrap(), rev);
        assert_eq!(nomes(&hub), antes);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), arquivos);
    }

    #[test]
    fn nome_tomado_por_outra_conexao_ganha_sufixo() {
        let hub = Hub::open_in_memory().unwrap();
        // Conexão homônima de outro endereço: a migração não a reaproveita.
        hub.add_connection(&NewConnection {
            name: "jira-jira.example.com".into(),
            kind: ProviderKind::Jira,
            endpoint: "https://jira2.example.com".into(),
            credential: CredentialSource::Command {
                cmd: JIRA_CMD.into(),
            },
            options: json!({ "config_file": "" }),
        })
        .unwrap();
        cadastra(&hub, "acme-web", JIRA_URL, "");
        let dir = tempfile::tempdir().unwrap();
        hub.migrate_legacy_connections(dir.path(), MIL).unwrap();
        assert!(nomes(&hub).contains(&"jira-jira.example.com-2".to_string()));
        let c = hub
            .connection_by_name("jira-jira.example.com-2")
            .unwrap()
            .unwrap();
        assert_eq!(c.endpoint, "https://jira.example.com");
    }

    #[test]
    fn conexao_github_padrao_sai_de_gh_api_url_e_do_token() {
        let hub = Hub::open_in_memory().unwrap();
        let c = hub
            .ensure_default_github_connection("https://api.github.com", None)
            .unwrap()
            .unwrap();
        assert_eq!(c.name, "github");
        assert_eq!(c.endpoint, "");
        assert_eq!(c.credential, CredentialSource::Cli);
        // Já existe: não mexe.
        assert!(hub
            .ensure_default_github_connection("https://gh.example.com/api/v3", None)
            .unwrap()
            .is_none());

        let hub2 = Hub::open_in_memory().unwrap();
        let d = tempfile::tempdir().unwrap();
        let tok = d.path().join("github.token");
        std::fs::write(&tok, "teste\n").unwrap();
        let c = hub2
            .ensure_default_github_connection("https://gh.example.com/api/v3", Some(&tok))
            .unwrap()
            .unwrap();
        assert_eq!(c.endpoint, "https://gh.example.com/api/v3");
        assert_eq!(
            c.credential,
            CredentialSource::File {
                path: tok.to_string_lossy().into_owned()
            }
        );
    }

    #[test]
    fn nome_derivado_do_host() {
        assert_eq!(
            base_name(ProviderKind::Jira, "https://Jira.Example.com:8443/x"),
            "jira-jira.example.com-8443"
        );
        assert_eq!(
            base_name(ProviderKind::Gitlab, "gitlab.example.com"),
            "gitlab-gitlab.example.com"
        );
        let longo = base_name(
            ProviderKind::Jira,
            &format!("https://{}.example.com", "a".repeat(80)),
        );
        assert!(longo.len() <= MAX_NAME, "{longo}");
    }
}
