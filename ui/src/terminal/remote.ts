// Remote Control do Claude Code nas sessões (#197): o estado que a UI
// desktop mostra a partir do `rc_session_id` da sessão e do pedido em curso.
import type { Session } from "../protocol/gen";

/** Quanto a UI espera o id depois de pedir para ligar (spec do épico #205 §6.3). */
export const RC_CONNECT_MS = 30_000;

/** Link da sessão no app Claude / claude.ai. */
export const rcUrl = (id: string) => `https://claude.ai/code/${id}`;

const LIVE = new Set(["starting", "running", "idle"]);

/** Ligado = sessão viva com `rc_session_id` (desligar dentro da sessão não gera evento). */
export const rcOn = (s: Session | null | undefined): boolean => !!s?.rc_session_id && LIVE.has(s.state);

export type RcView =
  | { kind: "on"; url: string }
  | { kind: "connecting" }
  | { kind: "failed" }
  | { kind: "off" };

/**
 * Estado do Remote Control de uma sessão: ligado (com o link), conectando
 * (pedido há menos de {@link RC_CONNECT_MS}), não conectou (prazo vencido sem
 * o id) ou desligado.
 */
export function rcView(s: Session | null | undefined, requestedAt: number | undefined, now: number): RcView {
  if (s && rcOn(s)) return { kind: "on", url: rcUrl(s.rc_session_id!) };
  if (requestedAt === undefined) return { kind: "off" };
  return now - requestedAt < RC_CONNECT_MS ? { kind: "connecting" } : { kind: "failed" };
}
