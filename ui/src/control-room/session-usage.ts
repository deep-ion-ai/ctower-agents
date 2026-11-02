// Visão por sessão (#109): as sessões vivas agrupadas pela conta do
// ambiente, com o custo estimado, o contexto da janela real, o tempo parada,
// o modelo e o esforço, para escolher qual continua dentro da mesma janela
// do plano. Funções puras aqui; o hook de leitura no fim e a lista em
// SessionUsageList.tsx.
import { useEffect, useState } from "react";
import type { DaemonApi } from "../api/daemon";
import type { DashboardWorkItem, Project, GestorScope, Session, SessionUsage } from "../protocol/gen";
import { accountOfEnv, type AccountPlanView, type PlanData } from "../plan/data";
import { USAGE_RELOAD_MS } from "../terminal/usage";
import { tileState } from "./TileGrid";

/** Leitura de uma sessão: `undefined` = ainda não lida; `null` = sem leitura (sem arquivo de uso). */
export type UsageSlot = SessionUsage | null | undefined;

export interface UsageRow {
  env: string;
  envName: string;
  s: Session;
  wi?: DashboardWorkItem;
  project?: Project;
  gestor?: boolean;
  scope?: GestorScope;
  usage: UsageSlot;
}

/** Ordenação: maior custo estimado ou parada há mais tempo primeiro. */
export type UsageSort = "cost" | "idle";

export const USAGE_SORTS: { id: UsageSort; label: string }[] = [
  { id: "cost", label: "Custo estimado" },
  { id: "idle", label: "Tempo parada" },
];

export const usageKey = (env: string, session: string) => `${env}|${session}`;

/**
 * Há quanto tempo a sessão está parada (ms): desde o último evento, para
 * quem espera você ou parou no turno; `null` para a que está rodando.
 */
export function idleMs(r: Pick<UsageRow, "s" | "wi">, now: number): number | null {
  if (tileState(r.s, r.wi?.status).tone === "running") return null;
  return Math.max(0, now - r.s.last_event_at);
}

/** Custo estimado da leitura; `null` sem leitura ou sem o campo. */
export function costOf(u: UsageSlot): number | null {
  return u?.cost?.total_cost_usd ?? null;
}

/**
 * Ordena (sem mudar a lista): pelo custo estimado ou pelo tempo parada, do
 * maior para o menor. Sem número (sem leitura, ou rodando) vai para o fim;
 * empate pela sessão mais antiga.
 */
export function sortRows<T extends Pick<UsageRow, "s" | "wi" | "usage">>(rows: T[], by: UsageSort, now: number): T[] {
  const val = (r: T) => (by === "cost" ? costOf(r.usage) : idleMs(r, now));
  return [...rows].sort((a, b) => {
    const va = val(a);
    const vb = val(b);
    if (va !== vb) {
      if (va === null) return 1;
      if (vb === null) return -1;
      return vb - va;
    }
    return a.s.started_at - b.s.started_at;
  });
}

export interface UsageGroup {
  /** Chave da conta, ou `unknown`. */
  key: string;
  /** Conta com o plano; `null` = conta desconhecida (ou ambiente ainda sem dado da conta). */
  account: AccountPlanView | null;
  rows: UsageRow[];
}

/**
 * Agrupa pela conta do ambiente, na ordem das contas do plano; ambiente de
 * conta desconhecida, ou que ainda não aparece no plano, num grupo à parte
 * no fim. Grupos vazios não aparecem.
 */
export function groupByAccount(rows: UsageRow[], plan: PlanData): UsageGroup[] {
  const byKey = new Map<string, UsageGroup>();
  for (const a of plan.accounts) byKey.set(a.key, { key: a.key, account: a, rows: [] });
  const unknown: UsageGroup = { key: "unknown", account: null, rows: [] };
  for (const r of rows) {
    const a = accountOfEnv(plan, r.env);
    const g = a && a !== "unknown" ? byKey.get(a.key) : undefined;
    (g ?? unknown).rows.push(r);
  }
  return [...byKey.values(), unknown].filter((g) => g.rows.length > 0);
}

const tokensK = (n: number) => (n >= 1_000_000 ? `${+(n / 1_000_000).toFixed(1)}M` : `${Math.round(n / 1000)}k`);

/** Contexto na janela real da leitura: "42% de 200k"; `null` sem o percentual. */
export function contextPctText(u: UsageSlot): string | null {
  const c = u?.context_window;
  if (c?.used_percentage == null) return null;
  const size = c.context_window_size;
  return `${Math.round(c.used_percentage)}%${size ? ` de ${tokensK(size)}` : ""}`;
}

/** Modelo e esforço da leitura ("Opus 4.1 · esforço high"); sem leitura, o modelo pedido ao iniciar. */
export function modelText(r: Pick<UsageRow, "s" | "usage">): string | null {
  const m = r.usage?.model?.display_name ?? r.usage?.model?.id ?? r.s.model;
  const effort = r.usage?.effort;
  if (!m) return effort ? `esforço ${effort}` : null;
  return effort ? `${m} · esforço ${effort}` : m;
}

/** Pushes da sessão que relêem a leitura dela. */
const USAGE_PUSHES: ReadonlySet<string> = new Set(["session_state", "session_exited"]);
/** Pushes juntos da mesma sessão viram uma leitura. */
export const USAGE_PUSH_DEBOUNCE_MS = 1_000;

/**
 * `session.usage` das sessões listadas: todas juntas ao abrir, a cada
 * `USAGE_RELOAD_MS` (com a janela visível) e, por sessão, nos pushes dela
 * (`session_state`, `session_exited`). Falha passageira mantém a última
 * leitura.
 */
export function useSessionsUsage(api: DaemonApi, keys: { env: string; session: string }[]): Record<string, UsageSlot> {
  const [slots, setSlots] = useState<Record<string, UsageSlot>>({});
  const sig = keys.map((k) => usageKey(k.env, k.session)).join(",");
  useEffect(() => {
    if (keys.length === 0) return;
    let alive = true;
    const listed = new Set(keys.map((k) => usageKey(k.env, k.session)));
    const read = async (list: { env: string; session: string }[]) => {
      const got = await Promise.all(
        list.map(async (k) => {
          try {
            const u = await api.call<SessionUsage | null>(k.env, "session.usage", { session_id: k.session });
            return [usageKey(k.env, k.session), u ?? null] as const;
          } catch {
            return null;
          }
        }),
      );
      if (alive) setSlots((prev) => ({ ...prev, ...Object.fromEntries(got.filter((x) => x !== null)) }));
    };
    void read(keys);
    const timer = setInterval(() => {
      if (document.visibilityState !== "hidden") void read(keys);
    }, USAGE_RELOAD_MS);
    const pending = new Map<string, ReturnType<typeof setTimeout>>();
    const off = api.onPush((e) => {
      const session = e.data?.session_id;
      if (!USAGE_PUSHES.has(e.event) || typeof session !== "string") return;
      const k = usageKey(e.env, session);
      if (!listed.has(k)) return;
      clearTimeout(pending.get(k));
      pending.set(
        k,
        setTimeout(() => {
          pending.delete(k);
          void read([{ env: e.env, session }]);
        }, USAGE_PUSH_DEBOUNCE_MS),
      );
    });
    return () => {
      alive = false;
      clearInterval(timer);
      off();
      for (const t of pending.values()) clearTimeout(t);
    };
    // `sig` resume `keys`: só refaz quando o conjunto de sessões muda.
  }, [api, sig]);
  return slots;
}
