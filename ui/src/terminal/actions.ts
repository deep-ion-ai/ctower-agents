// Ações do console por sessão (#424; spec da #87, §2.1–2.3 e §6): quais
// aparecem no cabeçalho do painel e no menu ⋯ conforme o tipo da sessão
// (issue, épico, sessão do projeto, gestor) e se ela está viva ou encerrada.
// Função pura; quem executa é o `PaneBar`. A paleta de comandos recebe as
// mesmas ações da sessão ativa por um store de módulo (`publishSessionActions`).
import { useSyncExternalStore } from "react";
import type { ReadyPrompt } from "../api/types";
import type { StartTarget } from "../shell/start";
import type { Tab } from "../state/nav";
import type { Review } from "../protocol/gen";
import {
  changeLabel,
  changeNewUrl,
  forgeLabel,
  itemUrl,
  PROJECT_ITEM,
  repoUrl,
  trackerLabel,
  type ProviderProject,
} from "../util/item";
import type { TabInfo } from "./SessionTree";

/** Tipo da sessão para as ações: issue, épico, sessão do projeto (`#0`) ou gestor. */
export type SessionKind = "issue" | "epic" | "project" | "gestor";

export type SessionActionId =
  | "resume"
  | "open-issue"
  | "open-repo"
  | "open-pr"
  | "review"
  | "open-item"
  | "vscode"
  | "restart"
  /** Mostrar/ocultar o painel de contexto (#426). */
  | "context"
  /** Enviar o prompt pronto de id `<id>` (#425). */
  | `prompt:${string}`;

export interface SessionAction {
  id: SessionActionId;
  label: string;
  /**
   * `bar` = botão no cabeçalho; `menu` = menu ⋯ (§2.1); `prompt` = menu
   * "Enviar prompt pronto" (§2.4).
   */
  place: "bar" | "menu" | "prompt";
  disabled?: boolean;
  /** Motivo de estar desabilitada. */
  title?: string;
}

export function sessionKind(info: TabInfo): SessionKind | null {
  if (info.gestor) return "gestor";
  if (!info.wi) return null;
  if (info.wi.gh_number === PROJECT_ITEM) return "project";
  return info.wi.kind === "epic" ? "epic" : "issue";
}

/** Pasta do "Abrir no VS Code": o worktree do item ou, no `#0`, o clone principal. */
export function vscodePath(info: TabInfo): string | null {
  if (info.gestor || !info.wi) return null;
  return info.wi.worktree_path ?? (info.wi.gh_number === PROJECT_ITEM ? (info.project?.repo_path ?? null) : null);
}

/**
 * Retomar: a sessão encerrada (o console viu o fim) com o id da ferramenta
 * para o `--resume`. A arquivada o daemon recusa.
 */
export function canResume(info: TabInfo, ended: boolean): boolean {
  return ended && !!info.s?.tool_session_id && info.s.state !== "archived";
}

/** Prompt do Retomar numa sessão que terminou normalmente (sem a palavra "interrompida"). */
export const RESUMED_PROMPT = "A sessão foi retomada. Verifique o estado do worktree e dos commits antes de continuar.";

/**
 * Prompt do Retomar: o da Recuperação (`RESUME_PROMPT`) só na interrompida
 * recuperável (o mesmo critério do "Abrir Recuperação"); nas que saíram
 * normalmente, o neutro.
 */
export function resumePrompt(recoverable: boolean, interrupted: string): string {
  return recoverable ? interrupted : RESUMED_PROMPT;
}

export interface ActionInput {
  info: TabInfo;
  /** O console viu o fim da sessão. */
  ended: boolean;
  /** O ambiente abre o VS Code (só o WSL, por ora). */
  vscode: boolean;
}

/**
 * As ações da sessão, na ordem do cabeçalho e do menu. O que não cabe no tipo
 * não aparece (no `#0`, sem PR nem item no projeto; no gestor, sem issue); o
 * que cabe, mas não dá agora (sem branch, sem worktree), fica desabilitado.
 */
