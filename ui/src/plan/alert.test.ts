// Alerta do plano da conta (#108): nível, texto e cruzamentos, com relógio fixo e leituras fictícias.
import { describe, expect, it } from "vitest";
import type { PlanView, WindowView } from "../protocol/gen";
import { accountAlert, alertText, crossings, prune, type AlertMemory } from "./alert";
import type { AccountPlanView } from "./data";
import { DEFAULT_PLAN_PREFS, type PlanPrefs } from "./thresholds";

// Hora local: os textos usam o relógio da máquina.
const at = (d: number, h: number, m = 0) => new Date(2026, 10, d, h, m).getTime();
const NOW = at(1, 12, 0);
const MIN = 60_000;
const RESET_5H = at(1, 14, 30);
const RESET_SEM = at(5, 9);

const ok = (p: number, resets = RESET_5H, read = NOW - 3 * MIN): WindowView => ({
  state: "ok",
  used_percentage: p,
  resets_at: resets,
  read_at: read,
});
const NONE: WindowView = { state: "none" };

const plan = (five: WindowView, seven: WindowView = ok(10, RESET_SEM)): PlanView => ({
  five_hour: five,
  seven_day: seven,
  read_at: NOW - 3 * MIN,
});

const conta = (key: string, p: PlanView, envs = ["env-1"]): AccountPlanView => ({
  key,
  account: { key, email: `${key}@example.com`, org_name: null, subscription_type: "max" },
  plan: p,
  envs: envs.map((name) => ({ env: name, name, statusline: null })),
});

const PREFS = DEFAULT_PLAN_PREFS;

describe("nível do alerta da conta", () => {
  it("abaixo do aviso, aviso e crítico pelos limiares padrão (80/95)", () => {
    expect(accountAlert(conta("conta-a", plan(ok(79))), PREFS, NOW)).toBeNull();
    expect(accountAlert(conta("conta-a", plan(ok(80))), PREFS, NOW)?.level).toBe("warn");
    expect(accountAlert(conta("conta-a", plan(ok(95))), PREFS, NOW)?.level).toBe("critical");
  });

  it("a janela semanal alerta sozinha; o nível é o da pior janela", () => {
    const a = accountAlert(conta("conta-a", plan(ok(50), ok(96, RESET_SEM))), PREFS, NOW);
    expect(a).toEqual({ level: "critical", windows: [{ kind: "seven_day", level: "critical", pct: 96, resets_at: RESET_SEM }] });
    expect(accountAlert(conta("conta-a", plan(ok(85), ok(97, RESET_SEM))), PREFS, NOW)?.windows).toHaveLength(2);
  });

  it("limiares por janela e por conta", () => {
    const prefs: PlanPrefs = {
      ...PREFS,
      default: { five_hour: { warn: 60, critical: 70 }, seven_day: { warn: 80, critical: 95 } },
      accounts: { "conta-b": { five_hour: { warn: 90, critical: 99 }, seven_day: { warn: 80, critical: 95 } } },
    };
    expect(accountAlert(conta("conta-a", plan(ok(72))), prefs, NOW)?.level).toBe("critical");
    expect(accountAlert(conta("conta-b", plan(ok(72))), prefs, NOW)).toBeNull();
    // A semanal da conta-a segue 80/95.
    expect(accountAlert(conta("conta-a", plan(ok(10), ok(72, RESET_SEM))), prefs, NOW)).toBeNull();
  });

  it("janela renovada, sem leitura ou leitura velha demais: sem alerta", () => {
    const renewed: WindowView = { state: "renewed", resets_at: NOW - MIN, read_at: NOW - 10 * MIN };
    expect(accountAlert(conta("conta-a", plan(renewed, NONE)), PREFS, NOW)).toBeNull();
    expect(accountAlert(conta("conta-a", plan(NONE, NONE)), PREFS, NOW)).toBeNull();
    // Padrão: 60 min. Lida há 61 min não alerta; há 59, alerta.
    expect(accountAlert(conta("conta-a", plan(ok(99, RESET_5H, NOW - 61 * MIN))), PREFS, NOW)).toBeNull();
    expect(accountAlert(conta("conta-a", plan(ok(99, RESET_5H, NOW - 59 * MIN))), PREFS, NOW)?.level).toBe("critical");
    // Prazo configurável.
    const curto = { ...PREFS, stale_minutes: 5 };
    expect(accountAlert(conta("conta-a", plan(ok(99, RESET_5H, NOW - 10 * MIN))), curto, NOW)).toBeNull();
  });

  it("texto com o percentual e o horário de reset", () => {
    const a = conta("conta-a", plan(ok(96.4), ok(82, RESET_SEM)));
    expect(alertText(a, accountAlert(a, PREFS, NOW)!, NOW)).toBe(
      "Conta conta-a@example.com: janela de 5h em 96%, libera às 14:30; janela semanal em 82%, libera em 05/11 às 09:00.",
    );
  });
});

