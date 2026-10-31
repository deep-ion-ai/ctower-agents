import "./detail-panel.css";
import { Fragment, useEffect, useRef, useState, type ReactNode } from "react";
import type { PushEvent } from "../api/types";
import type {
  IssueRow,
  PrRow,
  PrState,
  WorkItemStatus,
  WorkItemSummary,
} from "../protocol/gen";
import type { HubDecision, HubHistory, HubRanking, HubSession, Priority } from "../protocol/hub-gen";
import { agentLabel } from "../settings/session-options";
import { useNav } from "../state/nav";
import { useTower } from "../state/tower";
import { useOpenShell } from "../terminal/shells";
import {
  changeLabel,
  changeRef,
  changeShort,
  forgeLabel,
  itemFullRef,
  itemShort,
  trackerLabel,
  type ProviderProject,
} from "../util/item";
import { ago, duration } from "../util/time";
import { versionNewer } from "../util/version";
import { AgentRules } from "../hub/AgentRules";
import { IssueBodyModal } from "./IssueBodyModal";
import { STATUS } from "./status";
import { StatusMenu } from "./StatusMenu";
import { mergedOpen, mergedOpenText } from "./TreeRow";
import { DesktopOnly } from "../web/DesktopOnly";

export const PR_TEXT: Record<PrState, string> = { open: "aberto", merged: "mesclado", closed: "fechado" };
export const ORIGIN_TEXT = { autonoma: "autônoma", orientada: "orientada", manual: "manual" } as const;
export const STATE_TEXT = {
  starting: "iniciando",
  running: "rodando",
  idle: "ociosa",
  exited: "encerrada",
  interrupted: "interrompida",
  archived: "arquivada",
} as const;
/** Eventos por página da linha do tempo. */
export const EVENTS_PAGE = 5;
/** Protocolo do hub com `hub.workitem.body` ("Ver issue", #434). */
export const WORKITEM_BODY_PROTOCOL = "1.10";

/** Nó da árvore (hub ou local) que o painel mostra. Prioridade só no hub. */
export interface PanelNode {
  issue: IssueRow;
  work_item: { status: WorkItemStatus; branch: string | null; priority?: Priority | null } | null;
  summary: WorkItemSummary;
}

const PRIORITIES: (Priority | null)[] = ["P0", "P1", "P2", "P3", null];
const priorityText = (p: Priority | null) => p ?? "não triado";

/** Sessão viva do item num ambiente conhecido pela UI (abre o terminal). */
export interface LiveRef {
  env: string;
  name: string;
  session: string;
}

/** Resumo curto do payload de um evento, para a linha do tempo. */
export function eventDetail(payload: unknown): string {
  if (payload === null || payload === undefined) return "";
  if (typeof payload !== "object") return String(payload);
  const text = Object.entries(payload as Record<string, unknown>)
    .filter(([, v]) => v !== null && typeof v !== "object")
    .map(([k, v]) => `${k}: ${v}`)
    .join(" · ");
  return text.length > 140 ? `${text.slice(0, 139)}…` : text;
}

/** Campos da decisão que o cartão usa (a do hub e a do `review.get`). */
export type DecisionView = Pick<
  HubDecision,
  "uid" | "origin" | "title" | "decision" | "consequences" | "reviewed_at" | "reviewed_in_pr" | "created_at"
>;

export const pendingDecision = (d: DecisionView) => d.origin === "autonoma" && d.reviewed_at === null;

/** Autônomas pendentes primeiro; dentro de cada grupo, as mais recentes antes. */
export function sortDecisions<D extends DecisionView>(all: D[]): D[] {
  return [...all].sort(
    (a, b) => Number(pendingDecision(b)) - Number(pendingDecision(a)) || b.created_at - a.created_at,
  );
}

const same = (a: string, b: string) => a.toLowerCase() === b.toLowerCase();

/** Linha da tabela de sub-issues do épico. */
export interface SubRow {
  node: PanelNode;
  live: LiveRef[];
  /** Pode iniciar sessão (aberta, não concluída, com ambiente). */
  canStart: boolean;
}

