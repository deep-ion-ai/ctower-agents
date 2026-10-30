import { describe, expect, test } from "vitest";
import type { WorkItemStatus } from "../protocol/gen";
import {
  NO_FILTERS,
  boardColumns,
  boardItems,
  byPriority,
  RECENT_MS,
  buildView,
  isExpanded,
  lastActivity,
  matchesQuery,
  sortNodes,
  statusKey,
  type TreeFilters,
  type ViewNode,
} from "./tree-view";

const NOW = 1_800_000_000_000;
const DAY = 24 * 60 * 60 * 1000;

function n(
  num: number,
  p: { state?: string; status?: WorkItemStatus | null; last?: number | null; kids?: ViewNode[]; title?: string } = {},
): ViewNode {
  return {
    issue: { gh_number: num, title: p.title ?? `item ${num}`, state: p.state ?? "open" },
    work_item: p.status === null ? null : { status: p.status ?? "em_execucao" },
    summary: { last_activity_at: p.last === undefined ? NOW : p.last },
    children: p.kids ?? [],
  };
}
const f = (p: Partial<TreeFilters> = {}): TreeFilters => ({ ...NO_FILTERS, ...p });
const nums = (l: ViewNode[]) => l.map((x) => x.issue.gh_number);
const noLive = () => false;

describe("visibilidade", () => {
  test("9 concluídas e 1 aberta: o épico aparece com as 10", () => {
    const kids = [
      ...Array.from({ length: 9 }, (_, i) => n(10 + i, { state: "closed", status: "concluido" })),
      n(20),
    ];
    const v = buildView([n(1, { kids })], f());
    expect(v.epics).toHaveLength(1);
    expect(v.epics[0].shown).toHaveLength(10);
  });

  test("épico totalmente fechado e avulsa fechada só com 'mostrar concluídos'", () => {
    const roots = [
      n(1, { state: "closed", status: "concluido", kids: [n(2, { state: "closed" })] }),
      n(3, { state: "closed" }),
      n(4),
    ];
    const padrao = buildView(roots, f());
    expect(padrao.epics).toHaveLength(0);
    expect(nums(padrao.loose)).toEqual([4]);
    const todos = buildView(roots, f({ showDone: true }));
    expect(nums(todos.epics.map((e) => e.epic))).toEqual([1]);
    expect(nums(todos.loose).sort()).toEqual([3, 4]);
  });

  test("épico fechado com uma sub-issue aberta aparece", () => {
    const v = buildView([n(1, { state: "closed", kids: [n(2), n(3, { state: "closed" })] })], f());
    expect(v.epics[0].shown).toHaveLength(2);
  });
});

describe("busca", () => {
  test("por #número, número (prefixo) e trecho do título", () => {
    const x = n(43, { title: "Filtro de Status" });
    expect(matchesQuery(x, "#43")).toBe(true);
    expect(matchesQuery(x, "43")).toBe(true);
    expect(matchesQuery(x, "4")).toBe(true);
    expect(matchesQuery(x, "44")).toBe(false);
    expect(matchesQuery(x, "status")).toBe(true);
    expect(matchesQuery(x, "  ")).toBe(true);
  });

  test("pela chave do Jira (#352): PROJ-43, sem diferenciar maiúsculas", () => {
    const x = n(43, { title: "Filtro de Status" });
    expect(matchesQuery(x, "PROJ-43")).toBe(true);
    expect(matchesQuery(x, "proj-4")).toBe(true);
    expect(matchesQuery(x, "PROJ-44")).toBe(false);
    // Trecho de título com hífen continua sendo título.
    expect(matchesQuery(n(7, { title: "pje-web 12" }), "pje-web")).toBe(true);
  });

  test("épico aparece quando uma sub-issue casa, só com as que casam", () => {
    const roots = [n(1, { kids: [n(2, { title: "alfa" }), n(3, { title: "beta" })] }), n(9, { kids: [n(10)] })];
    const v = buildView(roots, f({ query: "beta" }));
    expect(nums(v.epics.map((e) => e.epic))).toEqual([1]);
    expect(nums(v.epics[0].shown)).toEqual([3]);
  });

  test("épico que casa sozinho traz todas as filhas", () => {
    const v = buildView([n(1, { title: "Tela", kids: [n(2), n(3)] })], f({ query: "tela" }));
    expect(nums(v.epics[0].shown)).toEqual([2, 3]);
  });

  test("busca não traz concluídos escondidos", () => {
    const v = buildView([n(5, { state: "closed", status: "concluido" })], f({ query: "5" }));
    expect(v.loose).toHaveLength(0);
  });
});

