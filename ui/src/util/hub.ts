// Textos do hub (cthubd) e da configuração que cada ambiente puxa dele
// (spec do hub como plano de controle §3.3, §4.3, §7).
import type { EnvInfo } from "../protocol/gen";
import type { AssignmentState, HubAssignment, HubProject } from "../protocol/hub-gen";
import { ago } from "./time";

/** Configuração do hub no ambiente (`env.info.hub_config`) em uma linha; `warn` = pede atenção. */
export function configLine(info: EnvInfo | undefined, now: number): { text: string; warn: boolean } | null {
  const c = info?.hub_config;
  if (!c) return null;
  if (!c.linked) return { text: "ambiente não ligado a um hub", warn: true };
  if (c.legacy_hub) return { text: "hub antigo: sem configuração (projetos locais)", warn: true };
  if (c.config_rev === null || c.pulled_at === null) {
    return c.last_error
      ? { text: `configuração do hub não recebida: ${c.last_error}`, warn: true }
      : { text: "configuração do hub ainda não recebida", warn: false };
  }
  let text = `configuração rev ${c.config_rev} · ${ago(c.pulled_at, now)}`;
  if (c.last_error) text += " · pull falhou";
  // Passou de `secrets_max_age_hours` sem sincronizar (#166): as sessões
  // novas sobem com os secrets do cache, com aviso.
  const stale = c.max_age_ms !== null && now - c.pulled_at > c.max_age_ms;
  if (stale) text += " · desatualizada: sessões novas usam os secrets do cache";
  return { text, warn: !!c.last_error || stale };
}

export const ASSIGNMENT_TEXT: Record<AssignmentState, string> = {
  pending: "pendente",
  applied: "aplicada",
  error: "com erro",
};

/**
 * Estado da atribuição escrito. A mensagem do relato só vale no estado
 * `error`: com `pending`, `error` pode ser de uma revisão anterior.
 */
export function assignmentText(a: Pick<HubAssignment, "state" | "error">): string {
  return a.state === "error" && a.error ? `com erro: ${a.error}` : ASSIGNMENT_TEXT[a.state];
}

/** Caminho pedido pelo hub (vazio = padrão do ambiente). */
export function repoPathText(a: Pick<HubAssignment, "repo_path" | "gh_repo">): string {
  return a.repo_path || `padrão (<repos_root>/${a.gh_repo})`;
}

/**
 * Projeto `owner/repo` da lista do hub (`hub.projects` ou o dashboard) (sem diferenciar maiúsculas), para
 * o provedor das referências (#352) nas telas que só têm `owner`/`repo`.
 */
export function hubProjectOf(
  projects: readonly { project: HubProject }[] | null | undefined,
  owner: string | null | undefined,
  repo: string | null | undefined,
): HubProject | undefined {
  if (!owner || !repo) return undefined;
  const o = owner.toLowerCase();
  const r = repo.toLowerCase();
  return projects?.find((p) => p.project.gh_owner.toLowerCase() === o && p.project.gh_repo.toLowerCase() === r)
    ?.project;
}

/**
 * `owner/repo` dividido na última barra, como o `rsplit_once('/')` do hub:
 * no GitLab, o `owner` é o caminho com barras (`grupo/projeto` + `app`).
 */
export function splitProjectKey(key: string): { owner: string; repo: string } {
  const i = key.lastIndexOf("/");
  return { owner: key.slice(0, i), repo: key.slice(i + 1) };
}
