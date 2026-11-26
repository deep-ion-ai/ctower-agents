//! Chaves do cache de credencial do ambiente. O resolvedor mora no `ct-proc`
//! ([`ct_proc::resolver`]), que o hub também usa (#275).

pub(crate) use ct_proc::resolver::{Ask, CredentialResolver};

#[cfg(test)]
pub(crate) use ct_proc::resolver::no_lookup;

/// Chave do cache do Jira de um projeto.
pub(crate) fn jira_key(project_id: i64) -> String {
    format!("jira:{project_id}")
}

/// Chave do cache do GitLab de um projeto.
pub(crate) fn gitlab_key(project_id: i64) -> String {
    format!("gitlab:{project_id}")
}

/// Prefixo das chaves do GitHub ([`github_key`]).
pub(crate) const GITHUB_KEY_PREFIX: &str = "github:";

/// Chave do cache do GitHub de um projeto (#275).
pub(crate) fn github_key(project_id: i64) -> String {
    format!("{GITHUB_KEY_PREFIX}{project_id}")
}
