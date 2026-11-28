// Cards por projeto da Sala de controle (#358, épico #353): função pura que
// junta, por projeto, o que já chega à UI. Sem chamada nova por card:
//
// - sessões, fluxo e decisões do mesmo dashboard das linhas de projeto
//   (`hub.dashboard.get` com o hub; `dashboard.get` de cada ambiente sem ele);
// - o badge "N pedem você" dos itens de "Precisa de você" do projeto;
// - PRs/MRs, checks e épicos em execução de `hub.control_room.projects` (#354);
// - a release e as entregas sem release de `hub.projects.release` (#355).
//
// Sem hub, as partes que dependem dele (PRs, release, épicos) ficam de fora.
import type { Dashboard, DashboardWorkItem, PermissionRequest, Project, SyncState, WorkItemStatus } from "../protocol/gen";
import type {
  ControlRoomEpicRun,
  ControlRoomProject,
  HubDashboard,
  HubDashboardWorkItem,
  HubProject,
  ProjectRelease,
} from "../protocol/hub-gen";
import type { ProjectRow } from "../project/rows";
import { syncFailures, SYNC_FAIL_ALERT } from "../project/sync-health";
import { clock } from "../hub/epicRun";
import { isGitlabProject, isJiraProject, itemShort, itemTitle, noIssue, trackerLabel } from "../util/item";
import { ago, duration } from "../util/time";
import {
  FAILED_WINDOW_MS,
  JIRA_SYNC_INTERVAL_MS,
  SYNC_INTERVAL_MS,
  SYNC_LATE_FACTOR,
  type AttentionItem,
  type HubRoom,
} from "./attention";

const DAY = 86_400_000;
/** "Feitas em 7 dias" do fluxo. */
export const DONE_WINDOW_MS = 7 * DAY;

