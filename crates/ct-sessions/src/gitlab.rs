//! GitLab do ambiente pelo glab (spec do Jira e GitLab §7.2, §7.3, issue
//! #350): os MRs da branch de cada work item, para o sync, e a branch remota,
//! para a limpeza. O token vem do resolvedor de credencial (#236): o comando
//! do projeto (`forge.credential_cmd`) roda só sem token em cache, e uma
//! recusa de autenticação descarta o token e tenta de novo uma vez. O host
//! vem do cadastro (`forge.host`). Nada disso passa pela configuração global
//! do glab.

use crate::cleanup::RemoteBranches;
use crate::resolver::{gitlab_key, no_lookup, Ask, CredentialResolver};
use crate::{Result, SessionsError};
use ct_core::provider::CredentialSource;
use ct_core::GitlabConfig;
use ct_gitlab::{GitlabError, Glab};
use ct_jira::{Secret, CREDENTIAL_DEADLINE};
use ct_ledger::{PrRow, Project};
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
    /// Comandos de credencial deste ambiente (`[credentials]`, #143).
    pub credentials: ct_core::SharedCredentials,
    /// Cache de tokens do ambiente (#236); o do `Sessions`, no sync.
    pub(crate) resolver: Arc<CredentialResolver>,
}

impl GitlabRemote {
    pub fn new(glab_bin: &str) -> Self {
        GitlabRemote {
            glab_bin: glab_bin.to_string(),
            cli_deadline: ct_gitlab::DEADLINE,
            credential_deadline: CREDENTIAL_DEADLINE,
            credentials: Default::default(),
            resolver: Arc::new(CredentialResolver::new()),
        }
    }

    /// Com os comandos de credencial deste ambiente no lugar dos do
    /// cadastro (#143).
    pub fn with_credentials(mut self, credentials: ct_core::SharedCredentials) -> Self {
        self.credentials = credentials;
        self
    }

    /// O glab para o host do cadastro.
    pub fn cli(&self, gl: &GitlabConfig) -> Glab {
        Glab::new(&self.glab_bin, &gl.host).with_deadline(self.cli_deadline)
    }

    /// Fonte do token: o comando de credencial do projeto.
    fn source(gl: &GitlabConfig) -> CredentialSource {
        CredentialSource::Command {
            cmd: gl.credential_cmd.clone(),
        }
    }

    /// O token do projeto, pelo cache (o comando só roda sem token em
    /// cache). Sem o cadastro desta forja, o token fica de fora. `true` =
    /// veio do cache, sem rodar o comando (#268).
    pub fn token(
        &self,
        project_id: i64,
        gl: &GitlabConfig,
    ) -> std::result::Result<(Arc<Secret>, bool), String> {
        self.resolver
            .token_and_origin(Ask {
                key: &gitlab_key(project_id),
                what: "GitLab",
                source: &Self::source(gl),
                lookup: &no_lookup,
                deadline: self.credential_deadline,
            })
            .map_err(|e| e.to_string())
    }

    /// Configuração, glab e fonte do token de um projeto GitLab, com o
    /// comando de credencial deste ambiente quando houver.
    fn open(&self, project: &Project) -> Result<(GitlabConfig, Glab)> {
        let (mut tracker, mut forge) = (project.tracker.clone(), project.forge.clone());
        self.credentials.apply(
            &project.gh_owner,
            &project.gh_repo,
            &mut tracker,
            &mut forge,
        );
        let gl = forge.gitlab().cloned().ok_or_else(|| {
            SessionsError::Invalid(format!(
                "{}/{} não tem forja GitLab",
                project.gh_owner, project.gh_repo
            ))
        })?;
        let cli = self.cli(&gl);
        Ok((gl, cli))
    }

    /// Roda `op` com o glab e o token do projeto. Recusa de autenticação
    /// descarta o token do cache e tenta uma vez com um token novo.
    fn call<T>(
        &self,
        project: &Project,
        op: impl Fn(&Glab, &GitlabConfig, &Secret) -> std::result::Result<T, GitlabError>,
    ) -> Result<T> {
        let (gl, cli) = self.open(project)?;
        self.resolver
            .with_refresh(
                Ask {
                    key: &gitlab_key(project.id),
                    what: "GitLab",
                    source: &Self::source(&gl),
                    lookup: &no_lookup,
                    deadline: self.credential_deadline,
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
