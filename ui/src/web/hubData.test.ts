import type { HubDashboard, HubEnv, HubSession } from "../protocol/hub-gen";
import { HUB_DASHBOARD, hubAssignment, hubEnv, hubProject, hubProjectView } from "../test/fixtures";
import { dashboardsFromHub, ENV_ONLINE_MS, envStatusesFromHub, envViewsFromHub, OFFLINE } from "./hubData";

const now = 1_000_000_000;

function s(p: Partial<HubSession>): HubSession {
  return {
    id: "s1",
    env_id: "d-wsl",
    work_item_id: 1,
    tool: "claude",
    tool_session_id: null,
    cwd: "/w",
    state: "running",
    exit_code: null,
    started_at: 0,
    last_event_at: 0,
    ended_at: null,
    model: null,
    mode: "native",
    rc_session_id: null,
    ...p,
  };
}

const wi = (id: number, gh: number, project_id = 1) => ({
  title: `t${gh}`,
  pending_decisions: 0,
  id,
  project_id,
  gh_number: gh,
  kind: "issue" as const,
  status: "em_execucao" as const,
  branch: null,
  created_at: 0,
  closed_at: null,
  priority: null,
  rank: null,
});

test("hub 1.3 (sem online): ambientes e estado pelo último envio ao hub", () => {
  const old = (e: Parameters<typeof hubEnv>[0]) => {
    const { online: _omit, ...rest } = hubEnv(e);
    return rest as HubEnv;
  };
  const envs = [
    old({ env_id: "d-wsl", name: "wsl", last_seen_at: now - 10_000 }),
    old({ env_id: "d-mac", name: "mac", last_seen_at: now - ENV_ONLINE_MS - 1 }),
    old({ env_id: "d-new", name: "novo", last_seen_at: null }),
  ];
  expect(envViewsFromHub(envs).map((e) => [e.id, e.name])).toEqual([
    ["d-wsl", "wsl"],
    ["d-mac", "mac"],
    ["d-new", "novo"],
  ]);
  const st = envStatusesFromHub(envs, now);
  expect(st.map((x) => x.connected)).toEqual([true, false, false]);
  expect(st[1].error).toBe("sem envio ao hub há 2 min");
  expect(st[2].error).toBe("nunca enviou ao hub");
});

test("hub 1.4: online é o canal reverso, em tempo real, e não a idade do envio", () => {
  const envs = [
    hubEnv({ env_id: "a", last_seen_at: now - 60 * 60_000, online: true }),
    hubEnv({ env_id: "b", last_seen_at: now - 10_000, online: false }),
    hubEnv({ env_id: "c", last_seen_at: now - ENV_ONLINE_MS - 1, online: false }),
    hubEnv({ env_id: "d", last_seen_at: null, online: false }),
  ];
  const st = envStatusesFromHub(envs, now);
  expect(st.map((x) => x.connected)).toEqual([true, false, false, false]);
  expect(st[0].error).toBeNull();
  // Envia ao hub, mas sem canal: as ações não funcionam.
  expect(st[1].error).toBe(OFFLINE);
  expect(st[2].error).toBe("sem envio ao hub há 2 min");
  expect(st[3].error).toBe("nunca enviou ao hub");
});

test("dashboards por ambiente: sessões no ambiente delas, itens no dono do projeto sem repetir", () => {
  const ct = hubProject();
  const demo = hubProject({ id: 2, gh_repo: "demo", name: "demo" });
  const d: HubDashboard = {
    ...HUB_DASHBOARD,
    projects: [
      { project: ct, work_items: [wi(1, 10), wi(2, 20)] },
      { project: demo, work_items: [wi(3, 30, 2)] },
    ],
    live_sessions: [s({ id: "a", env_id: "d-wsl", work_item_id: 1 }), s({ id: "b", env_id: "d-mac", work_item_id: 2 })],
    interrupted_sessions: [s({ id: "c", env_id: "d-mac", work_item_id: 3, state: "interrupted", rc_session_id: "x" })],
    sync: [
      { project_id: 1, last_attempt_at: 0, last_ok_at: 5, last_error: null },
      { project_id: 2, last_attempt_at: 0, last_ok_at: 6, last_error: null },
    ],
    gestor_work_item_id: 99,
  };
  const views = [
    hubProjectView(ct, [hubAssignment({ env_id: "d-wsl" }), hubAssignment({ env_id: "d-mac" })]),
    hubProjectView(demo, [hubAssignment({ project_id: 2, env_id: "d-mac" })]),
  ];
  const out = dashboardsFromHub(d, views, [hubEnv({ env_id: "d-wsl" }), hubEnv({ env_id: "d-mac" })]);

  // ctower-agents: dono = d-wsl (primeira atribuição), com todos os itens e o sync.
  expect(out["d-wsl"].projects.map((p) => [p.project.gh_repo, p.work_items.map((w) => w.gh_number)])).toEqual([
    ["ctower-agents", [10, 20]],
  ]);
  expect(out["d-wsl"].sync.map((x) => x.project_id)).toEqual([1]);
  expect(out["d-wsl"].live_sessions.map((x) => x.id)).toEqual(["a"]);
  // d-mac: só o item da própria sessão do ctower-agents, e o demo inteiro.
  expect(out["d-mac"].projects.map((p) => [p.project.gh_repo, p.work_items.map((w) => w.gh_number)])).toEqual([
    ["ctower-agents", [20]],
    ["demo", [30]],
  ]);
  expect(out["d-mac"].sync.map((x) => x.project_id)).toEqual([2]);
  expect(out["d-mac"].interrupted_sessions[0]).toMatchObject({ id: "c", rc_session_id: "x", state: "interrupted" });
  expect(out["d-mac"].gestor_work_item_id).toBe(99);
  expect(out["d-mac"].projects[0].project).toMatchObject({ id: 1, kind: "github", gh_owner: "deep-ion-ai" });
});

test("sessão de um ambiente que o hub ainda não lista ganha dashboard próprio", () => {
  const d: HubDashboard = { ...HUB_DASHBOARD, live_sessions: [s({ env_id: "d-x" })] };
  expect(Object.keys(dashboardsFromHub(d, [], []))).toEqual(["d-x"]);
});
