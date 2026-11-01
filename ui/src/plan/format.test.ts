import { describe, expect, it } from "vitest";
import type { PlanView, StatuslineHealth } from "../protocol/gen";
import type { AccountPlanView } from "./data";
import {
  accountLabel,
  clockText,
  planText,
  planTitle,
  planTone,
  readAgoText,
  statuslineIssue,
  toneOf,
  whenText,
  windowLine,
} from "./format";

// Hora local: os textos usam o relógio da máquina.
const at = (d: number, h: number, m = 0) => new Date(2026, 10, d, h, m).getTime();
const NOW = at(1, 12, 0);
const MIN = 60_000;

const ok = (p: number, resets: number, read = NOW - 3 * MIN) =>
  ({ state: "ok", used_percentage: p, resets_at: resets, read_at: read }) as const;
const renewed = (resets: number, read = NOW - 3 * MIN) => ({ state: "renewed", resets_at: resets, read_at: read }) as const;
const NONE = { state: "none" } as const;

const view = (v: Partial<PlanView> = {}): PlanView => ({
  five_hour: ok(72, at(1, 14, 30)),
  seven_day: ok(64, at(5, 9)),
  read_at: NOW - 3 * MIN,
  ...v,
});

const account = (plan: PlanView, a: Partial<AccountPlanView["account"]> = {}, envs = ["env-1"]): AccountPlanView => ({
  key: "conta-a",
  account: { key: "conta-a", email: "fulano@example.com", org_name: null, subscription_type: "max", ...a },
  plan,
  envs: envs.map((name) => ({ env: name, name, statusline: null })),
});

describe("textos do selo", () => {
  it("horário de reset no mesmo dia e em outro dia", () => {
    expect(clockText(at(1, 9, 5))).toBe("09:05");
    expect(whenText(at(1, 14, 30), NOW)).toBe("às 14:30");
    expect(whenText(at(5, 9), NOW)).toBe("em 05/11 às 09:00");
  });

  it("as duas janelas com o reset da de 5h", () => {
    expect(planText(view(), NOW)).toBe("5h 72% · libera às 14:30 · sem. 64%");
    expect(readAgoText(view(), NOW)).toBe("atualizado há 3 min");
  });

  it("janela renovada: sem percentual", () => {
    const t = planText(view({ five_hour: renewed(at(1, 11)) }), NOW);
    expect(t).toBe("5h renovada às 11:00 · sem. 64% · sem leitura nova");
    expect(t).not.toContain("72");
    const both = planText(view({ five_hour: renewed(at(1, 11)), seven_day: renewed(at(1, 10)) }), NOW);
    expect(both).toBe("5h renovada às 11:00 · sem. renovada às 10:00 · sem leitura nova");
    expect(both).not.toMatch(/%/);
  });

  it("sem leitura: sem número", () => {
    const v = view({ five_hour: NONE, seven_day: NONE, read_at: null });
    expect(planText(v, NOW)).toBe("sem leitura do plano");
    expect(readAgoText(v, NOW)).toBeNull();
    expect(planTone(v)).toBe("none");
  });

  it("só uma janela na leitura", () => {
    expect(planText(view({ seven_day: NONE }), NOW)).toBe("5h 72% · libera às 14:30");
  });

  it("linhas por janela", () => {
    expect(windowLine("five_hour", ok(72, at(1, 14, 30)), NOW)).toBe("5h: 72% · libera às 14:30");
    expect(windowLine("seven_day", ok(64.4, at(5, 9)), NOW)).toBe("Semanal: 64% · libera em 05/11 às 09:00");
    expect(windowLine("five_hour", renewed(at(1, 11)), NOW)).toBe("5h: renovada às 11:00 · sem leitura nova");
    expect(windowLine("seven_day", NONE, NOW)).toBe("Semanal: sem leitura");
  });
});

describe("tom", () => {
  it("normal, aviso a partir de 80 e crítico a partir de 95", () => {
    expect(toneOf(79.9)).toBe("normal");
    expect(toneOf(80)).toBe("warn");
    expect(toneOf(95)).toBe("critical");
    expect(toneOf(null)).toBe("none");
    expect(toneOf(50, { warn: 40, critical: 60 })).toBe("warn");
  });

  it("a janela mais usada decide; renovada não conta", () => {
    expect(planTone(view({ seven_day: ok(96, at(5, 9)) }))).toBe("critical");
    expect(planTone(view({ five_hour: renewed(at(1, 11)), seven_day: ok(40, at(5, 9)) }))).toBe("normal");
  });

  it("limiares por janela (#108): cada janela com o seu", () => {
    const t = { five_hour: { warn: 70, critical: 90 }, seven_day: { warn: 50, critical: 60 } };
    // 5h 72% (aviso no limiar dela), semanal 64% (crítico no dela).
    expect(planTone(view(), t)).toBe("critical");
    expect(planTone(view({ seven_day: ok(10, at(5, 9)) }), t)).toBe("warn");
  });

  it("leitura velha demais (#108): o número fica, sem tom de alerta", () => {
    const v = view({ five_hour: ok(97, at(1, 14, 30), NOW - 90 * MIN), seven_day: NONE });
    expect(planTone(v)).toBe("critical");
    expect(planTone(v, undefined, { now: NOW, staleMs: 60 * MIN })).toBe("none");
    expect(planTone(v, undefined, { now: NOW, staleMs: 120 * MIN })).toBe("critical");
  });
});

describe("conta", () => {
  it("rótulo: e-mail, organização ou, na web, os ambientes", () => {
    expect(accountLabel(account(view()))).toBe("fulano@example.com");
    expect(accountLabel(account(view(), { email: null, org_name: "Acme" }))).toBe("Acme");
    expect(accountLabel(account(view(), { email: null, key: "conta-1" }, ["env-1", "env-2", "env-3"]))).toBe(
      "Conta de env-1, env-2 e env-3",
    );
  });

  it("tooltip diz que o percentual é da conta toda", () => {
    const t = planTitle(account(view()), NOW);
    expect(t).toContain("Plano da conta fulano@example.com (max)");
    expect(t).toContain("5h: 72% · libera às 14:30");
    expect(t).toContain("Última leitura há 3 min.");
    expect(t).toMatch(/da conta toda .* não só deste projeto/);
  });

  it("saúde do statusline", () => {
    const s = (p: Partial<StatuslineHealth>): StatuslineHealth => ({
      installed: true,
      version: 1,
      current: 1,
      chained: false,
      error: null,
      ...p,
    });
    expect(statuslineIssue(s({}))).toBeNull();
    expect(statuslineIssue(null)).toBeNull();
    expect(statuslineIssue(s({ installed: false, version: null }))).toBe("statusline não instalado: sem leitura do plano");
    expect(statuslineIssue(s({ version: 1, current: 2 }))).toBe("statusline desatualizado");
    expect(statuslineIssue(s({ error: "settings.json inválido" }))).toBe("statusline com erro: settings.json inválido");
  });
});
