import type { BudgetDecision } from "../protocol/hub-gen";
import {
  bandSummary,
  endInput,
  hhmmToMin,
  closingText,
  idleText,
  localTime,
  minToHHMM,
  nextOpenText,
  offsetLabel,
  OFFSETS,
  previewHeadline,
  reasonText,
} from "./autopilot";

// Terça, 2026-11-10 10:00 em GMT-3.
const TUE_10 = Date.UTC(2026, 10, 10, 13, 0);

function decision(d: Partial<BudgetDecision> = {}): BudgetDecision {
  return {
    can_start: true,
    reasons: [],
    band: "Fim de semana",
    five_hour_cap: 95,
    five_hour_used: 40,
    weekly_used: 30,
    weekly: {
      resets_at: TUE_10 + 86_400_000,
      workdays_total: 5,
      workdays_left: 0,
      reserve_limit: 100,
      pace: 0.755,
      pace_limit: 75.1,
      effective: 75.1,
    },
    next_open_at: TUE_10,
    now: TUE_10,
    ...d,
  };
}

test("horas: minutos ⇄ HH:MM, com 24:00 no fim", () => {
  expect(minToHHMM(480)).toBe("08:00");
  expect(minToHHMM(1440)).toBe("24:00");
  expect(hhmmToMin("08:00")).toBe(480);
  expect(hhmmToMin("8:30")).toBe(510);
  expect(hhmmToMin("24:00")).toBeNull();
  expect(hhmmToMin("00:00", true)).toBe(1440);
  expect(hhmmToMin("24:00", true)).toBe(1440);
  expect(hhmmToMin("25:00")).toBeNull();
  expect(hhmmToMin("ab")).toBeNull();
  expect(endInput(1440)).toBe("00:00");
  expect(endInput(1080)).toBe("18:00");
});

test("fuso", () => {
  expect(offsetLabel(-180)).toBe("GMT-3");
  expect(offsetLabel(330)).toBe("GMT+5:30");
  expect(offsetLabel(0)).toBe("GMT");
  expect(OFFSETS[0]).toBe(-720);
  expect(OFFSETS[OFFSETS.length - 1]).toBe(840);
  expect(OFFSETS).toContain(-180);
});

test("motivos em texto", () => {
  expect(reasonText({ kind: "band_closed", band: "Horário comercial" })).toBe("faixa fechada (Horário comercial)");
  expect(reasonText({ kind: "five_hour_over_cap", used: 61, cap: 60 })).toBe("janela de 5h em 61%, teto da faixa 60%");
  expect(reasonText({ kind: "weekly_over_limit", used: 40, limit: 13.25 })).toBe("semanal em 40%, limite agora 13.3%");
  expect(reasonText({ kind: "no_reading", window: "seven_day" })).toMatch(/semanal/);
  expect(reasonText({ kind: "after_cutoff" })).toMatch(/corte/);
});

test("prévia: agora pode usar até X%", () => {
  expect(previewHeadline(decision())).toBe("Agora pode usar até 75.1% da semanal e 95% da janela de 5h");
  expect(previewHeadline(decision({ five_hour_cap: null }))).toBe(
    "Agora pode usar até 75.1% da semanal, mas a faixa atual está fechada",
  );
  expect(previewHeadline(decision({ five_hour_cap: null, weekly: null }))).toMatch(/não pode usar nada/);
});

test("próxima faixa no fuso da política", () => {
  expect(localTime(TUE_10, -180)).toBe("ter 10:00");
  expect(nextOpenText(decision(), -180)).toBeNull();
  expect(nextOpenText(decision({ next_open_at: TUE_10 + 8 * 3600_000 }), -180)).toBe(
    "próxima faixa elegível abre ter 18:00 (GMT-3)",
  );
  expect(nextOpenText(decision({ next_open_at: null }), -180)).toMatch(/nenhuma faixa aberta/);
});

test("resumo da faixa", () => {
  expect(
    bandSummary({ label: "c", days: [0, 1, 2, 3, 4], start_min: 480, end_min: 1080, five_hour_cap: null, workday: true }),
  ).toBe("seg–sex 08:00–18:00 · fechada · dia útil");
  expect(bandSummary({ label: "f", days: [6, 5], start_min: 0, end_min: 1440, five_hour_cap: 95, workday: false })).toBe(
    "sáb, dom 00:00–24:00 · 5h até 95%",
  );
});

test("sessão autônoma ociosa", () => {
  expect(idleText(30)).toBe(
    "A sessão autônoma ociosa com PR aberto é encerrada (dá para retomá-la), e a sem PR depois de 30 min ociosa.",
  );
  expect(idleText(0)).toBe("A sessão autônoma ociosa com PR aberto é encerrada (dá para retomá-la).");
  expect(idleText(undefined)).toBe("");
  expect(idleText(30, true)).toContain("depois do merge do PR na branch de integração");
});

test("fechamento autônomo: tentativas do CI e “Aguardando você” (#184)", () => {
  expect(closingText({ idle_timeout_min: 30 })).toBe("");
  expect(closingText({ max_ci_attempts: 3, waiting_timeout_min: 20 })).toBe(
    "Com o PR na branch de integração, o hub devolve a falha do CI ao agente até 3 vezes antes de bloquear a issue e pede o merge com o CI verde; a sessão parada em “Aguardando você” por mais de 20 min é encerrada e a issue fica bloqueada.",
  );
  expect(closingText({ max_ci_attempts: 0, waiting_timeout_min: 0 })).toBe(
    "Com o PR na branch de integração, a primeira falha do CI bloqueia a issue e pede o merge com o CI verde; a sessão parada em “Aguardando você” não é bloqueada.",
  );
});
