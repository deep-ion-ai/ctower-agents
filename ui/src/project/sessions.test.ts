import type { HubSession } from "../protocol/hub-gen";
import { HUB_DASHBOARD, dashboard, hubProject, project, session, workItem } from "../test/fixtures";
import type { Clone } from "./clones";
import { projectSessions, sessionCounts, type HubSource } from "./sessions";

const WI5 = workItem({ id: 5, gh_number: 5, title: "cinco" });
const WI0 = workItem({ id: 9, gh_number: 0, title: null });
const LIVE = session({ id: "live-5", work_item_id: 5 });
const INT = session({ id: "int-0", work_item_id: 9, state: "interrupted" });
const FAIL = session({ id: "fail-5", work_item_id: 5, state: "exited", exit_code: 1 });
// De outro projeto no mesmo ambiente: fica de fora.
const OTHER = session({ id: "other", work_item_id: 77 });

const clone = (env: string, name: string): Clone => ({
  env,
  name,
  project: project(),
  items: [WI5, WI0],
  live: [LIVE],
  state: "applied",
});

const hubSession = (s: Partial<HubSession>): HubSession => ({
  id: "h",
  env_id: "daemon-wsl",
  work_item_id: 105,
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
  ...s,
});

test("pelos ambientes conectados: vivas, interrompidas e encerradas com erro do projeto", () => {
  const dash = dashboard({ live_sessions: [LIVE, OTHER], interrupted_sessions: [INT], failed_sessions: [FAIL] }, [WI5, WI0]);
  const [g, ...rest] = projectSessions([clone("wsl", "WSL")], { wsl: dash });
  expect(rest).toEqual([]);
  expect(g).toMatchObject({ key: "wsl", name: "WSL", env: "wsl" });
  expect(g.live.map((s) => [s.id, s.gh, s.title])).toEqual([["live-5", 5, "cinco"]]);
  expect(g.interrupted.map((s) => [s.id, s.gh])).toEqual([["int-0", 0]]);
  expect(g.ended.map((s) => [s.id, s.exit_code])).toEqual([["fail-5", 1]]);
  expect(sessionCounts([g])).toEqual({ live: 1, interrupted: 1, ended: 1 });
});

test("o hub completa: sem repetir as do ambiente e com os ambientes que a UI não alcança (só leitura)", () => {
  const hp = hubProject({ id: 7 });
  const items = [
    { ...WI5, id: 105, project_id: 7, priority: null, rank: null },
    { ...WI0, id: 109, project_id: 7, priority: null, rank: null },
  ].map(({ worktree_path: _w, parent_number: _p, ...w }) => w);
  const hub: HubSource = {
    dashboard: {
      ...HUB_DASHBOARD,
      projects: [{ project: hp, work_items: items }],
      // A viva do WSL já veio do ambiente (mesmo id): não repete.
      live_sessions: [
        hubSession({ id: "live-5", env_id: "daemon-wsl" }),
        hubSession({ id: "nb-1", env_id: "daemon-nb", work_item_id: 109 }),
      ],
      interrupted_sessions: [hubSession({ id: "wsl-int", env_id: "daemon-wsl", work_item_id: 105, state: "interrupted" })],
      failed_sessions: [hubSession({ id: "x", env_id: "daemon-nb", work_item_id: 999 })],
    },
    envs: [
      { env_id: "daemon-wsl", name: "wsl-pc" },
      { env_id: "daemon-nb", name: "notebook" },
    ],
    envByDaemon: { "daemon-wsl": "wsl" },
    projectId: 7,
  };
  const groups = projectSessions([clone("wsl", "WSL")], { wsl: dashboard({ live_sessions: [LIVE] }, [WI5, WI0]) }, hub);
  expect(groups.map((g) => [g.key, g.name, g.env])).toEqual([
    ["wsl", "WSL", "wsl"],
    ["hub:daemon-nb", "notebook", null],
  ]);
  expect(groups[0].live.map((s) => s.id)).toEqual(["live-5"]);
  // A interrompida que só o hub relata entra no grupo do ambiente conectado.
  expect(groups[0].interrupted.map((s) => s.id)).toEqual(["wsl-int"]);
  expect(groups[1].live.map((s) => [s.id, s.gh])).toEqual([["nb-1", 0]]);
  // Item de outro projeto: fora.
  expect(groups[1].ended).toEqual([]);
});

test("sem clones e sem o projeto no hub: nada", () => {
  expect(projectSessions([], {}, null)).toEqual([]);
  expect(
    projectSessions([], {}, { dashboard: HUB_DASHBOARD, envs: [], envByDaemon: {}, projectId: 7 }),
  ).toEqual([]);
});
