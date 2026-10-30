// Construtores de dados do protocolo para os testes.
import type {
  Dashboard,
  DashboardWorkItem,
  EnvInfo,
  HubConfigStatus,
  Project,
  RemoteControlStatus,
  Session,
  SyncState,
} from "../protocol/gen";
import type {
  BackupStatus,
  DocsBackupStatus,
  HubAssignment,
  HubDashboard,
  HubEnv,
  HealthSignal,
  HubHealth,
  HubInfo,
  HubProject,
  HubProposal,
  HubProposals,
  HubProjectView,
  HubStatus,
  RecoveryStatus,
} from "../protocol/hub-gen";

export const NOW = Date.UTC(2026, 9, 15, 15, 0, 0);
export const MIN = 60_000;

export function project(p: Partial<Project> = {}): Project {
  return {
    id: 1,
    name: "ctower-agents",
    gh_owner: "deep-ion-ai",
    gh_repo: "ctower-agents",
    repo_path: "/home/user/projetos/ctower-agents",
    base_branch: "develop",
    protected_branches: ["develop", "main"],
    created_at: 0,
    config_rev: null,
    unassigned_at: null,
    location_state: null,
    location_error: null,
    applied_rev: null,
    kind: "github",
    tracker: { kind: "github" },
    forge: { kind: "github" },
    branch_pattern: null,
    attribution: true,
    sync_role: false,
    docs: { enabled: false, project_dirs: [], epic_dirs: [] },
    ...p,
  };
}

/** Projeto Jira + GitLab do exemplo no ambiente (#352); a barra final da URL do Jira é de propósito. */
export function jiraProject(p: Partial<Project> = {}): Project {
  return project({
    id: 7,
    name: "app",
    gh_owner: "grupo/projeto",
    gh_repo: "app",
    repo_path: "/home/user/projetos/app",
    tracker: {
      kind: "jira",
      url: "https://jira.example.com/",
      project_key: "PROJ",
      jql: "",
      credential_cmd: "pass show jira | head -1",
      config_file: "",
      epic_link_field: "customfield_10601",
      epic_name_field: "customfield_10604",
      type_map: { Epic: "epic", Bug: "bug" },
    },
    forge: {
      kind: "gitlab",
      host: "gitlab.example.com",
      path: "grupo/projeto/app",
      clone_url: "https://gitlab.example.com/grupo/projeto/app.git",
      credential_cmd: "pass show gitlab | head -1",
    },
    branch_pattern: "{key}",
    attribution: false,
    sync_role: true,
    ...p,
  });
}

/** `env.info.hub_config` de um ambiente sem hub. */
export const HUB_CONFIG_UNLINKED: HubConfigStatus = {
  linked: false,
  config_rev: null,
  pulled_at: null,
  age_ms: null,
  last_attempt_at: null,
  last_error: null,
  failures: 0,
  legacy_hub: false,
  agent_token: false,
  max_age_ms: null,
  stale: false,
};

/** `env.info.remote_control` de um ambiente sem impedimento (#197). */
export const RC_AVAILABLE: RemoteControlStatus = { available: true, reason: null };

/** `env.info` de um ambiente sem hub, com `docs.dir` (protocolo 2.8, #444) e sem docs. */
export function envInfo(p: Partial<EnvInfo> = {}): EnvInfo {
  return {
    protocol: "2.8",
    version: "0.14.0",
    commit: "",
    hostname: "wsl",
    worktrees_root: "/home/wsl/worktrees",
    env_id: "daemon-wsl",
    env_name: "wsl",
    hub_sync: null,
    hub_config: HUB_CONFIG_UNLINKED,
    update: null,
    remote_control: RC_AVAILABLE,
    docs: { root: null, reachable: false },
    ...p,
  };
}

