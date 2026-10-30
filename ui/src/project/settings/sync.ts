import { ago } from "../../util/time";
import type { HubProjectView, SyncState } from "../../protocol/hub-gen";
import { plural, type Tone } from "./form";

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

/**
 * Estado do projeto numa linha (índice de Projetos, Visão geral do
 * projeto): atribuição com erro ou pendente pede atenção antes da
 * sincronização; sem nada disso, ambientes e sincronização. Veio do antigo
 * Hub › Projetos e atribuições, removido na #18.
 */
export function projectState(p: HubProjectView, now: number): { text: string; tone: Tone } {
  const errors = p.assignments.filter((a) => a.state === "error").length;
  if (errors > 0) return { text: plural(errors, "atribuição com erro", "atribuições com erro"), tone: "error" };
  const pending = p.assignments.filter((a) => a.state === "pending").length;
  if (pending > 0) return { text: plural(pending, "atribuição pendente", "atribuições pendentes"), tone: "attention" };
  const sync = syncText(p.sync, now);
  const envs = p.assignments.length === 0 ? "sem ambiente" : plural(p.assignments.length, "ambiente", "ambientes");
  return { text: `${envs} · ${sync.text}`, tone: p.assignments.length === 0 && sync.tone === "ok" ? "neutral" : sync.tone };
}
