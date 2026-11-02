// Limiares do plano da conta (#107, configuráveis na #108): normal abaixo de
// `warn`, aviso a partir dele, crítico a partir de `critical` (em % de uso da
// janela). Por janela (5h e semanal) e por conta, com o padrão para as
// contas sem limiar próprio. Ficam nas preferências do app (`ui.json`, por
// máquina; na web, no navegador), em `plan`.

export interface PlanThresholds {
  /** % a partir do qual o selo fica em aviso. */
  warn: number;
  /** % a partir do qual o selo fica crítico. */
  critical: number;
}

export const DEFAULT_PLAN_THRESHOLDS: PlanThresholds = { warn: 80, critical: 95 };

/** As duas janelas do plano. */
export type PlanWindowKind = "five_hour" | "seven_day";

export const PLAN_WINDOWS: readonly PlanWindowKind[] = ["five_hour", "seven_day"];

/** Limiares de cada janela. */
export interface PlanWindowThresholds {
  five_hour: PlanThresholds;
  seven_day: PlanThresholds;
}

export const DEFAULT_WINDOW_THRESHOLDS: PlanWindowThresholds = {
  five_hour: DEFAULT_PLAN_THRESHOLDS,
  seven_day: DEFAULT_PLAN_THRESHOLDS,
};

/** Preferências do alerta do plano (`ui.json`, chave `plan`). */
export interface PlanPrefs {
  /** Notificação do sistema ao cruzar um limiar. */
  notify: boolean;
  /** Leitura mais velha que isto (min) não dispara alerta. */
  stale_minutes: number;
  /** Limiares das contas sem limiar próprio. */
  default: PlanWindowThresholds;
  /** Limiares próprios, pela chave da conta (`account.key`). */
  accounts: Record<string, PlanWindowThresholds>;
}

/** Padrão do prazo de leitura velha demais, em minutos. */
export const DEFAULT_STALE_MINUTES = 60;
export const MIN_STALE_MINUTES = 1;
/** Uma semana: a janela mais longa do plano. */
export const MAX_STALE_MINUTES = 7 * 24 * 60;

export const DEFAULT_PLAN_PREFS: PlanPrefs = {
  notify: true,
  stale_minutes: DEFAULT_STALE_MINUTES,
  default: DEFAULT_WINDOW_THRESHOLDS,
  accounts: {},
};

/** Erro dos limiares de uma janela, ou `null` se valem: inteiros de 1 a 100, aviso < crítico. */
export function thresholdsError(t: PlanThresholds): string | null {
  const ok = (n: number) => Number.isInteger(n) && n >= 1 && n <= 100;
  if (!ok(t.warn) || !ok(t.critical)) return "Use números inteiros de 1 a 100.";
  if (t.warn >= t.critical) return "O aviso precisa ser menor que o crítico.";
  return null;
}

/** Erro do prazo de leitura velha demais, ou `null`. */
export function staleError(min: number): string | null {
  return Number.isInteger(min) && min >= MIN_STALE_MINUTES && min <= MAX_STALE_MINUTES
    ? null
    : `Use minutos inteiros de ${MIN_STALE_MINUTES} a ${MAX_STALE_MINUTES}.`;
}

const isObj = (x: unknown): x is Record<string, unknown> => typeof x === "object" && x !== null && !Array.isArray(x);

/** Limiares de uma janela lidos do arquivo; inválidos ou ausentes: `fallback`. */
function readThresholds(raw: unknown, fallback: PlanThresholds): PlanThresholds {
  if (!isObj(raw)) return fallback;
  const t = { warn: Number(raw.warn), critical: Number(raw.critical) };
  return thresholdsError(t) === null ? t : fallback;
}

function readWindows(raw: unknown, fallback: PlanWindowThresholds): PlanWindowThresholds {
  const r = isObj(raw) ? raw : {};
  return {
    five_hour: readThresholds(r.five_hour, fallback.five_hour),
    seven_day: readThresholds(r.seven_day, fallback.seven_day),
  };
}

/**
 * Preferências do plano a partir do `ui.json` (ausente em arquivos antigos,
 * ou editado à mão): o que não vale volta ao padrão, campo a campo.
 */
export function planPrefs(raw: unknown): PlanPrefs {
  if (!isObj(raw)) return DEFAULT_PLAN_PREFS;
  const def = readWindows(raw.default, DEFAULT_WINDOW_THRESHOLDS);
  const accounts: Record<string, PlanWindowThresholds> = {};
  if (isObj(raw.accounts)) {
    for (const [k, v] of Object.entries(raw.accounts)) accounts[k] = readWindows(v, def);
  }
  const stale = Number(raw.stale_minutes);
  return {
    notify: raw.notify !== false,
    stale_minutes: staleError(stale) === null ? stale : DEFAULT_STALE_MINUTES,
    default: def,
    accounts,
  };
}

/** Limiares de uma conta: os próprios ou o padrão. */
export function thresholdsFor(p: PlanPrefs, key: string | null | undefined): PlanWindowThresholds {
  return (key !== null && key !== undefined && p.accounts[key]) || p.default;
}

/** Prazo de leitura velha demais em ms. */
export const staleMs = (p: PlanPrefs) => p.stale_minutes * 60_000;

/** Limiares de uma janela, de um par único (todas as janelas) ou por janela. */
export function windowThresholds(t: PlanThresholds | PlanWindowThresholds, k: PlanWindowKind): PlanThresholds {
  return "five_hour" in t ? t[k] : t;
}

export const sameThresholds = (a: PlanWindowThresholds, b: PlanWindowThresholds) =>
  PLAN_WINDOWS.every((k) => a[k].warn === b[k].warn && a[k].critical === b[k].critical);
