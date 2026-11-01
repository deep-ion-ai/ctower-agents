// Selo do plano de uma conta (#107): as duas janelas, o reset e a idade da
// leitura, com o tom pelos limiares. Recebe a conta pronta (de `usePlan`);
// quem usa decide onde fica (barra de status, lista de contas, cabeçalho
// de grupo na visão por sessão).
import "./plan.css";
import { refreshPlan, type AccountPlanView } from "./data";
import { noReading, planText, planTitle, planTone, readAgoText, windowLine, windowTone } from "./format";
import { DEFAULT_PLAN_THRESHOLDS, type PlanThresholds } from "./thresholds";

export interface PlanBadgeProps {
  account: AccountPlanView;
  /** Relógio da UI (ms); a visão já deve vir com a regra do reset em `now` (`usePlan`). */
  now: number;
  /** `inline`: uma linha (barra de status); `lines`: uma janela por linha (lista de contas). */
  variant?: "inline" | "lines";
  thresholds?: PlanThresholds;
}

export function PlanBadge({ account, now, variant = "inline", thresholds = DEFAULT_PLAN_THRESHOLDS }: PlanBadgeProps) {
  // Idempotente: protege quem passar uma visão sem a regra do reset em `now`.
  const v = refreshPlan(account.plan, now);
  const title = planTitle({ ...account, plan: v }, now);
  const read = readAgoText(v, now);
  if (variant === "inline") {
    return (
      <span className="plan-badge" data-tone={planTone(v, thresholds)} title={title}>
        <span className="plan-dot" aria-hidden="true" />
        {planText(v, now)}
        {read && <span className="plan-age">· {read}</span>}
      </span>
    );
  }
  if (noReading(v)) {
    return (
      <span className="plan-lines" title={title}>
        <span className="plan-line" data-tone="none">
          sem leitura do plano
        </span>
      </span>
    );
  }
  return (
    <span className="plan-lines" title={title}>
      {(["five_hour", "seven_day"] as const).map((k) => (
        <span key={k} className="plan-line" data-tone={windowTone(v[k], thresholds)}>
          {windowLine(k, v[k], now)}
        </span>
      ))}
      {read && <span className="plan-age">{read}</span>}
    </span>
  );
}
