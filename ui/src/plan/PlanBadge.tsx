// Selo do plano de uma conta (#107): as duas janelas, o reset e a idade da
// leitura, com o tom pelos limiares. Recebe a conta pronta (de `usePlan`);
// quem usa decide onde fica (barra de status, lista de contas, cabeçalho
// de grupo na visão por sessão). Sem `thresholds`, os limiares da conta nas
// preferências (#108); leitura velha demais fica sem tom de alerta.
import "./plan.css";
import { refreshPlan, type AccountPlanView } from "./data";
import { noReading, planText, planTitle, planTone, readAgoText, windowLine, windowTone } from "./format";
import { staleMs, thresholdsFor, windowThresholds, type PlanThresholds, type PlanWindowThresholds } from "./thresholds";
import { usePlanPrefs } from "./usePlan";

export interface PlanBadgeProps {
  account: AccountPlanView;
  /** Relógio da UI (ms); a visão já deve vir com a regra do reset em `now` (`usePlan`). */
  now: number;
  /** `inline`: uma linha (barra de status); `lines`: uma janela por linha (lista de contas). */
  variant?: "inline" | "lines";
  /** Limiares fixos (um par para as duas janelas, ou por janela); padrão: os da conta nas preferências. */
  thresholds?: PlanThresholds | PlanWindowThresholds;
}

export function PlanBadge({ account, now, variant = "inline", thresholds }: PlanBadgeProps) {
  const prefs = usePlanPrefs();
  const t = thresholds ?? thresholdsFor(prefs, account.key);
  const stale = { now, staleMs: staleMs(prefs) };
  // Idempotente: protege quem passar uma visão sem a regra do reset em `now`.
  const v = refreshPlan(account.plan, now);
  const title = planTitle({ ...account, plan: v }, now);
  const read = readAgoText(v, now);
  if (variant === "inline") {
    return (
      <span className="plan-badge" data-tone={planTone(v, t, stale)} title={title}>
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
        <span key={k} className="plan-line" data-tone={windowTone(v[k], windowThresholds(t, k), stale)}>
          {windowLine(k, v[k], now)}
        </span>
      ))}
      {read && <span className="plan-age">{read}</span>}
    </span>
  );
}
