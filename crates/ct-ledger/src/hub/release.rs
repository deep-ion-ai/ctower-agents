//! Configuração de release do projeto no hub (#204): só projetos GitHub +
//! GitHub. O bloco chega aos ambientes pelo pull da configuração.

use super::HubProject;
use crate::{LedgerError, Result};
use ct_core::{ForgeKind, ReleaseConfig, TrackerKind};

/// Configuração de release normalizada e conferida: só projetos GitHub +
/// GitHub (o fluxo é o do release-please, com a API do GitHub). Projeto
/// Jira + GitLab recusa o campo, mesmo desligado.
pub(super) fn checked_release(cur: &HubProject, r: &ReleaseConfig) -> Result<ReleaseConfig> {
    if cur.forge.kind() != ForgeKind::Github || cur.tracker.kind() != TrackerKind::Github {
        return Err(LedgerError::Conflict(
            "release: só em projetos GitHub + GitHub (o fluxo é o do release-please)".into(),
        ));
    }
    r.validate().map_err(LedgerError::Conflict)?;
    Ok(r.normalized())
}

#[cfg(test)]
mod tests {
    use crate::hub::{Hub, HubProjectPatch, NewHubProject, ProjectProvider};
    use ct_core::{ForgeConfig, GitlabConfig, JiraConfig, ReleaseConfig, TrackerConfig};

    fn novo(owner: &str, repo: &str) -> NewHubProject {
        NewHubProject {
            owner: owner.into(),
            repo: repo.into(),
            name: repo.into(),
            base_branch: "develop".into(),
            protected_branches: vec!["develop".into(), "main".into()],
        }
    }

    fn ligar(hub: &Hub, owner: &str, repo: &str, r: ReleaseConfig) -> crate::Result<()> {
        hub.update_project(
            owner,
            repo,
            &HubProjectPatch {
                release: Some(r),
                ..Default::default()
            },
        )
        .map(|_| ())
    }

    #[test]
    fn release_no_projeto_github_vai_no_pull() {
        let hub = Hub::open_in_memory().unwrap();
        hub.add_project(&novo("acme", "acme-web")).unwrap();
        let p = hub.find_project("acme", "acme-web").unwrap().unwrap();
        assert_eq!(p.release, ReleaseConfig::default());

        let rev0 = hub.config_rev().unwrap();
        ligar(
            &hub,
            "acme",
            "acme-web",
            ReleaseConfig {
                enabled: true,
                token_secret: "  ACME_RELEASE_TOKEN ".into(),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(hub.config_rev().unwrap() > rev0);
        let p = hub.find_project("acme", "acme-web").unwrap().unwrap();
        assert!(p.release.enabled);
        assert_eq!(p.release.token_secret, "ACME_RELEASE_TOKEN");

        hub.ingest("env-a", "a", &[]).unwrap();
        hub.add_assignment("acme", "acme-web", "env-a", None)
            .unwrap();
        let ep = Some(hub.config_epoch().unwrap());
        let c = hub.env_config("env-a", None, ep).unwrap();
        let cp = &c.projects.as_ref().unwrap()[0];
        assert!(cp.release.enabled);
        assert_eq!(cp.release.token_secret, "ACME_RELEASE_TOKEN");

        // Configuração inválida é recusada.
        let err = ligar(
            &hub,
            "acme",
            "acme-web",
            ReleaseConfig {
                target: "develop".into(),
                ..Default::default()
            },
        )
        .unwrap_err();
        assert!(err.to_string().contains("diferentes"), "{err}");
    }

    #[test]
    fn projeto_jira_gitlab_recusa_o_campo() {
        let hub = Hub::open_in_memory().unwrap();
        let pv = ProjectProvider {
            tracker: TrackerConfig::Jira(JiraConfig {
                url: "https://jira.example.com".into(),
                project_key: "ACME".into(),
                credential_cmd: "pass show jira | head -1".into(),
                type_map: ct_core::default_type_map(),
                active_sprint_only: true,
                ..Default::default()
            }),
            forge: ForgeConfig::Gitlab(GitlabConfig {
                host: "gitlab.example.com".into(),
                path: "acme/acme-web".into(),
                clone_url: "https://gitlab.example.com/acme/acme-web.git".into(),
                credential_cmd: "pass show gitlab | head -1".into(),
            }),
            branch_pattern: Some("{key}".into()),
            attribution: false,
            ..Default::default()
        };
        hub.add_project_with(&novo("acme", "acme-web"), &pv)
            .unwrap();
        for enabled in [true, false] {
            let err = ligar(
                &hub,
                "acme",
                "acme-web",
                ReleaseConfig {
                    enabled,
                    ..Default::default()
                },
            )
            .unwrap_err();
            assert!(err.to_string().contains("GitHub + GitHub"), "{err}");
        }
    }
}
