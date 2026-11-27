import type { WorkItemStatus } from "../protocol/gen";
import type { Priority } from "../protocol/hub-gen";
import { isAssignee } from "./assignee";
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
  issue: {
    gh_number: number;
    title: string;
    state: string;
    assignees?: readonly string[];
    /** Rótulos do espelho (o selo "aguardando aprovação", #124). */
    labels?: readonly string[];
  } & SprintIssue;
  work_item: {
    status: WorkItemStatus;
    priority?: Priority | null;
    rank?: number | null;
    /** Modelo do agente na issue (#182), só no hub. */
    model?: string | null;
  } | null;
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
  /**
   * "Só minhas" (#151): só as issues atribuídas ao "eu" do rastreador
   * (`SyncState.me`), que cada função recebe à parte. Com o "eu"
   * desconhecido, a tela o desliga (`mineFilters`).
   */
  mine: boolean;
}

export const NO_FILTERS: TreeFilters = {
  query: "",
  chips: new Set(),
  sort: "activity",
  showDone: false,
  sprint: null,
  mine: false,
};

/** Há busca, chip, sprint escolhida ou "Só minhas": os épicos aparecem expandidos. */
export const narrowing = (f: TreeFilters) =>
  f.query.trim() !== "" || f.chips.size > 0 || (f.sprint ?? ALL_SPRINTS) !== ALL_SPRINTS || f.mine === true;

/**
 * Filtros guardados antes do "Só minhas" (sem `mine`) valem como desligado;
 * sem o "eu" do rastreador (`me`), o filtro também fica desligado (não
 * esconde a árvore toda), mas a escolha guardada continua lá.
 */
export const mineFilters = (f: TreeFilters, me: string | null | undefined): TreeFilters => ({
  ...f,
  mine: f.mine === true && !!me?.trim(),
});

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
const matchesMine = (n: ViewNode, f: TreeFilters, me: string | null | undefined) =>
  f.mine !== true || isAssignee(n.issue.assignees, me);
/** `me`: o "eu" do rastreador, para o "Só minhas" (sem ele, nada é meu). */
export const matches = (n: ViewNode, f: TreeFilters, me?: string | null) =>
  matchesQuery(n, f.query) &&
  matchesChips(n, f) &&
  matchesSprint(n.issue, f.sprint ?? ALL_SPRINTS) &&
  matchesMine(n, f, me);

/** O mínimo para percorrer a árvore: o número e as filhas. */
export interface TreeShape {
  issue: { gh_number: number };
  children: readonly TreeShape[];
}

/**
 * Descendentes do nó em profundidade (filhas, netas…), sem o próprio nó, na
 * ordem da árvore. Protegido contra ciclo no `parent_number`: um nó já
 * visitado (pelo número) não é visitado de novo (#217).
 */
export function descendants<T extends TreeShape>(n: T): T[] {
  const out: T[] = [];
  const seen = new Set<number>([n.issue.gh_number]);
  const walk = (x: T) => {
    for (const c of x.children as T[]) {
      if (seen.has(c.issue.gh_number)) continue;
      seen.add(c.issue.gh_number);
      out.push(c);
      walk(c);
    }
  };
  walk(n);
  return out;
}

/** Todos os nós da árvore (raízes e descendentes), cada número uma vez só. */
export function flattenTree<T extends TreeShape>(roots: readonly T[]): T[] {
  const out: T[] = [];
  const seen = new Set<number>();
  for (const r of roots) {
    for (const x of [r, ...descendants(r)]) {
      if (seen.has(x.issue.gh_number)) continue;
      seen.add(x.issue.gh_number);
      out.push(x);
    }
  }
  return out;
}

/**
 * Caminho da raiz até o nó `gh` (inclusive), ou `null` fora da árvore:
 * `[épico, sub-épico, neta]`. Protegido contra ciclo como `descendants`.
 */
export function pathTo<T extends TreeShape>(roots: readonly T[], gh: number): T[] | null {
  const seen = new Set<number>();
  const walk = (x: T, path: T[]): T[] | null => {
    if (seen.has(x.issue.gh_number)) return null;
    seen.add(x.issue.gh_number);
    const here = [...path, x];
    if (x.issue.gh_number === gh) return here;
    for (const c of x.children as T[]) {
      const found = walk(c, here);
      if (found) return found;
    }
    return null;
  };
  for (const r of roots) {
    const found = walk(r, []);
    if (found) return found;
  }
  return null;
}

