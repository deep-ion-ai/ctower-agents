// Gerado por `cargo test -p cthubd --features ts export_ts` a partir de
// `crates/cthubd/src/methods.rs`. Não editar à mão.
import type { AssignmentAdded } from "./AssignmentAdded";
import type { AssignmentRemoved } from "./AssignmentRemoved";
import type { AutonomyWip } from "./AutonomyWip";
import type { AutopilotEvent } from "./AutopilotEvent";
import type { AutopilotPolicyView } from "./AutopilotPolicyView";
import type { AutopilotStatus } from "./AutopilotStatus";
import type { BackupConfigView } from "./BackupConfigView";
import type { BudgetDecision } from "./BudgetDecision";
import type { BudgetPolicy } from "./BudgetPolicy";
import type { DeleteImpact } from "./DeleteImpact";
import type { DependencyEdge } from "./DependencyEdge";
import type { EpicDecision } from "./EpicDecision";
import type { EpicNode } from "./EpicNode";
import type { EpicPlan } from "./EpicPlan";
import type { EpicRun } from "./EpicRun";
import type { EpicRunEvent } from "./EpicRunEvent";
import type { EpicRunView } from "./EpicRunView";
import type { HubAssignment } from "./HubAssignment";
import type { HubDashboard } from "./HubDashboard";
import type { HubEnv } from "./HubEnv";
import type { HubHealth } from "./HubHealth";
import type { HubHistory } from "./HubHistory";
import type { HubInfo } from "./HubInfo";
import type { HubPlanAccounts } from "./HubPlanAccounts";
import type { HubProject } from "./HubProject";
import type { HubProjectView } from "./HubProjectView";
import type { HubProposal } from "./HubProposal";
import type { HubProposals } from "./HubProposals";
import type { HubPushDevice } from "./HubPushDevice";
import type { HubRanking } from "./HubRanking";
import type { HubReleaseStatus } from "./HubReleaseStatus";
import type { HubRuleVersion } from "./HubRuleVersion";
import type { HubRules } from "./HubRules";
import type { HubRulesHistory } from "./HubRulesHistory";
import type { HubRulesProposal } from "./HubRulesProposal";
import type { HubRulesProposals } from "./HubRulesProposals";
import type { HubStatus } from "./HubStatus";
import type { HubTree } from "./HubTree";
import type { HubUpdateStatus } from "./HubUpdateStatus";
import type { HubWebSession } from "./HubWebSession";
import type { HubWorkItem } from "./HubWorkItem";
import type { HubWorkItemBody } from "./HubWorkItemBody";
import type { MorningDigest } from "./MorningDigest";
import type { ProjectAdded } from "./ProjectAdded";
import type { ProjectDeleted } from "./ProjectDeleted";
import type { ProposalDecided } from "./ProposalDecided";
import type { PushRemoved } from "./PushRemoved";
import type { PushTestResult } from "./PushTestResult";
import type { PushVapidKey } from "./PushVapidKey";
import type { QuarantineDiscarded } from "./QuarantineDiscarded";
import type { QuarantineGroup } from "./QuarantineGroup";
import type { RecoveryGenerated } from "./RecoveryGenerated";
import type { RecoveryStatus } from "./RecoveryStatus";
import type { SecretChanged } from "./SecretChanged";
import type { SecretsList } from "./SecretsList";
import type { TrackerIssueAdded } from "./TrackerIssueAdded";
import type { WebSessionsRevoked } from "./WebSessionsRevoked";