describe("chips de status", () => {
  const roots = [
    n(1, { kids: [n(2, { status: "bloqueado" }), n(3, { status: "pausado" }), n(4, { status: "em_execucao" })] }),
    n(7, { status: "backlog" }),
  ];
  test("multisseleção: une os status", () => {
    const v = buildView(roots, f({ chips: new Set<WorkItemStatus>(["bloqueado", "pausado", "backlog"]) }));
    expect(nums(v.epics[0].shown)).toEqual([2, 3]);
    expect(nums(v.loose)).toEqual([7]);
  });
  test("sem item no status: nada aparece", () => {
    const v = buildView(roots, f({ chips: new Set<WorkItemStatus>(["em_revisao"]) }));
    expect(v.epics).toHaveLength(0);
    expect(v.loose).toHaveLength(0);
  });
  test("status sem work item deriva da issue", () => {
    expect(statusKey(n(1, { status: null }))).toBe("backlog");
    expect(statusKey(n(1, { status: null, state: "closed" }))).toBe("concluido");
  });
});

describe("ordenação", () => {
  test("atividade recente; a do épico é a da família; sem work item no fim", () => {
    const roots = [
      n(1, { last: NOW - 5 * DAY, kids: [n(2, { last: NOW - 1000 })] }),
      n(3, { last: NOW - DAY }),
      n(4, { status: null, last: null }),
      n(5, { last: NOW - 9 * DAY }),
    ];
    expect(nums(sortNodes(roots, "activity"))).toEqual([1, 3, 5, 4]);
    expect(nums(sortNodes(roots, "number"))).toEqual([1, 3, 4, 5]);
  });
  test("empate desempata por número e não muda a lista original", () => {
    const l = [n(8, { last: 5 }), n(2, { last: 5 })];
    expect(nums(sortNodes(l, "activity"))).toEqual([2, 8]);
    expect(nums(l)).toEqual([8, 2]);
  });
  test("lastActivity ignora nulos", () => {
    expect(lastActivity(n(1, { last: null, kids: [n(2, { last: 7 })] }))).toBe(7);
    expect(lastActivity(n(1, { last: null }))).toBeNull();
  });
});

describe("expansão", () => {
  const velho = n(1, { last: NOW - RECENT_MS - DAY, kids: [n(2, { last: NOW - RECENT_MS - DAY })] });
  test("recolhido sem sessão viva nem atividade em 7 dias", () => {
    expect(isExpanded(velho, f(), undefined, NOW, noLive)).toBe(false);
  });
  test("expandido com atividade recente (inclusive da filha)", () => {
    const e = n(1, { last: null, kids: [n(2, { last: NOW - 2 * DAY })] });
    expect(isExpanded(e, f(), undefined, NOW, noLive)).toBe(true);
  });
  test("expandido com sessão viva em uma filha", () => {
    expect(isExpanded(velho, f(), undefined, NOW, (x) => x.issue.gh_number === 2)).toBe(true);
  });
  test("escolha manual vale", () => {
    expect(isExpanded(velho, f(), true, NOW, noLive)).toBe(true);
    const novo = n(1, { kids: [n(2)] });
    expect(isExpanded(novo, f(), false, NOW, noLive)).toBe(false);
  });
  test("com busca ou chip ativo, expandido", () => {
    expect(isExpanded(velho, f({ query: "x" }), false, NOW, noLive)).toBe(true);
    expect(isExpanded(velho, f({ chips: new Set<WorkItemStatus>(["backlog"]) }), undefined, NOW, noLive)).toBe(true);
  });
});

