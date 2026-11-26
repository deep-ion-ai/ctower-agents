// Hub › Execução de épicos (#304, épico #295): as execuções do hub e a de
// uma escolhida, em três telas conforme a fase: a aprovação do plano
// (ondas, modelo e esforço editáveis, áreas com as sobreposições,
// justificativa, propostas, paralelismo e /code-review), a execução
// monitorada (nós, "devolver ao motor", decisões, eventos e controles) e a
// integração (checklist e o PR do épico com os Closes). Lê
// `hub.epic_run.list|status` e relê no push `hub.epic_run.changed`.
import { useCallback, useEffect, useMemo, useState } from "react";
import type { EpicDecision, EpicNode, EpicPlan, EpicRun, EpicRunView } from "../protocol/hub-gen";
import { useNav } from "../state/nav";
import { useTower } from "../state/tower";
import { ago } from "../util/time";
import { CLAUDE_EFFORTS, CLAUDE_MODELS } from "../settings/session-options";
import { errText } from "./calls";
import {
  EPIC_RUN_PROTOCOL,
  NODE_STATE_TEXT,
  OPTION_TEXT,
  ORIGIN_TEXT,
  RUN_STATUS_TEXT,
  agentText,
  blockersOf,
  clock,
  closesLines,
  isActive,
  isIntegrating,
  mergedCount,
  nodeTone,
  overlapPairs,
  overlaps,
  runTone,
  waves,
} from "./epicRun";
import "./epic-run.css";

/** Paralelismo digitado: 1 a 8; vazio ou inválido = `fallback`. */
export const parallelOf = (text: string, fallback: number) => {
  const n = Number.parseInt(text, 10);
  return Number.isFinite(n) ? Math.max(1, Math.min(8, n)) : fallback;
};

type Act = (method: string, params: object, ok?: string) => Promise<boolean>;

export function EpicRuns({ now, initialRun }: { now: number; initialRun?: number }) {
  const { api, hub } = useTower();
  const [runs, setRuns] = useState<EpicRun[] | null>(null);
  const [selected, setSelected] = useState<number | null>(initialRun ?? null);
  const [view, setView] = useState<EpicRunView | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [msg, setMsg] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const load = useCallback(async () => {
    try {
      const list = await api.hubCall<EpicRun[]>("hub.epic_run.list");
      setRuns(list);
      setError(null);
      const id = selected ?? list[0]?.id ?? null;
      if (id !== null) {
        setSelected(id);
        setView(await api.hubCall<EpicRunView>("hub.epic_run.status", { run_id: id }));
      } else {
        setView(null);
      }
    } catch (e) {
      const t = errText(e);
      setError(
        t.includes("método desconhecido")
          ? `Este hub não executa épicos (anterior ao protocolo ${EPIC_RUN_PROTOCOL}): atualize o hub.`
          : t,
      );
    }
  }, [api, selected]);

  useEffect(() => {
    if (!hub.connected) return;
    void load();
    return api.onHubPush((e) => {
      if (e.event === "hub.epic_run.changed") void load();
    });
  }, [api, load, hub.connected]);

  const act: Act = async (method, params, ok) => {
    setBusy(true);
    setMsg(null);
    try {
      await api.hubCall(method, params);
      if (ok) setMsg(ok);
      void load();
      return true;
    } catch (e) {
      setError(errText(e));
      return false;
    } finally {
      setBusy(false);
    }
  };

  return (
    <section aria-labelledby="hub-epic-runs" className="panel settings-section epic-runs">
      <div className="hub-page-head">
        <h2 id="hub-epic-runs">Execução de épicos</h2>
      </div>
      {error && (
        <p role="alert" className="start-error">
          {error}
        </p>
      )}
      {msg && (
        <p role="status" className="state-on">
          {msg}
        </p>
      )}
      {runs === null && !error && <p className="muted">Lendo as execuções…</p>}
      {runs !== null && runs.length === 0 && (
        <p className="muted">
          Nenhuma execução ativa. Para começar, abra um épico e use <span className="strong">Executar épico</span>.
        </p>
      )}
      {runs !== null && runs.length > 1 && (
        <ul className="hub-card-list" aria-label="Execuções ativas">
          {runs.map((r) => (
            <li key={r.id} aria-current={r.id === selected ? "true" : undefined}>
              <button type="button" className="linklike" onClick={() => setSelected(r.id)}>
                <span className="mono">
                  {r.owner}/{r.repo}#{r.epic}
                </span>{" "}
                · {RUN_STATUS_TEXT[r.status]}
              </button>
            </li>
          ))}
        </ul>
      )}
      {view && <RunDetail view={view} now={now} act={act} busy={busy} />}
    </section>
  );
}

