// Plano da conta na modal de início de sessão (#108): o ambiente define a
// conta. A modal mostra o uso da conta de cada ambiente (para preferir um de
// outra conta com janela livre) e, antes do botão, o aviso da conta
// escolhida; no crítico, o início pede um passo de confirmação (não bloqueia:
// pode haver sessão urgente).
import "./plan.css";
import { accountOfEnv, type AccountPlanView } from "./data";
import { accountAlert, alertText, type PlanAlert } from "./alert";
import { accountLabel, planText, planTone, readAgoText } from "./format";
import { staleMs, thresholdsFor } from "./thresholds";
import { usePlan } from "./usePlan";

/** Conta do ambiente e o alerta dela em `now` (`null` sem conta conhecida ou sem alerta). */
export function usePlanGate(env: string | null | undefined, now: number): { account: AccountPlanView | null; alert: PlanAlert | null } {
  const { data, prefs } = usePlan(now);
  const a = env ? accountOfEnv(data, env) : null;
  const account = a && a !== "unknown" ? a : null;
  return { account, alert: account ? accountAlert(account, prefs, now) : null };
}

/**
 * Uso da conta de cada ambiente candidato; com mais de um, cada linha
 * escolhe o ambiente. Nada quando nenhum deles tem dado do plano.
 */
export function PlanEnvChoices({
  envs,
  chosen,
  onChoose,
  now,
}: {
  envs: { env: string; name: string }[];
  chosen: string | null | undefined;
  onChoose(env: string): void;
  now: number;
}) {
  const { data, prefs } = usePlan(now);
  const rows = envs.map((e) => ({ ...e, a: accountOfEnv(data, e.env) }));
  if (!rows.some((r) => r.a !== null)) return null;
  const pick = envs.length > 1;
  return (
    <div className="sm-field">
      <span className="sm-label">Uso do plano da conta {pick ? "por ambiente" : "do ambiente"}</span>
      <ul className="plan-choices" aria-label="Uso do plano por ambiente">
        {rows.map(({ env, name, a }) => {
          const known = a && a !== "unknown" ? a : null;
          const tone = known ? planTone(known.plan, thresholdsFor(prefs, known.key), { now, staleMs: staleMs(prefs) }) : "none";
          const text = known
            ? `${accountLabel(known)} · ${planText(known.plan, now)}${readAgoText(known.plan, now) ? ` · ${readAgoText(known.plan, now)}` : ""}`
            : a === "unknown"
              ? "conta desconhecida"
              : "sem dado do plano";
          const body = (
            <>
              <span className="plan-dot" aria-hidden="true" />
              <span className="plan-choice-env">{name}</span>
              <span className="plan-choice-text">{text}</span>
            </>
          );
          return (
            <li key={env} className="plan-choice" data-tone={tone} data-chosen={env === chosen ? "true" : undefined}>
              {pick ? (
                <button type="button" aria-pressed={env === chosen} aria-label={`${name}: ${text}`} onClick={() => onChoose(env)}>
                  {body}
                </button>
              ) : (
                <span className="plan-choice-row">{body}</span>
              )}
            </li>
          );
        })}
      </ul>
    </div>
  );
}

/** Aviso da conta escolhida, antes do botão: percentual e horário de reset. */
export function PlanStartNote({ account, alert, now }: { account: AccountPlanView; alert: PlanAlert; now: number }) {
  const critical = alert.level === "critical";
  return (
    <div className="sm-note plan-start-note" data-tone={alert.level} role="status" aria-label="Plano da conta">
      <span className="sp-dot" data-tone={critical ? "error" : "attention"} aria-hidden="true" />
      <span className="sm-note-text">
        {alertText(account, alert, now)}{" "}
        {critical
          ? "A janela está quase esgotada: iniciar pede confirmação. Se puder, escolha um ambiente de outra conta."
          : "A janela está perto do limite."}
      </span>
    </div>
  );
}

/** Passo de confirmação no rodapé, no lugar de Cancelar e Iniciar. */
export function PlanConfirm({ onBack, onConfirm, busy }: { onBack(): void; onConfirm(): void; busy: boolean }) {
  return (
    <>
      <span className="plan-confirm-text" role="alert">
        Janela do plano quase esgotada. Iniciar mesmo assim?
      </span>
      <span className="sm-spacer" />
      <button type="button" className="btn" onClick={onBack}>
        Voltar
      </button>
      <button type="button" className="btn btn-danger" disabled={busy} onClick={onConfirm}>
        {busy ? "Iniciando…" : "Iniciar mesmo assim"}
      </button>
    </>
  );
}
