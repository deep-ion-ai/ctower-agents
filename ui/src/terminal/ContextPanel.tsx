// Painel de contexto ao lado do console (#426; spec da #87, §3): como a barra
// lateral secundária do VS Code, à direita, com o contexto da sessão que tem
// o foco. Seções: Work item (status, branch, worktree, sessões anteriores),
// PR (ou "Criar PR"), Decisões e Eventos. Os dados vêm ao montar (abrir o
// painel ou trocar de sessão: a tela monta com `key` por sessão) e voltam a
// ser lidos por push do mesmo work item, com debounce de 1 s.
import "../project/detail-panel.css";
import { useEffect, useRef, useState } from "react";
import { useNow } from "../App";
import type { PushEvent } from "../api/types";
import { gestorLabel } from "../control-room/attention";
import type { Event, Review, SessionState, Tool, WorkItemStatus } from "../protocol/gen";
import type { HubHistory } from "../protocol/hub-gen";
import { STATUS } from "../project/status";
import { StatusMenu } from "../project/StatusMenu";
import { DecisionCard, EVENTS_PAGE, EventItem, pendingDecision, PR_TEXT, STATE_TEXT } from "../project/WorkItemPanel";
import { Icon } from "../shell/icons";
import { useNav, type Tab } from "../state/nav";
import { useTower } from "../state/tower";
import { changeLabel, changeRef, forgeLabel, itemTitle, PROJECT_ITEM } from "../util/item";
import { ago, duration } from "../util/time";
import { prUrl } from "./actions";
import type { TabInfo } from "./SessionTree";

/** Debounce da recarga por push (§3.1). */
export const CONTEXT_RELOAD_MS = 1000;
/** Pushes do mesmo work item que recarregam o painel. */
export const CONTEXT_PUSHES: ReadonlySet<string> = new Set([
  "work_item_status",
  "session_state",
  "session_exited",
  "needs_human",
  "sync_status",
]);
/** Recarga: no máximo isto de eventos (o máximo do `events.list`, menos o da sondagem). */
const MAX_RELOAD = 99;

const KEY = "ctower.terminal.context";

/** Aberto/fechado, por máquina (fechado na primeira vez). */
export function loadContextOpen(): boolean {
  try {
    return localStorage.getItem(KEY) === "1";
  } catch {
    return false;
  }
}

export function saveContextOpen(open: boolean): void {
  try {
    localStorage.setItem(KEY, open ? "1" : "0");
  } catch {
    // Sem armazenamento: vale só nesta execução.
  }
}

/**
 * Ambiente ou hub anterior ao `events.list` (#422): -32601, que chega como
 * a mensagem ("método desconhecido" do daemon, "método indisponível pelo
 * canal" do hub); o código não atravessa a ponte do Tauri.
 */
export function isUnknownMethod(e: unknown): boolean {
  return /-32601|m[ée]todo (desconhecido|indispon[íi]vel)|method not found/i.test(String(e));
}

export const EVENTS_OLD = "Este ambiente ainda não lista os eventos do item: atualize o ambiente.";

/**
 * O push diz respeito ao item do painel: o mesmo work item, uma sessão dele
 * (o `session_exited` só traz o id da sessão) ou, no `sync_status`, o projeto
 * (o sync traz os PRs).
 */
export function pushConcerns(
  e: PushEvent,
  env: string,
  item: { workItemId: number; projectId?: number; sessions: ReadonlySet<string> },
): boolean {
  if (e.env !== env || !CONTEXT_PUSHES.has(e.event)) return false;
  const d = e.data ?? {};
  if (e.event === "sync_status") return item.projectId !== undefined && d.project_id === item.projectId;
  if (d.work_item_id === item.workItemId) return true;
  return typeof d.session_id === "string" && item.sessions.has(d.session_id);
}

/** Sessão anterior do item: do dashboard do ambiente ou do histórico do hub. */
interface PastSession {
  id: string;
  tool: Tool;
  state: SessionState;
  started_at: number;
  ended_at: number | null;
  exit_code: number | null;
}

const same = (a: string, b: string) => a.toLowerCase() === b.toLowerCase();

