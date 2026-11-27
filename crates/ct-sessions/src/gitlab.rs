//! GitLab do ambiente pelo glab (spec do Jira e GitLab §7.2, §7.3, issue
//! #350): os MRs da branch de cada work item, para o sync, e a branch remota,
//! para a limpeza. O token vem do resolvedor de credencial (#236), pela fonte
//! efetiva da forja do projeto (a conexão do hub com a sobrescrita do
//! ambiente, #274): a fonte só é consultada sem token em cache, e uma recusa
//! de autenticação descarta o token e tenta de novo uma vez. O host vem da
//! conexão do GitLab (#277). Nada disso passa pela configuração global do
//! glab.

use crate::cleanup::RemoteBranches;
use crate::resolver::{gitlab_key, Ask, CredentialResolver};
use crate::{Result, SessionsError};
use ct_core::provider::{CredentialSource, ProviderKind};
use ct_core::{GitlabConfig, ResolvedConnections};
use ct_gitlab::{GitlabError, Glab};
use ct_jira::{Secret, CREDENTIAL_DEADLINE};
use ct_ledger::{PrRow, Project};
use ct_secrets::EnvCache;
use std::sync::Arc;
use std::time::Duration;

/// O glab do ambiente para os projetos com forja GitLab.
#[derive(Debug, Clone)]
pub struct GitlabRemote {
    /// `glab_bin` do `config.toml` (padrão `"glab"`).
    pub glab_bin: String,
    /// Prazo de cada chamada ao glab.
    pub cli_deadline: Duration,
    /// Prazo do comando de credencial.
    pub credential_deadline: Duration,
    /// Sobrescritas de credencial e conexões do hub deste ambiente
    /// (`[credentials]`, #143; `[connections]`, #274).
    pub credentials: ct_core::SharedCredentials,
    /// Cache de secrets do ambiente, para a fonte `Secret` (#274); `None` =
    /// sem busca de secret.
    pub env_cache: Option<EnvCache>,
    /// Cache de tokens do ambiente (#236).
    pub(crate) resolver: Arc<CredentialResolver>,
}

impl GitlabRemote {
    pub fn new(glab_bin: &str) -> Self {
        GitlabRemote {
            glab_bin: glab_bin.to_string(),
            cli_deadline: ct_gitlab::DEADLINE,
            credential_deadline: CREDENTIAL_DEADLINE,
            credentials: Default::default(),
            env_cache: None,
            resolver: Arc::new(CredentialResolver::new()),
        }
    }

    /// Com as sobrescritas de credencial e as conexões deste ambiente.
    pub fn with_credentials(mut self, credentials: ct_core::SharedCredentials) -> Self {
        self.credentials = credentials;
        self
    }

    /// Com o cache de secrets do ambiente, para a fonte `Secret` (#274).
    pub fn with_env_cache(mut self, cache: EnvCache) -> Self {
        self.env_cache = Some(cache);
        self
    }

    /// O glab para o host da conexão do GitLab do projeto.
    pub fn cli(&self, conns: &ResolvedConnections) -> Glab {
        Glab::new(&self.glab_bin, &conns.gitlab_host()).with_deadline(self.cli_deadline)
    }

    /// Configuração, glab e fonte do token de um projeto GitLab neste
    /// ambiente.
    fn open(&self, project: &Project) -> Result<(GitlabConfig, Glab, CredentialSource)> {
        let gl = project.forge.gitlab().cloned().ok_or_else(|| {
            SessionsError::Invalid(format!(
                "{}/{} não tem forja GitLab",
                project.gh_owner, project.gh_repo
            ))
        })?;
        let (conns, _) = self.credentials.resolve(
            &project.gh_owner,
            &project.gh_repo,
            &project.tracker,
            &project.forge,
        );
        if conns.forge.is_none() {
            return Err(SessionsError::Invalid(format!(
                "{}/{} sem conexão do GitLab neste ambiente",
                project.gh_owner, project.gh_repo
            )));
        }
        let source = self
            .credentials
            .source(
                &project.gh_owner,
                &project.gh_repo,
                &project.tracker,
                &project.forge,
                ProviderKind::Gitlab,
            )
            .map_or(CredentialSource::None, |s| s.source);
        let cli = self.cli(&conns);
        Ok((gl, cli, source))
    }

    /// Roda `op` com o glab e o token do projeto. Recusa de autenticação
    /// descarta o token do cache e tenta uma vez com um token novo.
    fn call<T>(
        &self,
        project: &Project,
        op: impl Fn(&Glab, &GitlabConfig, &Secret) -> std::result::Result<T, GitlabError>,
    ) -> Result<T> {
        let (gl, cli, source) = self.open(project)?;
        let lookup = |name: &str| {
            self.env_cache
                .as_ref()
                .and_then(|c| crate::secrets::cached_secret(c, project, name))
        };
        self.resolver
            .with_refresh(
                Ask {
                    key: &gitlab_key(project.id),
                    what: "GitLab",
                    source: &source,
                    lookup: &lookup,
                    deadline: self.credential_deadline,
                    cli: None,
                },
                |e: &GitlabError| matches!(e, GitlabError::Unauthorized),
                |t| op(&cli, &gl, t),
            )
            .map_err(gitlab_error)
    }
}

fn gitlab_error(e: GitlabError) -> SessionsError {
    SessionsError::Invalid(format!("GitLab: {e}"))
}

/// Caminho do projeto no GitLab: o do cadastro, ou a chave `owner/repo`.
pub fn project_path(project: &Project, gl: &GitlabConfig) -> String {
    match gl.path.trim_matches('/') {
        "" => format!("{}/{}", project.gh_owner, project.gh_repo),
        p => p.to_string(),
    }
}

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
    use crate::test_support::{FakeClaude, TestEnv};
    use ct_core::provider::{Connection, ProviderKind};

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
