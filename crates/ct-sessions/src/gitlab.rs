//! GitLab do ambiente pelo glab (spec do Jira e GitLab §7.2, §7.3, issue
//! #350): os MRs da branch de cada work item, para o sync, e a branch remota,
//! para a limpeza. O token vem do comando de credencial do projeto
//! (`forge.credential_cmd`), roda a cada uso e só existe na memória até o
//! fim da chamada; o host vem do cadastro (`forge.host`). Nada disso passa
//! pela configuração global do glab.

use crate::cleanup::RemoteBranches;
use crate::{Result, SessionsError};
use ct_core::GitlabConfig;
use ct_gitlab::{GitlabError, Glab};
use ct_jira::{run_credential, Secret, CREDENTIAL_DEADLINE};
use ct_ledger::{PrRow, Project};
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
}

impl GitlabRemote {
    pub fn new(glab_bin: &str) -> Self {
        GitlabRemote {
            glab_bin: glab_bin.to_string(),
            cli_deadline: ct_gitlab::DEADLINE,
            credential_deadline: CREDENTIAL_DEADLINE,
            credentials: Default::default(),
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

    /// O token do comando de credencial do projeto.
    pub fn token(&self, gl: &GitlabConfig) -> std::result::Result<Secret, String> {
        run_credential("GitLab", &gl.credential_cmd, self.credential_deadline)
            .map_err(|e| e.to_string())
    }

    /// Configuração, glab e token de um projeto GitLab, com o comando de
    /// credencial deste ambiente quando houver.
    fn open(&self, project: &Project) -> Result<(GitlabConfig, Glab, Secret)> {
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
        let token = self.token(&gl).map_err(SessionsError::Invalid)?;
        let cli = self.cli(&gl);
        Ok((gl, cli, token))
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
        let (gl, cli, token) = self.open(project)?;
        let sha = cli
            .mr_head_sha(&token, &project_path(project, &gl), pr.number)
            .map_err(gitlab_error)?;
        Ok(sha)
    }

    fn remote_branch_sha(&self, project: &Project, branch: &str) -> Result<Option<String>> {
        let (gl, cli, token) = self.open(project)?;
        cli.branch_sha(&token, &project_path(project, &gl), branch)
            .map_err(gitlab_error)
    }

    fn delete_remote_branch(&self, project: &Project, branch: &str) -> Result<bool> {
        let (gl, cli, token) = self.open(project)?;
        cli.delete_branch(&token, &project_path(project, &gl), branch)
            .map_err(gitlab_error)
    }
}