export function workItem(w: Partial<DashboardWorkItem> = {}): DashboardWorkItem {
  const n = w.gh_number ?? 31;
  return {
    id: n,
    project_id: 1,
    gh_number: n,
    kind: "issue",
    status: "em_execucao",
    branch: `feat/${n}-item-${n}`,
    worktree_path: `/home/user/worktrees/ctower-agents/feat__${n}-item-${n}`,
    created_at: 0,
    closed_at: null,
    title: `Item ${n}`,
    parent_number: null,
    pending_decisions: 0,
    ...w,
  };
}

export function session(s: Partial<Session> = {}): Session {
  return {
    id: "11111111-1111-1111-1111-111111111111",
    work_item_id: 31,
    tool: "claude",
    tool_session_id: null,
    transcript_path: null,
    backup_path: null,
    cwd: "/w",
    launch_cmd: [],
    state: "running",
    exit_code: null,
    pid: 10,
    started_at: NOW - 47 * MIN,
    last_event_at: NOW - MIN,
    ended_at: null,
    model: null,
    mode: "native",
    rc_session_id: null,
    ...s,
  };
}

export function sync(s: Partial<SyncState> = {}): SyncState {
  return { project_id: 1, last_attempt_at: NOW - MIN, last_ok_at: NOW - MIN, last_error: null, ...s };
}

export function dashboard(d: Partial<Dashboard> = {}, items: DashboardWorkItem[] = []): Dashboard {
  return {
    projects: [{ project: project(), work_items: items }],
    live_sessions: [],
    interrupted_sessions: [],
    failed_sessions: [],
    sync: [sync()],
    gestor_work_item_id: null,
    gestor_scopes: {},
    ...d,
  };
}

// Hub (cthubd): construtores do /ws do hub.

export const HUB_BACKUP: BackupStatus = {
  dir: "/Volumes/Backup/cthub",
  interval_hours: 6,
  keep: 28,
  newest_at: NOW - 30 * MIN,
  last_ok_at: NOW - 30 * MIN,
  last_error: null,
  same_device: false,
  remote: {
    kind: "none",
    target: "",
    encrypt: true,
    credentials_configured: false,
    last_ok_at: null,
    last_name: null,
    last_size: null,
    last_error: null,
    failures: 0,
  },
};

/** Backup dos docs (#442) desligado: hub sem `docs_dir`. */
export const HUB_DOCS_BACKUP: DocsBackupStatus = {
  docs_dir: null,
  interval_hours: 24,
  keep: 7,
  max_mb: 1024,
  newest_at: null,
  newest_name: null,
  last_ok_at: null,
  last_size: null,
  last_error: null,
  last_skipped: null,
  too_large_bytes: null,
  remote: { last_ok_at: null, last_name: null, last_size: null, last_error: null, failures: 0 },
};

export function hubInfo(p: Partial<HubInfo> = {}): HubInfo {
  return { protocol: "1.0", version: "0.3.0", commit: "", name: "mac-mini", config_rev: 7, backup: HUB_BACKUP, docs_dir: null, ...p };
}

export const NO_PROPOSALS: HubProposals = { proposals: [], truncated: false };

export function hubProposal(p: Partial<HubProposal> = {}): HubProposal {
  return {
    id: 1,
    uid: "u1",
    kind: "priority",
    owner: "deep-ion-ai",
    repo: "ctower-agents",
    number: 215,
    ref: "deep-ion-ai/ctower-agents#215",
    title: "prioridade e ordem",
    current: { priority: null },
    proposed: { priority: "P0" },
    justification: "bloqueia o resto do épico",
    state: "pendente",
    env_id: "daemon-wsl",
    env_name: "wsl-pc",
    session_id: "s-gestor-1234",
    created_at: NOW - 5 * MIN,
    decided_at: null,
    decision_note: null,
    applied_event_uid: null,
    ...p,
  };
}

export const NO_HEALTH: HubHealth = { generated_at: NOW, signals: [], counts: { critico: 0, aviso: 0, info: 0 } };

