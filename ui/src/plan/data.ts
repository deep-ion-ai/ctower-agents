// Plano das contas na UI (#107): uma lista de contas, cada uma com as duas
// janelas e os ambientes que a usam, montada de `hub.plan.accounts` (com
// hub) ou dos `account.get` de cada ambiente (sem hub), agrupando pela
// conta. Funções puras; o hook fica em usePlan.tsx.

import type { EnvAccount, PlanAccount, PlanReading, PlanView, StatuslineHealth, WindowView } from "../protocol/gen";
import type { HubPlanAccounts } from "../protocol/hub-gen";
import { refreshView, windowView } from "./rule";

/** Um ambiente numa conta (ou de conta desconhecida). */
export interface PlanEnvRef {
  /** Id do ambiente na UI; o `env_id` do daemon quando a UI não conhece o ambiente. */
  env: string;
  name: string;
  /** Saúde do statusline; `null` = não informada. */
  statusline: StatuslineHealth | null;
}

/** Uma conta com o plano dela e os ambientes que a usam. */
export interface AccountPlanView {
  /** Chave de agrupamento (`account.key`); na web, `conta-N`. */
  key: string;
  account: PlanAccount;
  /** Regra do reset aplicada no relógio da fonte: reaplique com `planAt`. */
  plan: PlanView;
  envs: PlanEnvRef[];
}

export interface PlanData {
  accounts: AccountPlanView[];
  /** Ambientes de conta desconhecida (sem login ou `claude auth status` falhou). */
  unknown: PlanEnvRef[];
  /** De onde veio: o hub, os ambientes, ou nada lido ainda. */
  source: "hub" | "envs" | null;
}

export const EMPTY_PLAN: PlanData = { accounts: [], unknown: [], source: null };

/**
 * Resposta do hub no formato da UI. `envByDaemon` troca o `env_id` do
 * daemon pelo id do ambiente na UI; `names` dá o nome que a UI usa.
 */
export function fromHub(
  h: HubPlanAccounts,
  envByDaemon: Record<string, string>,
  names: Record<string, string> = {},
): PlanData {
  const ref = (e: HubPlanAccounts["unknown"][number]): PlanEnvRef => {
    const env = envByDaemon[e.env_id] ?? e.env_id;
    return { env, name: names[env] ?? e.name, statusline: e.statusline };
  };
  return {
    accounts: h.accounts.map((a) => ({ key: a.account.key, account: a.account, plan: a.plan, envs: a.envs.map(ref) })),
    unknown: h.unknown.map(ref),
    source: "hub",
  };
}

/** `account.get` de um ambiente. */
export interface EnvAccountEntry {
  env: string;
  name: string;
  data: EnvAccount;
}

/**
 * Plano de várias leituras da mesma conta: por janela, vale a leitura mais
 * recente que a traz (como o agregador do daemon e do hub), com a regra do
 * reset em `now`.
 */
export function mergeReadings(readings: (PlanReading | null | undefined)[], now: number): PlanView {
  const pick = (k: "five_hour" | "seven_day"): { view: WindowView; at: number | null } => {
    let best: PlanReading | null = null;
    for (const r of readings) {
      if (r?.[k] && (!best || r.read_at > best.read_at)) best = r;
    }
    return best ? { view: windowView(best[k], best.read_at, now), at: best.read_at } : { view: { state: "none" }, at: null };
  };
  const five = pick("five_hour");
  const seven = pick("seven_day");
  const ats = [five.at, seven.at].filter((x): x is number => x !== null);
  return { five_hour: five.view, seven_day: seven.view, read_at: ats.length ? Math.max(...ats) : null };
}

/** Sem hub: agrupa os ambientes pela `account.key`, em `now` (relógio da UI). */
export function fromEnvs(list: EnvAccountEntry[], now: number): PlanData {
  const groups = new Map<string, { account: PlanAccount; entries: EnvAccountEntry[] }>();
  const unknown: PlanEnvRef[] = [];
  for (const e of list) {
    const ref = { env: e.env, name: e.name, statusline: e.data.statusline };
    const a = e.data.account;
    if (!a) {
      unknown.push(ref);
      continue;
    }
    const g = groups.get(a.key);
    if (g) g.entries.push(e);
    else groups.set(a.key, { account: a, entries: [e] });
  }
  const accounts = [...groups.values()].map(({ account, entries }) => ({
    key: account.key,
    account,
    // Ambiente sem `latest` (daemon que só manda a visão): a visão dele não entra na mistura.
    plan: mergeReadings(
      entries.map((e) => e.data.latest),
      now,
    ),
    envs: entries.map((e) => ({ env: e.env, name: e.name, statusline: e.data.statusline })),
  }));
  return { accounts, unknown, source: "envs" };
}

/** Reaplica a regra do reset às duas janelas, com o relógio da UI. */
export function refreshPlan(v: PlanView, now: number): PlanView {
  return { ...v, five_hour: refreshView(v.five_hour, now), seven_day: refreshView(v.seven_day, now) };
}

/** Todas as contas com a regra do reset em `now` (o reset pode passar sem leitura nova). */
export function planAt(d: PlanData, now: number): PlanData {
  return { ...d, accounts: d.accounts.map((a) => ({ ...a, plan: refreshPlan(a.plan, now) })) };
}

/**
 * Plano de um ambiente: a conta dele, `"unknown"` (conta desconhecida) ou
 * `null` (o ambiente não está nos dados).
 */
export function accountOfEnv(d: PlanData, env: string): AccountPlanView | "unknown" | null {
  const a = d.accounts.find((x) => x.envs.some((e) => e.env === env));
  if (a) return a;
  return d.unknown.some((e) => e.env === env) ? "unknown" : null;
}