const plural = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`;
const same = (a: string, b: string) => a.toLowerCase() === b.toLowerCase();
const sameRepo = (a: { owner: string; repo: string }, b: { owner: string; repo: string }) =>
  same(a.owner, b.owner) && same(a.repo, b.repo);

export type CardSessionState = "running" | "waiting" | "failed";

export const SESSION_STATE_TEXT: Record<CardSessionState, string> = {
  running: "Rodando",
  waiting: "Aguardando",
  failed: "Falhou",
};

export interface CardSession {
  key: string;
  /** "#31", "PROJ-31", "sessão do projeto". */
  ref: string;
  /** Título da issue, sem a referência; vazio sem título no cache. */
  title: string;
  state: CardSessionState;
  /** Sessão viva: abre o terminal. Falhou: `null`, abre o projeto. */
  terminal: { env: string; session: string } | null;
}

export interface CardFlow {
  backlog: number;
  doing: number;
  review: number;
  done: number;
}

export interface CardPrs {
  /** "PRs abertos" ou "MRs abertos". */
  label: string;
  count: number;
  /** "Nenhum", "2 · checks ok", "1 · e2e vermelho", "3 · checks rodando". */
  text: string;
  tone: "error" | "running" | "neutral";
  /** Uma linha por PR/MR, para o tooltip. */
  lines: string[];
}

export interface CardRelease {
  /** "v1.4.2", "Sem release" ou "—" (GitLab, sem leitura, hub anterior). */
  text: string;
  /** "5 entregas na develop sem release"; `null` com "—". */
  note: string | null;
  warn: boolean;
  /** Entregas sem release, para o tooltip. */
  lines: string[];
}

export interface CardEpic {
  runId: number;
  epic: number;
  /** "#482" ou a chave do Jira. */
  ref: string;
  title: string;
  done: number;
  total: number;
  /** "4 de 7 sub-issues" ou "Sem plano aprovado". */
  progress: string;
  /** "Em andamento: #481 · próxima #483" ou o motivo de estar parado. */
  step: string;
  stopped: boolean;
}

export type CardBadge = { text: string; tone: "attention" | "running" | "idle" };

export interface ProjectCard {
  row: ProjectRow;
  key: string;
  name: string;
  /** "acme/acme-web" ou "Jira ACME · grupo/app". */
  repo: string;
  autonomous: boolean;
  needs: number;
  badge: CardBadge;
  sessions: CardSession[];
  /** Sessões vivas (rodando ou aguardando): o filtro "Com sessão rodando". */
  live: number;
  /** Sem sessão no card: "Nenhuma sessão rodando" ou "Sem sessão há N dias". */
  idleText: string | null;
  flow: CardFlow;
  decisions: number;
  /** Só com o hub. */
  prs?: CardPrs;
  /** Só com o hub. */
  release?: CardRelease;
  /** Só com o hub. */
  epics?: CardEpic[];
  sync: { text: string; warn: boolean };
}

export interface CardsInput {
  rows: ProjectRow[];
  items: AttentionItem[];
  dashboards: Record<string, Dashboard>;
  /** Dashboard consolidado do hub, quando as linhas vêm dele. */
  hubDashboard: HubDashboard | null;
  permissions?: Record<string, PermissionRequest[]>;
  /** Leitura do hub (#354); sem hub, `undefined`. */
  room?: HubRoom;
  /** `hub.projects.release` (#355); `null` = hub sem o método ou fora do ar. */
  releases: ProjectRelease[] | null;
  now: number;
}

type AnySession = { id: string; work_item_id: number; state: string; last_event_at: number; ended_at: number | null; exit_code: number | null };
type AnyItem = DashboardWorkItem | HubDashboardWorkItem;

interface Source {
  project: Project | HubProject | null;
  items: AnyItem[];
  live: { s: AnySession; env: string }[];
  failed: AnySession[];
  interrupted: AnySession[];
}

/** O dashboard do projeto da linha: o do hub (linha do hub) ou o do ambiente. */
function sourceOf(row: ProjectRow, dashboards: Record<string, Dashboard>, hd: HubDashboard | null): Source {
  const key = { owner: row.project.gh_owner, repo: row.project.gh_repo };
  const mine = (p: { gh_owner: string; gh_repo: string }) => sameRepo(key, { owner: p.gh_owner, repo: p.gh_repo });
  if (row.env === null) {
    const p = hd?.projects.find((x) => mine(x.project));
    if (!hd || !p) return { project: null, items: [], live: [], failed: [], interrupted: [] };
    const ids = new Set(p.work_items.map((w) => w.id));
    const of = (ss: typeof hd.live_sessions) => ss.filter((s) => ids.has(s.work_item_id));
    return {
      project: p.project,
      items: p.work_items,
      live: of(hd.live_sessions).map((s) => ({ s, env: s.env_id })),
      failed: of(hd.failed_sessions),
      interrupted: of(hd.interrupted_sessions),
    };
  }
  const d = dashboards[row.env.id];
  const p = d?.projects.find((x) => mine(x.project));
  if (!d || !p) return { project: null, items: [], live: [], failed: [], interrupted: [] };
  const ids = new Set(p.work_items.map((w) => w.id));
  const of = (ss: typeof d.live_sessions) => ss.filter((s) => ids.has(s.work_item_id));
  const env = row.env.id;
  return {
    project: p.project,
    items: p.work_items,
    live: of(d.live_sessions).map((s) => ({ s, env })),
    failed: of(d.failed_sessions),
    interrupted: of(d.interrupted_sessions),
  };
}

const DOING: WorkItemStatus[] = ["em_execucao", "aguardando_voce", "bloqueado", "pausado"];
const REVIEW: WorkItemStatus[] = ["em_revisao", "aguardando_epico"];

/** Fluxo das issues do projeto (sem épicos e sem os itens sem issue). */
export function flowOf(items: readonly AnyItem[], now: number): CardFlow {
  const f: CardFlow = { backlog: 0, doing: 0, review: 0, done: 0 };
  for (const w of items) {
    if (w.kind !== "issue" || noIssue(w.gh_number)) continue;
    if (w.status === "backlog") f.backlog++;
    else if (DOING.includes(w.status)) f.doing++;
    else if (REVIEW.includes(w.status)) f.review++;
    else if (w.status === "concluido" && w.closed_at !== null && now - w.closed_at <= DONE_WINDOW_MS) f.done++;
  }
  return f;
}

/**
 * Decisões autônomas a revisar do projeto. O épico já soma as das
 * sub-issues: dele entra só o que passa da soma das sub-issues conhecidas
 * (`parent_number`, que o dashboard do hub não traz).
 */
export function decisionsOf(items: readonly AnyItem[]): number {
  let n = 0;
  for (const w of items) if (w.kind === "issue") n += w.pending_decisions;
  for (const e of items) {
    if (e.kind !== "epic") continue;
    const subs = items
      .filter((w) => w.kind === "issue" && "parent_number" in w && w.parent_number === e.gh_number)
      .reduce((acc, w) => acc + w.pending_decisions, 0);
    n += Math.max(0, e.pending_decisions - subs);
  }
  return n;
}

function sessionsOf(src: Source, permissions: CardsInput["permissions"], now: number): CardSession[] {
  const wiOf = (id: number) => src.items.find((w) => w.id === id);
  const card = (s: AnySession, state: CardSessionState, terminal: CardSession["terminal"]): CardSession => {
    const wi = wiOf(s.work_item_id);
    const project = src.project ?? undefined;
    const ref = wi ? (noIssue(wi.gh_number) ? itemTitle(wi, project) : itemShort(project, wi.gh_number)) : "sessão";
    const title = wi && !noIssue(wi.gh_number) ? (wi.title ?? "") : "";
    return { key: `${state}:${s.id}`, ref, title, state, terminal };
  };
  const live = src.live.map(({ s, env }) => {
    const asking = permissions?.[env]?.some((p) => p.session_id === s.id) ?? false;
    const waiting = asking || wiOf(s.work_item_id)?.status === "aguardando_voce";
    return card(s, waiting ? "waiting" : "running", { env, session: s.id });
  });
  // Falhou nas últimas 24 h e o item não voltou a ter sessão viva.
  const liveItems = new Set(src.live.map(({ s }) => s.work_item_id));
  const failed = src.failed
    .filter((s) => now - (s.ended_at ?? s.last_event_at) <= FAILED_WINDOW_MS && !liveItems.has(s.work_item_id))
    .map((s) => card(s, "failed", null));
  const rank: Record<CardSessionState, number> = { waiting: 0, failed: 1, running: 2 };
  return [...live, ...failed].sort((a, b) => rank[a.state] - rank[b.state]);
}

/** Sem sessão no card: há quanto tempo, pela sessão encerrada mais recente que o dashboard conhece. */
function idleOf(src: Source, now: number): string {
  const last = Math.max(0, ...[...src.failed, ...src.interrupted].map((s) => s.ended_at ?? s.last_event_at));
  const days = last > 0 ? Math.floor((now - last) / DAY) : 0;
  return days >= 1 ? `Sem sessão há ${plural(days, "dia", "dias")}` : "Nenhuma sessão rodando";
}

/** PRs/MRs abertos com o estado dos checks (#354). */
export function prsOf(p: ControlRoomProject | undefined, gitlab: boolean): CardPrs {
  const label = (p ? p.forge === "gitlab" : gitlab) ? "MRs abertos" : "PRs abertos";
  const prs = p?.prs ?? [];
  const short = p?.forge === "gitlab" ? "!" : "#";
  const lines = prs.map(
    (pr) => `${short}${pr.number}${pr.title ? ` ${pr.title}` : ""}${pr.checks.text ? ` — ${pr.checks.text}` : ""}`,
  );
  if (prs.length === 0) return { label, count: 0, text: "Nenhum", tone: "neutral", lines };
  const n = prs.length;
  const failing = prs.filter((pr) => pr.checks.state === "failure");
  if (failing.length > 0) {
    const names = [...new Set(failing.flatMap((pr) => pr.checks.failed_names))];
    return { label, count: n, text: `${n} · ${names.length ? names.join(", ") : "check"} vermelho`, tone: "error", lines };
  }
  if (prs.some((pr) => pr.checks.state === "pending")) return { label, count: n, text: `${n} · checks rodando`, tone: "running", lines };
  if (prs.some((pr) => pr.checks.state === "success")) return { label, count: n, text: `${n} · checks ok`, tone: "neutral", lines };
  return { label, count: n, text: String(n), tone: "neutral", lines };
}

/** Release atual e entregas na base sem release (#355); "—" no GitLab e sem leitura. */
export function releaseOf(r: ProjectRelease | undefined, base: string, gitlab: boolean): CardRelease {
  const none: CardRelease = { text: "—", note: null, warn: false, lines: [] };
  if (gitlab || !r || !r.supported) return none;
  if (!r.synced) return { ...none, note: "Ainda sem leitura" };
  const n = r.unreleased;
  const count = `${n}${r.truncated ? "+" : ""}`;
  const note =
    n > 0 ? `${count} ${n === 1 && !r.truncated ? "entrega" : "entregas"} na ${base} sem release` : `Nada na ${base} sem release`;
  return {
    text: r.release ? r.release.tag : "Sem release",
    note,
    warn: n > 0,
    lines: r.deliveries.map((d) => `#${d.number} ${d.title}`),
  };
}

