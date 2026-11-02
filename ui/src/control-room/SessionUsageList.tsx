// Visão "Uso por sessão" da Sala de controle (#109): as sessões vivas por
// conta, com o selo da conta no cabeçalho de cada grupo, para escolher qual
// continua dentro da mesma janela do plano.
import { useState } from "react";
import { PlanBadge } from "../plan/PlanBadge";
import { accountLabel, UNKNOWN_ACCOUNT_TEXT } from "../plan/format";
import { usePlan } from "../plan/usePlan";
import { costText } from "../terminal/usage";
import { useNav } from "../state/nav";
import { useTower } from "../state/tower";
import { itemRef } from "../util/item";
import { duration } from "../util/time";
import { GESTOR_LABEL, itemLabel, lookup } from "./attention";
import {
  contextPctText,
  costOf,
  groupByAccount,
  idleMs,
  modelText,
  sortRows,
  USAGE_SORTS,
  usageKey,
  useSessionsUsage,
  type UsageGroup,
  type UsageRow,
  type UsageSort,
} from "./session-usage";
import { tileState } from "./TileGrid";

const COST_HINT = "Estimativa do Claude Code para a sessão, não é a cobrança";

function Row({ r, now, showEnv }: { r: UsageRow; now: number; showEnv: boolean }) {
  const nav = useNav();
  const st = tileState(r.s, r.wi?.status);
  const where = r.gestor ? GESTOR_LABEL : (r.project?.name ?? "Sem projeto");
  const title = r.wi?.title ?? itemLabel(r.project, r.wi, r.gestor, r.scope);
  const idle = idleMs(r, now);
  const cost = costOf(r.usage);
  const ctx = contextPctText(r.usage);
  const model = modelText(r);
  // Ainda não lida: reticências; lida e sem arquivo de uso: "sem leitura"; leitura sem o campo: traço.
  const empty = r.usage === undefined ? "…" : r.usage === null ? "sem leitura" : "—";
  return (
    <tr>
      <td>
        <button
          type="button"
          className="su-open"
          onClick={() => nav.openTerminal({ env: r.env, session: r.s.id })}
          title="Abrir o terminal da sessão"
        >
          <span className="badge" data-tone={st.tone}>
            {st.text}
          </span>
          <span className="su-item">
            <span className="mono muted">
              {where}
              {r.wi && !r.gestor ? ` ${itemRef(r.wi.gh_number, r.project)}` : ""}
              {showEnv ? ` · ${r.envName}` : ""}
            </span>
            <span className="su-title">{title}</span>
          </span>
        </button>
      </td>
      <td className="su-num" data-label="Custo estimado" title={cost !== null ? COST_HINT : undefined}>
        {cost !== null ? (
          <>
            {costText(cost)} <span className="muted">(estimativa)</span>
          </>
        ) : (
          <span className="muted">{empty}</span>
        )}
      </td>
      <td className="su-num" data-label="Contexto">{ctx ?? <span className="muted">{empty}</span>}</td>
      <td className="su-num" data-label="Parada há">{idle === null ? <span className="muted">rodando</span> : duration(idle)}</td>
      <td data-label="Modelo e esforço">{model ? <span className="mono">{model}</span> : <span className="muted">—</span>}</td>
    </tr>
  );
}

function Group({ g, now, sort, multiEnv }: { g: UsageGroup; now: number; sort: UsageSort; multiEnv: boolean }) {
  const label = g.account ? accountLabel(g.account) : "Conta desconhecida";
  const envs = g.account ? g.account.envs.map((e) => e.name) : [...new Set(g.rows.map((r) => r.envName))];
  return (
    <section className="su-group" aria-label={`Conta ${label}`}>
      <header className="su-group-head">
        <h3 className="tile-group-title">
          {label} <span className="muted">· {g.rows.length}</span>
        </h3>
        {g.account ? (
          <PlanBadge account={g.account} now={now} variant="inline" />
        ) : (
          <span className="cr-sub">{UNKNOWN_ACCOUNT_TEXT}</span>
        )}
        {envs.length > 0 && <span className="cr-sub">Ambientes: {envs.join(", ")}</span>}
      </header>
      <table className="su-table">
        <thead>
          <tr>
            <th scope="col">Sessão</th>
            <th scope="col" className="su-num" title={COST_HINT}>
              Custo estimado
            </th>
            <th scope="col" className="su-num">
              Contexto
            </th>
            <th scope="col" className="su-num">
              Parada há
            </th>
            <th scope="col">Modelo e esforço</th>
          </tr>
        </thead>
        <tbody>
          {sortRows(g.rows, sort, now).map((r) => (
            <Row key={usageKey(r.env, r.s.id)} r={r} now={now} showEnv={multiEnv} />
          ))}
        </tbody>
      </table>
    </section>
  );
}

export function SessionUsageList({ now }: { now: number }) {
  const { envs, dashboards, api } = useTower();
  const { data } = usePlan(now);
  const [sort, setSort] = useState<UsageSort>("cost");
  const live = envs.flatMap((e) =>
    (dashboards[e.id]?.live_sessions ?? []).map((s) => ({ env: e.id, envName: e.name, s, ...lookup(dashboards[e.id], s) })),
  );
  const slots = useSessionsUsage(
    api,
    live.map((r) => ({ env: r.env, session: r.s.id })),
  );
  const rows: UsageRow[] = live.map((r) => ({ ...r, usage: slots[usageKey(r.env, r.s.id)] }));
  const groups = groupByAccount(rows, data);

  return (
    <section aria-labelledby="uso-sessao" className="tiles-section su">
      <div className="su-head">
        <h2 id="uso-sessao" className="section-title">
          Uso por sessão · {rows.length}
        </h2>
        <span className="control-spacer" />
        <div role="group" aria-label="Ordenar por" className="control-seg">
          {USAGE_SORTS.map((o) => (
            <button key={o.id} type="button" className="btn btn-sm" aria-pressed={sort === o.id} onClick={() => setSort(o.id)}>
              {o.label}
            </button>
          ))}
        </div>
      </div>
      <p className="cr-sub">
        O custo é uma estimativa do Claude Code, não a cobrança. O plano é da conta: sessões de contas diferentes não
        disputam a mesma janela.
      </p>
      {rows.length === 0 ? (
        <p className="empty">Nenhuma sessão viva.</p>
      ) : (
        groups.map((g) => <Group key={g.key} g={g} now={now} sort={sort} multiEnv={envs.length > 1} />)
      )}
    </section>
  );
}