export function healthSignal(p: Partial<HealthSignal> = {}): HealthSignal {
  return {
    kind: "bloqueado",
    severity: "critico",
    owner: "deep-ion-ai",
    repo: "ctower-agents",
    number: 176,
    ref: "deep-ion-ai/ctower-agents#176",
    title: "atribuições",
    priority: "P1",
    since: NOW - 8 * 24 * 60 * MIN,
    value: "8 dias",
    threshold: "7 dias",
    detail: "bloqueado: aguardando revisão externa",
    env: null,
    ...p,
  };
}

export function hubStatus(p: Partial<HubStatus> = {}): HubStatus {
  return { warnings: [], backup: HUB_BACKUP, quarantine: [], ...p };
}

/** `hub.recovery.status` com a chave configurada e verificada agora. */
export function recoveryStatus(p: Partial<RecoveryStatus> = {}): RecoveryStatus {
  const now = Date.now();
  return {
    configured: true,
    public_key: "age1recuperacaoxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx",
    pending: false,
    pending_public_key: null,
    last_verified_at: now,
    reverify_days: 90,
    next_verify_at: now + 90 * 24 * 60 * 60 * 1000,
    reverify_due: false,
    ...p,
  };
}

export function hubProject(p: Partial<HubProject> = {}): HubProject {
  return {
    id: 1,
    gh_owner: "deep-ion-ai",
    gh_repo: "ctower-agents",
    name: "ctower-agents",
    base_branch: "develop",
    protected_branches: ["develop", "main"],
    created_at: 0,
    rank: null,
    tracker: { kind: "github" },
    forge: { kind: "github" },
    branch_pattern: null,
    attribution: true,
    sync_env_id: null,
    docs: { enabled: false, project_dirs: [], epic_dirs: [] },
    ...p,
  };
}

/** Projeto Jira + GitLab do exemplo (#348), com o ambiente de sync `daemon-wsl`. */
export function jiraHubProject(p: Partial<HubProject> = {}): HubProject {
  return hubProject({
    id: 7,
    gh_owner: "grupo/projeto",
    gh_repo: "app",
    name: "app",
    tracker: {
      kind: "jira",
      url: "https://jira.example.com",
      project_key: "PROJ",
      jql: "",
      credential_cmd: "pass show jira | head -1",
      config_file: "",
      epic_link_field: "customfield_10601",
      epic_name_field: "customfield_10604",
      type_map: { Epic: "epic", Bug: "bug" },
    },
    forge: {
      kind: "gitlab",
      host: "gitlab.example.com",
      path: "grupo/projeto/app",
      clone_url: "https://gitlab.example.com/grupo/projeto/app.git",
      credential_cmd: "pass show gitlab | head -1",
    },
    branch_pattern: "{key}",
    attribution: false,
    sync_env_id: "daemon-wsl",
    ...p,
  });
}

export function hubAssignment(a: Partial<HubAssignment> = {}): HubAssignment {
  return {
    project_id: 1,
    gh_owner: "deep-ion-ai",
    gh_repo: "ctower-agents",
    env_id: "daemon-wsl",
    env_name: "wsl-pc",
    repo_path: "",
    created_at: 0,
    rev: 5,
    state: "applied",
    error: null,
    applied_rev: 5,
    reported_path: "/home/user/projetos/ctower-agents",
    reported_at: NOW - MIN,
    ...a,
  };
}

export function hubProjectView(project: HubProject, assignments: HubAssignment[] = []): HubProjectView {
  return { project, assignments, locations: [], sync: null, tracker_status: null };
}

export function hubEnv(e: Partial<HubEnv> = {}): HubEnv {
  return { env_id: "daemon-wsl", name: "wsl-pc", created_at: 0, last_seen_at: NOW - MIN, last_seq: 10, pending: 0, cleanup: null, online: false, ...e };
}

export const HUB_DASHBOARD: HubDashboard = {
  projects: [],
  live_sessions: [],
  interrupted_sessions: [],
  failed_sessions: [],
  sync: [],
  envs: [],
  gestor_work_item_id: null,
};
