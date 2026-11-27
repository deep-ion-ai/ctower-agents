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
import { changeUrl } from "../util/item";
import { epicRunTab } from "./sections";
import { useEpicRuns, type EpicRunItem } from "./useEpicRuns";
import { ago } from "../util/time";
import { CLAUDE_EFFORTS, CLAUDE_MODELS } from "../settings/session-options";
import { errText } from "./calls";
import {
  EPIC_RUN_PROTOCOL,
  envStatusOf,
  NODE_STATE_TEXT,
  OPTION_TEXT,
  ORIGIN_TEXT,
  RUN_STATUS_TEXT,
  agentText,
  blockersOf,
  clock,
  closesLines,
  projectNameOf,
  isActive,
  isIntegrating,
  mergedCount,
  nodeTone,
  occupies,
  overlapPairs,
  overlaps,
  readyWait,
  runTone,
  waves,
} from "./epicRun";
import "./epic-run.css";
import { ExternalLink } from "../shell/ExternalLink";

/** Paralelismo digitado: 1 a 8; vazio ou inválido = `fallback`. */
export const parallelOf = (text: string, fallback: number) => {
  const n = Number.parseInt(text, 10);
  return Number.isFinite(n) ? Math.max(1, Math.min(8, n)) : fallback;
};

type Act = (method: string, params: object, ok?: string) => Promise<boolean>;

const NO_CONNECTION = "O ambiente desta execução não está nas conexões deste app.";

/** O que "Devolver ao motor" faz (a sub-issue assumida, #301). */
const HANDBACK_HINT =
  "Você terminou de intervir: o motor volta a conduzir esta sub-issue. Ele confere o PR e o CI, manda as " +
  "correções e o pedido de merge na wip e volta a contar a falta de progresso. A sessão continua a mesma.";

/** A conexão deste app com o ambiente da execução (o `env_id` do hub). */
function useRunConnection(run: EpicRun): string | null {
  return useTower().hub.envByDaemon[run.env_id] ?? null;
}

/**
 * Hub › Execução de épicos. Com `initialRun`, a aba daquela execução (uma por
 * execução, #341); sem, o índice das execuções ativas, que abre a aba de
 * cada uma.
 */
export function EpicRuns({ now, initialRun }: { now: number; initialRun?: number }) {
  if (initialRun === undefined) return <EpicRunIndex now={now} />;
  return <EpicRunTab now={now} run={initialRun} />;
}

/** Índice: as execuções em andamento e o histórico dos últimos 7 dias. */
function EpicRunIndex({ now }: { now: number }) {
  const { runs, error } = useEpicRuns({ history: true });
  const active = (runs ?? []).filter((i) => isActive(i.run.status));
  const ended = (runs ?? []).filter((i) => !isActive(i.run.status));
  return (
    <section aria-labelledby="hub-epic-runs" className="panel settings-section epic-runs">
      <div className="hub-page-head">
        <h2 id="hub-epic-runs">Execução de épicos</h2>
        <p className="muted small">Cada execução abre na própria aba. Para começar outra, abra um épico e use Executar épico.</p>
      </div>
      {error && (
        <p role="alert" className="start-error">
          {error.includes("método desconhecido")
            ? `Este hub não executa épicos (anterior ao protocolo ${EPIC_RUN_PROTOCOL}): atualize o hub.`
            : error}
        </p>
      )}
      {runs === null && !error && <p className="muted">Lendo as execuções…</p>}
      {runs !== null && (
        <>
          <h3 className="er-index-head">
            Em andamento <span className="muted">{active.length}</span>
          </h3>
          {active.length === 0 ? (
            <p className="muted small">Nenhuma execução em andamento.</p>
          ) : (
            <ul className="er-index" aria-label="Execuções ativas">
              {active.map((i) => (
                <IndexRow key={i.run.id} item={i} now={now} />
              ))}
            </ul>
          )}
          <h3 className="er-index-head">
            Histórico <span className="muted">· últimos 7 dias</span>
          </h3>
          {ended.length === 0 ? (
            <p className="muted small">Nenhuma execução concluída ou cancelada nos últimos 7 dias.</p>
          ) : (
            <ul className="er-index" aria-label="Histórico de execuções">
              {ended.map((i) => (
                <IndexRow key={i.run.id} item={i} now={now} />
              ))}
            </ul>
          )}
        </>
      )}
    </section>
  );
}