export function sessionActions({ info, ended, vscode }: ActionInput): SessionAction[] {
  const kind = sessionKind(info);
  if (!kind) return [];
  const { wi, project } = info;
  const out: SessionAction[] = [];
  if (canResume(info, ended)) out.push({ id: "resume", label: "Retomar", place: "bar" });
  if (kind === "issue" || kind === "epic") {
    out.push({ id: "open-issue", label: `Abrir issue no ${trackerLabel(project)}`, place: "menu", disabled: !project });
    out.push({
      id: "open-pr",
      label: `Abrir ${changeLabel(project)}`,
      place: "menu",
      disabled: !wi || !project,
    });
    out.push({ id: "open-item", label: "Abrir item no projeto", place: "menu", disabled: !project });
  }
  if (kind === "project") {
    out.push({ id: "open-repo", label: `Abrir repositório no ${forgeLabel(project)}`, place: "menu", disabled: !project });
  }
  if (kind === "gestor") {
    out.push({ id: "review", label: "Revisar decisões do gestor", place: "menu", disabled: !info.s });
  } else {
    const path = vscodePath(info);
    out.push({
      id: "vscode",
      label: "Abrir no VS Code",
      place: "menu",
      disabled: !path || !vscode,
      title: !path ? "O item ainda não tem worktree" : !vscode ? "Só nos ambientes WSL" : undefined,
    });
  }
  out.push({
    id: "restart",
    label: "Reiniciar",
    place: "menu",
    disabled: !info.s || (kind !== "gestor" && !project),
  });
  return out;
}

/**
 * "Enviar prompt — <nome>" de cada prompt pronto (§2.4), na ordem das
 * Configurações. `blocked` (sessão encerrada, permissão pendente) desliga
 * todos com o motivo; a paleta só lista os habilitados.
 */
export function promptActions(prompts: ReadyPrompt[], blocked: string | null): SessionAction[] {
  return prompts.map((p) => ({
    id: `prompt:${p.id}`,
    label: `Enviar prompt — ${p.name}`,
    place: "prompt",
    disabled: !!blocked,
    title: blocked ?? undefined,
  }));
}

/** Id do prompt numa ação `prompt:<id>`; `null` nas demais. */
export function promptIdOf(id: SessionActionId): string | null {
  return id.startsWith("prompt:") ? id.slice("prompt:".length) : null;
}

/** Link de "Abrir issue" (`/issues/{n}`, ou o do Jira) e de "Abrir repositório" (§2.3). */
export function actionUrl(id: "open-issue" | "open-repo", info: TabInfo): string | null {
  const { wi, project } = info;
  if (!project) return null;
  if (id === "open-repo") return repoUrl(project);
  return wi ? itemUrl(project, wi.gh_number) : null;
}

/**
 * URL do PR (ou MR) do work item, ou a de criação se ainda não houver: o
 * compare do GitHub ou o `merge_requests/new` do GitLab (#352).
 */
export function prUrl(review: Review, project: ProviderProject, base: string): string | null {
  const open = review.prs.find((p) => p.state === "open") ?? review.prs[review.prs.length - 1];
  if (open) return open.url;
  const branch = review.work_item.branch;
  if (!branch) return null;
  return changeNewUrl(project, branch, base);
}

/** "Sessão: Mostrar contexto" / "Ocultar contexto" (#426): painel ao lado do console. */
export function contextAction(open: boolean): SessionAction {
  return { id: "context", label: open ? "Ocultar contexto" : "Mostrar contexto", place: "bar" };
}

/**
 * Alvo da modal de início no Reiniciar (§2.2): o mesmo work item (sem número
 * no `#0`, que vira `session.start_project`), o modelo e o modo da sessão
 * anterior; `kill` com a sessão viva. A sessão nova ocupa a aba de `tab`.
 */
export function restartTarget(tab: Tab, info: TabInfo, live: boolean): StartTarget | null {
  const { s, wi, project } = info;
  if (!s) return null;
  const base = { env: tab.env, replace: tab, kill: live, model: s.model, mode: s.mode };
  if (info.gestor) return { ...base, gestor: info.scope ?? { kind: "portfolio" } };
  if (!wi || !project) return null;
  return {
    ...base,
    projectId: project.id,
    ...(wi.gh_number === PROJECT_ITEM ? {} : { ghNumber: wi.gh_number }),
  };
}

/** Ações da sessão ativa para a paleta: o título dela e quem executa. */
export interface PublishedActions {
  /** Título da sessão (linha de apoio na paleta). */
  title: string;
  actions: SessionAction[];
  run(id: SessionActionId): void;
}

let published: PublishedActions | null = null;
const listeners = new Set<() => void>();

/** O cabeçalho da sessão ativa publica as ações dela. */
export function publishSessionActions(p: PublishedActions): void {
  published = p;
  for (const l of listeners) l();
}

/** Ao desmontar: só limpa se ninguém publicou outra no lugar. */
export function unpublishSessionActions(p: PublishedActions): void {
  if (published !== p) return;
  published = null;
  for (const l of listeners) l();
}

const subscribe = (l: () => void) => {
  listeners.add(l);
  return () => listeners.delete(l);
};

export function usePublishedActions(): PublishedActions | null {
  return useSyncExternalStore(subscribe, () => published);
}
