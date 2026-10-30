// Onde um projeto pode iniciar sessões: os ambientes conectados que o têm
// (na visão do hub, os atribuídos). Usado pela tela Projeto (sessões vivas)
// e pela modal de início (escolha do ambiente).
import type { DashboardWorkItem, Project, Session } from "../protocol/gen";
import type { AssignmentState } from "../protocol/hub-gen";
import type { Tower } from "../state/tower";

const same = (a: string, b: string) => a.toLowerCase() === b.toLowerCase();

/** Ambiente conectado com o projeto, com o projeto do cache de lá. */
export interface Clone {
  env: string;
  name: string;
  project: Project;
  items: DashboardWorkItem[];
  live: Session[];
  /** Estado da atribuição no hub (`null` = o hub não conhece este ambiente). */
  state: AssignmentState | null;
}

export const STATE_SUFFIX: Record<AssignmentState, string> = {
  applied: "",
  pending: " · pendente",
  error: " · com erro",
};

type Src = Pick<Tower, "envs" | "status" | "dashboards" | "infos" | "hub">;

/**
 * Ambientes conectados com o projeto atribuído (no cache da configuração
 * deles); as atribuições aplicadas vêm primeiro. Com o hub no ar, um
 * ambiente que o hub conhece e sem atribuição deste projeto fica de fora
 * (o cache dele está atrasado); com o hub fora, vale o cache.
 */
export function hubClones(t: Src, owner: string, repo: string): Clone[] {
  const { envs, status, dashboards, infos, hub } = t;
  const hubEnvIds = new Set(hub.connected ? hub.envs.map((x) => x.env_id) : []);
  const assignments =
    hub.projects.find((x) => same(x.project.gh_owner, owner) && same(x.project.gh_repo, repo))?.assignments ?? [];
  const stateOf = (env: string): AssignmentState | null =>
    assignments.find((a) => hub.envByDaemon[a.env_id] === env)?.state ?? null;
  const rank = (s: AssignmentState | null) => (s === "applied" || s === null ? 0 : s === "pending" ? 1 : 2);
  const clones: Clone[] = envs.flatMap((e) => {
    const d = dashboards[e.id];
    if (!status[e.id]?.connected || !d) return [];
    const p = d.projects.find((x) => same(x.project.gh_owner, owner) && same(x.project.gh_repo, repo));
    if (!p) return [];
    const daemonId = infos[e.id]?.env_id;
    const state = stateOf(e.id);
    if (state === null && daemonId !== undefined && hubEnvIds.has(daemonId)) return [];
    const ids = new Set(p.work_items.map((w) => w.id));
    return [
      {
        env: e.id,
        name: e.name,
        project: p.project,
        items: p.work_items,
        live: d.live_sessions.filter((s) => ids.has(s.work_item_id)),
        state,
      },
    ];
  });
  return clones.sort((a, b) => rank(a.state) - rank(b.state));
}

/** O projeto local `projectId` do ambiente `env`, se conectado (visão sem o hub). */
export function localClone(t: Src, env: string, projectId: number): Clone | null {
  const d = t.dashboards[env];
  const p = d?.projects.find((x) => x.project.id === projectId);
  if (!t.status[env]?.connected || !d || !p) return null;
  const ids = new Set(p.work_items.map((w) => w.id));
  return {
    env,
    name: t.envs.find((e) => e.id === env)?.name ?? env,
    project: p.project,
    items: p.work_items,
    live: d.live_sessions.filter((s) => ids.has(s.work_item_id)),
    state: null,
  };
}

/** Sessão viva de um item num ambiente que a UI conhece. */
export interface LiveSession {
  env: string;
  name: string;
  session: Session;
}

/** Sessões vivas do item `gh` (0 = sessão do projeto) em cada clone. */
export function liveSessions(clones: Clone[], gh: number): LiveSession[] {
  return clones.flatMap((c) =>
    c.live
      .filter((s) => c.items.find((w) => w.id === s.work_item_id)?.gh_number === gh)
      .map((s) => ({ env: c.env, name: c.name, session: s })),
  );
}