/** Uma execução no índice: a linha inteira abre a aba dela. */
function IndexRow({ item, now }: { item: EpicRunItem; now: number }) {
  const nav = useNav();
  const { hub } = useTower();
  const { run: r, title, pending, merged, total } = item;
  const project = projectNameOf(hub.projects, r);
  const tone = pending > 0 ? "attention" : runTone(r.status);
  const when =
    r.ended_at != null
      ? `${r.status === "cancelado" ? "cancelada" : "concluída"} ${ago(r.ended_at, now)}`
      : `iniciada ${ago(r.created_at, now)}`;
  return (
    <li>
      <button type="button" className="er-index-row" onClick={() => nav.openTab(epicRunTab(r))}>
        <span className="er-index-dot" data-tone={tone} aria-hidden="true" />
        <span className="er-index-main">
          <span className="er-index-title">
            <span className="mono muted">#{r.epic}</span> {title ?? `${r.owner}/${r.repo}`}
          </span>
          <span className="small muted">
            <span className="er-index-project">{project}</span>
            {project !== `${r.owner}/${r.repo}` && ` · ${r.owner}/${r.repo}`}
            {total > 0 && ` · ${merged} de ${total} mescladas`} · {when}
          </span>
        </span>
        <span className="er-index-badges">
          {pending > 0 && (
            <span className="badge" data-tone="attention">
              {pending === 1 ? "1 decisão pendente" : `${pending} decisões pendentes`}
            </span>
          )}
          <span className="badge" data-tone={runTone(r.status)}>
            {RUN_STATUS_TEXT[r.status]}
          </span>
        </span>
      </button>
    </li>
  );
}

function EpicRunTab({ now, run }: { now: number; run: number }) {
  const { api, hub } = useTower();
  const [view, setView] = useState<EpicRunView | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [msg, setMsg] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const load = useCallback(async () => {
    try {
      setView(await api.hubCall<EpicRunView>("hub.epic_run.status", { run_id: run }));
      setError(null);
    } catch (e) {
      const t = errText(e);
      setError(
        t.includes("método desconhecido")
          ? `Este hub não executa épicos (anterior ao protocolo ${EPIC_RUN_PROTOCOL}): atualize o hub.`
          : t,
      );
    }
  }, [api, run]);

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
    <section aria-label="Execução de épicos" className="panel settings-section epic-runs">
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
      {!view && !error && <p className="muted">Lendo a execução…</p>}
      {view && <RunDetail view={view} now={now} act={act} busy={busy} />}
    </section>
  );
}

function RunHead({ view, children }: { view: EpicRunView; children?: React.ReactNode }) {
  const { run } = view;
  const [done, total] = mergedCount(view.nodes);
  const { hub } = useTower();
  const env = envStatusOf(
    hub.envs.find((e) => e.env_id === run.env_id),
    run.env_id,
  );
  const now = Date.now();
  return (
    <>
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
              <span className="strong">{projectNameOf(hub.projects, run)}</span> · {run.owner}/{run.repo}
            </span>
            <span title={run.env_id}>
              Ambiente: <span className="strong">{env.name}</span>{" "}
              <span className="badge" data-tone={env.tone} aria-label="Estado do ambiente">
                {env.text}
                {env.since !== null && ` · último contato ${ago(env.since, now)}`}
              </span>
            </span>
            {total > 0 && (
              <span>
                {done} de {total} mescladas
              </span>
            )}
          </div>
        </div>
        <div className="actions">{children}</div>
      </div>
      {env.known && !env.online && isActive(run.status) && (
        <p className="banner" aria-label="Ambiente fora do ar">
          O ambiente {env.name} está fora do ar. O motor não inicia nem retoma sessões até o canal reabrir; quando
          ele voltar, as sessões interrompidas são retomadas sozinhas.
        </p>
      )}
    </>
  );
}

