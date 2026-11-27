//! Branch remota e head do MR no GitLab para a limpeza, pelo glab do
//! ambiente ([`GitlabRemote`]), com o mesmo token do sync.

use crate::RemoteBranches;
use ct_ledger::{PrRow, Project};
use ct_sessions::Result;
use ct_sync::gitlab::project_path;
use ct_sync::GitlabRemote;

impl RemoteBranches for GitlabRemote {
    /// O `sha` do MR na API (o `mr list` sempre traz; isto é a reserva, e
    /// não grava no espelho: o próximo ciclo do sync grava).
    fn pr_head_sha(&self, project: &Project, pr: &PrRow) -> Result<Option<String>> {
        self.call(project, |cli, gl, token| {
            cli.mr_head_sha(token, &project_path(project, gl), pr.number)
        })
    }

    fn remote_branch_sha(&self, project: &Project, branch: &str) -> Result<Option<String>> {
        self.call(project, |cli, gl, token| {
            cli.branch_sha(token, &project_path(project, gl), branch)
        })
    }

    fn delete_remote_branch(&self, project: &Project, branch: &str) -> Result<bool> {
        self.call(project, |cli, gl, token| {
            cli.delete_branch(token, &project_path(project, gl), branch)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ct_core::provider::CredentialSource;
    use ct_core::provider::{Connection, ProviderKind};
    use ct_core::GitlabConfig;
    use ct_sessions::test_support::{FakeClaude, TestEnv};

    const HOST: &str = "gitlab.example.com";
    const TOKEN: &str = "tok-acme-gitlab-123";

    /// O glab da limpeza usa a fonte da conexão do projeto (#274): com
    /// `Secret`, o token vem do cache de secrets do ambiente, no projeto.
    #[test]
    fn limpeza_usa_o_secret_da_conexao() {
        let env = TestEnv::new(FakeClaude::exit(0));
        let glab = ct_gitlab::fake::FakeGlab::new(env.tmp.path(), HOST, TOKEN);
        env.assign_provider(
            ct_core::TrackerConfig::Jira(ct_core::JiraConfig {
                project_key: "ACME".into(),
                connection: Some(7),
                ..Default::default()
            }),
            ct_core::ForgeConfig::Gitlab(GitlabConfig {
                connection: Some(8),
                path: "acme/acme-web".into(),
                clone_url: format!("https://{HOST}/acme/acme-web.git"),
            }),
            false,
            1,
        );
        env.cfg.credentials.set_connections(
            vec![Connection {
                id: 8,
                name: "gitlab-acme".into(),
                kind: ProviderKind::Gitlab,
                endpoint: HOST.into(),
                credential: CredentialSource::Secret {
                    name: "GITLAB_TOKEN_ACME".into(),
                },
                options: serde_json::json!({}),
                created_at: 0,
                updated_at: 0,
            }],
            Default::default(),
        );
        env.write_secrets_cache(&[(Some("deep-ion-ai/demo"), "GITLAB_TOKEN_ACME", TOKEN)]);
        let sha = "0123456789abcdef0123456789abcdef01234567";
        glab.set_branch("ACME-9", sha);
        let project = env.sessions.ledger().get_project(env.project_id).unwrap();
        let bin = glab.bin().to_str().unwrap();

        // Sem o cache de secrets, o secret não resolve.
        let sem_cache = GitlabRemote::new(bin).with_credentials(env.cfg.credentials.clone());
        let e = sem_cache
            .remote_branch_sha(&project, "ACME-9")
            .unwrap_err()
            .to_string();
        assert!(e.contains("GITLAB_TOKEN_ACME") && !e.contains(TOKEN), "{e}");

        let remote = GitlabRemote::new(bin)
            .with_credentials(env.cfg.credentials.clone())
            .with_env_cache(env.sessions.env_cache());
        assert_eq!(
            remote.remote_branch_sha(&project, "ACME-9").unwrap(),
            Some(sha.to_string())
        );
        assert!(glab.calls().iter().all(|c| !c.contains(TOKEN)));
    }
}
