//! Sincronização do ambiente com a forja e o rastreador (#242): o GitHub
//! ([`GithubSync`]), o Jira e o GitLab ([`JiraGitlabSync`]). Recebe do
//! `ct-sessions` só o [`SyncHandle`] (ledger, aviso à UI e credenciais das
//! conexões), sem o ciclo de vida das sessões. Síncrono; o servidor chama
//! daqui via `spawn_blocking`.

pub mod github;
pub mod gitlab;
pub mod tracker_sync;

pub use ct_sessions::SyncHandle;
pub use github::{GithubSync, ProjectSync};
pub use gitlab::GitlabRemote;
pub use tracker_sync::{JiraGitlabSync, TrackerSyncConfig};