/**
 * Item aberto como aba de editor (#317; spec da tela Projeto §3): cabeçalho
 * com tipo, status, prioridade e branch, as ações (GitHub, Iniciar sessão,
 * status, prioridade), no épico a barra de progresso e a tabela de
 * sub-issues, e o histórico do hub (`hub.workitem.history`) em Decisões
 * (com atalho para a Revisão), Eventos, Sessões e PRs. Sem o hub, mostra só
 * as ações (visão parcial) e a mudança de status fica desabilitada.
 * Use `key={número}` ao montar: o estado (páginas, menu) é por item.
 */
export function WorkItemPanel({
  node,
  project,
  baseBranch,
  now,
  live,
  crumbs,
  where,
  subs,
  onStart,
  onOpenItem,
  onStartItem,
  onReview,
  onStatusChanged,
}: {
  node: PanelNode;
  /** O projeto do item: `owner`/`repo` e o provedor das referências (#352). */
  project: ProviderProject;
  /** Base do projeto: PR mesclado nela com a issue aberta ganha aviso (#375). */
  baseBranch?: string;
  now: number;
  live: LiveRef[];
  /** Trilha acima do cabeçalho (projeto › grupo › #n). */
  crumbs?: ReactNode;
  /** Ambientes com sessão viva do item, inclusive os que esta UI não conhece. */
  where?: string[];
  /** Sub-issues do épico (tabela com status, sessão, PR e ▶). */
  subs?: SubRow[];
  /** Abre a modal de início; ausente quando o item não pode iniciar (concluído, fechado, sem ambiente). */
  onStart?: (origin: HTMLElement) => void;
  onOpenItem?: (ghNumber: number) => void;
  onStartItem?: (ghNumber: number, origin: HTMLElement) => void;
  /** Abre a Revisão das decisões do item. */
  onReview?: () => void;
  /** O status mudou no hub: a tela relê a árvore. */
  onStatusChanged?: () => void;
}) {
  const { api, hub, dashboards, web, envs } = useTower();
  const openShell = useOpenShell();
  const nav = useNav();
  const owner = project.gh_owner;
  const repo = project.gh_repo;
  const hubUp = hub.up;
  // Referências no provedor do projeto: PROJ-5450 e MR !7 no Jira + GitLab.
  const ref = (x: number) => itemShort(project, x);
  const change = (x: number) => changeRef(project, x);
  const tracker = trackerLabel(project);
  const forge = forgeLabel(project);
  const n = node.issue.gh_number;
  const hasWi = node.work_item !== null;
  // Shell livre (#423): nos ambientes onde o item tem worktree; só no desktop.
  const shellEnvs = web
    ? []
    : envs.flatMap((e) => {
        const wi = dashboards[e.id]?.projects
          .find((p) => same(p.project.gh_owner, owner) && same(p.project.gh_repo, repo))
          ?.work_items.find((w) => w.gh_number === n);
        return wi?.worktree_path ? [{ env: e.id, name: e.name, workItemId: wi.id }] : [];
      });
  // O que do hub diz respeito a este item: se mudar, o histórico é relido.
  const hubItem = hub.dashboard?.projects
    .find((p) => same(p.project.gh_owner, owner) && same(p.project.gh_repo, repo))
    ?.work_items.find((w) => w.gh_number === n);
  // Com o hub no ar, o status é o do hub: na tela local o work item do ledger local não recebe a mudança.
  const current: WorkItemStatus =
    (hubUp ? hubItem?.status : undefined) ?? node.work_item?.status ?? (node.issue.state === "closed" ? "concluido" : "backlog");
  const st = STATUS[current];
  const isEpic = node.issue.has_sub_issues;
  // Metadados do tracker (#69): ausentes no hub ou ambiente anteriores à #67.
  const meta = node.issue.meta ?? [];
  // 0 é o espelho sem data: sem subtítulo.
  const updatedAt = node.issue.updated_at || null;
  const priority: Priority | null = (hubUp ? hubItem?.priority : undefined) ?? node.work_item?.priority ?? null;

  const [history, setHistory] = useState<HubHistory | null>(null);
  const [historyError, setHistoryError] = useState<string | null>(null);
  const [tick, setTick] = useState(0);
  const [pages, setPages] = useState(1);
  const [prioMenu, setPrioMenu] = useState(false);
  const [busy, setBusy] = useState(false);
  const [actionError, setActionError] = useState<string | null>(null);
  const [viewing, setViewing] = useState(false);
  const seq = useRef(0);
  const sessionIds = useRef<Set<string>>(new Set());
  sessionIds.current = new Set(history?.sessions.map((s) => s.id));

  const signature = hubItem
    ? [
        hubItem.status,
        hubItem.pending_decisions,
        ...(hub.dashboard?.live_sessions.filter((s) => s.work_item_id === hubItem.id).map((s) => s.id) ?? []),
      ].join("|")
    : "";

  // Push de um ambiente que envolve o item (o work item local ou uma sessão dele).
  useEffect(() => {
    return api.onPush((e: PushEvent) => {
      const localId = dashboards[e.env]?.projects
        .find((p) => same(p.project.gh_owner, owner) && same(p.project.gh_repo, repo))
        ?.work_items.find((w) => w.gh_number === n)?.id;
      const wi = e.data?.work_item_id;
      const sid = e.data?.session_id;
      if ((localId !== undefined && wi === localId) || (typeof sid === "string" && sessionIds.current.has(sid))) {
        setTick((t) => t + 1);
      }
    });
  }, [api, dashboards, owner, repo, n]);

  useEffect(() => {
    if (!hubUp || !hasWi) return;
    const id = ++seq.current;
    api.hubCall<HubHistory>("hub.workitem.history", { owner, repo, number: n }).then(
      (h) => {
        if (id !== seq.current) return;
        setHistory(h);
        setHistoryError(null);
      },
      (e) => {
        if (id === seq.current) setHistoryError(String(e));
      },
    );
  }, [api, hubUp, hasWi, owner, repo, n, signature, tick]);

  const open = (url: string) => api.openUrl(url).catch((e) => setActionError(String(e)));

  // "Ver issue" (#434) lê pelo hub: fora do ar ou anterior ao método, desabilitado.
  const hubProtocol = hub.info?.protocol;
  const viewBlocked = !hubUp
    ? "A descrição é lida pelo hub, que está fora do ar"
    : hubProtocol && versionNewer(WORKITEM_BODY_PROTOCOL, hubProtocol)
      ? `Atualize o hub (protocolo ${hubProtocol}; ver a issue no app pede ${WORKITEM_BODY_PROTOCOL} ou maior)`
      : null;

  const setStatus = async (status: WorkItemStatus, reason: string | undefined) => {
    if (!hubUp) return false;
    setBusy(true);
    setActionError(null);
    try {
      await api.hubCall("hub.workitem.set_status", { owner, repo, number: n, status, ...(reason ? { reason } : {}) });
      setTick((t) => t + 1);
      onStatusChanged?.();
      return true;
    } catch (e) {
      setActionError(String(e));
      return false;
    } finally {
      setBusy(false);
    }
  };

  /** Uma mudança de prioridade ou de ordem no hub; o painel e a tela relêem. */
  const hubChange = async (method: string, params: object) => {
    setBusy(true);
    setActionError(null);
    try {
      await api.hubCall(method, { owner, repo, number: n, ...params });
      setPrioMenu(false);
      setTick((t) => t + 1);
      onStatusChanged?.();
    } catch (e) {
      setActionError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const setPriority = (p: Priority | null) => {
    if (p === priority) {
      setPrioMenu(false);
      return;
    }
    void hubChange("hub.workitem.set_priority", { priority: p });
  };

  /** Sobe ou desce uma posição dentro da classe (vizinho pelo `hub.ranking.get` do projeto). */
  const step = async (dir: -1 | 1) => {
    if (priority === null) return;
    try {
      const r = await api.hubCall<HubRanking>("hub.ranking.get", { owner, repo, priority, limit: 200 });
      const i = r.items.findIndex((x) => x.number === n);
      const other = r.items[i + dir];
      if (i < 0 || !other) return;
      await hubChange("hub.workitem.move", dir < 0 ? { before: other.number } : { after: other.number });
    } catch (e) {
      setActionError(String(e));
    }
  };

  const pr = node.summary.pr;
  const warn = mergedOpen(node, baseBranch);
  const decisions = sortDecisions(history?.decisions ?? []);
  const pending = decisions.filter(pendingDecision).length;
  const events = history?.events ?? [];
  const shown = events.slice(0, EVENTS_PAGE * pages);

  const sessionRow = (s: HubSession) => {
    const l = live.find((x) => x.session === s.id);
    const when = `início ${ago(s.started_at, now)}${
      s.ended_at !== null
        ? ` · fim ${ago(s.ended_at, now)} · duração ${duration(s.ended_at - s.started_at)}`
        : " · em andamento"
    }`;
    return (
      <li key={s.id} className={l ? "sess alive" : "sess"}>
        <span className={l ? "live" : "live off"} aria-hidden="true" />
        <span>
          <span className="mono">{s.tool}</span> · {agentLabel(s)} · {STATE_TEXT[s.state]}
        </span>
        {l ? (
          <button
            type="button"
            className="linklike mono"
            aria-label={`Abrir terminal da sessão ${s.tool} em ${l.name}`}
            onClick={() => nav.openTerminal({ env: l.env, session: l.session })}
          >
            abrir ↗
          </button>
        ) : (
          <span className="muted mono">{s.exit_code !== null ? `saída ${s.exit_code}` : "sem código de saída"}</span>
        )}
        <span className="when">{when}</span>
      </li>
    );
  };

  const prLine = (p: PrRow) => (
    <li key={p.number} className="prline">
      <button
        type="button"
        className="pr"
        data-state={p.state}
        aria-label={`Abrir ${change(p.number)} no ${forge} (${PR_TEXT[p.state]})`}
        onClick={() => void open(p.url)}
      >
        {change(p.number)} {PR_TEXT[p.state]} ↗
      </button>
      <span className="mono muted wrap">
        {p.head} → {p.base}
      </span>
    </li>
  );

  const decisionCard = (d: HubDecision) => <DecisionCard key={d.uid} d={d} change={change} />;

  const subRow = ({ node: c, live: l, canStart }: SubRow) => {
    const cn = c.issue.gh_number;
    const cst = STATUS[c.work_item?.status ?? (c.issue.state === "closed" ? "concluido" : "backlog")];
    const cpr = c.summary.pr;
    const envsLive = l.length > 0 ? l.map((x) => x.name) : c.summary.live_envs;
    return (
      <tr key={cn}>
        <td>
          <button type="button" className="linklike iv-sub" onClick={() => onOpenItem?.(cn)}>
            <span className="mono muted">{ref(cn)}</span> {c.issue.title}
          </button>
        </td>
        <td>
          <span className="badge" data-tone={cst.tone}>
            {cst.text}
          </span>
        </td>
        <td>
          {l.length > 0 ? (
            l.map((x) => (
              <button
                key={`${x.env}:${x.session}`}
                type="button"
                className="linklike"
                aria-label={`Abrir terminal de ${ref(cn)} em ${x.name}`}
                onClick={() => nav.openTerminal({ env: x.env, session: x.session })}
              >
                {x.name} ↗
              </button>
            ))
          ) : envsLive.length > 0 ? (
            <span className="muted">{envsLive.join(", ")}</span>
          ) : (
            <span className="muted">—</span>
          )}
        </td>
        <td>
          {cpr ? (
            <button
              type="button"
              className="linklike mono"
              aria-label={`Abrir ${change(cpr.number)} de ${ref(cn)} no ${forge} (${PR_TEXT[cpr.state]})`}
              onClick={() => void open(cpr.url)}
            >
              {changeShort(project, cpr.number)}
            </button>
          ) : (
            <span className="muted">—</span>
          )}
        </td>
        <td className="iv-act">
          {canStart && onStartItem && (
            <button
              type="button"
              className="ibtn iv-play"
              aria-label={`Iniciar sessão para ${ref(cn)}`}
              title="Iniciar sessão"
              onClick={(e) => onStartItem(cn, e.currentTarget)}
            >
              <PlayIcon />
            </button>
          )}
        </td>
      </tr>
    );
  };

  const subStatus = (x: PanelNode) => x.work_item?.status ?? (x.issue.state === "closed" ? "concluido" : "backlog");
  const doneSubs = subs?.filter((x) => subStatus(x.node) === "concluido").length ?? 0;
  const waitingSubs = subs?.filter((x) => subStatus(x.node) === "aguardando_epico").length ?? 0;
  const total = subs?.length ?? 0;
  const progressText =
    waitingSubs > 0
      ? `${doneSubs} ${doneSubs === 1 ? "concluída" : "concluídas"}, ${waitingSubs} aguardando o épico de ${total}`
      : `${doneSubs} de ${total} sub-issues concluídas`;

  // Coluna lateral (Decisões e Eventos): só com o histórico do hub.
  const showSide = hasWi && hubUp && history !== null && !historyError;

  return (
    <section aria-label="Detalhe do item" className="item-view">
      {crumbs}
      <div className="iv-head">
        <div className="iv-title">
          <div className="iv-chips">
            <span className="iv-chip" data-kind={isEpic ? "epic" : "issue"}>
              {isEpic ? "épico" : "issue"}
            </span>
            <span className="badge" data-tone={st.tone}>
              {st.text}
            </span>
            {priority !== null && (
              <span className="prio" data-p={priority} title={`prioridade ${priority}`}>
                {priority}
              </span>
            )}
            {warn && (
              <span
                className="badge"
                data-tone="attention"
                title={`O ${change(pr?.number ?? 0)} foi mesclado em ${baseBranch} sem fechar a issue (faltou Closes ${ref(n)})`}
              >
                {mergedOpenText(project)}
              </span>
            )}
            {node.work_item?.branch && (
              <span className="iv-chip">
                branch <span className="mono">{node.work_item.branch}</span>
              </span>
            )}
            {where && where.length > 0 && <span className="muted">rodando em {where.join(", ")}</span>}
            {node.summary.last_activity_at !== null && (
              <span className="muted">última atividade {ago(node.summary.last_activity_at, now)}</span>
            )}
          </div>
          <h1>
            <span className="mono muted">{ref(n)}</span> {node.issue.title}
          </h1>
        </div>
        <div className="actions">
          {live.map((l) => (
            <button
              key={`${l.env}:${l.session}`}
              type="button"
              className="btn"
              onClick={() => nav.openTerminal({ env: l.env, session: l.session })}
            >
              Abrir terminal · {l.name}
            </button>
          ))}
          {shellEnvs.map((x) => (
            <button
              key={`shell:${x.env}`}
              type="button"
              className="btn"
              onClick={() => void openShell(x.env, x.workItemId)}
            >
              {shellEnvs.length > 1 ? `Abrir shell no worktree · ${x.name}` : "Abrir shell no worktree"}
            </button>
          ))}
          <button type="button" className="btn" onClick={() => void open(node.issue.url)}>
            {warn ? `Fechar a issue no ${tracker} ↗` : `Abrir issue no ${tracker} ↗`}
          </button>
          <button
            type="button"
            className="btn"
            disabled={viewBlocked !== null}
            title={viewBlocked ?? `Mostrar a descrição da issue, lida agora no ${tracker}`}
            onClick={() => setViewing(true)}
          >
            Ver issue
          </button>
          {pr && (
            <button type="button" className="btn" onClick={() => void open(pr.url)}>
              Abrir {change(pr.number)} no {forge} ↗
            </button>
          )}
          <StatusMenu
            current={current}
            enabled={hubUp}
            busy={busy}
            describedBy="status-off"
            placeholder={`ex.: aguardando a API do ${tracker}`}
            subIssue={node.issue.parent_number !== null}
            onSet={setStatus}
          />
          <div className="status-menu">
            <button
              type="button"
              className="btn"
              aria-controls="prio-pop"
              aria-expanded={prioMenu}
              disabled={!hubUp}
              onClick={() => setPrioMenu((m) => !m)}
            >
              Prioridade: {priorityText(priority)} ▾
            </button>
            {prioMenu && hubUp && (
              <div id="prio-pop" className="status-pop" role="group" aria-label="Mudar prioridade">
                {PRIORITIES.map((p) => (
                  <button
                    key={p ?? "none"}
                    type="button"
                    className="opt"
                    disabled={busy}
                    aria-current={p === priority ? "true" : undefined}
                    onClick={() => setPriority(p)}
                  >
                    <span>{priorityText(p)}</span>
                    {p === priority && <span className="muted">atual</span>}
                  </button>
                ))}
                {/* Web: `hub.workitem.move` fica fora do /web/ws (spec do épico #205 §4.1). */}
                {priority !== null && !web && (
                  <>
                    <button type="button" className="opt" disabled={busy} onClick={() => void step(-1)}>
                      Subir na ordem de {priority}
                    </button>
                    <button type="button" className="opt" disabled={busy} onClick={() => void step(1)}>
                      Descer na ordem de {priority}
                    </button>
                  </>
                )}
                {/* Na web, desabilitados com o motivo (#26). */}
                {priority !== null && web && (
                  <DesktopOnly
                    className="opt"
                    labels={[`Subir na ordem de ${priority}`, `Descer na ordem de ${priority}`]}
                    note="Reordenar fica no app desktop"
                  />
                )}
              </div>
            )}
          </div>
          {onStart && (
            <button type="button" className="btn btn-primary" onClick={(e) => onStart(e.currentTarget)}>
              <PlayIcon /> Iniciar sessão
            </button>
          )}
        </div>
      </div>
      {!hubUp && (
        <p id="status-off" className="muted small" role="status">
          {hub.expected
            ? "Hub indisponível: visão parcial. Histórico e mudança de status e de prioridade voltam quando o hub responder."
            : "Sem conexão com o hub: histórico e mudança de status e de prioridade dependem dele (Configurações → Hub)."}
        </p>
      )}
      {actionError && (
        <p role="alert" className="start-error">
          {actionError}
        </p>
      )}

      {isEpic && total > 0 && (
        <div className="iv-progress">
          <div
            className="iv-bar"
            role="progressbar"
            aria-label="Progresso do épico"
            aria-valuemin={0}
            aria-valuemax={total}
            aria-valuenow={doneSubs}
            aria-valuetext={progressText}
          >
            <span style={{ width: `${(100 * doneSubs) / total}%` }} />
          </div>
          <span className="muted">{progressText}</span>
        </div>
      )}

      <div className={["iv-grid", meta.length > 0 && "iv-has-meta", showSide && "iv-has-side"].filter(Boolean).join(" ")}>
        {meta.length > 0 && (
          <section className="sec iv-card iv-meta" aria-label={`No ${tracker}`}>
            <h3>
              No {tracker}
              {updatedAt ? <span className="n">atualizada {ago(updatedAt, now)}</span> : null}
            </h3>
            <dl className="iv-meta-list">
              {meta.map((m, i) => (
                <Fragment key={`${m.field}-${i}`}>
                  <dt>{m.label}</dt>
                  <dd>{m.value}</dd>
                </Fragment>
              ))}
            </dl>
          </section>
        )}
        <div className="iv-col iv-main">
          {subs && subs.length > 0 && (
            <section className="sec iv-card" aria-label="Sub-issues">
              <h3>
                Sub-issues <span className="n">{subs.length}</span>
              </h3>
              <table className="iv-table">
                <thead>
                  <tr>
                    <th scope="col">Issue</th>
                    <th scope="col">Status</th>
                    <th scope="col">Sessão</th>
                    <th scope="col">{changeLabel(project)}</th>
                    <th scope="col">
                      <span className="sr-only">Ação</span>
                    </th>
                  </tr>
                </thead>
                <tbody>{subs.map(subRow)}</tbody>
              </table>
            </section>
          )}
          {!hasWi ? (
            <div className="sec iv-card">
              <p className="none">
                Sem histórico: esta issue ainda não teve sessão. Use Iniciar sessão para criar o work item
                {hubUp ? ", ou mude o status manualmente" : ""}.
              </p>
            </div>
          ) : !hubUp ? null : historyError ? (
            <div className="sec iv-card">
              <p role="alert" className="start-error">
                Não foi possível carregar o histórico: {historyError}
              </p>
              <button type="button" className="more" onClick={() => setTick((t) => t + 1)}>
                Tentar de novo
              </button>
            </div>
          ) : !history ? (
            <div className="sec iv-card">
              <p className="none" role="status">
                Carregando histórico…
              </p>
            </div>
          ) : (
            <>
              <section className="sec iv-card" aria-label="Sessões">
                <h3>
                  Sessões <span className="n">{history.sessions.length}</span>
                </h3>
                {history.sessions.length ? (
                  <ul className="list">{history.sessions.map(sessionRow)}</ul>
                ) : (
                  <p className="none">Nenhuma sessão.</p>
                )}
              </section>
              <section className="sec iv-card" aria-label={`${changeLabel(project)}s`}>
                <h3>
                  {changeLabel(project)}s <span className="n">{history.prs.length}</span>
                </h3>
                {history.prs.length ? (
                  <ul className="list">{history.prs.map(prLine)}</ul>
                ) : (
                  <p className="none">Nenhum {changeLabel(project)}.</p>
                )}
              </section>
            </>
          )}
        </div>
        {showSide && (
          <div className="iv-col iv-side">
            <section className="sec iv-card" aria-label="Decisões">
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
                {onReview && (
                  <button type="button" className="btn btn-sm iv-review" onClick={onReview}>
                    Revisar
                  </button>
                )}
              </h3>
              {decisions.length ? (
                <ul className="list">{decisions.map(decisionCard)}</ul>
              ) : (
                <p className="none">Nenhuma decisão registrada.</p>
              )}
            </section>
            <section className="sec iv-card" aria-label="Eventos">
              <h3>
                Eventos <span className="n">{events.length}</span>
              </h3>
              {shown.length ? (
                <ul className="list">
                  {shown.map((e) => (
                    <EventItem key={e.uid} e={e} now={now} />
                  ))}
                </ul>
              ) : (
                <p className="none">Nenhum evento.</p>
              )}
              {events.length > shown.length && (
                <button type="button" className="more" onClick={() => setPages((p) => p + 1)}>
                  Mostrar mais ({events.length - shown.length})
                </button>
              )}
            </section>
          </div>
        )}
      </div>

      {isEpic && hub.expected && (
        <AgentRules owner={owner} repo={repo} epic={n} label={itemFullRef(project, n)} now={now} className="sec iv-card iv-rules" />
      )}
      {viewing && (
        <IssueBodyModal
          project={project}
          number={n}
          title={node.issue.title}
          url={node.issue.url}
          onClose={() => setViewing(false)}
          onOpenItem={onOpenItem}
        />
      )}
    </section>
  );
}

/**
 * Cartão de uma decisão do ledger: origem, marca de pendente ou revisada,
 * título, decisão e consequências (também no painel de contexto, #426).
 */
export function DecisionCard({ d, change }: { d: DecisionView; change: (n: number) => string }) {
  return (
    <li className={pendingDecision(d) ? "decision pending" : "decision"}>
      <span>
        <span className={`origin ${d.origin}`}>{ORIGIN_TEXT[d.origin]}</span>{" "}
        {pendingDecision(d) ? (
          <span className="badge" data-tone="attention">
            pendente de revisão
          </span>
        ) : d.reviewed_at !== null ? (
          <span className="muted small">
            revisada{d.reviewed_in_pr !== null ? ` no ${change(d.reviewed_in_pr)}` : ""}
          </span>
        ) : null}
      </span>
      <span className="dt">{d.title}</span>
      <p>
        <span className="lbl">decisão</span>
        {d.decision}
      </p>
      {d.consequences && (
        <p>
          <span className="lbl">consequências</span>
          {d.consequences}
        </p>
      )}
    </li>
  );
}

/** Linha da linha do tempo: há quanto tempo, o tipo e o resumo do payload. */
export function EventItem({ e, now }: { e: { ts: number; kind: string; payload: unknown }; now: number }) {
  return (
    <li className="ev">
      <time>{ago(e.ts, now)}</time>
      <span>
        <span className="k">{e.kind}</span> <span className="d">{eventDetail(e.payload)}</span>
      </span>
    </li>
  );
}

/** ▶ (mesmo traço do mockup). */
export function PlayIcon({ size = 13 }: { size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinejoin="round" aria-hidden="true">
      <path d="M7 4l12 8-12 8z" />
    </svg>
  );
}