function RunHead({ view, children }: { view: EpicRunView; children?: React.ReactNode }) {
  const { run } = view;
  const [done, total] = mergedCount(view.nodes);
  return (
    <div className="er-head">
      <div className="er-title">
        <div className="er-chips">
          <span className="er-chip" data-kind="epic">
            ÉPICO #{run.epic}
          </span>
          <span className="badge" data-tone={runTone(run.status)} aria-label="Estado da execução">
            {RUN_STATUS_TEXT[run.status]}
            {view.plan && run.status === "executando" && ` · plano v${run.plan_version ?? view.plan.version}`}
          </span>
        </div>
        <h3>{view.titles[run.epic] ?? `${run.owner}/${run.repo}#${run.epic}`}</h3>
        <div className="muted small er-meta">
          <span>
            {run.owner}/{run.repo}
          </span>
          <span>Ambiente: {run.env_id}</span>
          {total > 0 && (
            <span>
              {done} de {total} mescladas
            </span>
          )}
        </div>
      </div>
      <div className="actions">{children}</div>
    </div>
  );
}

function RunDetail({ view, now, act, busy }: { view: EpicRunView; now: number; act: Act; busy: boolean }) {
  const { run } = view;
  if (run.status === "planejando" || (run.status === "aguardando_aprovacao" && !view.plan)) {
    return (
      <div className="er">
        <RunHead view={view}>
          <CancelButton run={run} act={act} busy={busy} />
        </RunHead>
        <p role="status">
          Sessão de plano {run.plan_session ? "em andamento" : "iniciando"} (
          {agentText(run.plan_model ?? "opus", run.plan_effort ?? "high")}): ela grava o plano e encerra.
        </p>
        <Events view={view} />
      </div>
    );
  }
  if (run.status === "aguardando_aprovacao" && view.plan) {
    return <Approval view={view} plan={view.plan} act={act} busy={busy} />;
  }
  if (isIntegrating(run.status)) return <Integration view={view} act={act} busy={busy} />;
  return <Execution view={view} now={now} act={act} busy={busy} />;
}

function CancelButton({ run, act, busy }: { run: EpicRun; act: Act; busy: boolean }) {
  if (!isActive(run.status)) return null;
  return (
    <button
      type="button"
      className="btn btn-danger"
      disabled={busy}
      onClick={() => {
        if (window.confirm("Cancelar a execução? O motor para de dirigir as sessões; nada é desfeito.")) {
          void act("hub.epic_run.cancel", { run_id: run.id }, "Execução cancelada.");
        }
      }}
    >
      Cancelar
    </button>
  );
}

// --- Aprovação do plano ---------------------------------------------------

