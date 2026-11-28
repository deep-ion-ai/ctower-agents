//! Última release e entregas na base sem release, por projeto (#355): o que
//! o sync do hub lê do GitHub (`ct_gh::deliveries`) e os cards da Sala de
//! controle mostram. Só projetos GitHub + GitHub; os outros ficam com o
//! campo vazio (`supported = false`), e a UI mostra "—".

use crate::{Hub, HubProject, Result};
use ct_core::{ForgeKind, TrackerKind};
use ct_gh::deliveries::{merged_pulls_path, releases_path};
use ct_gh::deliveries::{DeliveriesInput, DeliveriesSnapshot, Delivery, ReleaseRef};
use rusqlite::{params, OptionalExtension};
use serde::Serialize;

/// Leitura por projeto: a última release e as entregas sem release.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct ProjectRelease {
    pub project_id: i64,
    pub owner: String,
    pub repo: String,
    /// O hub lê isso do projeto (GitHub + GitHub). `false`: a UI mostra "—".
    pub supported: bool,
    /// Já houve leitura. Sem ela, `release` vazio não quer dizer "sem
    /// release", e sim "ainda não sei".
    pub synced: bool,
    /// Última release publicada; `None` com `synced` = "sem release".
    pub release: Option<ReleaseRef>,
    /// Quantos PRs foram mesclados na base depois da release (todos, se não
    /// há release). Igual a `deliveries.len()`.
    pub unreleased: usize,
    /// Os PRs, do merge mais recente ao mais antigo, para o tooltip.
    pub deliveries: Vec<Delivery>,
    /// A leitura parou no limite de páginas: `unreleased` é um mínimo.
    pub truncated: bool,
    /// Quando a leitura gravada mudou pela última vez.
    pub synced_at: Option<i64>,
}

fn supported(p: &HubProject) -> bool {
    p.forge.kind() == ForgeKind::Github && p.tracker.kind() == TrackerKind::Github
}

impl Hub {
    /// Entrada da busca de release e entregas do projeto, ou `None` se o hub
    /// não lê isso dele (Jira ou GitLab) ou ele não existe.
    pub fn deliveries_input(
        &self,
        project_id: i64,
        now_ms: i64,
    ) -> Result<Option<DeliveriesInput>> {
        let Some(p) = self
            .list_projects()?
            .into_iter()
            .find(|p| p.id == project_id)
            .filter(supported)
        else {
            return Ok(None);
        };
        let (o, r) = (&p.gh_owner, &p.gh_repo);
        Ok(Some(DeliveriesInput {
            project_id,
            owner: o.clone(),
            repo: r.clone(),
            release_branch: p.release.target.clone(),
            releases_etag: self.etag(&releases_path(o, r))?,
            pulls_etag: self.etag(&merged_pulls_path(o, r, &p.base_branch))?,
            stored: self.stored_release(project_id)?,
            base: p.base_branch,
            now_ms,
        }))
    }

    fn stored_release(&self, project_id: i64) -> Result<Option<ReleaseRef>> {
        Ok(self
            .conn
            .query_row(
                "SELECT tag, name, published_at, url FROM project_release
                 WHERE project_id = ?1 AND tag IS NOT NULL",
                [project_id],
                |r| {
                    Ok(ReleaseRef {
                        tag: r.get(0)?,
                        name: r.get(1)?,
                        published_at: r.get(2)?,
                        url: r.get(3)?,
                    })
                },
            )
            .optional()?)
    }

