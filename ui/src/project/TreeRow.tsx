// Texto e selos das linhas de item (árvore do painel lateral, Kanban).
import type { PrState, WorkItemSummary } from "../protocol/gen";
import { changeLabel, changeRef, itemShort, type ProviderProject } from "../util/item";
import { ago } from "../util/time";
import { AWAITING_APPROVAL_TEXT, awaitingApproval, type ApprovalProject } from "./approval";
import { currentSprint } from "./sprints";
import { STATUS } from "./status";
import { priorityOf, statusKey, type ViewNode } from "./tree-view";

const PR_TEXT: Record<PrState, string> = { open: "aberto", merged: "mesclado", closed: "fechado" };

/** Nó com o resumo completo (`WorkItemSummary`), igual na árvore local e na do hub. */
export type RowNode = ViewNode & { summary: WorkItemSummary };

/**
 * PR mais recente mesclado na base do projeto com a issue ainda aberta
 * (#375): o PR não trouxe `Closes #n`. O PR para a branch do épico não conta,
 * porque a sub-issue só fecha com o PR do épico.
 */
export const mergedOpen = (
  n: { issue: { state: string }; summary: WorkItemSummary },
  base: string | undefined,
): boolean =>
  base !== undefined && n.issue.state === "open" && n.summary.pr?.state === "merged" && n.summary.pr.base === base;

export const MERGED_OPEN_TEXT = "PR mesclado, issue aberta";

/** O aviso acima com o nome da mudança na forja do projeto (`MR mesclado` no GitLab). */
export const mergedOpenText = (p?: ProviderProject | null) => `${changeLabel(p)} mesclado, issue aberta`;

/**
 * Projeto no modo autônomo (piloto automático, #181): `autonomy.enabled` do
 * projeto do hub. Ausente (hub anterior à #181 ou projeto local), o projeto
 * não é autônomo e o selo de modelo faltando não aparece.
 */
export const isAutonomous = (p: unknown): boolean =>
  typeof p === "object" &&
  p !== null &&
  (p as { autonomy?: { enabled?: unknown } | null }).autonomy?.enabled === true;

export const MISSING_MODEL_TEXT = "sem modelo";

/**
 * Falta o modelo do agente (#182): issue aberta, que não é épico, sem modelo
 * no hub, num projeto autônomo. O agendador (#183) só inicia issue com modelo.
 */
export const missingModel = (n: ViewNode & { issue: { has_sub_issues?: boolean } }, project: unknown): boolean =>
  isAutonomous(project) &&
  n.issue.state === "open" &&
  !n.issue.has_sub_issues &&
  statusKey(n) !== "concluido" &&
  !n.work_item?.model;

/** Texto do contador de decisões: `◆3`, ou `◆3!` com autônomas pendentes de revisão. */
export const decisionsText = (d: WorkItemSummary["decisions"]) =>
  `◆${d.total}${d.pending_autonomous > 0 ? "!" : ""}`;

/** Dica do contador, para quem não vê a cor nem o símbolo. */
export function decisionsTitle(d: WorkItemSummary["decisions"]): string {
  const base = `${d.total} ${d.total === 1 ? "decisão" : "decisões"}`;
  if (d.pending_autonomous === 0) return base;
  const p = d.pending_autonomous;
  return `${base}, ${p} ${p === 1 ? "autônoma pendente" : "autônomas pendentes"} de revisão`;
}

/** Nome acessível da linha: título, status, PR, decisões e última atividade (não só o título). */
export function rowLabel(
  node: RowNode,
  now: number,
  base?: string,
  project?: (ProviderProject & ApprovalProject) | null,
): string {
  const { pr, decisions, last_activity_at } = node.summary;
  const parts = [`${itemShort(project, node.issue.gh_number)} ${node.issue.title}`, STATUS[statusKey(node)].text];
  const p = priorityOf(node);
  if (p !== null) parts.push(`prioridade ${p}`);
  const sprint = currentSprint(node.issue);
  if (sprint) parts.push(`sprint ${sprint.name}`);
  if (pr) parts.push(`${changeRef(project, pr.number)} ${PR_TEXT[pr.state]}`);
  if (mergedOpen(node, base)) parts.push(mergedOpenText(project));
  if (missingModel(node, project)) parts.push(`${MISSING_MODEL_TEXT} (o piloto automático não a inicia)`);
  if (awaitingApproval(node, project)) parts.push(AWAITING_APPROVAL_TEXT);
  if (decisions.total > 0) parts.push(decisionsTitle(decisions));
  parts.push(last_activity_at !== null ? `última atividade ${ago(last_activity_at, now)}` : "sem atividade");
  return parts.join(", ");
}

/**
 * Selo "aguardando aprovação" (#124): o projeto exige o rótulo para iniciar
 * sessão e a issue aberta não o tem. Decorativo: o texto vai no nome
 * acessível da linha (`rowLabel`).
 */
export function ApprovalBadge({ node, project }: { node: ViewNode; project?: ApprovalProject | null }) {
  if (!awaitingApproval(node, project)) return null;
  const label = project?.approval?.label.trim() ?? "";
  return (
    <span className="pj-approval" aria-hidden="true" title={`Sem o rótulo "${label}": o projeto o exige para iniciar sessão`}>
      {AWAITING_APPROVAL_TEXT}
    </span>
  );
}

/** Selo `P0`–`P3` (nada para item não triado). */
export function PriorityBadge({ node }: { node: ViewNode }) {
  const p = priorityOf(node);
  if (p === null) return null;
  return (
    <span className="prio" data-p={p} title={`prioridade ${p}`}>
      {p}
    </span>
  );
}
