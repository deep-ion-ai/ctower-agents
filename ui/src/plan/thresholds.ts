// Limiares do tom do selo do plano (#107): normal abaixo de `warn`, aviso a
// partir dele, crítico a partir de `critical` (em % de uso da janela). Um
// lugar só: a sub-issue dos alertas (#108) os torna configuráveis.

export interface PlanThresholds {
  /** % a partir do qual o selo fica em aviso. */
  warn: number;
  /** % a partir do qual o selo fica crítico. */
  critical: number;
}

export const DEFAULT_PLAN_THRESHOLDS: PlanThresholds = { warn: 80, critical: 95 };