/** Métodos por escopo: `ui` (o app desktop, o /ws) e `web` (o /web/ws). */
export const HUB_METHOD_SCOPES = {
  ui: [
    "hub.info",
    "hub.status",
    "hub.projects.list",
    "hub.projects.add",
    "hub.projects.update",
    "hub.projects.move",
    "hub.projects.delete",
    "hub.projects.impact",
    "hub.tree.get",
    "hub.dashboard.get",
    "hub.envs.list",
    "hub.envs.set_autonomous",
    "hub.plan.accounts",
    "hub.connections.list",
    "hub.connections.add",
    "hub.connections.update",
    "hub.connections.delete",
    "hub.connections.test",
    "hub.assignments.list",
    "hub.assignments.add",
    "hub.assignments.remove",
    "hub.quarantine.list",
    "hub.quarantine.discard",
    "hub.workitem.history",
    "hub.workitem.body",
    "hub.workitem.pr_feedback",
    "hub.workitem.set_status",
    "hub.workitem.set_priority",
    "hub.workitem.set_model",
    "hub.workitem.move",
    "hub.ranking.get",
    "hub.health.get",
    "hub.proposals.list",
    "hub.proposals.decide",
    "hub.rules.get",
    "hub.rules.set",
    "hub.rules.import",
    "hub.rules.history",
    "hub.rules.proposals.list",
    "hub.rules.proposals.decide",
    "hub.web.sessions.list",
    "hub.web.sessions.revoke",
    "hub.push.vapid_key",
    "hub.push.subscribe",
    "hub.push.list",
    "hub.push.update",
    "hub.push.remove",
    "hub.push.test",
    "hub.env.call",
    "hub.update.get",
    "hub.update.check",
    "hub.update.start",
    "hub.update.rollback",
    "hub.logs.tail",
    "hub.logs.follow",
    "hub.logs.unfollow",
    "hub.autopilot.status",
    "hub.autopilot.events",
    "hub.autopilot.pause",
    "hub.autopilot.resume",
    "hub.autopilot.digest",
    "hub.autopilot.wip",
    "hub.autopilot.deps.set",
    "hub.autopilot.set",
    "hub.epic_run.list",
    "hub.epic_run.status",
    "hub.epic_run.events",
    "hub.epic_run.start",
    "hub.epic_run.approve",
    "hub.epic_run.replan",
    "hub.epic_run.pause",
    "hub.epic_run.interrupt",
    "hub.epic_run.resume",
    "hub.epic_run.cancel",
    "hub.epic_run.set_parallel",
    "hub.epic_run.set_code_review",
    "hub.epic_run.decide",
    "hub.epic_run.set_node",
    "hub.epic_run.handback",
    "hub.autopilot.policy.get",
    "hub.autopilot.policy.set",
    "hub.autopilot.policy.preview",
    "hub.backup.get",
    "hub.backup.set",
    "hub.backup.test",
    "hub.backup.run",
    "hub.backup.remote.list",
    "hub.recovery.status",
    "hub.recovery.generate",
    "hub.recovery.verify",
    "hub.secrets.list",
    "hub.secrets.set",
    "hub.secrets.delete",
    "hub.secrets.rotate_pending_dismiss",
    "hub.tracker.add_issue",
    "hub.release.status",
  ],
  web: [
    "hub.info",
    "hub.status",
    "hub.projects.list",
    "hub.tree.get",
    "hub.dashboard.get",
    "hub.envs.list",
    "hub.plan.accounts",
    "hub.assignments.list",
    "hub.quarantine.list",
    "hub.workitem.history",
    "hub.workitem.body",
    "hub.workitem.set_status",
    "hub.workitem.set_priority",
    "hub.workitem.set_model",
    "hub.ranking.get",
    "hub.health.get",
    "hub.proposals.list",
    "hub.proposals.decide",
    "hub.rules.get",
    "hub.rules.history",
    "hub.rules.proposals.list",
    "hub.web.sessions.list",
    "hub.web.sessions.revoke",
    "hub.push.vapid_key",
    "hub.push.subscribe",
    "hub.push.list",
    "hub.push.update",
    "hub.push.remove",
    "hub.push.test",
    "hub.env.call",
    "hub.update.get",
    "hub.logs.tail",
    "hub.logs.follow",
    "hub.logs.unfollow",
    "hub.autopilot.status",
    "hub.autopilot.events",
    "hub.autopilot.pause",
    "hub.autopilot.resume",
    "hub.autopilot.digest",
    "hub.autopilot.wip",
    "hub.autopilot.deps.set",
    "hub.epic_run.list",
    "hub.epic_run.status",
    "hub.epic_run.events",
    "hub.epic_run.start",
    "hub.epic_run.approve",
    "hub.epic_run.replan",
    "hub.epic_run.pause",
    "hub.epic_run.interrupt",
    "hub.epic_run.resume",
    "hub.epic_run.cancel",
    "hub.epic_run.set_parallel",
    "hub.epic_run.set_code_review",
    "hub.epic_run.decide",
    "hub.epic_run.set_node",
    "hub.epic_run.handback",
    "hub.tracker.add_issue",
    "hub.release.status",
  ],
} as const;

