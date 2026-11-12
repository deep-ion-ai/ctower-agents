import type { HubProjectView, WipLimits, WipUsage } from "../protocol/hub-gen";

/**
 * Limite de trabalho em andamento por projeto (#126). O hub conta o uso
 * (sessões de implementação vivas de todos os ambientes e PRs abertos do
 * espelho) e manda em `HubProjectView.wip`; um hub anterior à 1.15 não
 * manda, e aqui isso vale como sem limite.
 */

/** Protocolo do hub com `wip_limits` e `wip`. */
export const WIP_PROTOCOL = "1.17";

/** Texto do limite atingido (o mesmo do hub e do gestor). */
export const WIP_LIMIT_REACHED = "limite de trabalho em andamento";

/** Erro do hub ao recusar o início automático no limite (`cthubd::wip`). */
export const WIP_LIMIT_REACHED_CODE = -32051;

export const NO_WIP_LIMITS: WipLimits = { max_sessions: null, max_prs: null };

/** Limites do cadastro; ausente (hub anterior) = sem limite. */
export function wipLimitsOf(p: { wip_limits?: WipLimits | null }): WipLimits {
  return { ...NO_WIP_LIMITS, ...(p.wip_limits ?? {}) };
}

/** Uso do projeto, se o hub manda e há algum limite. */
export function wipOf(view: Pick<HubProjectView, "wip"> | null | undefined): WipUsage | null {
  const u = view?.wip;
  if (!u) return null;
  const l: WipLimits = { ...NO_WIP_LIMITS, ...(u.limits ?? {}) };
  return l.max_sessions === null && l.max_prs === null ? null : { ...u, limits: l };
}

const full = (n: number, max: number | null) => max !== null && n >= max;

/** Algum limite atingido (ou passado). */
export function wipFull(u: WipUsage | null): boolean {
  return !!u && (full(u.sessions, u.limits.max_sessions) || full(u.prs, u.limits.max_prs));
}

/** "N de M" de cada limite configurado, para o cartão do projeto. */
export function wipParts(u: WipUsage | null): string[] {
  if (!u) return [];
  const out: string[] = [];
  if (u.limits.max_sessions !== null) out.push(`${u.sessions} de ${u.limits.max_sessions} sessões`);
  if (u.limits.max_prs !== null) out.push(`${u.prs} de ${u.limits.max_prs} PRs abertos`);
  return out;
}

/** Motivo com só os limites atingidos; `null` = há vaga. */
export function wipReason(u: WipUsage | null): string | null {
  if (!u || !wipFull(u)) return null;
  const parts: string[] = [];
  if (full(u.sessions, u.limits.max_sessions)) parts.push(`${u.sessions} de ${u.limits.max_sessions} sessões`);
  if (full(u.prs, u.limits.max_prs)) parts.push(`${u.prs} de ${u.limits.max_prs} PRs abertos`);
  return `${WIP_LIMIT_REACHED} (${parts.join(", ")})`;
}

/**
 * Campo do formulário (texto) para o limite: vazio = sem limite; inteiro de
 * 1 a 1000. Devolve o erro em texto quando não serve.
 */
export function parseWipField(text: string): { value: number | null } | { error: string } {
  const t = text.trim();
  if (t === "") return { value: null };
  if (!/^\d+$/.test(t)) return { error: "use um número inteiro, ou deixe vazio para sem limite" };
  const n = Number(t);
  if (n < 1 || n > 1000) return { error: "use de 1 a 1000, ou deixe vazio para sem limite" };
  return { value: n };
}
