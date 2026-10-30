import { ago } from "../../util/time";
import type { SyncState } from "../../protocol/hub-gen";
import type { Tone } from "./form";

/**
 * Sincronização em uma linha: a do GitHub pelo hub ou, no projeto Jira, a
 * que o ambiente de sync relatou (o mesmo `sync_state`).
 */
export function syncText(s: SyncState | null, now: number): { text: string; tone: Tone } {
  if (!s) return { text: "ainda não sincronizado", tone: "neutral" };
  if (s.last_error) return { text: `sincronização com erro ${ago(s.last_attempt_at, now)}`, tone: "error" };
  if (s.last_ok_at === null) return { text: "ainda não sincronizado", tone: "neutral" };
  return { text: `sincronizado ${ago(s.last_ok_at, now)}`, tone: "ok" };
}
