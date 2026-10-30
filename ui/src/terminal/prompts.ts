// Prompts prontos do console (#425; spec da #87, §2.4): o texto com as
// variáveis dos templates e o envio por bracketed paste, para um prompt de
// várias linhas chegar inteiro ao Claude Code antes do Enter.
import type { ReadyPrompt } from "../api/types";
import { renderGestorTemplate, renderTemplate } from "../project/templates";
import type { TabInfo } from "./SessionTree";

export const PASTE_START = "\x1b[200~";
export const PASTE_END = "\x1b[201~";
/** Espera entre o fim da colagem e o Enter: o agente fecha a colagem antes de ver o `\r`. */
export const ENTER_DELAY_MS = 80;

/** Motivo do menu desligado com um pedido de permissão pendente. */
export const PERMISSION_PENDING =
  "O agente está esperando a resposta do pedido de permissão. Responda antes de mandar um prompt.";

/**
 * O texto colado: quebras de linha como `\n` (o Enter fica só para o fim) e
 * sem marcadores de colagem no meio, que encerrariam a colagem antes da hora.
 */
export function pasteSequence(text: string): string {
  const clean = text
    .replace(/\r\n?/g, "\n")
    .replaceAll(PASTE_START, "")
    .replaceAll(PASTE_END, "");
  return `${PASTE_START}${clean}${PASTE_END}`;
}

/**
 * Texto do prompt com as variáveis da sessão: as dos templates de issue
 * (`{{number}}`, `{{branch}}`, `{{key}}`…) quando há work item e projeto; as
 * do gestor (`{{env}}`, `{{scope}}`…) no gestor. Variável sem valor fica como está.
 */
export function renderPrompt(text: string, info: TabInfo, envName: string): string {
  const { wi, project } = info;
  if (info.gestor) return renderGestorTemplate(text, envName, info.scope ?? { kind: "portfolio" });
  if (!wi || !project) return text;
  return renderTemplate(text, {
    kind: wi.kind,
    number: wi.gh_number,
    owner: project.gh_owner,
    repo: project.gh_repo,
    branch: wi.branch ?? "",
    protected: project.protected_branches,
    title: wi.title ?? "",
    base: project.base_branch,
    tracker: project.tracker,
    forge: project.forge,
  });
}

export interface PromptInput {
  input(env: string, session: string, data: string): Promise<void>;
}

export interface SendGuard {
  /** A sessão está viva (o menu fica desligado na encerrada). */
  live: boolean;
  /** Há um pedido de permissão pendente nesta sessão. */
  permissionPending: boolean;
}

/** Por que não dá para mandar agora; `null` = pode. */
export function promptBlocked({ live, permissionPending }: SendGuard): string | null {
  if (!live) return "A sessão está encerrada";
  if (permissionPending) return PERMISSION_PENDING;
  return null;
}

const sleep = (ms: number) => new Promise<void>((r) => setTimeout(r, ms));

/**
 * Manda o prompt por `session.input`: a colagem (`ESC[200~` texto `ESC[201~`)
 * e, depois de `ENTER_DELAY_MS`, o `\r`. Recusa (sem enviar nada) com a
 * sessão encerrada ou com um pedido de permissão pendente.
 */
export async function sendReadyPrompt(
  api: PromptInput,
  env: string,
  session: string,
  text: string,
  guard: SendGuard,
  wait: (ms: number) => Promise<void> = sleep,
): Promise<void> {
  const blocked = promptBlocked(guard);
  if (blocked) throw new Error(blocked);
  await api.input(env, session, pasteSequence(text));
  await wait(ENTER_DELAY_MS);
  await api.input(env, session, "\r");
}

/** O prompt pelo id (as ações da paleta levam só o id). */
export function findPrompt(list: ReadyPrompt[], id: string): ReadyPrompt | undefined {
  return list.find((p) => p.id === id);
}
