// Modo web (épico #205): a FakeDaemonApi no modo web (`asWeb`) com o hub
// respondendo a visão consolidada, para os testes de componente da web.
import { FakeDaemonApi, HUB } from "../api/fake";
import type { HubDashboard, HubDashboardWorkItem, HubSession, HubWebSession } from "../protocol/hub-gen";
import {
  HUB_DASHBOARD,
  NO_HEALTH,
  NO_PROPOSALS,
  hubAssignment,
  hubEnv,
  hubInfo,
  hubProject,
  hubProjectView,
  hubStatus,
} from "./fixtures";

export const now = Date.now();
export const ENV = "daemon-wsl";

export function hubItem(w: Partial<HubDashboardWorkItem> = {}): HubDashboardWorkItem {
  return {
    title: "WebDaemonApi",
    pending_decisions: 0,
    id: 11,
    project_id: 1,
    gh_number: 201,
    kind: "issue",
    status: "em_execucao",
    branch: "feat/201-ui-web-daemon-api",
    created_at: now - 3600_000,
    closed_at: null,
    priority: null,
    rank: null,
    ...w,
  };
}

export function hubSession(s: Partial<HubSession> = {}): HubSession {
  return {
    id: "s-201",
    env_id: ENV,
    work_item_id: 11,
    tool: "claude",
    tool_session_id: null,
    cwd: "/w/ct-201",
    state: "running",
    exit_code: null,
    started_at: now - 30 * 60_000,
    last_event_at: now - 5 * 60_000,
    ended_at: null,
    model: "opus",
    mode: "native",
    rc_session_id: null,
    ...s,
  };
}

export function webApi(d: Partial<HubDashboard> = {}, items: HubDashboardWorkItem[] = [hubItem()]) {
  const project = hubProject();
  const dash: HubDashboard = {
    ...HUB_DASHBOARD,
    projects: [{ project, work_items: items }],
    live_sessions: [hubSession()],
    envs: [hubEnv({ last_seen_at: now, online: true })],
    ...d,
  };
  return new FakeDaemonApi()
    .asWeb()
    .onHub("hub.info", hubInfo({ protocol: "1.3" }))
    .onHub("hub.status", hubStatus())
    .onHub("hub.projects.list", [hubProjectView(project, [hubAssignment()])])
    .onHub("hub.envs.list", [hubEnv({ last_seen_at: now, online: true })])
    .onHub("hub.dashboard.get", () => dash)
    .onHub("hub.proposals.list", NO_PROPOSALS)
    .onHub("hub.health.get", NO_HEALTH)
    .onHub("hub.tree.get", { project, roots: [], sync: null })
    .onHub("hub.web.sessions.list", [] as HubWebSession[]);
}

/** Nenhuma chamada a ambiente: na fase 1 tudo vem do hub. */
export const envCalls = (api: FakeDaemonApi) => api.calls.filter((c) => c.env !== HUB);

