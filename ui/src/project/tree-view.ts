import type { WorkItemStatus } from "../protocol/gen";
import type { Priority } from "../protocol/hub-gen";
import { ALL_SPRINTS, matchesSprint, sprintShowsDone, type SprintFilter, type SprintIssue } from "./sprints";

/** Janela de "atividade recente" para o épico começar expandido. */
export const RECENT_MS = 7 * 24 * 60 * 60 * 1000;

/** Ordem dos chips de status (a mesma do mockup). */
export const STATUS_ORDER: WorkItemStatus[] = [
  "backlog",
  "em_execucao",
  "aguardando_voce",
  "em_revisao",
  "bloqueado",
  "pausado",
  "aguardando_epico",
  "concluido",
];

/**
 * Forma mínima de um nó, comum à árvore local (`TreeNode`) e à do hub
 * (`HubTreeNode`). Prioridade e ordem só existem na visão do hub.
 */
export interface ViewNode {
  issue: { gh_number: number; title: string; state: string } & SprintIssue;
  work_item: { status: WorkItemStatus; priority?: Priority | null; rank?: number | null } | null;
  summary: { last_activity_at: number | null };
  children: ViewNode[];
}

export type SortKey = "activity" | "number" | "priority";

export interface TreeFilters {
  query: string;
  chips: ReadonlySet<WorkItemStatus>;
  sort: SortKey;
  showDone: boolean;
  /**
   * Sprint do Jira (`sprints.ts`); `""` = todas; `null` = ainda não
   * escolhida nesta execução (vale a guardada do projeto).
   */
  sprint: SprintFilter | null;
}

export const NO_FILTERS: TreeFilters = {
  query: "",
  chips: new Set(),
  sort: "activity",
  showDone: false,
  sprint: null,
};

/** Há busca, chip ou sprint escolhida: os épicos aparecem expandidos. */
export const narrowing = (f: TreeFilters) =>
  f.query.trim() !== "" || f.chips.size > 0 || (f.sprint ?? ALL_SPRINTS) !== ALL_SPRINTS;

export const isOpen = (n: ViewNode) => n.issue.state === "open";

/** Status do nó: o do work item, ou derivado da issue sem work item. */
export function statusKey(n: ViewNode): WorkItemStatus {
  if (n.work_item) return n.work_item.status;
  return n.issue.state === "closed" ? "concluido" : "backlog";
}

/**
 * `#43`, `43`, a chave do Jira `PROJ-43` (número exato ou prefixo) ou
 * trecho do título, sem diferenciar maiúsculas. A árvore é de um projeto só,
 * então qualquer prefixo `CHAVE-` antes do número vale (#352).
 */
