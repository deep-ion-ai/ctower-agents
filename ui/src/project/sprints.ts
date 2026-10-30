// Sprints do Jira na árvore e no quadro: o filtro por sprint, a lista de
// sprints do projeto (derivada das issues do espelho) e o chip da sprint
// atual de cada item. Só projetos Jira têm sprints; no GitHub a lista vem
// vazia e nada disso aparece.
import type { Sprint, SprintState } from "../protocol/gen";

/**
 * Filtro por sprint: `""` = todas, `"active"` = a sprint ativa (qualquer
 * issue com uma sprint `active`), `"none"` = sem sprint, ou o id de uma
 * sprint (`"2273"`).
 */
export type SprintFilter = string;

export const ALL_SPRINTS: SprintFilter = "";
export const ACTIVE_SPRINT: SprintFilter = "active";
export const NO_SPRINT: SprintFilter = "none";

/** O que o filtro lê da issue (`sprints` ausente = hub ou ambiente anterior). */
export interface SprintIssue {
  sprints?: Sprint[] | null;
}

const sprintsOf = (i: SprintIssue): Sprint[] => i.sprints ?? [];

/** A issue pertence ao filtro: está na lista de sprints dela. */
export function matchesSprint(i: SprintIssue, f: SprintFilter): boolean {
  if (f === ALL_SPRINTS) return true;
  const list = sprintsOf(i);
  if (f === NO_SPRINT) return list.length === 0;
  if (f === ACTIVE_SPRINT) return list.some((s) => s.state === "active");
  return list.some((s) => String(s.id) === f);
}

/** Filtro que mostra também os itens concluídos (a sprint inteira). */
export const sprintShowsDone = (f: SprintFilter) => f !== ALL_SPRINTS && f !== NO_SPRINT;

/** Sprint atual da issue: a ativa, ou a última da lista (a mais recente). */
export function currentSprint(i: SprintIssue): Sprint | null {
  const list = sprintsOf(i);
  return list.find((s) => s.state === "active") ?? list[list.length - 1] ?? null;
}

/**
 * Sprints conhecidas do projeto (as das issues), sem repetição, da mais
 * recente para a mais antiga: pelo início (a futura, sem início, vem antes)
 * e, no empate, pelo id.
 */
export function projectSprints(issues: Iterable<SprintIssue>): Sprint[] {
  const byId = new Map<number, Sprint>();
  for (const i of issues) for (const s of sprintsOf(i)) if (!byId.has(s.id)) byId.set(s.id, s);
  const start = (s: Sprint) => s.start_date ?? Number.POSITIVE_INFINITY;
  return [...byId.values()].sort((a, b) => {
    const d = start(b) - start(a);
    return Number.isNaN(d) || d === 0 ? b.id - a.id : d;
  });
}

const STATE_TEXT: Record<SprintState, string> = { active: "ativa", closed: "fechada", future: "futura" };

/** `26/10` na hora local. */
const day = (ms: number) => {
  const d = new Date(ms);
  return `${String(d.getDate()).padStart(2, "0")}/${String(d.getMonth() + 1).padStart(2, "0")}`;
};

/** `Sprint 48 · ativa · 26/10–06/11` (sem as datas que a sprint não tem). */
export function sprintLabel(s: Sprint): string {
  const parts = [s.name || `Sprint ${s.id}`, STATE_TEXT[s.state] ?? s.state];
  const start = s.start_date !== null ? day(s.start_date) : null;
  const end = s.end_date !== null ? day(s.end_date) : null;
  if (start || end) parts.push(`${start ?? "?"}–${end ?? "?"}`);
  return parts.join(" · ");
}

/** Filtro guardado que não vale mais (sprint que sumiu do espelho) volta a "todas". */
export function effectiveSprint(f: SprintFilter, known: readonly Sprint[]): SprintFilter {
  if (f === ALL_SPRINTS || f === ACTIVE_SPRINT || f === NO_SPRINT) return f;
  return known.some((s) => String(s.id) === f) ? f : ALL_SPRINTS;
}

const KEY = "ct.project.sprint";

/** Filtro de sprint guardado do projeto (localStorage, por chave do projeto). */
export function readSprintChoice(project: string): SprintFilter {
  try {
    const v = JSON.parse(localStorage.getItem(KEY) ?? "{}") as Record<string, unknown>;
    return typeof v[project] === "string" ? (v[project] as string) : ALL_SPRINTS;
  } catch {
    return ALL_SPRINTS;
  }
}

export function saveSprintChoice(project: string, f: SprintFilter): void {
  try {
    const v = JSON.parse(localStorage.getItem(KEY) ?? "{}") as Record<string, unknown>;
    if (f === ALL_SPRINTS) delete v[project];
    else v[project] = f;
    localStorage.setItem(KEY, JSON.stringify(v));
  } catch {
    /* sem storage: vale só enquanto o app está aberto */
  }
}
