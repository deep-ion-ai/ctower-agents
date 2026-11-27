import type { EnvStatus, HubConnView } from "../api/types";

/**
 * Status do hub depois de uma releitura (`reloadHub`). A resposta do
 * `hub_status` e o evento `hub://status` correm por canais diferentes: a
 * leitura feita antes do handshake pode chegar depois do evento
 * "conectado" e, gravada por cima, deixaria a tela em "conectando…" até
 * outro evento, que não vem enquanto a conexão segue de pé.
 *
 * - sem conexão configurada: nenhum status;
 * - um status que chegou por evento durante a releitura (`since` desde
 *   `startedAt`) vale sobre a leitura, se a leitura não for mais nova;
 * - senão, vale a leitura.
 */
export function mergeHubStatus(
  prev: EnvStatus | null,
  fetched: EnvStatus | null,
  conn: HubConnView | null,
  startedAt: number,
): EnvStatus | null {
  if (!conn) return null;
  const fresh = prev && prev.since >= startedAt ? prev : null;
  if (!fresh) return fetched;
  if (!fetched) return fresh;
  return fetched.since >= fresh.since ? fetched : fresh;
}