export function matchesQuery(n: ViewNode, query: string): boolean {
  const q = query.trim().toLowerCase().replace(/^#/, "");
  if (q === "") return true;
  const title = n.issue.title.toLowerCase().includes(q);
  const num = q.replace(/^[a-z][a-z0-9_]*-(?=\d+$)/, "");
  return /^\d+$/.test(num) ? String(n.issue.gh_number).startsWith(num) || title : title;
}

const matchesChips = (n: ViewNode, f: TreeFilters) => f.chips.size === 0 || f.chips.has(statusKey(n));
export const matches = (n: ViewNode, f: TreeFilters) =>
  matchesQuery(n, f.query) && matchesChips(n, f) && matchesSprint(n.issue, f.sprint ?? ALL_SPRINTS);

/** Atividade mais recente do nó e das filhas (null = sem work item/atividade). */
export function lastActivity(n: ViewNode): number | null {
  let best: number | null = null;
  for (const x of [n, ...n.children]) {
    const t = x.summary.last_activity_at;
    if (t !== null && (best === null || t > best)) best = t;
  }
  return best;
}

/** Prioridade do nó (`null` = não triado). */
export const priorityOf = (n: ViewNode): Priority | null => n.work_item?.priority ?? null;

const PRIORITY_INDEX: Record<Priority, number> = { P0: 0, P1: 1, P2: 2, P3: 3 };

/**
 * Ordem da spec do gestor §4.3 dentro de um projeto: prioridade (não
 * triado no fim), depois o `rank` (nulo no fim). Empate = 0, para a
 * ordenação estável manter a ordem anterior.
 */
export function byPriority(a: ViewNode, b: ViewNode): number {
  const pa = priorityOf(a);
  const pb = priorityOf(b);
  const ia = pa === null ? 4 : PRIORITY_INDEX[pa];
  const ib = pb === null ? 4 : PRIORITY_INDEX[pb];
  if (ia !== ib) return ia - ib;
  const ra = a.work_item?.rank ?? null;
  const rb = b.work_item?.rank ?? null;
  if (ra === rb) return 0;
  if (ra === null) return 1;
  if (rb === null) return -1;
  return ra - rb;
}

/** Ordena sem mutar: atividade recente (sem atividade vai para o fim), número ou prioridade. */
export function sortNodes<T extends ViewNode>(list: readonly T[], sort: SortKey): T[] {
  return [...list].sort((a, b) => {
    if (sort === "number") return a.issue.gh_number - b.issue.gh_number;
    if (sort === "priority") return byPriority(a, b) || a.issue.gh_number - b.issue.gh_number;
    const ka = lastActivity(a);
    const kb = lastActivity(b);
    if (ka === kb) return a.issue.gh_number - b.issue.gh_number;
    if (ka === null) return 1;
    if (kb === null) return -1;
    return kb - ka;
  });
}

export interface EpicView<T extends ViewNode> {
  epic: T;
  /** Sub-issues a mostrar (todas, ou só as que casam com busca/chips). */
  shown: T[];
}

export interface TreeView<T extends ViewNode> {
  epics: EpicView<T>[];
  loose: T[];
}

/**
 * Aplica visibilidade, busca, chips e ordenação.
 * Visibilidade: por padrão só itens em aberto; um épico aberto, ou com ao menos
 * uma sub-issue aberta, aparece completo. "Mostrar concluídos" (ou o chip
 * "Concluído") traz avulsas
 * fechadas e épicos totalmente fechados; uma sprint escolhida (ou a ativa)
 * também, para mostrar a sprint inteira.
 */
export function buildView<T extends ViewNode>(roots: readonly T[], f: TreeFilters): TreeView<T> {
  const epics: EpicView<T>[] = [];
  const loose: T[] = [];
  // O chip "Concluído" pede os encerrados: sem isso o filtro nunca mostraria nada.
  const sprint = f.sprint ?? ALL_SPRINTS;
  const showDone = f.showDone || f.chips.has("concluido") || sprintShowsDone(sprint);
  for (const r of roots) {
    const kids = r.children as T[];
    if (kids.length > 0) {
      if (!showDone && !isOpen(r) && !kids.some(isOpen)) continue;
      let shown = kids;
      if (narrowing(f)) {
        const hits = kids.filter((c) => matches(c, f));
        if (!matches(r, f) && hits.length === 0) continue;
        // Épico que casa só com a busca (sem chips nem sprint) traz todas as filhas.
        shown = matchesQuery(r, f.query) && f.chips.size === 0 && sprint === ALL_SPRINTS ? kids : hits;
      }
      epics.push({ epic: r, shown: sortNodes(shown, f.sort) });
    } else if ((showDone || isOpen(r)) && matches(r, f)) {
      loose.push(r);
    }
  }
  const order = sortNodes(
    epics.map((e) => e.epic),
    f.sort,
  );
  return {
    epics: order.map((o) => epics.find((e) => e.epic === o)!),
    loose: sortNodes(loose, f.sort),
  };
}

export interface BoardItem<T extends ViewNode> {
  node: T;
  /** Número do épico pai (sub-issue); null em épico e avulsa. */
  epicOf: number | null;
}

/**
 * Itens do Kanban a partir da mesma visão da árvore (visibilidade, busca, chips):
 * o épico entra como card (sempre, sem filtro; com filtro, só se ele mesmo casa),
 * seguido das sub-issues mostradas e das avulsas. A ordem de cada coluna é a da árvore.
 */
export function boardItems<T extends ViewNode>(view: TreeView<T>, f: TreeFilters): BoardItem<T>[] {
  const items: BoardItem<T>[] = [];
  for (const { epic, shown } of view.epics) {
    if (!narrowing(f) || matches(epic, f)) items.push({ node: epic, epicOf: null });
    for (const c of shown) items.push({ node: c, epicOf: epic.issue.gh_number });
  }
  for (const l of view.loose) items.push({ node: l, epicOf: null });
  return items;
}

/** Colunas visíveis: uma por status; com chips ativos, só as dos chips. */
export const boardColumns = (f: TreeFilters): WorkItemStatus[] =>
  STATUS_ORDER.filter((k) => f.chips.size === 0 || f.chips.has(k));

/** Sessão viva no épico ou em alguma filha (`isLive` vem de cada tela). */
export const familyLive = <T extends ViewNode>(n: T, isLive: (n: T) => boolean) =>
  [n, ...(n.children as T[])].some(isLive);

/**
 * Expansão do épico: com busca/chips ativos fica expandido; senão vale a escolha
 * manual; sem ela, só com sessão viva ou atividade nos últimos 7 dias.
 */
export function isExpanded<T extends ViewNode>(
  epic: T,
  f: TreeFilters,
  manual: boolean | undefined,
  now: number,
  isLive: (n: T) => boolean,
): boolean {
  if (narrowing(f)) return true;
  if (manual !== undefined) return manual;
  const t = lastActivity(epic);
  return familyLive(epic, isLive) || (t !== null && now - t <= RECENT_MS);
}

/** Linha do épico recolhido: o motivo de estar fechado. */
export function collapsedHint(epic: ViewNode): string {
  const n = epic.children.length;
  return `${n} ${n === 1 ? "sub-issue" : "sub-issues"} · sem sessão viva nem atividade nos últimos 7 dias`;
}

export const EMPTY_FILTERED =
  "Nada casa com a busca e os filtros. Limpe a busca, desmarque os chips de status ou escolha outra sprint.";
export const EMPTY_ACTIVE_SPRINT = "Nenhum item na sprint ativa. Escolha outra sprint ou “Todas”.";
/** Projeto Jira sem nenhuma issue com sprint no espelho. */
export const SPRINTS_UNREAD = "sprints ainda não lidas (aguarde o próximo sync)";
export const EMPTY_OPEN = "Nenhum item em aberto. Marque “mostrar concluídos” para ver os encerrados.";