export function ContextPanel({ tab, info, onClose }: { tab: Tab; info: TabInfo; onClose: () => void }) {
  const { api, hub, infos, dashboards } = useTower();
  const nav = useNav();
  const now = useNow();
  const { s, wi, project, gestor, scope } = info;
  const env = tab.env;
  const d = dashboards[env];
  const wiId = s?.work_item_id ?? wi?.id ?? null;
  const zero = wi?.gh_number === PROJECT_ITEM;
  // Sessões anteriores pelo hub (o ambiente não lista as sessões encerradas).
  const hubRef = !gestor && wi && project ? { owner: project.gh_owner, repo: project.gh_repo, number: wi.gh_number } : null;
  const hubKey = hub.up && hubRef ? `${hubRef.owner}/${hubRef.repo}#${hubRef.number}` : null;

  const [review, setReview] = useState<Review | null>(null);
  const [reviewError, setReviewError] = useState<string | null>(null);
  const [events, setEvents] = useState<Event[] | null>(null);
  const [eventsMore, setEventsMore] = useState(false);
  const [eventsError, setEventsError] = useState<string | null>(null);
  const [history, setHistory] = useState<HubHistory | null>(null);
  const [tick, setTick] = useState(0);
  const [busy, setBusy] = useState(false);
  const [actionError, setActionError] = useState<string | null>(null);
  const seq = useRef(0);
  // Eventos já mostrados: a recarga mantém as páginas abertas.
  const loaded = useRef(EVENTS_PAGE);

  useEffect(() => {
    if (wiId === null) return;
    const id = ++seq.current;
    const ok = () => id === seq.current;
    api.call<Review>(env, "review.get", { work_item_id: wiId }).then(
      (r) => {
        if (!ok()) return;
        setReview(r);
        setReviewError(null);
      },
      (e) => ok() && setReviewError(String(e)),
    );
    // Um a mais que o mostrado: diz se há mais sem uma página vazia.
    const keep = Math.min(MAX_RELOAD, Math.max(loaded.current, EVENTS_PAGE));
    api.call<Event[]>(env, "events.list", { work_item_id: wiId, limit: keep + 1 }).then(
      (list) => {
        if (!ok()) return;
        setEvents(list.slice(0, keep));
        setEventsMore(list.length > keep);
        setEventsError(null);
      },
      (e) => ok() && setEventsError(isUnknownMethod(e) ? EVENTS_OLD : String(e)),
    );
    if (hubKey && hubRef)
      // Só as sessões interessam: os eventos vêm do ambiente (`limit` mínimo).
      api.hubCall<HubHistory>("hub.workitem.history", { ...hubRef, limit: 1 }).then(
        (h) => ok() && setHistory(h),
        () => {},
      );
    // `hubRef` acompanha `hubKey`.
  }, [api, env, wiId, hubKey, tick]);

  const moreEvents = async () => {
    const last = events?.[events.length - 1];
    if (!last || wiId === null) return;
    const id = seq.current;
    try {
      const list = await api.call<Event[]>(env, "events.list", {
        work_item_id: wiId,
        before: last.id,
        limit: EVENTS_PAGE + 1,
      });
      if (id !== seq.current) return;
      const next = [...(events ?? []), ...list.slice(0, EVENTS_PAGE)];
      loaded.current = next.length;
      setEvents(next);
      setEventsMore(list.length > EVENTS_PAGE);
    } catch (e) {
      setEventsError(isUnknownMethod(e) ? EVENTS_OLD : String(e));
    }
  };

  // Sessões do item: as do dashboard (vivas, interrompidas, que falharam) e as do hub.
  const local = [...(d?.live_sessions ?? []), ...(d?.interrupted_sessions ?? []), ...(d?.failed_sessions ?? [])].filter(
    (x) => x.work_item_id === wiId,
  );
  const past = new Map<string, PastSession>();
  for (const x of [...local, ...(history?.sessions ?? [])]) if (!past.has(x.id)) past.set(x.id, x);
  past.delete(tab.session);
  const sessions = [...past.values()].sort((a, b) => b.started_at - a.started_at);

  // Push do mesmo item: relê com debounce.
  const sessionIds = useRef<ReadonlySet<string>>(new Set());
  sessionIds.current = new Set([tab.session, ...past.keys()]);
  const projectId = wi?.project_id ?? review?.work_item.project_id;
  const projectRef = useRef(projectId);
  projectRef.current = projectId;
  useEffect(() => {
    if (wiId === null) return;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const off = api.onPush((e) => {
      if (!pushConcerns(e, env, { workItemId: wiId, projectId: projectRef.current, sessions: sessionIds.current })) return;
      clearTimeout(timer);
      timer = setTimeout(() => setTick((t) => t + 1), CONTEXT_RELOAD_MS);
    });
    return () => {
      off();
      clearTimeout(timer);
    };
  }, [api, env, wiId]);

  // Status: o do hub quando ele está no ar (a mudança passa por ele), senão o do ambiente.
  const hubItem =
    hub.up && hubRef
      ? hub.dashboard?.projects
          .find((p) => same(p.project.gh_owner, hubRef.owner) && same(p.project.gh_repo, hubRef.repo))
          ?.work_items.find((w) => w.gh_number === hubRef.number)
      : undefined;
  const current: WorkItemStatus | undefined = hubItem?.status ?? review?.work_item.status ?? wi?.status;
  // O seletor da tela Projeto só quando o ambiente fala com o hub (e a UI também).
  const canSetStatus = hub.up && !!infos[env]?.hub_sync && !!hubRef && !zero;
  const setStatus = async (status: WorkItemStatus, reason: string | undefined) => {
    if (!hubRef) return false;
    setBusy(true);
    setActionError(null);
    try {
      await api.hubCall("hub.workitem.set_status", { ...hubRef, status, ...(reason ? { reason } : {}) });
      setTick((t) => t + 1);
      return true;
    } catch (e) {
      setActionError(String(e));
      return false;
    } finally {
      setBusy(false);
    }
  };

  const open = (url: string) => api.openUrl(url).catch((e) => setActionError(String(e)));
  const change = (n: number) => changeRef(project, n);
  const title = gestor ? gestorLabel(scope) : wi ? itemTitle(wi, project) : "Sessão";
  const decisions = [...(review?.decisions ?? [])].sort((a, b) => b.created_at - a.created_at);
  const pending = decisions.filter(pendingDecision).length;
  const prs = review?.prs ?? [];
  const createUrl = review && project ? prUrl(review, project, project.base_branch) : null;
  const interrupted = new Set(d?.interrupted_sessions.map((x) => x.id));
  const live = new Set(d?.live_sessions.map((x) => x.id));
  const worktree = wi?.worktree_path ?? (zero && project ? project.repo_path : null);

  const sessionRow = (x: PastSession) => {
    const when = `início ${ago(x.started_at, now)}${
      x.ended_at !== null ? ` · duração ${duration(x.ended_at - x.started_at)}` : ""
    }`;
    return (
      <li key={x.id} className={live.has(x.id) ? "sess alive" : "sess"}>
        <span className={live.has(x.id) ? "live" : "live off"} aria-hidden="true" />
        <span>
          <span className="mono">{x.tool}</span> · {STATE_TEXT[x.state]}
          {x.exit_code !== null && x.exit_code !== 0 ? ` · saída ${x.exit_code}` : ""}
        </span>
        {live.has(x.id) ? (
          <button
            type="button"
            className="linklike"
            aria-label={`Abrir terminal da sessão ${x.tool} (${when})`}
            onClick={() => nav.openTerminal({ env, session: x.id })}
          >
            abrir ↗
          </button>
        ) : interrupted.has(x.id) ? (
          <button
            type="button"
            className="linklike"
            aria-label={`Retomar pela Recuperação a sessão ${x.tool} (${when})`}
            onClick={() => nav.go({ name: "recovery", env, session: x.id })}
          >
            Recuperação ↗
          </button>
        ) : (
          <span />
        )}
        <span className="when">{when}</span>
      </li>
    );
  };

  return (
    <aside className="term-context" aria-label="Contexto da sessão">
      <header className="ctx-head">
        <span className="ctx-title">Contexto</span>
        <span className="ctx-item" title={title}>
          {title}
        </span>
        <button type="button" className="ibtn" aria-label="Fechar contexto" title="Fechar contexto" onClick={onClose}>
          <Icon name="close" size={14} />
        </button>
      </header>
      <div className="ctx-body">
        {actionError && (
          <p role="alert" className="session-error ctx-error">
            {actionError}
          </p>
        )}
        {reviewError && (
          <p role="alert" className="session-error ctx-error">
            {`Não foi possível carregar o contexto: ${reviewError} `}
            <button type="button" className="btn btn-sm" onClick={() => setTick((t) => t + 1)}>
              Tentar de novo
            </button>
          </p>
        )}

        {!gestor && wi && (
          <section className="sec" aria-label="Work item">
            <h3>Work item</h3>
            <dl className="ctx-facts">
              {!zero && current && (
                <>
                  <dt>Status</dt>
                  <dd className="ctx-status">
                    <span className="badge" data-tone={STATUS[current].tone}>
                      {STATUS[current].text}
                    </span>
                    {canSetStatus && (
                      <StatusMenu
                        current={current}
                        enabled
                        busy={busy}
                        className="btn btn-sm"
                        subIssue={wi.parent_number !== null}
                        onSet={setStatus}
                      />
                    )}
                  </dd>
                </>
              )}
              {!zero && (
                <>
                  <dt>Branch</dt>
                  <dd className="mono">{wi.branch ?? "—"}</dd>
                </>
              )}
              <dt>{zero ? "Pasta" : "Worktree"}</dt>
              <dd className="mono">
                {worktree ?? "—"}
                {zero && worktree && <span className="pane-warn"> clone principal</span>}
              </dd>
            </dl>
            <h4 className="ctx-sub">
              Sessões anteriores <span className="n">{sessions.length}</span>
            </h4>
            {sessions.length ? (
              <ul className="list">{sessions.map(sessionRow)}</ul>
            ) : (
              <p className="none">{hub.up ? "Nenhuma outra sessão." : "Nenhuma outra sessão neste ambiente."}</p>
            )}
          </section>
        )}

        {!gestor && !zero && wi && (
          <section className="sec" aria-label={changeLabel(project)}>
            <h3>{changeLabel(project)}</h3>
            {!review ? (
              <p className="none" role="status">
                Carregando…
              </p>
            ) : prs.length ? (
              <ul className="list">
                {prs.map((p) => (
                  <li key={p.number} className="prline">
                    <button
                      type="button"
                      className="pr"
                      data-state={p.state}
                      aria-label={`Abrir ${change(p.number)} no ${forgeLabel(project)} (${PR_TEXT[p.state]})`}
                      onClick={() => void open(p.url)}
                    >
                      {change(p.number)} {PR_TEXT[p.state]} ↗
                    </button>
                    <span className="mono muted ctx-wrap">
                      {p.head} → {p.base}
                    </span>
                  </li>
                ))}
              </ul>
            ) : createUrl ? (
              <button type="button" className="btn btn-sm ctx-create" onClick={() => void open(createUrl)}>
                {`Criar ${changeLabel(project)} ↗`}
              </button>
            ) : (
              <p className="none">{`Sem ${changeLabel(project)}: o item ainda não tem branch.`}</p>
            )}
          </section>
        )}

        <section className="sec" aria-label="Decisões">
          <h3>
            Decisões{" "}
            <span className="n">
              {decisions.length}
              {pending > 0 && (
                <>
                  {" · "}
                  <span className="pend">
                    {pending} {pending > 1 ? "pendentes" : "pendente"}
                  </span>
                </>
              )}
            </span>
          </h3>
          {!review ? (
            !reviewError && (
              <p className="none" role="status">
                Carregando…
              </p>
            )
          ) : decisions.length ? (
            <ul className="list">
              {decisions.map((x) => (
                <DecisionCard key={x.uid} d={x} change={change} />
              ))}
            </ul>
          ) : (
            <p className="none">Nenhuma decisão registrada.</p>
          )}
          {wiId !== null && (
            <button
              type="button"
              className="more"
              onClick={() => nav.go({ name: "review", env, workItemId: wiId })}
            >
              Revisar decisões
            </button>
          )}
        </section>

        <section className="sec" aria-label="Eventos">
          <h3>
            Eventos {events && <span className="n">{eventsMore ? `${events.length}+` : events.length}</span>}
          </h3>
          {eventsError ? (
            <p role={eventsError === EVENTS_OLD ? "status" : "alert"} className="none">
              {eventsError === EVENTS_OLD ? EVENTS_OLD : `Não foi possível carregar os eventos: ${eventsError}`}
            </p>
          ) : !events ? (
            <p className="none" role="status">
              Carregando…
            </p>
          ) : events.length ? (
            <ul className="list">
              {events.map((e) => (
                <EventItem key={e.uid} e={e} now={now} />
              ))}
            </ul>
          ) : (
            <p className="none">Nenhum evento.</p>
          )}
          {!eventsError && eventsMore && (
            <button type="button" className="more" onClick={() => void moreEvents()}>
              Mostrar mais
            </button>
          )}
        </section>
      </div>
    </aside>
  );
}