/** Andamento ou motivo de parada de uma execução de épico (#354). */
export function epicStep(run: ControlRoomEpicRun, project?: Project | HubProject | null): string {
  const ref = (n: number) => itemShort(project ?? undefined, n);
  const s = run.stop;
  if (s) {
    const why = s.reason ? `: ${s.reason}` : "";
    switch (s.kind) {
      case "pausado":
        return `Pausada${why}`;
      case "estacionado":
        return `Estacionada${s.until !== null ? ` até ${clock(s.until)}` : ""}${why}`;
      case "aguardando_decisao":
        return `Parada${s.number !== null ? ` em ${ref(s.number)}` : ""}: aguardando sua decisão`;
      case "aguardando_aprovacao":
        return "Plano aguardando sua aprovação";
      case "falha":
        return `Falha do motor${why}`;
      case "ambiente_fora":
        return "Parada: o ambiente da execução está fora do ar";
    }
  }
  const parts: string[] = [];
  if (run.current.length > 0) parts.push(`Em andamento: ${run.current.map((c) => ref(c.number)).join(", ")}`);
  if (run.next) parts.push(run.current.length > 0 ? `próxima ${ref(run.next.number)}` : `Próxima: ${ref(run.next.number)}`);
  if (parts.length === 0) return run.status === "integrando" ? "Integrando as sub-issues" : "Sem sub-issue em andamento";
  return parts.join(" · ");
}

