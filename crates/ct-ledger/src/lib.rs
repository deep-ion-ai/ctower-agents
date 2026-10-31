//! Ledger do Control Tower: persistência SQLite de projetos, work items,
//! sessões, decisões e eventos (spec §5).

mod backup;
mod decisions;
mod error;
mod events;
mod github;
mod hub;
mod hub_config;
mod projects;
mod prune;
mod rules;
mod schema;
mod sessions;
mod sync;
mod tracker_mirror;
mod tree;
mod work_items;

pub use backup::{db_user_version, snapshot_db};
pub use decisions::{Decision, NewDecision};
pub use error::{LedgerError, Result};
pub use events::{Event, NewEvent};
pub use github::{
    gh_epic_merged_uid, gh_issue_closed_uid, gh_issue_gone_uid, gh_pr_uid, GithubApplied,
    GithubSnapshot, GoneIssue, IssueMeta, IssueRow, PrRow, SyncState, GH_ISSUE_GONE_CAUSE,
    GH_ISSUE_GONE_REASON,
};
pub use hub::{list_snapshots, rotate_snapshots, IssuedEnvToken, AGENT_TOKEN_PREFIX};
pub use hub::{
    AgentAssignment, AgentDecision, AgentDecisionFilter, AgentDecisions, AgentEnv, AgentEvent,
    AgentItemRef, AgentNode, AgentOverview, AgentPr, AgentProject, AgentProjectSummary,
    AgentSession, AgentWorkItem,
};
pub use hub::{
    AssignmentAdded, AssignmentFilter, AssignmentState, DeleteImpact, HubAssignment,
    HubProjectPatch, HubTrackerStatus, LegacyAdoption, NewHubProject, ProjectAdded,
    ProjectProvider, QuarantineGroup,
};
pub use hub::{
    ChannelStates, HubPushDevice, NewPushSubscription, PushEvent, PushNotice, PushTarget,
    CHANNEL_STALE_MS, PUSH_FAILURES_MAX, PUSH_SUBSCRIPTIONS_MAX,
};
pub use hub::{
    CleanupSummary, DecisionCounts, DecisionScope, EnvAssignmentRev, EnvPubkey, EnvSecretsInput,
    Hub, HubDashboard, HubDashboardProject, HubDashboardWorkItem, HubDecision, HubEnv, HubEvent,
    HubHistory, HubLocation, HubProject, HubProjectView, HubSession, HubTree, HubTreeItem,
    HubTreeNode, HubWorkItem, IngestResult, SharedHub, WorkItemSummary,
};
pub use hub::{
    HealthCounts, HealthFilter, HealthKind, HealthSignal, HealthThresholds, HubHealth, Severity,
};
pub use hub::{
    HubProposal, HubProposals, NewProposal, ProposalDecided, ProposalFilter, PROPOSAL_PENDING_MAX,
    PROPOSAL_TTL_MS,
};
pub use hub::{HubRanking, HubRankingItem, Placement, RankingFilter, RANKING_LIMIT, RANKING_MAX};
pub use hub::{
    HubRuleVersion, HubRules, HubRulesHistory, HubRulesProposal, HubRulesProposals,
    RulesProposalFilter,
};
pub use hub::{HubWebSession, IssuedWebSession, WEB_SESSION_TOUCH_MS};
pub use hub_config::{
    ConfigApplied, ConfigDone, ConfigProject, ConfigRemoved, ConfigResponse, DoneResponse,
};
#[cfg(test)]
pub(crate) use projects::ProjectPatch;
pub use projects::{NewProject, Project};
pub use prune::{PruneReport, PruneSkip, PrunedSession};
pub use rules::{
    check_rules_size, ConfigRules, EpicRuleText, RuleText, RulesProposalChange, SessionRules,
    RULES_MAX_BYTES, RULES_REASON_MAX, RULES_WARN_BYTES,
};
pub use sessions::{NewSession, Session};
pub use sync::{
    new_uid, project_key, work_item_key, DecisionChange, EventChange, IssueMirrorChange,
    LocationState, MirrorPr, OutboxEntry, OutboxStats, PrMirrorChange, ProjectChange,
    SessionChange, SyncChange, TrackerStatusChange, WorkItemChange,
};
pub use tree::{ProjectTree, TreeNode};
pub use work_items::WorkItem;

