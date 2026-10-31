// "Sincronizar este projeto" de qualquer lugar (#90): o cartão "Buscar
// issues", o cabeçalho do projeto, o menu da lista de Projetos e a paleta
// usam o mesmo plano (`syncPlan`), o mesmo `sync.now { project_id }` e o
// mesmo feedback (`syncFeedback`). O andamento e o último resultado de cada
// projeto ficam num store de módulo: disparado da lista ou da paleta, o
// cartão e o cabeçalho mostram o mesmo "Buscando issues…" e o resultado.
import { useSyncExternalStore } from "react";
import type { ProjectSync, SyncState } from "../protocol/gen";
import type { Screen } from "../state/nav";
import type { Tower } from "../state/tower";
import { isJira } from "../hub/tracker";
import { changeLabel } from "../util/item";
import { hubClones, localClone, type Clone } from "./clones";
import { pickSync, syncFeedback, syncPlan, type SyncOutcome, type SyncPlan } from "./overview";

const same = (a: string, b: string) => a.toLowerCase() === b.toLowerCase();

/** Projeto do sync: pela visão do hub (`owner/repo`) ou local (ambiente e id). */
export type SyncAddr = { owner: string; repo: string } | { env: string; projectId: number };

/** Chave do projeto no store (a mesma por qualquer caminho até ele). */
export function syncKey(a: SyncAddr): string {
  return "owner" in a ? `hub:${a.owner.toLowerCase()}/${a.repo.toLowerCase()}` : `env:${a.env}:${a.projectId}`;
}

type Src = Pick<Tower, "envs" | "status" | "dashboards" | "infos" | "hub">;

/**
 * Que visão o endereço do projeto abre, pelo mesmo critério do
 * `ProjectRoute` (App.tsx): com o hub no ar, a do hub por `owner/repo`; sem
 * ele, o projeto local (o do endereço ou o de algum ambiente com o mesmo
 * `owner/repo`). `null` = endereço sem projeto (o índice) ou projeto que
 * nenhuma visão conhece.
 */
export function syncAddrOf(t: Src, s: Screen): SyncAddr | null {
  if (s.name !== "project") return null;
  let key = s.owner && s.repo ? { owner: s.owner, repo: s.repo } : null;
  if (!key && s.env && s.projectId !== undefined) {
    const p = t.dashboards[s.env]?.projects.find((x) => x.project.id === s.projectId)?.project;
    if (p) key = { owner: p.gh_owner, repo: p.gh_repo };
  }
  if (t.hub.up && t.hub.dashboard) return key;
  if (s.env && s.projectId !== undefined) return { env: s.env, projectId: s.projectId };
  if (!key) return null;
  for (const e of t.envs) {
    const p = t.dashboards[e.id]?.projects.find((x) => same(x.project.gh_owner, key.owner) && same(x.project.gh_repo, key.repo));
    if (p) return { env: e.id, projectId: p.project.id };
  }
  return null;
}

/** O que o "Sincronizar este projeto" precisa saber do projeto. */
export interface ProjectSyncInfo {
  key: string;
  name: string;
  plan: SyncPlan;
  /** "PR" ou "MR", pela forja (texto do feedback). */
  change: string;
  /** Estado da última rodada (o pior entre hub e ambientes); `null` = sem dado. */
  state: SyncState | null;
}

const envSync = (t: Src, c: Clone): SyncState | null =>
  t.dashboards[c.env]?.sync.find((x) => x.project_id === c.project.id) ?? null;

/**
 * Plano, rótulo e estado do sync do projeto `a`. Na visão do hub, o projeto
 * Jira sincroniza só no ambiente de sync (id do daemon → ambiente da UI); o
 * estado é o pior entre o do hub e o de cada ambiente conectado com o
 * projeto (a falha do sync de um ambiente não chega ao `sync_state` do hub
 * num projeto GitHub, que o hub espelha por conta própria).
 */