function epicsOf(p: ControlRoomProject | undefined, project: Project | HubProject | null): CardEpic[] {
  return (p?.epic_runs ?? []).map((r) => ({
    runId: r.run_id,
    epic: r.epic,
    ref: itemShort(project ?? undefined, r.epic),
    title: r.title ?? "",
    done: r.done,
    total: r.total,
    progress: r.total > 0 ? `${r.done} de ${r.total} sub-issues` : "Sem plano aprovado",
    step: epicStep(r, project),
    stopped: r.stop !== null,
  }));
}

/** Última linha: o sync do rastreador, com aviso quando falha ou atrasa. */
export function syncLine(s: SyncState | null | undefined, project: ProjectRow["project"], now: number): { text: string; warn: boolean } {
  const t = trackerLabel(project);
  if (!s) return { text: `${t} · ainda não sincronizado`, warn: false };
  const failures = syncFailures(s);
  if (failures >= SYNC_FAIL_ALERT) return { text: `${t} · sync falhando há ${failures} ciclos`, warn: true };
  if (s.last_error) return { text: `${t} · sincronização com erro ${ago(s.last_attempt_at, now)}`, warn: true };
  if (s.last_ok_at === null) return { text: `${t} · ainda não sincronizado`, warn: false };
  const interval = isJiraProject(project) ? JIRA_SYNC_INTERVAL_MS : SYNC_INTERVAL_MS;
  if (now - s.last_ok_at > SYNC_LATE_FACTOR * interval) {
    return { text: `${t} · sync atrasado (último há ${duration(now - s.last_ok_at)})`, warn: true };
  }
  return { text: `${t} · sincronizado ${ago(s.last_ok_at, now)}`, warn: false };
}

