//! Chaves do cache de credencial do ambiente. O resolvedor mora no `ct-proc`
//! ([`ct_proc::resolver`]), que o hub também usa (#275).

use ct_core::provider::ProviderKind;
pub use ct_proc::resolver::{Ask, CredentialResolver};

#[cfg(test)]
pub(crate) use ct_proc::resolver::no_lookup;

/// Chave do cache do token do tipo `kind` de um projeto: `<tipo>:<id>`
/// (`jira:7`, `gitlab:7`, `github:7`).
pub fn cache_key(kind: ProviderKind, project_id: i64) -> String {
    format!("{kind}:{project_id}")
}

/// Chave do cache do Jira de um projeto.
pub fn jira_key(project_id: i64) -> String {
    cache_key(ProviderKind::Jira, project_id)
}

/// Chave do cache do GitLab de um projeto.
pub fn gitlab_key(project_id: i64) -> String {
    cache_key(ProviderKind::Gitlab, project_id)
}

/// Chave do cache do GitHub de um projeto (#275).
pub fn github_key(project_id: i64) -> String {
    cache_key(ProviderKind::Github, project_id)
}