function Approval({ view, plan, act, busy }: { view: EpicRunView; plan: EpicPlan; act: Act; busy: boolean }) {
  const { run } = view;
  const [parallel, setParallel] = useState(String(run.max_parallel));
  const [codeReview, setCodeReview] = useState(run.code_review);
  const ws = useMemo(() => waves(plan), [plan]);
  const over = useMemo(() => overlaps(plan), [plan]);
  const blockers = useMemo(() => blockersOf(plan), [plan]);
  const byNumber = new Map(plan.nodes.map((n) => [n.number, n]));
  const missing = plan.nodes.filter((n) => !n.model).map((n) => `#${n.number}`);
  return (
    <div className="er">
      <RunHead view={view}>
        <button
          type="button"
          className="btn"
          disabled={busy}
          onClick={() => void act("hub.epic_run.replan", { run_id: run.id }, "Outro plano pedido.")}
        >
          Pedir outro plano
        </button>
        <button
          type="button"
          className="btn btn-primary"
          disabled={busy || missing.length > 0}
          title={missing.length ? `Defina o modelo de ${missing.join(", ")}` : undefined}
          onClick={() =>
            void act(
              "hub.epic_run.approve",
              {
                run_id: run.id,
                version: plan.version,
                max_parallel: parallelOf(parallel, run.max_parallel),
                code_review: codeReview,
              },
              "Plano aprovado: o motor começa as sub-issues.",
            )
          }
        >
          Aprovar e executar
        </button>
      </RunHead>
      <p className="muted small">
        Plano v{plan.version}
        {run.plan_version !== null && run.plan_version !== plan.version && ` (substitui o v${run.plan_version})`}, gerado
        pela sessão de plano ({agentText(run.plan_model ?? "opus", run.plan_effort ?? "high")}).
      </p>
      <div className="er-body">
        <div className="er-main">
          <h4>Ordem e paralelismo</h4>
          <p className="muted small">Cada onda começa quando os bloqueantes estão mesclados na wip.</p>
          <div className="er-waves" role="list" aria-label="Ondas do plano">
            {ws.map((w, i) => (
              <div key={i} className="er-wave" role="listitem" aria-label={`Onda ${i + 1}`}>
                <div className="er-wave-label">
                  ONDA {i + 1}
                  {w.length > 1 && ` · ${w.length} em paralelo`}
                </div>
                {w.map((n) => {
                  const pn = byNumber.get(n)!;
                  const o = over.get(n);
                  const deps = blockers.get(n) ?? [];
                  return (
                    <article key={n} className="er-card" data-tone={o ? "attention" : undefined} aria-label={`#${n}`}>
                      <div className="er-card-title">
                        <span className="mono muted">#{n}</span> {view.titles[n] ?? ""}
                      </div>
                      {deps.length > 0 && (
                        <div className="small muted">Depois de {deps.map((d) => `#${d}`).join(", ")}</div>
                      )}
                      <NodeAgent run={run} node={pn} act={act} busy={busy} />
                      <div className="er-areas">
                        {pn.areas.length === 0 && <span className="small muted">sem áreas (não serializa)</span>}
                        {pn.areas.map((a) => (
                          <span key={a} className="er-area mono" data-tone={o?.areas.has(a) ? "attention" : undefined}>
                            {a}
                          </span>
                        ))}
                      </div>
                      {o && (
                        <div className="small er-attention">
                          Área sobreposta com {o.with.map((x) => `#${x}`).join(", ")}: não rodam juntas
                        </div>
                      )}
                      {pn.reason && <div className="small muted">{pn.reason}</div>}
                    </article>
                  );
                })}
              </div>
            ))}
          </div>
          {plan.summary && (
            <div className="er-box">
              <h4>Justificativa da sessão de plano</h4>
              <p>{plan.summary}</p>
            </div>
          )}
        </div>
        <aside className="er-side">
          <section className="er-box" aria-label="Execução">
            <h4>Execução</h4>
            <label className="er-field">
              Sessões em paralelo
              <input
                type="number"
                min={1}
                max={8}
                value={parallel}
                onChange={(e) => setParallel(e.target.value)}
              />
            </label>
            <label className="er-check">
              <input type="checkbox" checked={codeReview} onChange={(e) => setCodeReview(e.target.checked)} />
              <span>
                Rodar /code-review na integração
                <span className="small muted"> Desligue se o code-review já roda no CI do projeto.</span>
              </span>
            </label>
            <dl className="er-facts">
              <dt>Sub-issues</dt>
              <dd>{plan.nodes.length}</dd>
              <dt>Ondas</dt>
              <dd>{ws.length}</dd>
              <dt>Pares serializados por área</dt>
              <dd>{overlapPairs(plan)}</dd>
              <dt>Merge na wip</dt>
              <dd>autônomo, com rebase</dd>
              <dt>Merge do épico</dt>
              <dd>você</dd>
            </dl>
          </section>
          {plan.proposals.length > 0 && (
            <section className="er-box" data-tone="attention" aria-label="Propostas da sessão de plano">
              <h4>Propostas da sessão de plano</h4>
              <ul className="er-plain">
                {plan.proposals.map((p, i) => (
                  <li key={i}>
                    <span className="strong">{p.kind === "dividir" ? `Dividir #${p.from}` : "Nova"}:</span> {p.title}
                    {p.body && <div className="small muted">{p.body}</div>}
                  </li>
                ))}
              </ul>
              <p className="small muted">
                Viram sub-issues do épico quando você aprova o plano (no GitHub; no Jira, crie à mão) e entram no
                próximo plano.
              </p>
            </section>
          )}
        </aside>
      </div>
    </div>
  );
}