/** "acme/acme-web" ou, no Jira, "Jira ACME · grupo/app". */
function repoText(p: ProjectRow["project"]): string {
  const repo = `${p.gh_owner}/${p.gh_repo}`;
  return p.tracker?.kind === "jira" ? `Jira ${p.tracker.project_key} · ${repo}` : repo;
}

/** Itens de "Precisa de você" do projeto da linha. */
function needsOf(row: ProjectRow, items: readonly AttentionItem[]): number {
  const repo = `${row.project.gh_owner}/${row.project.gh_repo}`;
  return items.filter(
    (i) => i.repo !== undefined && same(i.repo, repo) && (row.env === null || i.env === row.env.id || i.env === "hub"),
  ).length;
}

/** Um card por linha de projeto, ordenados pelo que pede você e, depois, pela ordem das linhas. */
export function projectCards(input: CardsInput): ProjectCard[] {
  const { rows, items, dashboards, hubDashboard, permissions, room, releases, now } = input;
  const cards = rows.map((row, i): ProjectCard & { order: number } => {
    const src = sourceOf(row, dashboards, hubDashboard);
    const key = { owner: row.project.gh_owner, repo: row.project.gh_repo };
    const hubProject = room?.projects.find((p) => sameRepo(p, key));
    const sessions = sessionsOf(src, permissions, now);
    const live = sessions.filter((s) => s.state !== "failed").length;
    const needs = needsOf(row, items);
    const epics = room ? epicsOf(hubProject, src.project) : undefined;
    const moving = live > 0 || (epics ?? []).some((e) => !e.stopped);
    const badge: CardBadge =
      needs > 0
        ? { text: needs === 1 ? "1 pede você" : `${needs} pedem você`, tone: "attention" }
        : moving
          ? { text: "Tudo andando", tone: "running" }
          : { text: "Parado", tone: "idle" };
    const gitlab = isGitlabProject(row.project);
    const base = src.project?.base_branch ?? "base";
    return {
      order: i,
      row,
      key: row.key,
      name: row.name,
      repo: repoText(row.project),
      autonomous: src.project?.autonomy?.enabled ?? false,
      needs,
      badge,
      sessions,
      live,
      idleText: sessions.length === 0 ? idleOf(src, now) : null,
      flow: flowOf(src.items, now),
      decisions: decisionsOf(src.items),
      ...(room
        ? {
            prs: prsOf(hubProject, gitlab),
            release: releaseOf(releases?.find((r) => sameRepo(r, key)), base, gitlab || hubProject?.forge === "gitlab"),
            epics,
          }
        : {}),
      sync: syncLine(row.sync ?? row.view?.sync, row.project, now),
    };
  });
  return cards.sort((a, b) => b.needs - a.needs || a.order - b.order).map(({ order: _order, ...c }) => c);
}

export type CardFilter = "all" | "attention" | "running";

export const CARD_FILTERS: { id: CardFilter; label: string }[] = [
  { id: "all", label: "Todos" },
  { id: "attention", label: "Com pendência" },
  { id: "running", label: "Com sessão rodando" },
];

export function filterCards(cards: readonly ProjectCard[], f: CardFilter): ProjectCard[] {
  if (f === "attention") return cards.filter((c) => c.needs > 0);
  if (f === "running") return cards.filter((c) => c.live > 0);
  return [...cards];
}
