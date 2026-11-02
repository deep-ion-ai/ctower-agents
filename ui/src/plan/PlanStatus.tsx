// Selo do plano na barra de status do shell (#107): a conta do ambiente
// ativo. Sem ambiente ativo (Sala de controle, Hub…), a conta única, se só
// houver uma; com várias, a lista fica na Sala de controle.
import "./plan.css";
import { accountOfEnv, type AccountPlanView, type PlanData } from "./data";
import { UNKNOWN_ACCOUNT_TEXT } from "./format";
import { PlanBadge } from "./PlanBadge";
import { usePlan } from "./usePlan";

/** Conta mostrada na barra: a do ambiente ativo, ou a única quando não há ambiente ativo. */
export function statusAccount(d: PlanData, env: string | null | undefined): AccountPlanView | "unknown" | null {
  if (env) return accountOfEnv(d, env);
  if (d.accounts.length === 1) return d.accounts[0];
  // Nenhuma conta conhecida, só ambientes sem login.
  return !d.accounts.length && d.unknown.length ? "unknown" : null;
}

export function PlanStatusItem({ env, now, onOpen }: { env: string | null | undefined; now: number; onOpen(): void }) {
  const { data } = usePlan(now);
  const a = statusAccount(data, env);
  if (!a) return null;
  if (a === "unknown") {
    return (
      <button type="button" className="sb-item" title={UNKNOWN_ACCOUNT_TEXT} onClick={onOpen}>
        Plano: conta desconhecida
      </button>
    );
  }
  return (
    <button type="button" className="sb-item sb-plan" onClick={onOpen}>
      <PlanBadge account={a} now={now} />
    </button>
  );
}
