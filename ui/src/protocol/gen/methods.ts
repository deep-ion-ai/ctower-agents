// Gerado por `cargo test -p ct-server --features ts export_ts` a partir de
// `ct_http::methods`. Não editar à mão.
import type { ArchivedItem } from "./ArchivedItem";
import type { AutonomyReport } from "./AutonomyReport";
import type { BranchCommits } from "./BranchCommits";
import type { CleanupPlan } from "./CleanupPlan";
import type { CleanupRun } from "./CleanupRun";
import type { CleanupStatus } from "./CleanupStatus";
import type { ConfigItem } from "./ConfigItem";
import type { ContextUsage } from "./ContextUsage";
import type { Dashboard } from "./Dashboard";
import type { DiffPatch } from "./DiffPatch";
import type { DiffSummary } from "./DiffSummary";
import type { DocsDir } from "./DocsDir";
import type { DocsFiles } from "./DocsFiles";
import type { EnvAccount } from "./EnvAccount";
import type { EnvConnection } from "./EnvConnection";
import type { EnvCredential } from "./EnvCredential";
import type { EnvInfo } from "./EnvInfo";
import type { Event } from "./Event";
import type { GestorScope } from "./GestorScope";
import type { HubSyncResult } from "./HubSyncResult";
import type { Marked } from "./Marked";
import type { MdFile } from "./MdFile";
import type { Peek } from "./Peek";
import type { PermissionRequest } from "./PermissionRequest";
import type { PrComment } from "./PrComment";
import type { Project } from "./Project";
import type { ProjectSync } from "./ProjectSync";
import type { ProjectTree } from "./ProjectTree";
import type { RecoveryItem } from "./RecoveryItem";
import type { RemoteControlEnable } from "./RemoteControlEnable";
import type { Review } from "./Review";
import type { Session } from "./Session";
import type { SessionUsage } from "./SessionUsage";
import type { SharedFile } from "./SharedFile";
import type { Shell } from "./Shell";
import type { StatuslineInstall } from "./StatuslineInstall";
import type { TokenStatus } from "./TokenStatus";
import type { TrackerIssueAdded } from "./TrackerIssueAdded";
import type { TranscriptTail } from "./TranscriptTail";
import type { UpdateStatus } from "./UpdateStatus";

/** Métodos por escopo: `ui` (o /ws), `web` e `channel` (o canal do hub). */
export const METHOD_SCOPES = {
  ui: [
    "env.info",
    "env.update",
    "env.rollback",
    "env.check_update",
    "env.config.get",
    "env.config.set",
    "env.credentials.get",
    "env.credentials.set",
    "env.connections.get",
    "env.connections.set",
    "env.statusline.install",
    "account.get",
    "projects.list",
    "dashboard.get",
    "recovery.list",
    "recovery.archived",
    "transcript.tail",
    "session.usage",
    "session.context",
    "session.start",
    "session.start_project",
    "session.start_release",
    "session.start_gestor",
    "autonomy.align",
    "sync.now",
    "tracker.issue_body",
    "tracker.add_issue",
    "github.status",
    "github.set_token",
    "github.clear_token",
    "github.pr.comment",
    "tree.get",
    "docs.dir",
    "file.read",
    "files.shared",
    "docs.list",
    "git.diff",
    "git.diff_file",
    "git.commits",
    "review.get",
    "events.list",
    "review.mark_reviewed",
    "session.resume",
    "session.continue",
    "session.remote_control.enable",
    "session.get",
    "session.archive",
    "session.kill",
    "session.peek",
    "session.permission.watch",
    "session.permission.list",
    "session.permission.respond",
    "shell.open",
    "shell.list",
    "shell.close",
    "session.attach",
    "session.detach",
    "session.input",
    "session.resize",
    "cleanup.status",
    "cleanup.plan",
    "cleanup.run",
    "logs.tail",
    "logs.follow",
    "logs.unfollow",
    "hub.config.changed",
    "hub.sync.now",
  ],
  web: [
    "env.info",
    "projects.list",
    "dashboard.get",
    "recovery.list",
    "recovery.archived",
    "transcript.tail",
    "session.usage",
    "session.context",
    "session.start",
    "session.start_project",
    "session.start_release",
    "session.start_gestor",
    "autonomy.align",
    "tracker.issue_body",
    "tracker.add_issue",
    "tree.get",
    "file.read",
    "files.shared",
    "docs.list",
    "git.diff",
    "git.diff_file",
    "git.commits",
    "review.get",
    "events.list",
    "review.mark_reviewed",
    "session.resume",
    "session.continue",
    "session.remote_control.enable",
    "session.get",
    "session.archive",
    "session.kill",
    "logs.tail",
    "hub.config.changed",
    "hub.sync.now",
  ],
  channel: [
    "env.info",
    "projects.list",
    "dashboard.get",
    "recovery.list",
    "recovery.archived",
    "transcript.tail",
    "session.usage",
    "session.context",
    "session.start",
    "session.start_project",
    "session.start_release",
    "session.start_gestor",
    "autonomy.align",
    "tracker.issue_body",
    "tracker.add_issue",
    "tree.get",
    "file.read",
    "files.shared",
    "docs.list",
    "git.diff",
    "git.diff_file",
    "git.commits",
    "review.get",
    "events.list",
    "review.mark_reviewed",
    "session.resume",
    "session.continue",
    "session.remote_control.enable",
    "session.get",
    "session.archive",
    "session.kill",
    "logs.tail",
    "hub.config.changed",
    "hub.sync.now",
  ],
} as const;

