// Hub › Orçamento do piloto (#180, épico #178): textos e conversões da
// política de orçamento por conta (`hub.autopilot.policy.get|set|preview`).
// A regra em si é do hub (`ct_core::plan::budget`); aqui só se mostra o
// resultado. Funções puras, testadas em autopilot.test.ts.
import type { BlockReason, BudgetDecision, BudgetPolicy, TimeBand } from "../protocol/hub-gen";

/** Protocolo do hub com a política do piloto automático (#180). */
export const AUTOPILOT_PROTOCOL = "1.14";

/** Dias da semana na ordem da política (0 = segunda). */
export const DAY_LABELS = ["seg", "ter", "qua", "qui", "sex", "sáb", "dom"] as const;

const pad = (n: number) => String(n).padStart(2, "0");

/** Minutos do dia → "HH:MM" (1440 = "24:00"). */
export function minToHHMM(m: number): string {
  return `${pad(Math.floor(m / 60))}:${pad(m % 60)}`;
}

/**
 * "HH:MM" → minutos do dia; `null` se inválido. No fim da faixa, "00:00"
 * e "24:00" valem 1440 (até a meia-noite): o campo de hora não aceita 24:00.
 */
export function hhmmToMin(s: string, end = false): number | null {
  const m = /^(\d{1,2}):(\d{2})$/.exec(s.trim());
  if (!m) return null;
  const h = Number(m[1]);
  const min = Number(m[2]);
  if (min > 59 || h > 24 || (h === 24 && min > 0)) return null;
  const v = h * 60 + min;
  if (end) return v === 0 ? 1440 : v;
  return v === 1440 ? null : v;
}

/** Valor do campo de hora para o fim da faixa (1440 aparece como 00:00). */
export const endInput = (m: number) => minToHHMM(m === 1440 ? 0 : m);

/** Fuso em minutos → "GMT-3", "GMT+5:30", "GMT". */
export function offsetLabel(min: number): string {
  if (min === 0) return "GMT";
  const sign = min < 0 ? "-" : "+";
  const a = Math.abs(min);
  const h = Math.floor(a / 60);
  const m = a % 60;
  return `GMT${sign}${h}${m ? `:${pad(m)}` : ""}`;
}

/** Fusos do seletor, de GMT-12 a GMT+14, de meia em meia hora. */
export const OFFSETS: number[] = Array.from({ length: (840 + 720) / 30 + 1 }, (_, i) => -720 + i * 30);

const pct = (v: number) => `${Number.isInteger(v) ? v : v.toFixed(1)}%`;

/** Por que não inicia agora, em texto. */
export function reasonText(r: BlockReason): string {
  switch (r.kind) {
    case "band_closed":
      return `faixa fechada (${r.band})`;
    case "no_band":
      return "fora das faixas de horário";
    case "after_cutoff":
      return "dia útil, depois do corte de início";
    case "five_hour_over_cap":
      return `janela de 5h em ${pct(r.used)}, teto da faixa ${pct(r.cap)}`;
    case "weekly_over_limit":
      return `semanal em ${pct(r.used)}, limite agora ${pct(r.limit)}`;
    case "no_reading":
      return `sem leitura da janela ${r.window === "five_hour" ? "de 5h" : "semanal"} (nenhuma sessão informou o uso)`;
  }
}

/** "Agora pode usar até X% da semanal e Y% da janela de 5h". */
export function previewHeadline(d: BudgetDecision): string {
  const weekly = d.weekly ? `${pct(d.weekly.effective)} da semanal` : null;
  const five = d.five_hour_cap != null ? `${pct(d.five_hour_cap)} da janela de 5h` : null;
  if (!weekly && !five) return "Agora não pode usar nada: faixa fechada e sem leitura da semanal";
  if (!five) return `Agora pode usar até ${weekly}, mas a faixa atual está fechada`;
  return `Agora pode usar até ${[weekly, five].filter(Boolean).join(" e ")}`;
}

/** Dia da semana e hora no fuso da política: "ter 18:00". */
export function localTime(ms: number, offsetMin: number): string {
  const t = new Date(ms + offsetMin * 60_000);
  const wd = (t.getUTCDay() + 6) % 7;
  return `${DAY_LABELS[wd]} ${pad(t.getUTCHours())}:${pad(t.getUTCMinutes())}`;
}

/** Quando o horário volta a permitir iniciar; `null` se já permite. */
export function nextOpenText(d: BudgetDecision, offsetMin: number): string | null {
  if (d.next_open_at == null) return "nenhuma faixa aberta na política";
  if (d.next_open_at <= d.now) return null;
  return `próxima faixa elegível abre ${localTime(d.next_open_at, offsetMin)} (${offsetLabel(offsetMin)})`;
}

/** Faixa nova para o editor: fim de semana inteiro, aberto até 90%. */
export const newBand = (): TimeBand => ({
  label: "Nova faixa",
  days: [5, 6],
  start_min: 0,
  end_min: 1440,
  five_hour_cap: 90,
  workday: false,
});

/** Resumo de uma faixa: "seg–sex 08:00–18:00 · fechada · dia útil". */
export function bandSummary(b: TimeBand): string {
  const days = [...b.days].sort((x, y) => x - y);
  const contiguous = days.length > 2 && days.every((d, i) => i === 0 || d === days[i - 1] + 1);
  const dayText = contiguous
    ? `${DAY_LABELS[days[0]]}–${DAY_LABELS[days[days.length - 1]]}`
    : days.map((d) => DAY_LABELS[d]).join(", ");
  const cap = b.five_hour_cap == null ? "fechada" : `5h até ${pct(b.five_hour_cap)}`;
  return [`${dayText} ${minToHHMM(b.start_min)}–${minToHHMM(b.end_min)}`, cap, b.workday ? "dia útil" : null]
    .filter(Boolean)
    .join(" · ");
}

/** Cópia profunda (o editor muda o rascunho, nunca o que veio do hub). */
export const clonePolicy = (p: BudgetPolicy): BudgetPolicy => structuredClone(p);