    /// Grava o que a busca trouxe, com os ETags, numa transação: os dois
    /// andam juntos, para um `304` nunca valer sobre dado que não foi
    /// gravado. Projeto excluído enquanto a busca rodava: nada a gravar.
    pub fn apply_deliveries(&self, s: &DeliveriesSnapshot) -> Result<()> {
        if self.live_project_rev(s.project_id)?.is_none() {
            return Ok(());
        }
        let tx = self.conn.unchecked_transaction()?;
        let r = s.release.as_ref();
        self.conn.execute(
            "INSERT INTO project_release (project_id, tag, name, published_at, url, truncated, synced_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT (project_id) DO UPDATE SET
                tag = excluded.tag, name = excluded.name, published_at = excluded.published_at,
                url = excluded.url, truncated = excluded.truncated, synced_at = excluded.synced_at",
            params![
                s.project_id,
                r.map(|r| r.tag.as_str()),
                r.map(|r| r.name.as_str()).unwrap_or_default(),
                r.map(|r| r.published_at),
                r.map(|r| r.url.as_str()).unwrap_or_default(),
                s.truncated,
                s.synced_at
            ],
        )?;
        self.conn.execute(
            "DELETE FROM project_delivery WHERE project_id = ?1",
            [s.project_id],
        )?;
        let mut ins = self.conn.prepare(
            "INSERT OR REPLACE INTO project_delivery (project_id, pr_number, title, merged_at)
             VALUES (?1, ?2, ?3, ?4)",
        )?;
        for d in &s.deliveries {
            ins.execute(params![s.project_id, d.number, d.title, d.merged_at])?;
        }
        drop(ins);
        for (key, etag) in &s.etags {
            self.conn.execute(
                "INSERT INTO gh_etag (key, etag, updated_at) VALUES (?1, ?2, ?3)
                 ON CONFLICT (key) DO UPDATE SET etag = excluded.etag, updated_at = excluded.updated_at",
                params![key, etag, s.synced_at],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Release e entregas de um projeto cadastrado.
    pub fn project_release(&self, project_id: i64) -> Result<Option<ProjectRelease>> {
        self.list_projects()?
            .into_iter()
            .find(|p| p.id == project_id)
            .map(|p| self.release_view(&p))
            .transpose()
    }

    /// Release e entregas de todos os projetos cadastrados, na ordem da lista.
    pub fn project_releases(&self) -> Result<Vec<ProjectRelease>> {
        self.list_projects()?
            .iter()
            .map(|p| self.release_view(p))
            .collect()
    }

    fn release_view(&self, p: &HubProject) -> Result<ProjectRelease> {
        let mut out = ProjectRelease {
            project_id: p.id,
            owner: p.gh_owner.clone(),
            repo: p.gh_repo.clone(),
            supported: supported(p),
            synced: false,
            release: None,
            unreleased: 0,
            deliveries: Vec::new(),
            truncated: false,
            synced_at: None,
        };
        if !out.supported {
            return Ok(out);
        }
        let row: Option<(bool, i64)> = self
            .conn
            .query_row(
                "SELECT truncated, synced_at FROM project_release WHERE project_id = ?1",
                [p.id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let Some((truncated, synced_at)) = row else {
            return Ok(out);
        };
        let mut stmt = self.conn.prepare(
            "SELECT pr_number, title, merged_at FROM project_delivery WHERE project_id = ?1
             ORDER BY merged_at DESC, pr_number DESC",
        )?;
        out.deliveries = stmt
            .query_map([p.id], |r| {
                Ok(Delivery {
                    number: r.get(0)?,
                    title: r.get(1)?,
                    merged_at: r.get(2)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        out.synced = true;
        out.release = self.stored_release(p.id)?;
        out.unreleased = out.deliveries.len();
        out.truncated = truncated;
        out.synced_at = Some(synced_at);
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use crate::connection::NewConnection;
    use crate::{Hub, NewHubProject, ProjectProvider};
    use ct_core::provider::{CredentialSource, ProviderKind};
    use ct_core::{ForgeConfig, GitlabConfig, JiraConfig, TrackerConfig};
    use ct_gh::deliveries::{DeliveriesSnapshot, Delivery, ReleaseRef};

    fn novo(hub: &Hub) -> i64 {
        hub.add_project(&NewHubProject {
            owner: "acme".into(),
            repo: "acme-web".into(),
            name: "acme-web".into(),
            base_branch: "develop".into(),
            protected_branches: vec!["develop".into(), "main".into()],
        })
        .unwrap()
        .project
        .id
    }

    fn release(tag: &str) -> ReleaseRef {
        ReleaseRef {
            tag: tag.into(),
            name: format!("Versão {tag}"),
            published_at: 1_000,
            url: format!("https://github.com/acme/acme-web/releases/tag/{tag}"),
        }
    }

    fn entrega(n: i64, at: i64) -> Delivery {
        Delivery {
            number: n,
            title: format!("Entrega {n}"),
            merged_at: at,
        }
    }

    fn snap(
        project_id: i64,
        release: Option<ReleaseRef>,
        deliveries: Vec<Delivery>,
    ) -> DeliveriesSnapshot {
        DeliveriesSnapshot {
            project_id,
            release,
            deliveries,
            truncated: false,
            etags: vec![(
                "/repos/acme/acme-web/releases?per_page=100".into(),
                "W/\"a\"".into(),
            )],
            synced_at: 5_000,
        }
    }

    #[test]
    fn antes_do_primeiro_sync_nao_ha_leitura() {
        let hub = Hub::open_in_memory().unwrap();
        let p = novo(&hub);
        let v = hub.project_release(p).unwrap().unwrap();
        assert!(v.supported && !v.synced);
        assert_eq!((v.release, v.unreleased, v.synced_at), (None, 0, None));
        let input = hub.deliveries_input(p, 9).unwrap().unwrap();
        assert_eq!(
            (input.owner.as_str(), input.base.as_str()),
            ("acme", "develop")
        );
        assert_eq!(input.release_branch, "main");
        assert!(input.releases_etag.is_none() && input.stored.is_none());
    }

    #[test]
    fn guarda_a_release_as_entregas_e_os_etags() {
        let hub = Hub::open_in_memory().unwrap();
        let p = novo(&hub);
        hub.apply_deliveries(&snap(
            p,
            Some(release("v1.2.0")),
            vec![entrega(7, 100), entrega(9, 300), entrega(8, 300)],
        ))
        .unwrap();
        let v = hub.project_release(p).unwrap().unwrap();
        assert!(v.synced);
        assert_eq!(v.release.as_ref().unwrap().tag, "v1.2.0");
        assert_eq!(v.unreleased, 3);
        // Mais recente primeiro; no empate, o número maior.
        let numbers: Vec<_> = v.deliveries.iter().map(|d| d.number).collect();
        assert_eq!(numbers, [9, 8, 7]);
        assert_eq!(v.deliveries[0].title, "Entrega 9");
        assert_eq!(v.synced_at, Some(5_000));
        // O ETag e a release gravada voltam na entrada da rodada seguinte.
        let input = hub.deliveries_input(p, 9).unwrap().unwrap();
        assert_eq!(input.releases_etag.as_deref(), Some("W/\"a\""));
        assert_eq!(input.stored.unwrap().tag, "v1.2.0");
    }

    #[test]
    fn release_nova_zera_as_entregas_e_sem_release_vira_vazio() {
        let hub = Hub::open_in_memory().unwrap();
        let p = novo(&hub);
        hub.apply_deliveries(&snap(p, Some(release("v1.2.0")), vec![entrega(7, 100)]))
            .unwrap();
        hub.apply_deliveries(&snap(p, Some(release("v1.3.0")), vec![]))
            .unwrap();
        let v = hub.project_release(p).unwrap().unwrap();
        assert_eq!(v.release.unwrap().tag, "v1.3.0");
        assert_eq!((v.unreleased, v.deliveries.len()), (0, 0));

        // Sem nenhuma release (lida): `synced` e sem release, com os PRs.
        hub.apply_deliveries(&snap(p, None, vec![entrega(1, 10)]))
            .unwrap();
        let v = hub.project_release(p).unwrap().unwrap();
        assert!(v.synced && v.release.is_none());
        assert_eq!(v.unreleased, 1);
        assert!(hub
            .deliveries_input(p, 9)
            .unwrap()
            .unwrap()
            .stored
            .is_none());
    }

    #[test]
    fn gitlab_fica_vazio_e_nao_e_lido() {
        let hub = Hub::open_in_memory().unwrap();
        let conn = |kind, endpoint: &str| {
            hub.add_connection(&NewConnection {
                name: format!("{kind}-acme"),
                kind,
                endpoint: endpoint.into(),
                credential: CredentialSource::Command {
                    cmd: format!("pass show {kind} | head -1"),
                },
                options: serde_json::json!({}),
            })
            .unwrap()
            .id
        };
        let pv = ProjectProvider {
            tracker: TrackerConfig::Jira(JiraConfig {
                project_key: "ACME".into(),
                type_map: ct_core::default_type_map(),
                active_sprint_only: true,
                connection: Some(conn(ProviderKind::Jira, "https://jira.example.com")),
                ..Default::default()
            }),
            forge: ForgeConfig::Gitlab(GitlabConfig {
                connection: Some(conn(ProviderKind::Gitlab, "gitlab.example.com")),
                path: "acme/acme-web".into(),
                clone_url: "https://gitlab.example.com/acme/acme-web.git".into(),
            }),
            branch_pattern: Some("{key}".into()),
            attribution: false,
            ..Default::default()
        };
        let p = hub
            .add_project_with(
                &NewHubProject {
                    owner: "acme".into(),
                    repo: "acme-web".into(),
                    name: "acme-web".into(),
                    base_branch: "develop".into(),
                    protected_branches: vec!["develop".into()],
                },
                &pv,
            )
            .unwrap()
            .project
            .id;
        let v = hub.project_release(p).unwrap().unwrap();
        assert!(!v.supported && !v.synced);
        assert_eq!((v.release, v.unreleased), (None, 0));
        assert!(hub.deliveries_input(p, 9).unwrap().is_none());
    }

    #[test]
    fn projeto_excluido_nao_grava_e_a_exclusao_limpa() {
        let hub = Hub::open_in_memory().unwrap();
        let p = novo(&hub);
        hub.apply_deliveries(&snap(p, Some(release("v1.2.0")), vec![entrega(7, 100)]))
            .unwrap();
        hub.delete_project("acme", "acme-web").unwrap();
        let n = |t: &str| -> i64 {
            hub.conn()
                .query_row(&format!("SELECT count(*) FROM {t}"), [], |r| r.get(0))
                .unwrap()
        };
        assert_eq!((n("project_release"), n("project_delivery")), (0, 0));
        // A busca que terminou depois da exclusão não grava nada.
        hub.apply_deliveries(&snap(p, Some(release("v9.9.9")), vec![entrega(1, 1)]))
            .unwrap();
        assert_eq!((n("project_release"), n("project_delivery")), (0, 0));
    }
}