describe("kanban", () => {
  test("boardItems: épico + filhas + avulsas; com filtro o épico só entra se casa", () => {
    const roots = [n(1, { kids: [n(2, { status: "bloqueado" }), n(3)] }), n(4)];
    const sem = boardItems(buildView(roots, f()), f());
    expect(sem.map((i) => [i.node.issue.gh_number, i.epicOf])).toEqual([
      [1, null],
      [2, 1],
      [3, 1],
      [4, null],
    ]);
    const chips = f({ chips: new Set<WorkItemStatus>(["bloqueado"]) });
    expect(boardItems(buildView(roots, chips), chips).map((i) => i.node.issue.gh_number)).toEqual([2]);
    const q = f({ query: "item 1" });
    expect(boardItems(buildView(roots, q), q).map((i) => i.node.issue.gh_number)).toEqual([1, 2, 3]);
  });

  test("boardColumns: 8 colunas, ou só as dos chips", () => {
    expect(boardColumns(f())).toHaveLength(8);
    expect(boardColumns(f({ chips: new Set<WorkItemStatus>(["pausado", "backlog"]) }))).toEqual(["backlog", "pausado"]);
  });
});

describe("chip Concluído sozinho", () => {
  const chip: TreeFilters = { ...NO_FILTERS, chips: new Set<WorkItemStatus>(["concluido"]) };

  test("traz a avulsa fechada e o épico todo fechado sem 'mostrar concluídos'", () => {
    const fechada = n(16, { state: "closed", status: null });
    const epico = n(4, { state: "closed", status: "concluido", kids: [n(5, { state: "closed", status: null })] });
    const aberta = n(7, { status: "em_execucao" });
    const v = buildView([fechada, epico, aberta], chip);
    expect(v.loose.map((x) => x.issue.gh_number)).toEqual([16]);
    expect(v.epics.map((e) => e.epic.issue.gh_number)).toEqual([4]);
    // Sem o chip, continuam escondidos.
    const sem = buildView([fechada, epico, aberta], NO_FILTERS);
    expect(sem.loose.map((x) => x.issue.gh_number)).toEqual([7]);
    expect(sem.epics).toHaveLength(0);
  });

  test("o Kanban recebe os concluídos na coluna Concluído", () => {
    const fechada = n(16, { state: "closed", status: null });
    const v = buildView([fechada], chip);
    expect(boardItems(v, chip).map((i) => i.node.issue.gh_number)).toEqual([16]);
  });

  test("outros chips não trazem concluídos", () => {
    const f: TreeFilters = { ...NO_FILTERS, chips: new Set<WorkItemStatus>(["backlog"]) };
    expect(buildView([n(16, { state: "closed", status: null })], f).loose).toHaveLength(0);
  });
});

describe("prioridade (visão do hub)", () => {
  const p = (num: number, priority: "P0" | "P1" | "P2" | "P3" | null, rank: number | null = null): ViewNode => ({
    ...n(num),
    work_item: { status: "backlog", priority, rank },
  });

  test("ordem: classe, depois rank, não triado no fim; empate mantém a ordem", () => {
    const list = [p(1, null), p(2, "P2", 1), p(3, "P0", 2), p(4, "P0", 1), p(5, "P1", null), p(6, "P1", 1), n(7)];
    expect(nums([...list].sort(byPriority))).toEqual([4, 3, 6, 5, 2, 1, 7]);
    expect(nums(sortNodes(list, "priority"))).toEqual([4, 3, 6, 5, 2, 1, 7]);
  });

  test("sem prioridade (árvore local): a ordenação por prioridade cai no número", () => {
    expect(nums(sortNodes([n(9), n(3), n(5)], "priority"))).toEqual([3, 5, 9]);
  });
});