function NodeAgent({
  run,
  node,
  act,
  busy,
}: {
  run: EpicRun;
  node: { number: number; model: string | null; effort: string | null };
  act: Act;
  busy: boolean;
}) {
  const set = (model: string, effort: string) =>
    void act("hub.epic_run.set_node", {
      run_id: run.id,
      number: node.number,
      model: model || null,
      effort: model && effort ? effort : null,
    });
  return (
    <div className="er-agent">
      <label>
        Modelo
        <select
          aria-label={`Modelo de #${node.number}`}
          value={node.model ?? ""}
          disabled={busy}
          onChange={(e) => set(e.target.value, node.effort ?? "")}
        >
          <option value="">sem modelo</option>
          {CLAUDE_MODELS.map((m) => (
            <option key={m.value} value={m.value}>
              {m.value}
            </option>
          ))}
        </select>
      </label>
      <label>
        Esforço
        <select
          aria-label={`Esforço de #${node.number}`}
          value={node.effort ?? ""}
          disabled={busy || !node.model}
          onChange={(e) => set(node.model ?? "", e.target.value)}
        >
          <option value="">padrão</option>
          {CLAUDE_EFFORTS.map((x) => (
            <option key={x.value} value={x.value}>
              {x.value}
            </option>
          ))}
        </select>
      </label>
    </div>
  );
}

// --- Execução monitorada --------------------------------------------------

function nodeLine(n: EpicNode, now: number): string {
  switch (n.state) {
    case "mesclada":
      return `Mesclada na wip${n.pr ? ` · PR #${n.pr}` : ""}`;
    case "em_pr":
      return `PR #${n.pr ?? "?"}${n.attempts > 0 ? ` · tentativas de CI ${n.attempts}` : ""}${
        n.rebase_head ? " · rebase pedido" : ""
      }`;
    case "assumida":
      return `Assumida por você${n.assumed_since != null ? ` ${ago(n.assumed_since, now)}` : ""}`;
    default:
      return NODE_STATE_TEXT[n.state];
  }
}

