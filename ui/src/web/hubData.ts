// Modo web, fase 1 (épico #205, spec §2.1 e §10.1): sem conexão direta com
// os ambientes, a UI monta a visão de cada um a partir do hub. Ambientes e
// o estado deles vêm de `hub.envs.list` (idade do último envio ao hub); os
// dashboards de cada ambiente, do `hub.dashboard.get`, no formato que as
// telas já usam. Funções puras, testadas em hubData.test.ts.
import type { EnvStatus, EnvView } from "../api/types";
import type { Dashboard, DashboardProject, DashboardWorkItem, Project, Session } from "../protocol/gen";
import type {
  HubDashboard,
  HubDashboardProject,
  HubDashboardWorkItem,
  HubEnv,
  HubProject,
  HubProjectView,
  HubSession,
} from "../protocol/hub-gen";
import { ago } from "../util/time";

/**
 * Hub 1.3 (sem `HubEnv.online`): ambiente "no ar" pelo hub, enviou há menos disso. Os ambientes enviam a
 * cada 5 s (`PUSH_EVERY`), com backoff só quando o hub falha; 2 min cobrem
 * uma reconexão sem dar falso "fora do ar".
 */
export const ENV_ONLINE_MS = 2 * 60_000;

/** Ambiente como a UI o vê, pelo hub: o id é o `env_id` do daemon. */
export function envViewsFromHub(envs: HubEnv[]): EnvView[] {
  return envs.map((e) => ({ id: e.env_id, name: e.name, url: "", kind: "remote", distro: null, has_token: false }));
}

/** Erro do ambiente sem canal reverso, mas enviando ao hub. */
export const OFFLINE = "sem canal com o hub: ações indisponíveis";

/**
 * Estado de cada ambiente. Com o hub 1.4, `online` é o canal reverso aberto
 * agora (fase 2): só com ele as ações do ambiente funcionam. Hub 1.3 (sem
 * `online`): pela idade do último envio, como na fase 1.
 */
export function envStatusesFromHub(envs: HubEnv[], now: number): EnvStatus[] {
  return envs.map((e) => {
    const seen = e.last_seen_at;
    const channel = typeof e.online === "boolean";
    if (channel && e.online) return { env: e.env_id, connected: true, error: null, since: seen ?? e.created_at };
    if (seen === null) return { env: e.env_id, connected: false, error: "nunca enviou ao hub", since: e.created_at };
    const recent = now - seen <= ENV_ONLINE_MS;
    if (channel) {
      return { env: e.env_id, connected: false, error: recent ? OFFLINE : `sem envio ao hub ${ago(seen, now)}`, since: seen };
    }
    return {
      env: e.env_id,
      connected: recent,
      error: recent ? null : `sem envio ao hub ${ago(seen, now)}`,
      since: seen,
    };
  });
}

function project(p: HubProject): Project {
  return {
    id: p.id,
    name: p.name,
    gh_owner: p.gh_owner,
    gh_repo: p.gh_repo,
    repo_path: "",
    base_branch: p.base_branch,
    protected_branches: p.protected_branches,
    created_at: p.created_at,
    config_rev: null,
    unassigned_at: null,
    location_state: null,
    location_error: null,
    applied_rev: null,
    kind: "github",
    // Rastreador e forja do cadastro do hub (#353): referências `PROJ-n`,
    // rótulos e links do Jira/GitLab também na web. Sem ambiente, sem papel de sync.
    tracker: p.tracker,
    forge: p.forge,
    branch_pattern: p.branch_pattern,
    attribution: p.attribution,
    sync_role: false,
    // Modelo de docs do cadastro (#440); o hub anterior não manda.
    docs: p.docs ?? { enabled: false, project_dirs: [], epic_dirs: [] },
  };
}

function workItem(w: HubDashboardWorkItem): DashboardWorkItem {
  return {
    title: w.title,
    parent_number: null,
    pending_decisions: w.pending_decisions,
    id: w.id,
    project_id: w.project_id,
    gh_number: w.gh_number,
    kind: w.kind,
    status: w.status,
    branch: w.branch,
    worktree_path: null,
    created_at: w.created_at,
    closed_at: w.closed_at,
  };
}

function session(s: HubSession): Session {
  return {
    id: s.id,
    work_item_id: s.work_item_id,
    tool: s.tool,
    tool_session_id: s.tool_session_id,
    transcript_path: null,
    backup_path: null,
    cwd: s.cwd,
    launch_cmd: [],
    state: s.state,
    exit_code: s.exit_code,
    pid: null,
    started_at: s.started_at,
    last_event_at: s.last_event_at,
    ended_at: s.ended_at,
    model: s.model,
    mode: s.mode,
    rc_session_id: s.rc_session_id,
  };
}

/**
 * Dashboard de cada ambiente a partir da visão consolidada do hub.
 *
 * Cada sessão fica no ambiente dela (`env_id`). Cada projeto tem um ambiente
 * "dono" (a primeira atribuição; sem atribuição, o primeiro ambiente), que
 * leva todos os work items e o estado do sync com o GitHub; os outros
 * ambientes levam só os work items das próprias sessões. Assim, os
 * contadores somados entre ambientes (concluídas hoje, revisão) não contam
 * o mesmo item duas vezes, e toda sessão acha o work item no ambiente dela.
 */
export function dashboardsFromHub(
  d: HubDashboard,
  projects: HubProjectView[],
  envs: HubEnv[],
): Record<string, Dashboard> {
  const ids = envs.map((e) => e.env_id);
  for (const s of [...d.live_sessions, ...d.interrupted_sessions, ...d.failed_sessions]) {
    if (!ids.includes(s.env_id)) ids.push(s.env_id);
  }
  const owner = (p: HubDashboardProject): string | undefined => {
    const view = projects.find((v) => v.project.id === p.project.id);
    const assigned = view?.assignments.find((a) => ids.includes(a.env_id));
    return assigned?.env_id ?? ids[0];
  };
  const out: Record<string, Dashboard> = {};
  for (const env of ids) {
    const mine = (list: HubSession[]) => list.filter((s) => s.env_id === env);
    const live = mine(d.live_sessions);
    const interrupted = mine(d.interrupted_sessions);
    const failed = mine(d.failed_sessions);
    const used = new Set([...live, ...interrupted, ...failed].map((s) => s.work_item_id));
    const dprojects: DashboardProject[] = [];
    const ownedIds = new Set<number>();
    for (const p of d.projects) {
      const owns = owner(p) === env;
      const items = owns ? p.work_items : p.work_items.filter((w) => used.has(w.id));
      const assigned = projects.find((v) => v.project.id === p.project.id)?.assignments.some((a) => a.env_id === env);
      if (!owns && !assigned && items.length === 0) continue;
      if (owns) ownedIds.add(p.project.id);
      dprojects.push({ project: project(p.project), work_items: items.map(workItem) });
    }
    out[env] = {
      projects: dprojects,
      live_sessions: live.map(session),
      interrupted_sessions: interrupted.map(session),
      failed_sessions: failed.map(session),
      sync: d.sync.filter((s) => ownedIds.has(s.project_id)),
      gestor_work_item_id: d.gestor_work_item_id,
      gestor_scopes: {},
    };
  }
  return out;
}