export function projectSyncInfo(t: Src, a: SyncAddr): ProjectSyncInfo {
  const key = syncKey(a);
  if ("env" in a) {
    const c = localClone(t, a.env, a.projectId);
    const project = c?.project ?? t.dashboards[a.env]?.projects.find((x) => x.project.id === a.projectId)?.project ?? null;
    return {
      key,
      name: project?.name ?? "Projeto",
      plan: syncPlan(c ? [c] : [], null),
      change: changeLabel(project),
      state: t.dashboards[a.env]?.sync.find((x) => x.project_id === a.projectId) ?? null,
    };
  }
  const { hub } = t;
  const clones = hubClones(t, a.owner, a.repo);
  const view = hub.projects.find((x) => same(x.project.gh_owner, a.owner) && same(x.project.gh_repo, a.repo)) ?? null;
  const hp = view?.project;
  const jira =
    hp && isJira(hp)
      ? {
          env: hp.sync_env_id ? (hub.envByDaemon[hp.sync_env_id] ?? null) : null,
          name: hp.sync_env_id
            ? (view.assignments.find((x) => x.env_id === hp.sync_env_id)?.env_name ?? hp.sync_env_id)
            : null,
        }
      : null;
  const project = hp ?? clones[0]?.project ?? null;
  return {
    key,
    name: project?.name ?? `${a.owner}/${a.repo}`,
    plan: syncPlan(clones, jira),
    change: changeLabel(project),
    state: pickSync([view?.sync, ...clones.map((c) => envSync(t, c))]),
  };
}

/** Andamento e último resultado do "Sincronizar este projeto". */
export interface SyncRun {
  busy: boolean;
  msg: { text: string; error: boolean } | null;
}

const IDLE: SyncRun = { busy: false, msg: null };
const runs = new Map<string, SyncRun>();
const listeners = new Set<() => void>();

function setRun(key: string, r: SyncRun): void {
  runs.set(key, r);
  for (const l of listeners) l();
}

function subscribe(l: () => void): () => void {
  listeners.add(l);
  return () => void listeners.delete(l);
}

/** Andamento do sync do projeto `key` (o mesmo no cartão, no cabeçalho e no menu). */
export function useSyncRun(key: string): SyncRun {
  return useSyncExternalStore(
    subscribe,
    () => runs.get(key) ?? IDLE,
    () => runs.get(key) ?? IDLE,
  );
}

/**
 * `sync.now` com o `project_id` em cada ambiente do plano, com o resultado
 * ou o erro de cada um (`syncFeedback`). Um segundo pedido enquanto o
 * primeiro roda é ignorado. Sem ambiente no plano, o feedback é o motivo.
 */
export async function runProjectSync(api: Pick<Tower["api"], "call">, info: ProjectSyncInfo): Promise<SyncRun["msg"]> {
  if (runs.get(info.key)?.busy) return null;
  if (info.plan.targets.length === 0) {
    const msg = { text: info.plan.reason ?? "Nenhum ambiente conectado tem este projeto.", error: true };
    setRun(info.key, { busy: false, msg });
    return msg;
  }
  setRun(info.key, { busy: true, msg: null });
  const outcomes = await Promise.all(
    info.plan.targets.map(
      (x): Promise<SyncOutcome> =>
        api.call<ProjectSync[]>(x.env, "sync.now", { project_id: x.projectId }).then(
          (result) => ({ name: x.name, result }),
          (e) => ({ name: x.name, error: e instanceof Error ? e.message : String(e) }),
        ),
    ),
  );
  const msg = syncFeedback(outcomes, info.change);
  setRun(info.key, { busy: false, msg });
  return msg;
}

/** Esquece os resultados (entre testes). */
export function resetSyncRuns(): void {
  runs.clear();
  for (const l of listeners) l();
}

/**
 * O item "Sincronizar este projeto" dos menus (lista de Projetos, Sala de
 * controle) e da paleta: roda `runProjectSync` com o plano do projeto. Na
 * web fica desabilitado (o canal reverso recusa `sync.now`); sem ambiente
 * no plano, `reason` diz por quê.
 */
export function syncAction(
  t: Src & Pick<Tower, "api" | "web">,
  a: SyncAddr,
): { run(): void; web: boolean; busy: boolean; reason: string | null; info: ProjectSyncInfo } {
  const info = projectSyncInfo(t, a);
  return {
    info,
    web: t.web,
    busy: runs.get(info.key)?.busy ?? false,
    reason: info.plan.targets.length === 0 ? info.plan.reason : null,
    run: () => void runProjectSync(t.api, info),
  };
}