/** Parâmetros (`null`: nenhum) e resultado de cada método do hub. */
export interface HubMethods {
  "hub.info": { params: null; result: HubInfo };
  "hub.status": { params: null; result: HubStatus };
  "hub.projects.list": { params: null; result: HubProjectView[] };
  "hub.projects.add": { params: { [key: string]: unknown }; result: ProjectAdded };
  "hub.projects.update": { params: { owner: string; repo: string; [key: string]: unknown }; result: HubProject };
  "hub.projects.move": { params: { owner: string; repo: string; before?: { owner: string; repo: string } | null; after?: { owner: string; repo: string } | null }; result: HubProject };
  "hub.projects.delete": { params: { owner: string; repo: string; confirm?: boolean | null }; result: ProjectDeleted };
  "hub.projects.impact": { params: { owner: string; repo: string }; result: DeleteImpact };
  "hub.tree.get": { params: { owner: string; repo: string }; result: HubTree };
  "hub.dashboard.get": { params: null; result: HubDashboard };
  "hub.envs.list": { params: null; result: HubEnv[] };
  "hub.envs.set_autonomous": { params: { env: string; autonomous: boolean }; result: HubEnv | null };
  "hub.plan.accounts": { params: null; result: HubPlanAccounts };
  "hub.connections.list": { params: null; result: unknown[] };
  "hub.connections.add": { params: { name: string; kind: "github" | "gitlab" | "jira"; endpoint?: string; credential: unknown; options?: { [key: string]: unknown } }; result: unknown };
  "hub.connections.update": { params: { id: number; name?: string | null; endpoint?: string | null; credential?: unknown; options?: { [key: string]: unknown } | null }; result: unknown };
  "hub.connections.delete": { params: { id: number }; result: unknown };
  "hub.connections.test": { params: { id: number }; result: unknown };
  "hub.assignments.list": { params: { env?: string | null; owner?: string | null; repo?: string | null } | null; result: HubAssignment[] };
  "hub.assignments.add": { params: { owner: string; repo: string; env: string; repo_path?: string | null }; result: AssignmentAdded };
  "hub.assignments.remove": { params: { owner: string; repo: string; env: string; confirm?: boolean | null }; result: AssignmentRemoved };
  "hub.quarantine.list": { params: null; result: QuarantineGroup[] };
  "hub.quarantine.discard": { params: { owner: string; repo: string; confirm?: boolean | null }; result: QuarantineDiscarded };
  "hub.workitem.history": { params: { owner: string; repo: string; number: number; limit?: number | null; before_hlc?: number | null; include_sub_issues?: boolean | null }; result: HubHistory };
  "hub.workitem.body": { params: { owner: string; repo: string; number: number }; result: HubWorkItemBody };
  "hub.workitem.pr_feedback": { params: { owner: string; repo: string; number: number }; result: { prs: unknown } };
  "hub.workitem.set_status": { params: { owner: string; repo: string; number: number; status: string; reason?: string | null }; result: HubWorkItem };
  "hub.workitem.set_priority": { params: { owner: string; repo: string; number: number; priority: "P0" | "P1" | "P2" | "P3" | null; reason?: string | null }; result: HubWorkItem };
  "hub.workitem.set_model": { params: { owner: string; repo: string; number: number; model?: string | null; effort?: string | null; reason?: string | null }; result: HubWorkItem };
  "hub.workitem.move": { params: { owner: string; repo: string; number: number; before?: number | null; after?: number | null }; result: HubWorkItem };
  "hub.ranking.get": { params: { owner?: string | null; repo?: string | null; priority?: string | null; limit?: number | null } | null; result: HubRanking };
  "hub.health.get": { params: { owner?: string | null; repo?: string | null; min_severity?: string | null } | null; result: HubHealth };
  "hub.proposals.list": { params: { state?: string | null; owner?: string | null; repo?: string | null; number?: number | null; limit?: number | null } | null; result: HubProposals };
  "hub.proposals.decide": { params: { id?: number | null; ids?: number[] | null; approve: boolean; note?: string | null; value?: unknown }; result: HubProposal | ProposalDecided[] };
  "hub.rules.get": { params: { owner: string; repo: string; epic_number?: number | null }; result: HubRules };
  "hub.rules.set": { params: { owner: string; repo: string; epic_number?: number | null; content: string; base_version?: number | null; note?: string | null }; result: HubRuleVersion };
  "hub.rules.import": { params: { owner: string; repo: string; epic_number?: number | null; content: string; base_version?: number | null; note?: string | null }; result: HubRuleVersion };
  "hub.rules.history": { params: { owner: string; repo: string; epic_number?: number | null; limit?: number | null }; result: HubRulesHistory };
  "hub.rules.proposals.list": { params: { state?: string | null; owner?: string | null; repo?: string | null; epic_number?: number | null; limit?: number | null } | null; result: HubRulesProposals };
  "hub.rules.proposals.decide": { params: { id: number; approve: boolean; note?: string | null }; result: HubRulesProposal };
  "hub.web.sessions.list": { params: null; result: HubWebSession[] };
  "hub.web.sessions.revoke": { params: { id?: string | null; all?: boolean | null }; result: WebSessionsRevoked };
  "hub.push.vapid_key": { params: null; result: PushVapidKey };
  "hub.push.subscribe": { params: { endpoint: string; keys: { p256dh: string; auth: string }; label?: string | null; events?: unknown }; result: HubPushDevice };
  "hub.push.list": { params: null; result: HubPushDevice[] };
  "hub.push.update": { params: { id: number; events?: unknown }; result: HubPushDevice };
  "hub.push.remove": { params: { id: number }; result: PushRemoved };
  "hub.push.test": { params: { id: number }; result: PushTestResult };
  "hub.env.call": { params: { env: string; method: string; params?: { [key: string]: unknown } | null }; result: unknown };
  "hub.update.get": { params: null; result: HubUpdateStatus };
  "hub.update.check": { params: null; result: HubUpdateStatus };
  "hub.update.start": { params: { force?: boolean | null } | null; result: unknown };
  "hub.update.rollback": { params: { force?: boolean | null } | null; result: unknown };
  "hub.logs.tail": { params: { level?: string | null; q?: string | null; since?: number | null; until?: number | null; limit?: number | null; before?: number | null } | null; result: unknown };
  "hub.logs.follow": { params: { level?: string | null; q?: string | null; since?: number | null; until?: number | null }; result: {} };
  "hub.logs.unfollow": { params: null; result: {} };
  "hub.autopilot.status": { params: null; result: AutopilotStatus };
  "hub.autopilot.events": { params: { limit?: number | null; before?: number | null } | null; result: AutopilotEvent[] };
  "hub.autopilot.pause": { params: null; result: unknown };
  "hub.autopilot.resume": { params: null; result: unknown };
  "hub.autopilot.digest": { params: { limit?: number | null } | null; result: MorningDigest };
  "hub.autopilot.wip": { params: { owner?: string | null; repo?: string | null } | null; result: AutonomyWip[] };
  "hub.autopilot.deps.set": { params: { owner: string; repo: string; action: string; [key: string]: unknown }; result: { dependencies: DependencyEdge[] } };
  "hub.autopilot.set": { params: { [key: string]: unknown }; result: unknown };
  "hub.epic_run.list": { params: { owner?: string | null; repo?: string | null; epic?: number | null; ended_since?: number | null } | null; result: EpicRun[] };
  "hub.epic_run.status": { params: { run_id: number }; result: EpicRunView };
  "hub.epic_run.events": { params: { run_id: number; after?: number | null; limit?: number | null }; result: EpicRunEvent[] };
  "hub.epic_run.start": { params: { owner: string; repo: string; epic: number; env_id: string; max_parallel?: number | null; plan_model?: string | null; plan_effort?: string | null }; result: EpicRun };
  "hub.epic_run.approve": { params: { run_id: number; version: number; max_parallel?: number | null; code_review?: boolean | null }; result: EpicRun };
  "hub.epic_run.replan": { params: { run_id: number; reason?: string | null }; result: EpicRun };
  "hub.epic_run.pause": { params: { run_id: number; reason?: string | null }; result: EpicRun };
  "hub.epic_run.interrupt": { params: { run_id: number }; result: EpicRun };
  "hub.epic_run.resume": { params: { run_id: number }; result: EpicRun };
  "hub.epic_run.cancel": { params: { run_id: number }; result: EpicRun };
  "hub.epic_run.set_parallel": { params: { run_id: number; max_parallel: number }; result: EpicRun };
  "hub.epic_run.set_code_review": { params: { run_id: number; code_review: boolean }; result: EpicRun };
  "hub.epic_run.decide": { params: { decision_id: number; answer: string; blocker?: number | null }; result: EpicDecision };
  "hub.epic_run.set_node": { params: { run_id: number; number: number; model?: string | null; effort?: string | null }; result: EpicPlan };
  "hub.epic_run.handback": { params: { run_id: number; number: number }; result: EpicNode };
  "hub.autopilot.policy.get": { params: { account: string }; result: AutopilotPolicyView };
  "hub.autopilot.policy.set": { params: { account: string; policy?: BudgetPolicy | null }; result: AutopilotPolicyView };
  "hub.autopilot.policy.preview": { params: { account: string; policy: BudgetPolicy }; result: BudgetDecision };
  "hub.backup.get": { params: null; result: BackupConfigView };
  "hub.backup.set": { params: { [key: string]: unknown }; result: BackupConfigView };
  "hub.backup.test": { params: { [key: string]: unknown } | null; result: { ok: boolean; error?: string } };
  "hub.backup.run": { params: { what?: string | null } | null; result: BackupConfigView };
  "hub.backup.remote.list": { params: { [key: string]: unknown } | null; result: unknown };
  "hub.recovery.status": { params: null; result: RecoveryStatus };
  "hub.recovery.generate": { params: null; result: RecoveryGenerated };
  "hub.recovery.verify": { params: { key: string }; result: unknown };
  "hub.secrets.list": { params: null; result: SecretsList };
  "hub.secrets.set": { params: { scope: "global" | { project: string }; name: string; kind: "secret" | "variable"; value: string }; result: SecretChanged };
  "hub.secrets.delete": { params: { scope: "global" | { project: string }; name: string }; result: SecretChanged | {} };
  "hub.secrets.rotate_pending_dismiss": { params: null; result: {} };
  "hub.tracker.add_issue": { params: { owner: string; repo: string; key: string | number }; result: TrackerIssueAdded };
  "hub.release.status": { params: { owner: string; repo: string }; result: HubReleaseStatus };
}

export type HubMethod = keyof HubMethods;
