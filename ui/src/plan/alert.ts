// Alerta do plano da conta (#108): o nível de cada janela pelos limiares da
// conta, o texto do aviso e a notificação ao cruzar um limiar. Funções puras;
// o provedor (usePlan.tsx) guarda a memória e notifica.
//
// Uma notificação por cruzamento e por conta: os ambientes da mesma conta
// já vêm juntos (`AccountPlanView`), e a memória guarda o último nível
// notificado de cada janela, no período dela (`resets_at`). Reler a mesma
// leitura não notifica de novo; janela renovada ou leitura velha demais não
// alerta.

import type { WindowView } from "../protocol/gen";
import type { AccountPlanView } from "./data";
import { accountLabel, isStale, whenText, windowTone, type StaleCheck } from "./format";
import { PLAN_WINDOWS, staleMs, thresholdsFor, type PlanPrefs, type PlanWindowKind } from "./thresholds";

export type AlertLevel = "warn" | "critical";

/** Uma janela acima de um limiar. */
export interface WindowAlert {
  kind: PlanWindowKind;
  level: AlertLevel;
  pct: number;
  resets_at: number;
}

/** Alerta de uma conta: o nível mais alto e as janelas que o causam. */
export interface PlanAlert {
  level: AlertLevel;
  windows: WindowAlert[];
}

const RANK: Record<AlertLevel, number> = { warn: 1, critical: 2 };

/** Nível da janela em `now`; `null` abaixo do aviso, sem percentual (renovada, sem leitura) ou leitura velha. */
export function windowLevel(w: WindowView, a: AccountPlanView, kind: PlanWindowKind, prefs: PlanPrefs, now: number): AlertLevel | null {
  const stale: StaleCheck = { now, staleMs: staleMs(prefs) };
  const tone = windowTone(w, thresholdsFor(prefs, a.key)[kind], stale);
  return tone === "warn" || tone === "critical" ? tone : null;
}

/** Alerta da conta em `now`, ou `null` (tudo abaixo do aviso, sem leitura válida). */
export function accountAlert(a: AccountPlanView, prefs: PlanPrefs, now: number): PlanAlert | null {
  const windows: WindowAlert[] = [];
  for (const kind of PLAN_WINDOWS) {
    const w = a.plan[kind];
    const level = windowLevel(w, a, kind, prefs, now);
    if (level && w.state === "ok") windows.push({ kind, level, pct: w.used_percentage, resets_at: w.resets_at });
  }
  if (!windows.length) return null;
  return { level: windows.some((w) => w.level === "critical") ? "critical" : "warn", windows };
}

/** "janela de 5h em 96%, libera às 14:30". */
export function windowAlertText(w: WindowAlert, now: number): string {
  return `janela ${w.kind === "five_hour" ? "de 5h" : "semanal"} em ${Math.round(w.pct)}%, libera ${whenText(w.resets_at, now)}`;
}

/** Texto do aviso da conta: "Conta fulano@example.com: janela de 5h em 96%, libera às 14:30." */
export function alertText(a: AccountPlanView, alert: PlanAlert, now: number): string {
  return `Conta ${accountLabel(a)}: ${alert.windows.map((w) => windowAlertText(w, now)).join("; ")}.`;
}

export const ALERT_TITLE: Record<AlertLevel, string> = {
  warn: "Plano da conta perto do limite",
  critical: "Plano da conta quase esgotado",
};

// --- Memória dos cruzamentos ---

/** Último nível notificado de uma janela, no período que termina em `resets_at`. */
export interface Notified {
  level: AlertLevel;
  resets_at: number;
}

/** Por conta (`account.key`) e janela. */
export type AlertMemory = Record<string, Partial<Record<PlanWindowKind, Notified>>>;

/**
 * Tolerância para o mesmo período: o `resets_at` de leituras da mesma
 * janela pode variar um pouco; um período novo começa horas depois.
 */
export const SAME_PERIOD_MS = 5 * 60_000;

const samePeriod = (a: number, b: number) => Math.abs(a - b) <= SAME_PERIOD_MS;

/** Notificação de uma conta. */
export interface PlanNotice {
  key: string;
  level: AlertLevel;
  title: string;
  body: string;
}

/**
 * Cruzamentos desde a última avaliação: a memória nova e uma notificação
 * por conta que subiu de nível numa janela (normal → aviso, aviso →
 * crítico, ou já acima do aviso num período novo).
 *
 * - Mesma leitura, ou leitura nova no mesmo nível: nada.
 * - Abaixo do aviso (ou limiar mudado para cima): a janela volta a armar.
 * - Janela renovada: a memória dela sai; o próximo período alerta de novo.
 * - Sem leitura ou leitura velha demais: nada muda (nem alerta, nem arma).
 */
export function crossings(
  memory: AlertMemory,
  accounts: AccountPlanView[],
  prefs: PlanPrefs,
  now: number,
): { memory: AlertMemory; notices: PlanNotice[] } {
  const next: AlertMemory = { ...memory };
  const notices: PlanNotice[] = [];
  const stale: StaleCheck = { now, staleMs: staleMs(prefs) };
  for (const a of accounts) {
    const mem: Partial<Record<PlanWindowKind, Notified>> = { ...(memory[a.key] ?? {}) };
    const crossed: WindowAlert[] = [];
    for (const kind of PLAN_WINDOWS) {
      const w = a.plan[kind];
      if (w.state === "renewed") {
        delete mem[kind];
        continue;
      }
      if (w.state !== "ok" || isStale(w, stale)) continue;
      const level = windowLevel(w, a, kind, prefs, now);
      const prev = mem[kind];
      const prevLevel = prev && samePeriod(prev.resets_at, w.resets_at) ? prev.level : null;
      if (!level) {
        delete mem[kind];
        continue;
      }
      if (!prevLevel || RANK[level] > RANK[prevLevel]) {
        crossed.push({ kind, level, pct: w.used_percentage, resets_at: w.resets_at });
      }
      mem[kind] = { level, resets_at: w.resets_at };
    }
    if (Object.keys(mem).length) next[a.key] = mem;
    else delete next[a.key];
    if (crossed.length) {
      const level: AlertLevel = crossed.some((c) => c.level === "critical") ? "critical" : "warn";
      notices.push({ key: a.key, level, title: ALERT_TITLE[level], body: alertText(a, { level, windows: crossed }, now) });
    }
  }
  return { memory: prune(next, now), notices };
}

/** Tira da memória os períodos já encerrados (o próximo alerta é de outro período). */
export function prune(memory: AlertMemory, now: number): AlertMemory {
  const out: AlertMemory = {};
  for (const [key, m] of Object.entries(memory)) {
    const kept = Object.fromEntries(Object.entries(m).filter(([, n]) => n && n.resets_at > now));
    if (Object.keys(kept).length) out[key] = kept;
  }
  return out;
}

/** Chave da memória no armazenamento do navegador (por máquina, como as preferências). */
export const ALERT_MEMORY_KEY = "ct.plan.alerts";

export function loadAlertMemory(): AlertMemory {
  try {
    const raw = localStorage.getItem(ALERT_MEMORY_KEY);
    const m: unknown = raw ? JSON.parse(raw) : {};
    return typeof m === "object" && m !== null && !Array.isArray(m) ? (m as AlertMemory) : {};
  } catch {
    return {};
  }
}

export function saveAlertMemory(m: AlertMemory): void {
  try {
    localStorage.setItem(ALERT_MEMORY_KEY, JSON.stringify(m));
  } catch {
    // armazenamento indisponível: a memória fica só nesta execução
  }
}
