//! Domínio do Control Tower: tipos, máquinas de estado e regras puras (sem
//! I/O, exceto os arquivos privados de `files`).

pub mod autonomy;
pub mod branch;
mod clock;
mod credentials;
pub mod docs;
pub mod files;
pub mod guard;
mod logs;
pub mod plan;
#[macro_use]
mod model;
mod repo_url;
pub mod session_templates;
pub mod tracker;

pub use autonomy::{AlignStrategy, AutonomyConfig, AutonomyReport, AutonomyState};
pub use clock::{civil_from_days, hlc, now_ms, parse_iso8601, utc_stamp};
pub use credentials::{
    CredentialApplied, CredentialOverride, CredentialOverrides, SharedCredentials,
};
pub use docs::DocsConfig;
pub use logs::LogsConfig;
pub use model::*;
pub use repo_url::{github_repo_from_url, repo_from_url, RepoRef};
pub use session_templates::SessionTemplates;
pub use tracker::{
    change_label, change_ref, change_short, change_url, default_type_map, glab_cli_prefix,
    is_assignee, issue_view_cmd, item_key, item_ref, item_short, item_url, jira_cli_prefix,
    same_identity, supported_combination, ForgeConfig, ForgeKind, GitlabConfig, GitlabReadiness,
    JiraConfig, JiraExtraField, JiraFieldInfo, JiraReadiness, JiraTypeRole, Sprint, SprintState,
    TrackerConfig, TrackerKind, TrackerProgress, TrackerReadiness, TrackerRef, ACTIVE_SPRINT_JQL,
    DEFAULT_MAX_ISSUES, MAX_ISSUES_CEILING, MAX_JIRA_EXTRA_FIELDS,
};

// "Adicionar issue pelo ID" (#148).
pub use tracker::TrackerIssueAdded;

/// Versão do Control Tower (a do workspace).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Commit do build (`git rev-parse HEAD`); vazio se compilado fora do git.
pub const BUILD_COMMIT: &str = env!("CTOWER_COMMIT");
