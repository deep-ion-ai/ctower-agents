// Visão por sessão (#109): ordenação, agrupamento por conta e textos.
import type { PlanAccount, PlanView, SessionUsage } from "../protocol/gen";
import type { PlanData } from "../plan/data";
import { session, workItem } from "../test/fixtures";
import { contextPctText, costOf, groupByAccount, idleMs, modelText, sortRows, type UsageRow } from "./session-usage";

const NOW = 1_800_000_000_000;
const MIN = 60_000;
const NONE: PlanView = { five_hour: { state: "none" }, seven_day: { state: "none" }, read_at: null };
const conta = (key: string): PlanAccount => ({ key, email: `${key}@example.com`, org_name: null, subscription_type: null });

const usage = (u: Partial<SessionUsage> = {}): SessionUsage => ({
  tool_session_id: "t-1",
  written_at: NOW - MIN,
  model: { id: "claude-opus", display_name: "Opus" },
  effort: "high",
  cost: { total_cost_usd: 1.5, total_duration_ms: null, total_api_duration_ms: null },
  context_window: { context_window_size: 200_000, used_percentage: 42.4, used_tokens: 84_800 },
  prompt_cache: null,
  plan: null,
  ...u,
});

const row = (id: string, env: string, r: Partial<UsageRow> = {}): UsageRow => ({
  env,
  envName: env,
  s: session({ id, state: "idle", started_at: NOW - 60 * MIN, last_event_at: NOW - 10 * MIN }),
  usage: undefined,
  ...r,
});

const cost = (usd: number) => ({ total_cost_usd: usd, total_duration_ms: null, total_api_duration_ms: null });

test("tempo parada: desde o último evento; rodando não conta", () => {
  expect(idleMs(row("s-1", "env-1"), NOW)).toBe(10 * MIN);
  expect(idleMs(row("s-1", "env-1", { wi: workItem({ status: "aguardando_voce" }) }), NOW)).toBe(10 * MIN);
  expect(idleMs(row("s-1", "env-1", { s: session({ state: "running" }) }), NOW)).toBeNull();
});

test("custo: só o da leitura, sem número inventado", () => {
  expect(costOf(usage())).toBe(1.5);
  expect(costOf(null)).toBeNull();
  expect(costOf(undefined)).toBeNull();
  expect(costOf(usage({ cost: null }))).toBeNull();
});

test("ordena por custo ou por tempo parada; sem número vai para o fim", () => {
  const a = row("s-1", "env-1", { usage: usage({ cost: cost(0.4) }) });
  const b = row("s-2", "env-1", {
    usage: usage(),
    s: session({ id: "s-2", state: "idle", started_at: NOW - 30 * MIN, last_event_at: NOW - 2 * MIN }),
  });
  const c = row("s-3", "env-1", { usage: null, s: session({ id: "s-3", state: "running", started_at: NOW - 5 * MIN }) });
  expect(sortRows([c, a, b], "cost", NOW).map((r) => r.s.id)).toEqual(["s-2", "s-1", "s-3"]);
  expect(sortRows([c, b, a], "idle", NOW).map((r) => r.s.id)).toEqual(["s-1", "s-2", "s-3"]);
});

test("agrupa pela conta do ambiente; conta desconhecida e ambiente sem dado à parte, no fim", () => {
  const plan: PlanData = {
    accounts: [
      {
        key: "conta-a",
        account: conta("conta-a"),
        plan: NONE,
        envs: [
          { env: "env-1", name: "env-1", statusline: null },
          { env: "env-2", name: "env-2", statusline: null },
        ],
      },
      { key: "conta-b", account: conta("conta-b"), plan: NONE, envs: [{ env: "env-4", name: "env-4", statusline: null }] },
    ],
    unknown: [{ env: "env-3", name: "env-3", statusline: null }],
    source: "envs",
  };
  const groups = groupByAccount([row("s-1", "env-3"), row("s-2", "env-2"), row("s-3", "env-1"), row("s-4", "env-9")], plan);
  expect(groups.map((g) => [g.key, g.rows.map((r) => r.s.id)])).toEqual([
    ["conta-a", ["s-2", "s-3"]],
    ["unknown", ["s-1", "s-4"]],
  ]);
  expect(groups[1].account).toBeNull();
});

test("contexto em % da janela real; modelo e esforço", () => {
  expect(contextPctText(usage())).toBe("42% de 200k");
  expect(contextPctText(usage({ context_window: { context_window_size: 1_000_000, used_percentage: 7, used_tokens: null } }))).toBe(
    "7% de 1M",
  );
  expect(contextPctText(usage({ context_window: null }))).toBeNull();
  expect(contextPctText(null)).toBeNull();
  expect(modelText({ s: session(), usage: usage() })).toBe("Opus · esforço high");
  expect(modelText({ s: session({ model: "sonnet" }), usage: null })).toBe("sonnet");
  expect(modelText({ s: session(), usage: null })).toBeNull();
});