use rusqlite::{types::Type, Connection, Row};
use std::path::Path;
use std::str::FromStr;
use std::time::Duration;

const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

pub struct Ledger {
    conn: Connection,
    /// Ambiente dono deste ledger (`<config_dir>/env_id`).
    env_id: String,
}

impl Ledger {
    pub fn open(path: &Path, env_id: &str) -> Result<Self> {
        let conn = Connection::open(path)?;
        // Antes do WAL: trocar o journal_mode também pode esbarrar em lock.
        conn.busy_timeout(BUSY_TIMEOUT)?;
        conn.query_row("PRAGMA journal_mode=WAL", [], |_| Ok(()))?;
        Self::init(conn, env_id)
    }

    /// Ledger em memória com um `env_id` aleatório (testes).
    pub fn open_in_memory() -> Result<Self> {
        Self::init(
            Connection::open_in_memory()?,
            &uuid::Uuid::new_v4().to_string(),
        )
    }

    fn init(conn: Connection, env_id: &str) -> Result<Self> {
        conn.busy_timeout(BUSY_TIMEOUT)?;
        conn.pragma_update(None, "foreign_keys", true)?;
        schema::migrate(&conn)?;
        Ok(Ledger {
            conn,
            env_id: env_id.to_string(),
        })
    }
}

/// Lê uma coluna TEXT e converte via `FromStr` (enums do domínio).
pub(crate) fn parse_col<T>(row: &Row, idx: usize) -> rusqlite::Result<T>
where
    T: FromStr,
    T::Err: std::error::Error + Send + Sync + 'static,
{
    let s: String = row.get(idx)?;
    s.parse::<T>()
        .map_err(|e| rusqlite::Error::FromSqlConversionFailure(idx, Type::Text, Box::new(e)))
}

/// Como `parse_col`, para coluna que aceita nulo.
pub(crate) fn opt_parse_col<T>(row: &Row, idx: usize) -> rusqlite::Result<Option<T>>
where
    T: FromStr,
    T::Err: std::error::Error + Send + Sync + 'static,
{
    let s: Option<String> = row.get(idx)?;
    s.map(|s| s.parse::<T>())
        .transpose()
        .map_err(|e| rusqlite::Error::FromSqlConversionFailure(idx, Type::Text, Box::new(e)))
}

/// Lê o par de colunas (`tracker_kind`, `tracker_config`) a partir de `idx`.
pub(crate) fn tracker_col(row: &Row, idx: usize) -> rusqlite::Result<ct_core::TrackerConfig> {
    let (kind, json): (String, String) = (row.get(idx)?, row.get(idx + 1)?);
    ct_core::TrackerConfig::from_columns(&kind, &json)
        .map_err(|e| rusqlite::Error::FromSqlConversionFailure(idx, Type::Text, Box::new(e)))
}

/// Lê o par de colunas (`forge_kind`, `forge_config`) a partir de `idx`.
pub(crate) fn forge_col(row: &Row, idx: usize) -> rusqlite::Result<ct_core::ForgeConfig> {
    let (kind, json): (String, String) = (row.get(idx)?, row.get(idx + 1)?);
    ct_core::ForgeConfig::from_columns(&kind, &json)
        .map_err(|e| rusqlite::Error::FromSqlConversionFailure(idx, Type::Text, Box::new(e)))
}

/// Lê uma coluna TEXT contendo JSON.
pub(crate) fn json_col<T: serde::de::DeserializeOwned>(
    row: &Row,
    idx: usize,
) -> rusqlite::Result<T> {
    let s: String = row.get(idx)?;
    serde_json::from_str(&s)
        .map_err(|e| rusqlite::Error::FromSqlConversionFailure(idx, Type::Text, Box::new(e)))
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;

    pub fn ledger_with_project() -> (Ledger, Project) {
        let ledger = Ledger::open_in_memory().unwrap();
        let project = ledger
            .add_project(&NewProject {
                name: "demo".into(),
                gh_owner: "deep-ion-ai".into(),
                gh_repo: "demo".into(),
                repo_path: "/home/u/projetos/demo".into(),
                base_branch: "develop".into(),
                protected_branches: vec!["develop".into(), "main".into()],
            })
            .unwrap();
        (ledger, project)
    }
}