function Execution({ view, now, act, busy }: { view: EpicRunView; now: number; act: Act; busy: boolean }) {
  const nav = useNav();
  const { run } = view;
  const plan = view.approved ?? view.plan;
  const [parallel, setParallel] = useState(String(run.max_parallel));
  useEffect(() => setParallel(String(run.max_parallel)), [run.max_parallel]);
  const ws = useMemo(() => (plan ? waves(plan) : [view.nodes.map((n) => n.number)]), [plan, view.nodes]);
  const nodes = new Map(view.nodes.map((n) => [n.number, n]));
  const planNode = new Map((plan?.nodes ?? []).map((n) => [n.number, n]));
  const blockers = plan ? blockersOf(plan) : new Map<number, number[]>();
  const pending = view.decisions.filter((d) => d.answer === null);
  const openSession = (s: string) => nav.openTerminal({ env: run.env_id, session: s });
  return (
    <div className="er">
      <RunHead view={view}>
        <label className="er-inline">
          Paralelo
          <input
            type="number"
            min={1}
            max={8}
            value={parallel}
            disabled={busy || !isActive(run.status)}
            onChange={(e) => setParallel(e.target.value)}
            onBlur={() => {
              const n = parallelOf(parallel, run.max_parallel);
              setParallel(String(n));
              if (n !== run.max_parallel) void act("hub.epic_run.set_parallel", { run_id: run.id, max_parallel: n });
            }}
          />
        </label>
        {run.status === "executando" && (
          <button
            type="button"
            className="btn"
            disabled={busy}
            onClick={() => void act("hub.epic_run.pause", { run_id: run.id }, "Pausado: nada novo inicia.")}
          >
            Pausar
          </button>
        )}
        {(run.status === "pausado" || run.status === "estacionado") && (
          <button
            type="button"
            className="btn"
            disabled={busy}
            onClick={() => void act("hub.epic_run.resume", { run_id: run.id }, "Retomado.")}
          >
            Retomar agora
          </button>
        )}
        {isActive(run.status) && (
          <button
            type="button"
            className="btn"
            disabled={busy}
            onClick={() =>
              void act("hub.epic_run.interrupt", { run_id: run.id }, "Interrompido: as sessões foram encerradas e estacionaram.")
            }
          >
            Interromper agora
          </button>
        )}
        <CancelButton run={run} act={act} busy={busy} />
      </RunHead>

      {run.status === "estacionado" && (
        <div className="er-banner" role="status">
          <span className="strong">Janela da conta esgotada.</span> Sessões estacionadas.
          {run.resume_at != null && (
            <>
              {" "}
              Retomada automática às <span className="strong">{clock(run.resume_at)}</span> (reset + 5 min).
            </>
          )}
        </div>
      )}
      {run.status === "pausado" && (
        <div className="er-banner" role="status">
          <span className="strong">Pausado.</span> Nenhuma sessão nova começa; as que rodavam terminam o turno e estacionam.
          O reset da conta não retoma um épico pausado por você.
        </div>
      )}
      {run.status === "replanejando" && (
        <div className="er-banner" role="status">
          <span className="strong">Replanejando.</span> Uma sessão de plano prepara a nova versão; nada novo inicia até você
          aprovar, e as sub-issues vivas continuam.
        </div>
      )}

      <div className="er-body">
        <div className="er-main">
          <ul className="er-legend small muted" aria-label="Legenda">
            <li data-tone="done">mesclada</li>
            <li data-tone="running">rodando / PR</li>
            <li data-tone="assumed">assumida por você</li>
            <li data-tone="attention">pede decisão</li>
            <li data-tone="idle">pendente</li>
          </ul>
          <div className="er-waves" role="list" aria-label="Sub-issues">
            {ws.map((w, i) => (
              <div key={i} className="er-wave" role="listitem" aria-label={`Onda ${i + 1}`}>
                <div className="er-wave-label">ONDA {i + 1}</div>
                {w.map((num) => {
                  const n = nodes.get(num);
                  if (!n) return null;
                  const pn = planNode.get(num);
                  const deps = blockers.get(num) ?? [];
                  return (
                    <article key={num} className="er-card" data-tone={nodeTone(n.state)} aria-label={`#${num}`}>
                      <div className="er-card-title">
                        <span className="mono muted">#{num}</span> {view.titles[num] ?? ""}
                      </div>
                      <div className="small er-state">{nodeLine(n, now)}</div>
                      <div className="small muted">
                        {agentText(pn?.model, pn?.effort)}
                        {n.state === "mesclada" && !n.worktree && !n.reason && " · worktree removido"}
                        {n.state === "pendente" && deps.length > 0 && ` · aguarda ${deps.map((d) => `#${d}`).join(", ")}`}
                      </div>
                      {n.reason && <div className="small muted">{n.reason}</div>}
                      {n.state === "assumida" && (
                        <div className="small muted">O motor não envia nada a esta sessão até você devolvê-la.</div>
                      )}
                      <div className="er-card-actions">
                        {n.session_id && n.state !== "mesclada" && (
                          <button type="button" className="linklike" onClick={() => openSession(n.session_id!)}>
                            Abrir sessão
                          </button>
                        )}
                        {n.state === "assumida" && (
                          <button
                            type="button"
                            className="btn btn-sm"
                            disabled={busy}
                            onClick={() =>
                              void act("hub.epic_run.handback", { run_id: run.id, number: num }, `#${num} devolvida ao motor.`)
                            }
                          >
                            Devolver ao motor
                          </button>
                        )}
                      </div>
                    </article>
                  );
                })}
              </div>
            ))}
          </div>
          <Events view={view} />
        </div>
        <aside className="er-side">
          <Decisions run={run} pending={pending} act={act} busy={busy} />
          <section className="er-box" aria-label="Vagas">
            <h4>Vagas</h4>
            <dl className="er-facts">
              <dt>Sessões vivas</dt>
              <dd>
                {view.nodes.filter((n) => ["em_execucao", "em_pr", "assumida", "estacionada"].includes(n.state)).length} de{" "}
                {run.max_parallel}
              </dd>
              <dt>Code-review na integração</dt>
              <dd>{run.code_review ? "sim" : "não"}</dd>
            </dl>
          </section>
        </aside>
      </div>
    </div>
  );
}

function Decisions({ run, pending, act, busy }: { run: EpicRun; pending: EpicDecision[]; act: Act; busy: boolean }) {
  const [blocker, setBlocker] = useState<Record<number, string>>({});
  if (pending.length === 0) return null;
  return (
    <section className="er-box" data-tone="attention" aria-label="Decisões pendentes">
      <div className="er-box-head">
        <h4>Decisões pendentes</h4>
        <span className="er-count">{pending.length}</span>
      </div>
      {pending.map((d) => (
        <div key={d.id} className="er-decision" aria-label={`Pedido ${d.id}`}>
          <div className="small">
            {d.number !== null && <span className="mono muted">#{d.number} · </span>}
            {ORIGIN_TEXT[d.origin]}
          </div>
          <blockquote>{d.text}</blockquote>
          {d.options.map((o) => (
            <div key={o} className="er-option">
              {o === "aguardar" && (
                <label className="er-inline small">
                  Esperar a issue #
                  <input
                    inputMode="numeric"
                    aria-label="Issue que a sub-issue espera"
                    value={blocker[d.id] ?? ""}
                    onChange={(e) => setBlocker((b) => ({ ...b, [d.id]: e.target.value.replace(/\D/g, "") }))}
                  />
                </label>
              )}
              <button
                type="button"
                className={o === "replanejar" ? "btn btn-primary" : "btn"}
                disabled={busy}
                onClick={() =>
                  void act(
                    "hub.epic_run.decide",
                    {
                      decision_id: d.id,
                      answer: o,
                      ...(o === "aguardar" && blocker[d.id] ? { blocker: Number(blocker[d.id]) } : {}),
                    },
                    `Pedido respondido: ${OPTION_TEXT[o]?.label ?? o}.`,
                  )
                }
              >
                {OPTION_TEXT[o]?.label ?? o}
                <span className="er-option-hint">{OPTION_TEXT[o]?.hint}</span>
              </button>
            </div>
          ))}
        </div>
      ))}
      {run.status === "aguardando_aprovacao" && <p className="small muted">O plano novo aguarda sua aprovação.</p>}
    </section>
  );
}

function Events({ view }: { view: EpicRunView }) {
  const evs = [...view.events].reverse().slice(0, 30);
  if (evs.length === 0) return null;
  return (
    <section className="er-box" aria-label="Eventos do motor">
      <h4>Eventos do motor</h4>
      <ol className="er-events">
        {evs.map((e) => (
          <li key={e.id}>
            <span className="mono muted">{clock(e.at)}</span>
            <span>{e.message}</span>
          </li>
        ))}
      </ol>
    </section>
  );
}

// --- Integração -----------------------------------------------------------

function Integration({ view, act, busy }: { view: EpicRunView; act: Act; busy: boolean }) {
  const nav = useNav();
  const { run } = view;
  const plan = view.approved ?? view.plan;
  const waiting = run.status === "aguardando_merge";
  const done = run.status === "concluido";
  const planNode = new Map((plan?.nodes ?? []).map((n) => [n.number, n]));
  const prOpen = waiting || done;
  return (
    <div className="er">
      <RunHead view={view}>
        {run.integration_session && (
          <button
            type="button"
            className="btn"
            onClick={() => nav.openTerminal({ env: run.env_id, session: run.integration_session! })}
          >
            Abrir sessão de integração
          </button>
        )}
        <CancelButton run={run} act={act} busy={busy} />
      </RunHead>
      <div className="er-body">
        <div className="er-main">
          <section className="er-box" aria-label="Sessão de integração">
            <h4>Sessão de integração</h4>
            <ol className="er-checklist">
              <li data-done={prOpen}>Build e testes na wip</li>
              <li data-done={prOpen}>Revisão do conjunto das sub-issues</li>
              <li data-done={prOpen} data-off={!run.code_review}>
                {run.code_review ? "/code-review (ligado na aprovação do plano)" : "/code-review desligado no plano"}
              </li>
              <li data-done={prOpen}>Abrir o PR do épico</li>
            </ol>
          </section>
          <section className="er-box" aria-label="PR do épico">
            <h4>PR do épico</h4>
            <p className="small muted">
              {prOpen ? "Aberto." : "Ainda não aberto."} O corpo fecha cada sub-issue e o épico no merge:
            </p>
            <pre className="er-pre">{closesLines(run, view.nodes).join("\n")}</pre>
            {waiting && (
              <p className="small er-attention">O motor acompanha o CI, mas não pede este merge: é seu.</p>
            )}
          </section>
          <Events view={view} />
        </div>
        <aside className="er-side">
          <section className="er-box" aria-label="Sub-issues">
            <h4>Sub-issues</h4>
            <ul className="er-plain">
              {view.nodes.map((n) => (
                <li key={n.number} className="er-row">
                  <span>
                    <span className="mono muted">#{n.number}</span> {view.titles[n.number] ?? ""}
                  </span>
                  <span className="small muted">
                    {agentText(planNode.get(n.number)?.model, planNode.get(n.number)?.effort)}
                  </span>
                </li>
              ))}
            </ul>
            <p className="small er-done">
              {done ? "Épico mesclado: execução concluída." : "Todas mescladas na wip, aguardando o merge do épico."}
            </p>
          </section>
        </aside>
      </div>
    </div>
  );
}
