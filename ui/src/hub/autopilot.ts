// Hub › Orçamento do piloto (#180, épico #178): textos e conversões da
// política de orçamento por conta (`hub.autopilot.policy.get|set|preview`).
// A regra em si é do hub (`ct_core::plan::budget`); aqui só se mostra o
// resultado. Funções puras, testadas em autopilot.test.ts.
import type {
  AutonomyWip,
  AutopilotStatus,
  BlockReason,
  BudgetDecision,
  BudgetPolicy,
  DigestProject,
  DigestUsage,
  TimeBand,
} from "../protocol/hub-gen";

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

// ---------------------------------------------------------------------------
// Painel do piloto automático e resumo da manhã (#185).

/** Protocolo do hub com o agendador e `hub.autopilot.status` (#183). */
export const AUTOPILOT_STATUS_PROTOCOL = "1.19";
/** Protocolo do hub com o resumo da manhã e `hub.autopilot.digest|wip` (#185). */
export const AUTOPILOT_DIGEST_PROTOCOL = "1.20";

export type PilotState = "paused" | "off" | "on" | "waiting_band" | "waiting_budget";

/**
 * Estado do piloto, pelo status: pausado; desligado (nenhuma conta com
 * política); ligado (alguma conta pode iniciar agora); aguardando faixa (o
 * horário não permite); ou aguardando orçamento (o uso está no limite).
 */
export function pilotState(s: Pick<AutopilotStatus, "settings" | "accounts">): PilotState {
  if (s.settings.paused) return "paused";
  if (s.accounts.length === 0) return "off";
  if (s.accounts.some((a) => a.decision.can_start)) return "on";
  const byTime = (r: BlockReason) => r.kind === "band_closed" || r.kind === "no_band" || r.kind === "after_cutoff";
  if (s.accounts.some((a) => a.decision.reasons.some(byTime))) return "waiting_band";
  return "waiting_budget";
}

export const PILOT_STATE_TEXT: Record<PilotState, string> = {
  paused: "Pausado",
  off: "Desligado",
  on: "Ligado",
  waiting_band: "Aguardando faixa",
  waiting_budget: "Aguardando orçamento",
};

export const PILOT_STATE_HINT: Record<PilotState, string> = {
  paused: "nenhuma sessão nova inicia; as em andamento terminam a issue",
  off: "nenhuma conta tem política de orçamento (Orçamento do plano, no app desktop)",
  on: "pode iniciar issues novas agora",
  waiting_band: "a faixa de horário atual não permite iniciar",
  waiting_budget: "o uso da conta está no limite da política",
};

/** "Agora pode usar até X% da semanal (em Y%)", pela decisão da conta. */
export function weeklyHeadline(d: BudgetDecision): string {
  if (!d.weekly) return "Sem leitura da semanal: nenhuma sessão informou o uso";
  const used = d.weekly_used != null ? ` (em ${pct(d.weekly_used)})` : "";
  return `Agora pode usar até ${pct(d.weekly.effective)} da semanal${used}`;
}

/**
 * Próxima faixa elegível, no fuso do aparelho (o status não traz a
 * política): "faixa elegível agora", "próxima faixa elegível: ter 18:00".
 */
export function nextBandText(d: BudgetDecision, offsetMin = -new Date(d.now).getTimezoneOffset()): string {
  if (d.next_open_at == null) return "nenhuma faixa aberta na política";
  if (d.next_open_at <= d.now) return "faixa elegível agora";
  return `próxima faixa elegível: ${localTime(d.next_open_at, offsetMin)}`;
}

/** Motivo de o item ter ficado de fora da fila (`QueueItem.skip`). */
export const SKIP_TEXT: Record<string, string> = {
  epic: "épico",
  status: "fora do backlog",
  not_in_mirror: "fora do espelho",
  not_approved: "sem rótulo de aprovação",
  no_priority: "sem prioridade",
  no_model: "sem modelo",
  live_session: "sessão em andamento",
};

export const skipText = (s: string) => SKIP_TEXT[s] ?? s;

/** Contagens de um projeto do resumo: "2 mescladas na wip · 1 bloqueada". */
export function digestCounts(p: DigestProject): string {
  const n = (v: number, one: string, many: string) => `${v} ${v === 1 ? one : many}`;
  const parts = [
    p.merged.length > 0 && n(p.merged.length, "mesclada na wip", "mescladas na wip"),
    p.blocked.length > 0 && n(p.blocked.length, "bloqueada", "bloqueadas"),
    p.decisions_pending > 0 && n(p.decisions_pending, "decisão para revisar", "decisões para revisar"),
    p.ahead > 0 && `${p.ahead} à frente da base`,
  ].filter(Boolean);
  return parts.length ? parts.join(" · ") : "nada novo";
}

/** Estado do CI do PR da wip: o do espelho, ou o aviso de que ele não tem. */
export const ciText = (ci: string | null) => (ci ? `CI: ${ci}` : "CI sem dados no espelho");

/** "conta-1: +15% no período, semanal em 35%". */
export function usageText(u: DigestUsage): string {
  if (u.weekly_used == null) return `${u.account}: sem leitura da semanal`;
  const delta = u.delta != null ? `+${pct(Math.round(u.delta * 10) / 10)} no período, ` : "";
  return `${u.account}: ${delta}semanal em ${pct(u.weekly_used)}`;
}

/** "2 issues à frente de develop" ou "alinhada com develop". */
export function aheadText(w: Pick<AutonomyWip, "ahead" | "base">): string {
  const n = w.ahead.length;
  return n === 0 ? `alinhada com ${w.base}` : `${n} ${n === 1 ? "issue" : "issues"} à frente de ${w.base}`;
}
