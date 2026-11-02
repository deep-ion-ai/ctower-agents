// Textos e tom do selo do plano (#107). Nunca mostra o percentual de janela
// renovada nem número que não veio de uma leitura.

import type { PlanView, StatuslineHealth, WindowView } from "../protocol/gen";
import { ago } from "../util/time";
import type { AccountPlanView } from "./data";
import { usedPercentage } from "./rule";
import {
  DEFAULT_PLAN_THRESHOLDS,
  PLAN_WINDOWS,
  windowThresholds,
  type PlanThresholds,
  type PlanWindowThresholds,
} from "./thresholds";

/** `none` = sem percentual válido (sem leitura ou janelas renovadas). */
export type PlanTone = "normal" | "warn" | "critical" | "none";

const pad = (n: number) => String(n).padStart(2, "0");

/** "14:30" (hora local). */
export function clockText(ms: number): string {
  const d = new Date(ms);
  return `${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

/** Horário de reset: "às 14:30" no mesmo dia de `now`; senão "em 05/11 às 14:30". */
export function whenText(ms: number, now: number): string {
  const d = new Date(ms);
  const n = new Date(now);
  const sameDay = d.getFullYear() === n.getFullYear() && d.getMonth() === n.getMonth() && d.getDate() === n.getDate();
  return sameDay ? `às ${clockText(ms)}` : `em ${pad(d.getDate())}/${pad(d.getMonth() + 1)} às ${clockText(ms)}`;
}

/** Tom pelo percentual: aviso a partir de `warn`, crítico a partir de `critical`. */
export function toneOf(pct: number | null, t: PlanThresholds = DEFAULT_PLAN_THRESHOLDS): PlanTone {
  if (pct === null) return "none";
  if (pct >= t.critical) return "critical";
  if (pct >= t.warn) return "warn";
  return "normal";
}

/** Relógio e prazo para descartar leitura velha demais (#108). */
export interface StaleCheck {
  now: number;
  /** Leitura mais velha que isto (ms) não vale para alerta. */
  staleMs: number;
}

/** Janela com percentual de uma leitura mais velha que o prazo. */
export function isStale(w: WindowView, s: StaleCheck | undefined): boolean {
  return !!s && w.state === "ok" && s.now - w.read_at > s.staleMs;
}

/**
 * Tom de uma janela (renovada ou sem leitura: `none`). Com `stale`, a
 * leitura velha demais também fica `none`: o número aparece, sem alerta.
 */
export function windowTone(w: WindowView, t: PlanThresholds = DEFAULT_PLAN_THRESHOLDS, stale?: StaleCheck): PlanTone {
  if (isStale(w, stale)) return "none";
  return toneOf(usedPercentage(w), t);
}

const RANK: Record<PlanTone, number> = { none: 0, normal: 1, warn: 2, critical: 3 };

/** O tom mais alto. */
export const maxTone = (tones: PlanTone[]): PlanTone => tones.reduce<PlanTone>((m, t) => (RANK[t] > RANK[m] ? t : m), "none");

/** Tom do selo: o da janela em pior situação, cada uma com o seu limiar. */
export function planTone(
  v: PlanView,
  t: PlanThresholds | PlanWindowThresholds = DEFAULT_PLAN_THRESHOLDS,
  stale?: StaleCheck,
): PlanTone {
  return maxTone(PLAN_WINDOWS.map((k) => windowTone(v[k], windowThresholds(t, k), stale)));
}

/** Percentual inteiro, como o Claude Code mostra. */
const pctText = (p: number) => `${Math.round(p)}%`;

/** A visão não tem nenhuma leitura. */
export const noReading = (v: PlanView) => v.five_hour.state === "none" && v.seven_day.state === "none";

/**
 * Texto curto do selo: "5h 72% · libera às 14:30 · sem. 64%". Janela
 * renovada: "5h renovada às 14:30 · … · sem leitura nova", sem percentual.
 * Sem leitura: "sem leitura do plano".
 */
export function planText(v: PlanView, now: number): string {
  if (noReading(v)) return "sem leitura do plano";
  const parts: string[] = [];
  const f = v.five_hour;
  if (f.state === "ok") parts.push(`5h ${pctText(f.used_percentage)}`, `libera ${whenText(f.resets_at, now)}`);
  else if (f.state === "renewed") parts.push(`5h renovada ${whenText(f.resets_at, now)}`);
  const s = v.seven_day;
  if (s.state === "ok") parts.push(`sem. ${pctText(s.used_percentage)}`);
  else if (s.state === "renewed") parts.push(`sem. renovada ${whenText(s.resets_at, now)}`);
  if (f.state === "renewed" || s.state === "renewed") parts.push("sem leitura nova");
  return parts.join(" · ");
}

/** Uma janela em linha própria (lista de contas, tooltip). */
export function windowLine(kind: "five_hour" | "seven_day", w: WindowView, now: number): string {
  const name = kind === "five_hour" ? "5h" : "Semanal";
  if (w.state === "ok") return `${name}: ${pctText(w.used_percentage)} · libera ${whenText(w.resets_at, now)}`;
  if (w.state === "renewed") return `${name}: renovada ${whenText(w.resets_at, now)} · sem leitura nova`;
  return `${name}: sem leitura`;
}

/** "atualizado há 3 min"; `null` sem leitura. */
export function readAgoText(v: PlanView, now: number): string | null {
  return v.read_at === null ? null : `atualizado ${ago(v.read_at, now)}`;
}

const list = (names: string[]) =>
  names.length <= 1 ? (names[0] ?? "") : `${names.slice(0, -1).join(", ")} e ${names[names.length - 1]}`;

/**
 * Rótulo da conta: e-mail ou organização (só na UI local); na web, sem
 * os dois, pelos ambientes que a usam.
 */
export function accountLabel(a: AccountPlanView): string {
  return a.account.email ?? a.account.org_name ?? (a.envs.length ? `Conta de ${list(a.envs.map((e) => e.name))}` : a.key);
}

/** Tooltip do selo: a conta, as janelas, a leitura e o aviso de que o uso é da conta toda. */
export function planTitle(a: AccountPlanView, now: number): string {
  const lines = [`Plano da conta ${accountLabel(a)}${a.account.subscription_type ? ` (${a.account.subscription_type})` : ""}`];
  if (noReading(a.plan)) lines.push("Sem leitura do plano: nenhuma sessão desta conta gravou uso ainda.");
  else {
    lines.push(windowLine("five_hour", a.plan.five_hour, now), windowLine("seven_day", a.plan.seven_day, now));
    if (a.plan.read_at !== null) lines.push(`Última leitura ${ago(a.plan.read_at, now)}.`);
  }
  lines.push("O percentual é da conta toda (todos os ambientes e projetos nela), não só deste projeto.");
  return lines.join("\n");
}

/** Explicação para ambiente de conta desconhecida. */
export const UNKNOWN_ACCOUNT_TEXT =
  "Conta desconhecida: sem login no Claude Code ou o claude auth status falhou. O ambiente fica fora do plano das contas.";

/**
 * Por que o ambiente pode não ter leitura do plano: statusline com erro,
 * não instalado ou desatualizado; `null` = instalado e em dia (ou sem dado).
 */
export function statuslineIssue(s: StatuslineHealth | null | undefined): string | null {
  if (!s) return null;
  if (s.error) return `statusline com erro: ${s.error}`;
  if (!s.installed) return "statusline não instalado: sem leitura do plano";
  if (s.version !== null && s.version < s.current) return "statusline desatualizado";
  return null;
}