/** Parâmetros (`null`: nenhum) e resultado de cada método do ambiente. */
export interface DaemonMethods {
  "env.info": { params: null; result: EnvInfo };
  "env.update": { params: { force?: boolean | null } | null; result: UpdateStatus };
  "env.rollback": { params: { force?: boolean | null } | null; result: UpdateStatus };
  "env.check_update": { params: null; result: UpdateStatus };
  "env.config.get": { params: null; result: ConfigItem[] };
  "env.config.set": { params: { key: string; value: unknown }; result: ConfigItem };
  "env.credentials.get": { params: null; result: EnvCredential[] };
  "env.credentials.set": { params: { owner: string; repo: string; tracker_cmd?: string | null; forge_cmd?: string | null }; result: EnvCredential[] };
  "env.connections.get": { params: null; result: EnvConnection[] };
  "env.connections.set": { params: { name: string; credential_cmd?: string | null }; result: EnvConnection[] };
  "env.statusline.install": { params: null; result: StatuslineInstall };
  "account.get": { params: null; result: EnvAccount };
  "projects.list": { params: { unassigned?: boolean | null } | null; result: Project[] };
  "dashboard.get": { params: null; result: Dashboard };
  "recovery.list": { params: null; result: RecoveryItem[] };
  "recovery.archived": { params: { since_ms?: number | null; limit?: number | null } | null; result: ArchivedItem[] };
  "transcript.tail": { params: { session_id: string; entries?: number | null }; result: TranscriptTail };
  "session.usage": { params: { session_id: string }; result: SessionUsage | null };
  "session.context": { params: { session_id: string }; result: ContextUsage };
  "session.start": { params: { project_id?: number | null; owner?: string | null; repo?: string | null; gh_number: number; kind?: "epic" | "issue" | null; title?: string | null; labels?: string[] | null; prompt?: string | null; autonomous?: boolean | null; override_approval?: boolean | null; cols?: number | null; rows?: number | null; model?: string | null; effort?: string | null; mode?: "native" | "subagents" | null; remote_control?: boolean | null }; result: Session };
  "session.start_project": { params: { project_id?: number | null; owner?: string | null; repo?: string | null; prompt: string; analysis?: boolean | null; cols?: number | null; rows?: number | null; model?: string | null; effort?: string | null; mode?: "native" | "subagents" | null; remote_control?: boolean | null }; result: Session };
  "session.start_release": { params: { project_id?: number | null; owner?: string | null; repo?: string | null; prompt: string; cols?: number | null; rows?: number | null; model?: string | null; effort?: string | null; mode?: "native" | "subagents" | null; remote_control?: boolean | null }; result: Session };
  "session.start_gestor": { params: { scope: GestorScope; prompt?: string | null; cols?: number | null; rows?: number | null; model?: string | null; effort?: string | null; mode?: "native" | "subagents" | null; remote_control?: boolean | null }; result: Session };
  "autonomy.align": { params: { project_id?: number | null; owner?: string | null; repo?: string | null }; result: AutonomyReport };
  "sync.now": { params: { project_id?: number | null } | null; result: ProjectSync[] };
  "tracker.issue_body": { params: { project_id?: number | null; owner?: string | null; repo?: string | null; number: number }; result: unknown };
  "tracker.add_issue": { params: { project_id?: number | null; owner?: string | null; repo?: string | null; key: string | number }; result: TrackerIssueAdded };
  "github.status": { params: null; result: TokenStatus };
  "github.set_token": { params: { token: string }; result: TokenStatus };
  "github.clear_token": { params: null; result: TokenStatus };
  "github.pr.comment": { params: { project_id?: number | null; owner?: string | null; repo?: string | null; pr: number; body: string }; result: PrComment };
  "tree.get": { params: { project_id: number }; result: ProjectTree };
  "docs.dir": { params: { project_id: number; epic?: number | null }; result: DocsDir };
  "file.read": { params: { path: string; session_id?: string | null; project_id?: number | null }; result: MdFile };
  "files.shared": { params: { session_id: string }; result: SharedFile[] };
  "docs.list": { params: { project_id: number; epic?: number | null }; result: DocsFiles };
  "git.diff": { params: { work_item_id: number; mode: "branch" | "uncommitted" | "commit"; commit?: string | null }; result: DiffSummary };
  "git.diff_file": { params: { work_item_id: number; mode: "branch" | "uncommitted" | "commit"; commit?: string | null; path: string; old_path?: string | null }; result: DiffPatch };
  "git.commits": { params: { work_item_id: number }; result: BranchCommits };
  "review.get": { params: { work_item_id: number }; result: Review };
  "events.list": { params: { work_item_id: number; before?: number | null; limit?: number | null }; result: Event[] };
  "review.mark_reviewed": { params: { work_item_id: number; pr?: number | null; decision_ids?: number[] | null }; result: Marked };
  "session.resume": { params: { session_id: string; prompt?: string | null; remote_control?: boolean | null }; result: Session };
  "session.continue": { params: { session_id: string; text?: string | null }; result: { continued: boolean } };
  "session.remote_control.enable": { params: { session_id: string }; result: RemoteControlEnable };
  "session.get": { params: { session_id: string }; result: Session & { work_item_concluded: boolean } };
  "session.archive": { params: { session_id: string }; result: {} };
  "session.kill": { params: { session_id: string }; result: {} };
  "session.peek": { params: { session_id: string; bytes?: number | null }; result: Peek };
  "session.permission.watch": { params: null; result: PermissionRequest[] };
  "session.permission.list": { params: null; result: PermissionRequest[] };
  "session.permission.respond": { params: { request_id: string; decision: string; message?: string | null }; result: unknown };
  "shell.open": { params: { work_item_id: number; cols?: number | null; rows?: number | null }; result: Shell };
  "shell.list": { params: null; result: Shell[] };
  "shell.close": { params: { shell_id: string }; result: {} };
  "session.attach": { params: { session_id: string }; result: unknown };
  "session.detach": { params: { session_id: string }; result: {} };
  "session.input": { params: { session_id: string; data: string }; result: {} };
  "session.resize": { params: { session_id: string; cols: number; rows: number }; result: {} };
  "cleanup.status": { params: null; result: CleanupStatus };
  "cleanup.plan": { params: null; result: CleanupPlan };
  "cleanup.run": { params: { work_item_id?: number | null; step?: string | null; dry_run?: boolean | null } | null; result: CleanupRun };
  "logs.tail": { params: { level?: string | null; q?: string | null; since?: number | null; until?: number | null; limit?: number | null; before?: number | null }; result: unknown };
  "logs.follow": { params: { level?: string | null; q?: string | null; since?: number | null; until?: number | null }; result: {} };
  "logs.unfollow": { params: null; result: {} };
  "hub.config.changed": { params: null; result: {} };
  "hub.sync.now": { params: null; result: HubSyncResult };
}

export type DaemonMethod = keyof DaemonMethods;
