// Contadores e resumos do shell (barra de atividades, barra de status e
// painel inferior), tirados do `useTower()`. Funções puras.
import type { EnvStatus, EnvView } from "../api/types";
import { attention, lookup, type AttentionItem } from "../control-room/attention";
import type { Dashboard } from "../protocol/gen";
import type { Tower } from "../state/tower";
import { itemShort, trackersText } from "../util/item";

/** Sessões vivas cujo work item espera o humano (permissão ou pergunta). */
export function waitingCount(dashboards: Record<string, Dashboard>): number {
  return Object.values(dashboards).reduce(
    (n, d) => n + d.live_sessions.filter((s) => lookup(d, s).wi?.status === "aguardando_voce").length,
    0,
  );
}

export function interruptedCount(dashboards: Record<string, Dashboard>): number {
  return Object.values(dashboards).reduce((n, d) => n + d.interrupted_sessions.length, 0);
}

/** Último sync com o rastreador (GitHub ou Jira) que deu certo, em qualquer ambiente. */
export function lastSync(dashboards: Record<string, Dashboard>): number | null {
  return Object.values(dashboards)
    .flatMap((d) => d.sync)
    .reduce<number | null>((m, s) => (s.last_ok_at && (!m || s.last_ok_at > m) ? s.last_ok_at : m), null);
}

/** Rastreadores dos projetos dos ambientes (`GitHub`, `Jira` ou `GitHub e Jira`), para o sync (#352). */
export function syncTrackers(dashboards: Record<string, Dashboard>): string {
  return trackersText(
    Object.values(dashboards).flatMap((d) => d.projects.map((p) => p.project).filter((p) => p.kind !== "gestor")),
  );
}

/** Referência do item de um push (`ref`, ou `gh_number` no projeto do `project_id`); vazio sem item. */
export function pushItemRef(dashboards: Record<string, Dashboard>, env: string, data: Record<string, unknown> | undefined): string {
  if (typeof data?.ref === "string" && data.ref) return data.ref;
  if (typeof data?.gh_number !== "number") return "";
  const project = dashboards[env]?.projects.find((p) => p.project.id === data.project_id)?.project;
  return itemShort(project, data.gh_number);
}

export function connectedEnvs(envs: EnvView[], status: Record<string, EnvStatus>): number {
  return envs.filter((e) => status[e.id]?.connected).length;
}

/** O que vai para a aba Avisos: hub, alertas de push, ambiente fora, docs inacessíveis, sync falhando ou atrasado. */
const WARNING_KINDS = new Set(["hub", "alert", "env_down", "docs", "sync_failing", "sync_late"]);

export function warnings(
  t: Pick<Tower, "envs" | "status" | "dashboards" | "alerts" | "hub"> & { web?: boolean; infos?: Tower["infos"] },
  now: number,
): AttentionItem[] {
  return attention({
    web: t.web,
    envs: t.envs,
    status: t.status,
    dashboards: t.dashboards,
    alerts: t.alerts,
    infos: t.infos,
    now,
    hub: t.hub.conn
      ? { status: t.hub.status, warnings: t.hub.health?.warnings ?? [], update: t.hub.update }
      : undefined,
  }).filter((a) => WARNING_KINDS.has(a.kind));
}

export const plural = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`;
