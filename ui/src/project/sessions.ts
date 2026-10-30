// Sessões do projeto por ambiente (aba Sessões, #17): vivas, interrompidas
// e encerradas com erro, juntando o que a UI vê nos ambientes conectados
// (`Clone` de clones.ts, o mesmo de `liveSessions`) com o que o hub relata
// de todos os ambientes (inclusive os que esta UI não alcança). Função pura,
// testada em sessions.test.ts.
import type { Dashboard } from "../protocol/gen";
import type { HubDashboard, HubEnv, HubSession } from "../protocol/hub-gen";
import type { Clone } from "./clones";

export type SessionKind = "live" | "interrupted" | "ended";

/** O que a aba mostra de uma sessão (a do ambiente ou a relatada ao hub). */
export interface ProjectSession {
  id: string;
  kind: SessionKind;
  /** Número do item (0 = sessão do projeto). */
  gh: number;
  title: string | null;
  started_at: number;
  last_event_at: number;
  ended_at: number | null;
  exit_code: number | null;
}

export interface EnvSessions {
  /** Estável: o id do ambiente na UI ou, sem ele, o `env_id` do hub. */
  key: string;
  name: string;
  /**
   * Ambiente da UI onde abrir o terminal ou a Recuperação; `null` = só o hub
   * conhece (esta UI não tem conexão com ele): só leitura.
   */
  env: string | null;
  live: ProjectSession[];
  interrupted: ProjectSession[];
  ended: ProjectSession[];
}

/** O projeto no dashboard do hub e o que mais a junção precisa dele. */
export interface HubSource {
  dashboard: Pick<HubDashboard, "projects" | "live_sessions" | "interrupted_sessions" | "failed_sessions">;
  envs: Pick<HubEnv, "env_id" | "name">[];
  /** `env_id` do daemon → id do ambiente na UI (`hub.envByDaemon`). */
  envByDaemon: Record<string, string>;
  /** Id do projeto no hub. */
  projectId: number;
}

type Raw = Pick<HubSession, "id" | "work_item_id" | "started_at" | "last_event_at" | "ended_at" | "exit_code">;
type Item = { id: number; gh_number: number; title: string | null };

const KINDS: SessionKind[] = ["live", "interrupted", "ended"];

function rows(kind: SessionKind, list: Raw[], items: Item[]): ProjectSession[] {
  const byId = new Map(items.map((w) => [w.id, w]));
  return list.flatMap((s) => {
    const wi = byId.get(s.work_item_id);
    if (!wi) return [];
    const { id, started_at, last_event_at, ended_at, exit_code } = s;
    return [{ id, kind, gh: wi.gh_number, title: wi.title, started_at, last_event_at, ended_at, exit_code }];
  });
}

/**
 * Sessões do projeto por ambiente. Os ambientes conectados (`clones`) vêm
 * primeiro, com os dados do próprio daemon (mais frescos); o hub completa
 * com as sessões que eles não trazem e com os ambientes que a UI não
 * alcança. Uma sessão aparece uma vez só, no estado que o ambiente der.
 * "Encerradas" são as que saíram com erro nas últimas 24 h (`failed_sessions`):
 * as que saíram sem erro não ficam no dashboard.
 */
export function projectSessions(clones: Clone[], dashboards: Record<string, Dashboard>, hub?: HubSource | null): EnvSessions[] {
  const out: EnvSessions[] = [];
  const seen = new Set<string>();
  for (const c of clones) {
    const d = dashboards[c.env];
    const g: EnvSessions = {
      key: c.env,
      name: c.name,
      env: c.env,
      live: rows("live", d?.live_sessions ?? c.live, c.items),
      interrupted: rows("interrupted", d?.interrupted_sessions ?? [], c.items),
      ended: rows("ended", d?.failed_sessions ?? [], c.items),
    };
    for (const k of KINDS) for (const s of g[k]) seen.add(s.id);
    out.push(g);
  }
  const hp = hub?.dashboard.projects.find((x) => x.project.id === hub.projectId);
  if (!hub || !hp) return out;
  const lists: Record<SessionKind, HubSession[]> = {
    live: hub.dashboard.live_sessions,
    interrupted: hub.dashboard.interrupted_sessions,
    ended: hub.dashboard.failed_sessions,
  };
  for (const k of KINDS) {
    for (const s of lists[k]) {
      if (seen.has(s.id)) continue;
      const [row] = rows(k, [s], hp.work_items);
      if (!row) continue;
      seen.add(s.id);
      const env = hub.envByDaemon[s.env_id] ?? null;
      const key = env ?? `hub:${s.env_id}`;
      let g = out.find((x) => x.key === key);
      if (!g) {
        const name = hub.envs.find((e) => e.env_id === s.env_id)?.name ?? s.env_id;
        g = { key, name, env, live: [], interrupted: [], ended: [] };
        out.push(g);
      }
      g[k].push(row);
    }
  }
  return out;
}

/** Total por estado, para a Visão geral e o cabeçalho da aba. */
export function sessionCounts(groups: EnvSessions[]): Record<SessionKind, number> {
  const n = { live: 0, interrupted: 0, ended: 0 };
  for (const g of groups) for (const k of KINDS) n[k] += g[k].length;
  return n;
}
