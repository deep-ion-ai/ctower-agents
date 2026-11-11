// Rótulo de aprovação por projeto (#124). A regra é a de
// `ct_core::ApprovalConfig` (crates/ct-core/src/approval.rs): rótulo vazio
// desliga; o item conta pelos próprios rótulos (o épico pelo dele, a
// sub-issue pelo dela); comparação sem diferenciar maiúsculas. Quem recusa o
// início é o ambiente; a UI só avisa antes e oferece "iniciar mesmo assim".
import type { ApprovalConfig } from "../protocol/hub-gen";

/** Rótulo padrão de um projeto (e de um hub anterior à #124). */
export const DEFAULT_APPROVAL_LABEL = "aprovada";

/** Protocolo do hub que traz `approval` em `HubProject`. */
export const APPROVAL_PROTOCOL = "1.18";

/** Texto do selo nas issues sem o rótulo. */
export const AWAITING_APPROVAL_TEXT = "aguardando aprovação";

/** Início fixo da mensagem de recusa do ambiente (`APPROVAL_REFUSAL_PREFIX`). */
export const APPROVAL_REFUSAL_PREFIX = "aguardando aprovação";

/** Código JSON-RPC da recusa (`ct_http::sync::APPROVAL_REQUIRED`). */
export const APPROVAL_REQUIRED = -32052;

/** Projeto (do hub ou do ambiente) no que a regra precisa; ausente = hub ou ambiente anterior. */
export interface ApprovalProject {
  approval?: ApprovalConfig | null;
}

/** O rótulo ativo, se a exigência vale (ligada e com rótulo). */
export function enforcedLabel(p: ApprovalProject | null | undefined): string | null {
  const a = p?.approval;
  if (!a?.required) return null;
  const label = a.label.trim();
  return label ? label : null;
}

/** O item tem o rótulo `label`. */
export function hasLabel(labels: readonly string[] | undefined, label: string): boolean {
  const want = label.trim().toLowerCase();
  return (labels ?? []).some((l) => l.trim().toLowerCase() === want);
}

/**
 * Selo "aguardando aprovação": exigência ligada no projeto, issue aberta e
 * sem o rótulo. Item concluído não leva o selo.
 */
export function awaitingApproval(
  n: { issue: { state: string; labels?: readonly string[] }; work_item?: { status: string } | null },
  p: ApprovalProject | null | undefined,
): boolean {
  const label = enforcedLabel(p);
  if (!label || n.issue.state !== "open" || n.work_item?.status === "concluido") return false;
  return !hasLabel(n.issue.labels, label);
}

/** A mensagem de erro é a recusa do ambiente por falta do rótulo. */
export const isApprovalRefusal = (message: string): boolean =>
  message.replace(/^Error:\s*/, "").startsWith(APPROVAL_REFUSAL_PREFIX);