/** Atividade mais recente do nó e dos descendentes (null = sem work item/atividade). */
export function lastActivity(n: ViewNode): number | null {
  let best: number | null = null;
  for (const x of [n, ...descendants(n)]) {
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
  /** Filhas diretas a mostrar (todas, ou só as que casam com busca/chips). */
  shown: T[];
  /**
   * Visão de cada filha mostrada que é sub-épico (#217), pelo número: as
   * mesmas regras valem em cada nível.
   */
  subs: Map<number, EpicView<T>>;
  /** O próprio épico entra no Kanban (sem filtro, ou ele mesmo casa). */
  hit: boolean;
}

export interface TreeView<T extends ViewNode> {
  epics: EpicView<T>[];
  loose: T[];
}

/**
 * Aplica visibilidade, busca, chips e ordenação, em qualquer profundidade
 * (épico › sub-épico › sub-issues, #217).
 * Visibilidade: por padrão só itens em aberto; um épico aberto, ou com ao menos
 * um descendente aberto, aparece completo. "Mostrar concluídos" (ou o chip
 * "Concluído") traz avulsas
 * fechadas e épicos totalmente fechados; uma sprint escolhida (ou a ativa)
 * também, para mostrar a sprint inteira.
 * Busca e chips: uma neta que casa mantém o sub-épico e o épico acima dela;
 * um épico (ou sub-épico) que casa com a busca traz todos os descendentes.
 * "Só minhas" (`me`, o "eu" do rastreador): o épico aparece se ele ou um
 * descendente for meu, e só com os descendentes meus.
 */
export function buildView<T extends ViewNode>(roots: readonly T[], f: TreeFilters, me?: string | null): TreeView<T> {
  const epics: EpicView<T>[] = [];
  const loose: T[] = [];
  // O chip "Concluído" pede os encerrados: sem isso o filtro nunca mostraria nada.
  const sprint = f.sprint ?? ALL_SPRINTS;
  const showDone = f.showDone || f.chips.has("concluido") || sprintShowsDone(sprint);
  const narrow = narrowing(f);
  // Números já postos na visão: um ciclo no `parent_number` não repete nó.
  const seen = new Set<number>();
  const kidsOf = (n: T) => (n.children as T[]).filter((c) => !seen.has(c.issue.gh_number));

  /**
   * Visão do épico `e` (já marcado em `seen`), ou null se nada dele aparece.
   * `byAncestor`: um épico acima casou com a busca e traz os descendentes
   * (com "Só minhas", os meus).
   */
  const epicView = (e: T, byAncestor: boolean): EpicView<T> | null => {
    // Épico que casa com a busca (sem chips nem sprint) traz todas as
    // filhas; com "Só minhas", todas as minhas.
    const byEpic = byAncestor || (matchesQuery(e, f.query) && f.chips.size === 0 && sprint === ALL_SPRINTS);
    const self = byAncestor ? matchesMine(e, f, me) : matches(e, f, me);
    const kids = kidsOf(e);
    for (const c of kids) seen.add(c.issue.gh_number);
    const shown: T[] = [];
    const subs = new Map<number, EpicView<T>>();
    for (const c of kids) {
      if (kidsOf(c).length > 0) {
        const sub = epicView(c, byEpic);
        if (sub) {
          shown.push(c);
          subs.set(c.issue.gh_number, sub);
        }
      } else if (!narrow || (byEpic ? matchesMine(c, f, me) : matches(c, f, me))) {
        shown.push(c);
      }
    }
    if (narrow && !self && shown.length === 0) return null;
    return { epic: e, shown: sortNodes(shown, f.sort), subs, hit: !narrow || self };
  };

  for (const r of roots) {
    if (seen.has(r.issue.gh_number)) continue;
    seen.add(r.issue.gh_number);
    if (kidsOf(r).length > 0) {
      if (!showDone && !isOpen(r) && !descendants(r).some(isOpen)) continue;
      const v = epicView(r, false);
      if (v) epics.push(v);
    } else if ((showDone || isOpen(r)) && matches(r, f, me)) {
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
  /** Número do épico pai direto (sub-issue ou sub-épico); null em épico raiz e avulsa. */
  epicOf: number | null;
}

/**
 * Itens do Kanban a partir da mesma visão da árvore (visibilidade, busca, chips):
 * o épico entra como card (sempre, sem filtro; com filtro, só se ele mesmo casa),
 * seguido das sub-issues mostradas (com as netas dos sub-épicos, #217) e das
 * avulsas. A ordem de cada coluna é a da árvore.
 */
export function boardItems<T extends ViewNode>(view: TreeView<T>): BoardItem<T>[] {
  const items: BoardItem<T>[] = [];
  const walk = (ev: EpicView<T>, epicOf: number | null) => {
    if (ev.hit) items.push({ node: ev.epic, epicOf });
    for (const c of ev.shown) {
      const sub = ev.subs.get(c.issue.gh_number);
      if (sub) walk(sub, ev.epic.issue.gh_number);
      else items.push({ node: c, epicOf: ev.epic.issue.gh_number });
    }
  };
  for (const ev of view.epics) walk(ev, null);
  for (const l of view.loose) items.push({ node: l, epicOf: null });
  return items;
}

/** Colunas visíveis: uma por status; com chips ativos, só as dos chips. */
export const boardColumns = (f: TreeFilters): WorkItemStatus[] =>
  STATUS_ORDER.filter((k) => f.chips.size === 0 || f.chips.has(k));

/** Sessão viva no épico ou em algum descendente (`isLive` vem de cada tela). */
export const familyLive = <T extends ViewNode>(n: T, isLive: (n: T) => boolean) =>
  [n, ...descendants(n)].some(isLive);

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
  "Nada casa com a busca e os filtros. Limpe a busca, desmarque os chips de status, escolha outra sprint ou desligue “Só minhas”.";
/** O "eu" do rastreador ainda não é conhecido: a dica do "Só minhas" desabilitado. */
export const MINE_UNKNOWN =
  "Seu usuário no rastreador ainda não é conhecido (o sync ainda não rodou, ou o “jira me” do ambiente de sync falhou).";
export const EMPTY_ACTIVE_SPRINT = "Nenhum item na sprint ativa. Escolha outra sprint ou “Todas”.";
/** Projeto Jira sem nenhuma issue com sprint no espelho. */
export const SPRINTS_UNREAD = "sprints ainda não lidas (aguarde o próximo sync)";
export const EMPTY_OPEN = "Nenhum item em aberto. Marque “mostrar concluídos” para ver os encerrados.";
