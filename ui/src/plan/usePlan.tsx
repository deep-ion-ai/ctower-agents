// Dados do plano das contas para a UI toda (#107): o selo na barra de
// status, a lista de contas na Sala de controle e, depois, a visão por
// sessão (#109) e os alertas (#108). Um provedor só lê e mantém em dia.
//
// Com hub (desktop ligado a ele, ou a web): `hub.plan.accounts`, em que a
// leitura mais recente de qualquer ambiente da conta vale para todos. Sem
// hub: `account.get` de cada ambiente conectado, agrupado aqui pela conta.
// Relê nos pushes de sessão e a cada `PLAN_POLL_MS`; a regra do reset é
// reaplicada com o relógio da UI em `usePlan(now)`.

import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import type { EnvAccount } from "../protocol/gen";
import type { HubPlanAccounts } from "../protocol/hub-gen";
import { ENV_SYNC_LAG_MS, useTower } from "../state/tower";
import { EMPTY_PLAN, accountOfEnv, fromEnvs, fromHub, planAt, type AccountPlanView, type EnvAccountEntry, type PlanData } from "./data";

/** Releitura periódica (a leitura chega a cada resposta das sessões). */
export const PLAN_POLL_MS = 60_000;
/** Pushes de sessão relidos juntos. */
export const PLAN_PUSH_DEBOUNCE_MS = 1_000;

/** Pushes do ambiente depois dos quais pode haver leitura nova do plano. */
export const PLAN_REFRESH_ON: ReadonlySet<string> = new Set([
  "session_started",
  "session_state",
  "session_exited",
  "session_resumed",
]);

type Raw = { kind: "hub"; h: HubPlanAccounts } | { kind: "envs"; list: EnvAccountEntry[]; at: number } | null;

export interface PlanState {
  /** Contas, ambientes e conta desconhecida, como a fonte respondeu. */
  data: PlanData;
  /** Já houve uma leitura (com ou sem dado). */
  loaded: boolean;
  /** Relê agora (ex.: depois de instalar o statusline). */
  reload(): Promise<void>;
}

const Ctx = createContext<PlanState | null>(null);

const NO_PROVIDER: PlanState = { data: EMPTY_PLAN, loaded: false, reload: async () => {} };

export function PlanProvider({ children }: { children: ReactNode }) {
  const { api, web, hub, envs, status } = useTower();
  const [raw, setRaw] = useState<Raw>(null);
  const useHub = web || hub.connected;
  const connected = envs
    .filter((e) => status[e.id]?.connected)
    .map((e) => e.id)
    .join(",");
  // Valores atuais para o `load` sem refazê-lo a cada render.
  const cur = useRef({ envs, connected });
  cur.current = { envs, connected };
  const seq = useRef(0);

  const load = useCallback(async () => {
    const n = ++seq.current;
    if (useHub) {
      try {
        const h = await api.hubCall<HubPlanAccounts>("hub.plan.accounts");
        if (n === seq.current) setRaw({ kind: "hub", h });
        return;
      } catch {
        // Hub anterior ao método ou fora do ar: no desktop, cai para os ambientes.
        if (web) {
          if (n === seq.current) setRaw({ kind: "envs", list: [], at: Date.now() });
          return;
        }
      }
    }
    const ids = cur.current.connected ? cur.current.connected.split(",") : [];
    const entries = await Promise.all(
      ids.map((id) =>
        api.call<EnvAccount>(id, "account.get").then(
          (data): EnvAccountEntry => ({ env: id, name: cur.current.envs.find((e) => e.id === id)?.name ?? id, data }),
          () => null,
        ),
      ),
    );
    if (n === seq.current) setRaw({ kind: "envs", list: entries.filter((e): e is EnvAccountEntry => e !== null), at: Date.now() });
  }, [api, web, useHub]);

  // Ao abrir, ao mudar a fonte ou os ambientes conectados, e periodicamente.
  useEffect(() => {
    void load();
    const id = setInterval(() => void load(), PLAN_POLL_MS);
    return () => clearInterval(id);
  }, [load, connected, hub.connected]);

  // Pushes de sessão: com hub, a leitura só chega a ele no próximo envio
  // do ambiente; relê de novo depois disso.
  useEffect(() => {
    let timers: ReturnType<typeof setTimeout>[] = [];
    const schedule = (delays: number[]) => {
      for (const t of timers) clearTimeout(t);
      timers = delays.map((ms) => setTimeout(() => void load(), ms));
    };
    const offPush = api.onPush((e) => {
      if (PLAN_REFRESH_ON.has(e.event)) schedule(useHub ? [PLAN_PUSH_DEBOUNCE_MS, ENV_SYNC_LAG_MS] : [PLAN_PUSH_DEBOUNCE_MS]);
    });
    // Envio de um ambiente ao hub (ou `env.push` na web): pode trazer leitura nova.
    const offHub = api.onHubPush(() => {
      if (useHub) schedule([PLAN_PUSH_DEBOUNCE_MS]);
    });
    return () => {
      offPush();
      offHub();
      for (const t of timers) clearTimeout(t);
    };
  }, [api, load, useHub]);

  const names = useMemo(() => Object.fromEntries(envs.map((e) => [e.id, e.name])), [envs]);
  const { envByDaemon } = hub;
  const data = useMemo<PlanData>(() => {
    if (!raw) return EMPTY_PLAN;
    if (raw.kind === "hub") return fromHub(raw.h, envByDaemon, names);
    return fromEnvs(raw.list, raw.at);
  }, [raw, envByDaemon, names]);

  const value = useMemo<PlanState>(() => ({ data, loaded: raw !== null, reload: load }), [data, raw, load]);
  return <Ctx.Provider value={value}>{children}</Ctx.Provider>;
}

/**
 * Plano das contas com a regra do reset reaplicada em `now` (o reset pode
 * passar sem leitura nova). Fora do `PlanProvider`, vazio.
 */
export function usePlan(now: number): PlanState {
  const s = useContext(Ctx) ?? NO_PROVIDER;
  const data = useMemo(() => planAt(s.data, now), [s.data, now]);
  return useMemo(() => ({ ...s, data }), [s, data]);
}

/** Conta de um ambiente em `now`: a conta, `"unknown"` ou `null` (sem dado). */
export function useEnvPlan(env: string | null | undefined, now: number): AccountPlanView | "unknown" | null {
  const { data } = usePlan(now);
  return env ? accountOfEnv(data, env) : null;
}
