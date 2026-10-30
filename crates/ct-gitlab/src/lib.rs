//! MRs e branches do GitLab pelo [glab](https://gitlab.com/gitlab-org/cli)
//! (spec do Jira e GitLab §7.2, §7.3, issue #350), no mesmo molde do
//! `ct-jira`: sem rede própria, o `glab` roda como processo externo, com
//! prazo, sem TTY e com `stdin` nulo.
//!
//! - [`Glab`]: as chamadas (`version`, `api version`, `mr list`, a ponta e o
//!   `DELETE` da branch remota), com `GITLAB_HOST` do cadastro e
//!   `GITLAB_TOKEN` do comando de credencial só no ambiente do processo
//!   filho. A configuração global do glab no ambiente não é tocada;
//! - [`mr_row`]: o JSON do MR (`mr list -F json`) → linha do espelho, com o
//!   `iid` no lugar do número do PR.

mod cli;
mod mr;

#[cfg(any(test, feature = "test-support"))]
pub mod fake;

pub use cli::{encode, parse_cli_version, GitlabError, Glab, DEADLINE, PAGE_SIZE};
pub use mr::{mr_row, mr_state};
