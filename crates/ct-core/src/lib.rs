//! Domínio do Control Tower: tipos, máquinas de estado e regras puras (sem
//! I/O, exceto os arquivos privados de `files`).

pub mod branch;
mod clock;
pub mod docs;
pub mod files;
pub mod guard;
mod logs;
#[macro_use]
mod model;
mod repo_url;
pub mod tracker;

pub use clock::{civil_from_days, hlc, now_ms, parse_iso8601, utc_stamp};
pub use docs::DocsConfig;
pub use logs::LogsConfig;
pub use model::*;
pub use repo_url::{github_repo_from_url, repo_from_url, RepoRef};
pub use tracker::{
    change_label, change_ref, change_short, change_url, default_type_map, glab_cli_prefix,
    issue_view_cmd, item_key, item_ref, item_short, item_url, jira_cli_prefix,
    supported_combination, ForgeConfig, ForgeKind, GitlabConfig, GitlabReadiness, JiraConfig,
    JiraReadiness, JiraTypeRole, Sprint, SprintState, TrackerConfig, TrackerKind, TrackerProgress,
    TrackerReadiness, TrackerRef, ACTIVE_SPRINT_JQL, DEFAULT_MAX_ISSUES, MAX_ISSUES_CEILING,
};

/// Versão do Control Tower (a do workspace).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Commit do build (`git rev-parse HEAD`); vazio se compilado fora do git.
pub const BUILD_COMMIT: &str = env!("CTOWER_COMMIT");