function RunDetail({ view, now, act, busy }: { view: EpicRunView; now: number; act: Act; busy: boolean }) {
  const { run } = view;
  if (run.status === "planejando" || (run.status === "aguardando_aprovacao" && !view.plan)) {
    const pending = view.decisions.filter((d) => d.answer === null);
    const stalled = pending.some((d) => d.origin === "plano_falhou");
    return (
      <div className="er">
        <RunHead view={view}>
          <CancelButton run={run} act={act} busy={busy} />
        </RunHead>
        <p role="status">
          {stalled ? (
            <>A sessão de plano não gravou o plano: nenhuma sessão nova começa até você decidir.</>
          ) : (
            <>
              Sessão de plano {run.plan_session ? "em andamento" : "iniciando"} (
              {agentText(run.plan_model ?? "opus", run.plan_effort ?? "high")}): ela grava o plano e encerra.
            </>
          )}
        </p>
        <Decisions run={run} pending={pending} act={act} busy={busy} />
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

/**
 * Cancelar com confirmação na própria tela: o `window.confirm` não abre na
 * janela do app desktop e responde "não" sozinho (#341).
 */
function CancelButton({ run, act, busy }: { run: EpicRun; act: Act; busy: boolean }) {
  const [asking, setAsking] = useState(false);
  if (!isActive(run.status)) return null;
  if (!asking) {
    return (
      <button type="button" className="btn btn-danger" disabled={busy} onClick={() => setAsking(true)}>
        Cancelar
      </button>
    );
  }
  return (
    <div role="group" aria-label="Confirmar o cancelamento da execução" className="confirm-box">
      <p className="small">Cancelar a execução? O motor para de dirigir as sessões; nada é desfeito.</p>
      <div className="form-actions">
        <button type="button" className="btn" onClick={() => setAsking(false)}>
          Voltar
        </button>
        <button
          type="button"
          className="btn btn-danger"
          disabled={busy}
          onClick={() => {
            setAsking(false);
            void act("hub.epic_run.cancel", { run_id: run.id }, "Execução cancelada.");
          }}
        >
          Cancelar a execução
        </button>
      </div>
    </div>
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

/** A linha de estado do card, com o "PR #n" como link para o PR. */
function NodeLine({ n, now, href }: { n: EpicNode; now: number; href: string | null }) {
  const line = nodeLine(n, now);
  const tag = n.pr ? `PR #${n.pr}` : null;
  const at = tag && href ? line.indexOf(tag) : -1;
  if (!tag || !href || at < 0) return <>{line}</>;
  return (
    <>
      {line.slice(0, at)}
      <ExternalLink href={href}>{tag}</ExternalLink>
      {line.slice(at + tag.length)}
    </>
  );
}

function Execution({ view, now, act, busy }: { view: EpicRunView; now: number; act: Act; busy: boolean }) {
  const nav = useNav();
  const { run } = view;
  const { hub } = useTower();
  const project = hub.projects.find((p) => p.project.gh_owner === run.owner && p.project.gh_repo === run.repo)
    ?.project ?? { gh_owner: run.owner, gh_repo: run.repo };
  const plan = view.approved ?? view.plan;
  const [parallel, setParallel] = useState(String(run.max_parallel));
  useEffect(() => setParallel(String(run.max_parallel)), [run.max_parallel]);
  const ws = useMemo(() => (plan ? waves(plan) : [view.nodes.map((n) => n.number)]), [plan, view.nodes]);
  const nodes = new Map(view.nodes.map((n) => [n.number, n]));
  const planNode = new Map((plan?.nodes ?? []).map((n) => [n.number, n]));
  const blockers = plan ? blockersOf(plan) : new Map<number, number[]>();
  const pending = view.decisions.filter((d) => d.answer === null);
  const conn = useRunConnection(run);
  const openSession = (s: string) => conn && nav.openTerminal({ env: conn, session: s });
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
                  const wait = n.state === "pronta" && plan ? readyWait(plan, view.nodes, num, run.max_parallel) : null;
                  return (
                    <article key={num} className="er-card" data-tone={nodeTone(n.state)} aria-label={`#${num}`}>
                      <div className="er-card-title">
                        <span className="mono muted">#{num}</span> {view.titles[num] ?? ""}
                      </div>
                      <div className="small er-state">
                        <NodeLine n={n} now={now} href={n.pr ? changeUrl(project, n.pr) : null} />
                      </div>
                      <div className="small muted">
                        {agentText(pn?.model, pn?.effort)}
                        {n.state === "mesclada" && !n.worktree && !n.reason && " · worktree removido"}
                        {n.state === "pendente" && deps.length > 0 && ` · aguarda ${deps.map((d) => `#${d}`).join(", ")}`}
                        {wait && ` · ${wait}`}
                      </div>
                      {n.reason && <div className="small muted">{n.reason}</div>}
                      {n.state === "assumida" && (
                        <div className="small muted">O motor não envia nada a esta sessão até você devolvê-la.</div>
                      )}
                      <div className="er-card-actions">
                        {n.session_id && n.state !== "mesclada" && (
                          <button
                            type="button"
                            className="linklike"
                            disabled={!conn}
                            title={conn ? undefined : NO_CONNECTION}
                            onClick={() => openSession(n.session_id!)}
                          >
                            Abrir sessão
                          </button>
                        )}
                        {n.state === "assumida" && (
                          <button
                            type="button"
                            className="btn btn-sm"
                            disabled={busy}
                            title={HANDBACK_HINT}
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
                {view.nodes.filter(occupies).length} de{" "}
                {run.max_parallel}
              </dd>
              <dt>Code-review na integração</dt>
              <dd className="er-inline">
                {run.code_review ? "sim" : "não"}
                {!run.integration_session && isActive(run.status) && (
                  <button
                    type="button"
                    className="btn btn-sm"
                    disabled={busy}
                    title="A sessão de integração roda o /code-review sobre o conjunto da wip antes de abrir o PR do épico. Dá para mudar até ela começar."
                    onClick={() =>
                      void act(
                        "hub.epic_run.set_code_review",
                        { run_id: run.id, code_review: !run.code_review },
                        run.code_review ? "/code-review desligado na integração." : "/code-review ligado na integração.",
                      )
                    }
                  >
                    {run.code_review ? "Desligar" : "Ligar"}
                  </button>
                )}
              </dd>
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

/** O que é a sessão de integração (#302), no botão que a abre. */
const INTEGRATION_HINT =
  "Abre o terminal da sessão do épico que o motor iniciou depois de todas as sub-issues mescladas: ela " +
  "valida a wip (build e testes), roda o /code-review se o plano pediu e abre o PR do épico para a base. " +
  "Use para acompanhar ou intervir; o motor continua acompanhando o CI e as reviews do PR, mas o merge é seu.";

const PR_STATE_TEXT: Record<"open" | "merged" | "closed", string> = {
  open: "aberto",
  merged: "mesclado",
  closed: "fechado sem merge",
};

function Integration({ view, act, busy }: { view: EpicRunView; act: Act; busy: boolean }) {
  const nav = useNav();
  const { run } = view;
  const conn = useRunConnection(run);
  const plan = view.approved ?? view.plan;
  const waiting = run.status === "aguardando_merge";
  const done = run.status === "concluido";
  const planNode = new Map((plan?.nodes ?? []).map((n) => [n.number, n]));
  const prOpen = waiting || done;
  const { hub } = useTower();
  const project = hub.projects.find((p) => p.project.gh_owner === run.owner && p.project.gh_repo === run.repo)
    ?.project ?? { gh_owner: run.owner, gh_repo: run.repo };
  const epicPr = view.epic_pr ?? null;
  const epicPrUrl = epicPr ? epicPr.url || changeUrl(project, epicPr.number) : null;
  return (
    <div className="er">
      <RunHead view={view}>
        {epicPr && epicPrUrl && (
          <ExternalLink className="btn" href={epicPrUrl}>
            Abrir PR #{epicPr.number}
          </ExternalLink>
        )}
        {run.integration_session && (
          <button
            type="button"
            className="btn"
            disabled={!conn}
            title={conn ? INTEGRATION_HINT : NO_CONNECTION}
            onClick={() => conn && nav.openTerminal({ env: conn, session: run.integration_session! })}
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
              {epicPr && epicPrUrl ? (
                <>
                  <ExternalLink href={epicPrUrl}>PR #{epicPr.number}</ExternalLink> {PR_STATE_TEXT[epicPr.state]}.
                </>
              ) : prOpen ? (
                "Aberto."
              ) : (
                "Ainda não aberto."
              )}{" "}
              O corpo fecha cada sub-issue e o épico no merge:
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
                    {n.pr && (
                      <>
                        <ExternalLink href={changeUrl(project, n.pr)}>PR #{n.pr}</ExternalLink> ·{" "}
                      </>
                    )}
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
