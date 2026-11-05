// "Adicionar issue pelo ID" (#150): traz ao espelho uma issue do Jira que
// está fora do JQL do cadastro, pelo `tracker.add_issue` do ambiente (visão
// local) ou pelo `hub.tracker.add_issue` (visão do hub, que repassa ao
// ambiente de sync). Fica disponível pelo mesmo critério do "Sincronizar
// este projeto" (#19, `syncPlan`): sem ambiente de sync, ou com ele
// desconectado, o botão fica desabilitado com o motivo.
import { useSyncExternalStore } from "react";
import type { TrackerIssueAdded } from "../protocol/gen";
import type { Tower } from "../state/tower";
import type { SyncAddr, ProjectSyncInfo } from "./sync-now";

/** Rótulo do botão do cabeçalho. */
export const ADD_ISSUE_LABEL = "Adicionar issue";
/** Rótulo do comando da paleta (sem o nome do projeto). */
export const ADD_ISSUE_COMMAND = "Adicionar issue pelo ID…";
/** Aviso do diálogo: a issue sem work item não fica no espelho. */
export const ADD_ISSUE_PRUNE_NOTE =
  "Sem work item, a issue sai do espelho na próxima reconciliação do sync. Para mantê-la, inicie uma sessão nela (ou inclua-a no JQL do projeto).";

/**
 * Confere o formato da chave antes do envio: `ACME-123`, `acme-123`, só o
 * número (`123`) ou o link da issue (`…/browse/ACME-123`). Devolve o que vai
 * no `key` (sem espaços) ou a mensagem do erro. A chave de outro projeto
 * passa daqui: quem diz isso é o ambiente, que conhece a chave do projeto.
 */
export function checkIssueKey(input: string, projectKey?: string | null): { key: string } | { error: string } {
  const raw = input.trim();
  const example = `${projectKey?.trim() || "ACME"}-123`;
  if (!raw) return { error: `Informe a chave da issue (como ${example}) ou só o número.` };
  // Link: o último trecho do caminho, sem query nem âncora.
  const last =
    raw
      .split(/[?#]/)[0]
      .replace(/\/+$/, "")
      .split("/")
      .pop() ?? "";
  if (/^\d+$/.test(last) || /^[A-Za-z][A-Za-z0-9_]*-\d+$/.test(last)) {
    return { key: /^\d+$/.test(last) ? last : last.toUpperCase() };
  }
  return { error: `Chave inválida: "${raw}". Use ${example} ou só o número.` };
}

/** O botão e o diálogo: habilitado ou o motivo de não estar. */
export function addIssueState(info: ProjectSyncInfo): { enabled: boolean; reason: string | null } {
  return info.plan.targets.length > 0
    ? { enabled: true, reason: null }
    : { enabled: false, reason: info.plan.reason ?? "Nenhum ambiente conectado tem este projeto." };
}

/**
 * Pede a issue: pelo hub (`owner/repo`) ou direto no ambiente do projeto
 * local (`project_id`). Rejeita com a mensagem do ambiente (ou do hub).
 */
export function addIssue(
  api: Pick<Tower["api"], "call" | "hubCall">,
  addr: SyncAddr,
  key: string,
): Promise<TrackerIssueAdded> {
  return "owner" in addr
    ? api.hubCall<TrackerIssueAdded>("hub.tracker.add_issue", { owner: addr.owner, repo: addr.repo, key })
    : api.call<TrackerIssueAdded>(addr.env, "tracker.add_issue", { project_id: addr.projectId, key });
}

/** Mensagem de erro sem o prefixo `Error: ` do `String(e)`. */
export function errorText(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

// Pedido de abrir o diálogo vindo de fora da tela do projeto (a paleta):
// fica guardado pela chave do projeto (`syncKey`) até a tela dele montar.
const requests = new Set<string>();
const listeners = new Set<() => void>();

function notify(): void {
  for (const l of listeners) l();
}

/** A paleta pede o diálogo do projeto `key`; a tela dele abre ao ver o pedido. */
export function requestAddIssue(key: string): void {
  requests.add(key);
  notify();
}

/** Atende o pedido (a tela abriu o diálogo). */
export function takeAddIssueRequest(key: string): boolean {
  if (!requests.delete(key)) return false;
  notify();
  return true;
}

/** Há pedido pendente para o projeto `key`? */
export function useAddIssueRequest(key: string | null): boolean {
  return useSyncExternalStore(
    (l) => {
      listeners.add(l);
      return () => void listeners.delete(l);
    },
    () => key !== null && requests.has(key),
    () => key !== null && requests.has(key),
  );
}

/** Esquece os pedidos (entre testes). */
export function resetAddIssueRequests(): void {
  requests.clear();
  notify();
}
