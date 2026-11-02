// Seção Sessão no topo do card Contexto (#110): modelo e esforço, contexto
// com a janela real, custo estimado e duração, cache de prompt e o plano da
// conta (5h e semanal, com a regra do reset no relógio da UI). Sem leitura
// de uso (statusline ausente), só o que o transcript dá: modelo e contexto.
import type { DaemonApi } from "../api/daemon";
import { planView } from "../plan/rule";
import type { Session } from "../protocol/gen";
import { ago, duration } from "../util/time";
import { contextFigures, contextTokens, contextWindow, useContextUsage } from "./context";
import { accountName, cacheText, costText, latestPlan, useSessionUsage, windowText } from "./usage";

export function SessionUsageSection({
  api,
  env,
  session,
  s,
  live,
  now,
}: {
  api: DaemonApi;
  env: string;
  session: string;
  s: Session | undefined;
  /** Sessão viva: relê no intervalo; parada ou encerrada mostra a última leitura. */
  live: boolean;
  now: number;
}) {
  const ctx = useContextUsage(api, env, session);
  const { usage, account, accountPlan } = useSessionUsage(api, env, session, { poll: live, withAccount: true });

  const model = usage?.model?.display_name ?? usage?.model?.id ?? ctx?.model ?? s?.model ?? null;
  const tokens = contextTokens(ctx, usage);
  const context = contextFigures(tokens, contextWindow(s?.model ?? ctx?.model, usage));
  if (!usage && !context && !ctx?.model) return null;

  const cost = usage?.cost;
  const cache = usage?.prompt_cache ? cacheText(usage.prompt_cache, now) : null;
  const plan = usage ? planView(latestPlan(usage.plan, accountPlan), now) : null;
  const name = accountName(account);

  return (
    <section className="sec" aria-label="Uso da sessão">
      <h3>Sessão</h3>
      <dl className="ctx-facts">
        {model && (
          <>
            <dt>Modelo</dt>
            <dd>
              <span className="mono">{model}</span>
              {usage?.effort && ` · esforço ${usage.effort}`}
            </dd>
          </>
        )}
        {context && (
          <>
            <dt>Contexto</dt>
            <dd>{context}</dd>
          </>
        )}
        {cost?.total_cost_usd != null && (
          <>
            <dt>Custo</dt>
            <dd title="Estimativa do Claude Code, não é a cobrança">{`${costText(cost.total_cost_usd)} (estimativa)`}</dd>
          </>
        )}
        {cost?.total_duration_ms != null && (
          <>
            <dt>Duração</dt>
            <dd>
              {duration(cost.total_duration_ms)}
              {cost.total_api_duration_ms != null && ` · API ${duration(cost.total_api_duration_ms)}`}
            </dd>
          </>
        )}
        {cache && (
          <>
            <dt>Cache</dt>
            <dd>{cache}</dd>
          </>
        )}
      </dl>
      {plan && plan.read_at !== null && (
        <>
          <h4 className="ctx-sub">{name ? `Plano da conta ${name}` : "Plano da conta"}</h4>
          <dl className="ctx-facts" aria-label="Plano da conta">
            <dt>5h</dt>
            <dd>{windowText(plan.five_hour, now)}</dd>
            <dt>Semanal</dt>
            <dd>{windowText(plan.seven_day, now)}</dd>
          </dl>
        </>
      )}
      {usage && (
        <p className="none" title={new Date(usage.written_at).toLocaleString("pt-BR")}>
          {live ? `Leitura ${ago(usage.written_at, now)}` : `Última leitura ${ago(usage.written_at, now)}`}
        </p>
      )}
    </section>
  );
}
