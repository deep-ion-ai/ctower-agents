import { describe, expect, it } from "vitest";
import type { EnvAccount, PlanAccount, PlanReading, StatuslineHealth } from "../protocol/gen";
import type { HubPlanAccounts } from "../protocol/hub-gen";
import { accountOfEnv, fromEnvs, fromHub, mergeReadings, planAt } from "./data";
import { statusAccount } from "./PlanStatus";

const H = 3_600_000;
const SL: StatuslineHealth = { installed: true, version: 1, current: 1, chained: false, error: null };
const A: PlanAccount = { key: "org-a:fulano@example.com", email: "fulano@example.com", org_name: null, subscription_type: "max" };
const B: PlanAccount = { key: "org-b:beltrano@example.com", email: "beltrano@example.com", org_name: null, subscription_type: "pro" };

const reading = (read_at: number, five: number | null, seven: number | null): PlanReading => ({
  read_at,
  five_hour: five === null ? null : { used_percentage: five, resets_at: 12 * H },
  seven_day: seven === null ? null : { used_percentage: seven, resets_at: 100 * H },
});

const envAccount = (account: PlanAccount | null, latest: PlanReading | null): EnvAccount => ({
  account,
  statusline: SL,
  plan: { five_hour: { state: "none" }, seven_day: { state: "none" }, read_at: null },
  latest,
  now: 0,
});

describe("sem hub: agrupa pela conta", () => {
  it("cada conta uma vez, com todos os ambientes; desconhecida à parte", () => {
    const d = fromEnvs(
      [
        { env: "env-1", name: "env-1", data: envAccount(A, reading(9 * H, 50, 30)) },
        { env: "env-2", name: "env-2", data: envAccount(A, reading(10 * H, 72, null)) },
        { env: "env-3", name: "env-3", data: envAccount(B, reading(9 * H, 10, 5)) },
        { env: "env-4", name: "env-4", data: envAccount(null, null) },
      ],
      11 * H,
    );
    expect(d.source).toBe("envs");
    expect(d.accounts.map((a) => [a.key, a.envs.map((e) => e.env)])).toEqual([
      [A.key, ["env-1", "env-2"]],
      [B.key, ["env-3"]],
    ]);
    // Por janela, a leitura mais recente que a traz.
    const a = d.accounts[0].plan;
    expect(a.five_hour).toEqual({ state: "ok", used_percentage: 72, resets_at: 12 * H, read_at: 10 * H });
    expect(a.seven_day).toEqual({ state: "ok", used_percentage: 30, resets_at: 100 * H, read_at: 9 * H });
    expect(a.read_at).toBe(10 * H);
    expect(d.unknown.map((e) => e.env)).toEqual(["env-4"]);
    expect(accountOfEnv(d, "env-2")).toBe(d.accounts[0]);
    expect(accountOfEnv(d, "env-4")).toBe("unknown");
    expect(accountOfEnv(d, "env-9")).toBeNull();
  });

  it("leituras de contas diferentes nunca se misturam", () => {
    const d = fromEnvs(
      [
        { env: "env-1", name: "env-1", data: envAccount(A, reading(9 * H, 50, null)) },
        { env: "env-3", name: "env-3", data: envAccount(B, reading(10 * H, 99, null)) },
      ],
      11 * H,
    );
    expect(d.accounts[0].plan.five_hour).toMatchObject({ used_percentage: 50 });
  });

  it("sem leitura em nenhum ambiente", () => {
    expect(mergeReadings([null, undefined], 0)).toEqual({ five_hour: { state: "none" }, seven_day: { state: "none" }, read_at: null });
  });
});

describe("com hub", () => {
  const h: HubPlanAccounts = {
    accounts: [
      {
        account: { key: "conta-1", email: null, org_name: null, subscription_type: "max" },
        plan: {
          five_hour: { state: "ok", used_percentage: 72, resets_at: 12 * H, read_at: 10 * H },
          seven_day: { state: "ok", used_percentage: 64, resets_at: 100 * H, read_at: 10 * H },
          read_at: 10 * H,
        },
        envs: [
          { env_id: "d-1", name: "env-1 (hub)", statusline: SL },
          { env_id: "d-remoto", name: "env-remoto", statusline: SL },
        ],
      },
    ],
    unknown: [{ env_id: "d-2", name: "env-2", statusline: { ...SL, installed: false, version: null } }],
    now: 11 * H,
  };

  it("troca o env_id do daemon pelo id da UI e usa o nome local", () => {
    const d = fromHub(h, { "d-1": "env-1", "d-2": "env-2" }, { "env-1": "env-1" });
    expect(d.source).toBe("hub");
    expect(d.accounts[0].envs.map((e) => [e.env, e.name])).toEqual([
      ["env-1", "env-1"],
      ["d-remoto", "env-remoto"],
    ]);
    expect(accountOfEnv(d, "env-1")).toBe(d.accounts[0]);
    expect(accountOfEnv(d, "env-2")).toBe("unknown");
  });

  it("reaplica a regra do reset no relógio da UI", () => {
    const d = planAt(fromHub(h, {}), 12 * H);
    expect(d.accounts[0].plan.five_hour).toEqual({ state: "renewed", resets_at: 12 * H, read_at: 10 * H });
    expect(d.accounts[0].plan.seven_day.state).toBe("ok");
  });

  it("barra de status: a conta do ambiente ativo, ou a única", () => {
    const d = fromHub(h, { "d-1": "env-1", "d-2": "env-2" });
    expect(statusAccount(d, "env-1")).toBe(d.accounts[0]);
    expect(statusAccount(d, "env-2")).toBe("unknown");
    expect(statusAccount(d, "env-9")).toBeNull();
    expect(statusAccount(d, undefined)).toBe(d.accounts[0]);
    expect(statusAccount({ ...d, accounts: [d.accounts[0], d.accounts[0]] }, undefined)).toBeNull();
  });
});