describe("notificação ao cruzar um limiar", () => {
  const run = (mem: AlertMemory, accounts: AccountPlanView[], now = NOW, prefs = PREFS) => crossings(mem, accounts, prefs, now);

  it("uma vez por cruzamento: reler a mesma leitura não notifica de novo", () => {
    const a = conta("conta-a", plan(ok(85)));
    const r1 = run({}, [a]);
    expect(r1.notices).toEqual([
      {
        key: "conta-a",
        level: "warn",
        title: "Plano da conta perto do limite",
        body: "Conta conta-a@example.com: janela de 5h em 85%, libera às 14:30.",
      },
    ]);
    expect(run(r1.memory, [a]).notices).toEqual([]);
    // Leitura nova, mesmo nível: nada.
    expect(run(r1.memory, [conta("conta-a", plan(ok(90, RESET_5H, NOW)))]).notices).toEqual([]);
  });

  it("aviso → crítico notifica de novo; crítico de novo, não", () => {
    const r1 = run({}, [conta("conta-a", plan(ok(85)))]);
    const r2 = run(r1.memory, [conta("conta-a", plan(ok(96)))]);
    expect(r2.notices.map((n) => [n.level, n.title])).toEqual([["critical", "Plano da conta quase esgotado"]]);
    expect(run(r2.memory, [conta("conta-a", plan(ok(98)))]).notices).toEqual([]);
  });

  it("vários ambientes da mesma conta: uma notificação só, com as duas janelas", () => {
    const a = conta("conta-a", plan(ok(96), ok(85, RESET_SEM)), ["env-1", "env-2", "env-3"]);
    const { notices } = run({}, [a]);
    expect(notices).toHaveLength(1);
    expect(notices[0].level).toBe("critical");
    expect(notices[0].body).toContain("janela de 5h em 96%");
    expect(notices[0].body).toContain("janela semanal em 85%");
  });

  it("contas diferentes: uma notificação para cada uma", () => {
    const { notices } = run({}, [conta("conta-a", plan(ok(85))), conta("conta-b", plan(ok(96))), conta("conta-c", plan(ok(10)))]);
    expect(notices.map((n) => [n.key, n.level])).toEqual([
      ["conta-a", "warn"],
      ["conta-b", "critical"],
    ]);
  });

  it("janela renovada e leitura velha não disparam; a renovada volta a armar", () => {
    const r1 = run({}, [conta("conta-a", plan(ok(85)))]);
    // Velha demais: nem alerta, nem esquece.
    const velha = run(r1.memory, [conta("conta-a", plan(ok(97, RESET_5H, NOW - 2 * 60 * MIN)))]);
    expect(velha.notices).toEqual([]);
    expect(velha.memory).toEqual(r1.memory);
    // Renovada (agora >= resets_at): sem alerta, e a memória da janela sai.
    const depois = RESET_5H + MIN;
    const renewed: WindowView = { state: "renewed", resets_at: RESET_5H, read_at: NOW };
    const r2 = run(r1.memory, [conta("conta-a", plan(renewed))], depois);
    expect(r2.notices).toEqual([]);
    expect(r2.memory["conta-a"]?.five_hour).toBeUndefined();
    // Período novo cruzando de novo: notifica.
    const novo = at(1, 19, 30);
    expect(run(r2.memory, [conta("conta-a", plan(ok(82, novo, depois)))], depois).notices).toHaveLength(1);
  });

  it("período novo (outro resets_at) já acima do aviso notifica, mesmo com a memória do anterior", () => {
    const r1 = run({}, [conta("conta-a", plan(ok(85)))]);
    const novo = at(1, 19, 30);
    expect(run(r1.memory, [conta("conta-a", plan(ok(85, novo)))]).notices).toHaveLength(1);
  });

  it("abaixo do aviso volta a armar", () => {
    const r1 = run({}, [conta("conta-a", plan(ok(85)))]);
    const baixo = run(r1.memory, [conta("conta-a", plan(ok(50)))]);
    expect(baixo.notices).toEqual([]);
    expect(baixo.memory["conta-a"]).toBeUndefined();
    expect(run(baixo.memory, [conta("conta-a", plan(ok(85)))]).notices).toHaveLength(1);
  });

  it("memória: períodos encerrados saem", () => {
    const mem: AlertMemory = {
      "conta-a": { five_hour: { level: "warn", resets_at: NOW - MIN }, seven_day: { level: "critical", resets_at: RESET_SEM } },
      "conta-b": { five_hour: { level: "warn", resets_at: NOW - MIN } },
    };
    expect(prune(mem, NOW)).toEqual({ "conta-a": { seven_day: { level: "critical", resets_at: RESET_SEM } } });
  });
});
