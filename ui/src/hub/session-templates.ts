// Templates de sessão do projeto em Configurações › Templates de sessão
// (#166): o estado de cada tipo (herdando o global ou com override) e o
// `session_templates` do `hub.projects.update`. Funções puras, testadas em
// session-templates.test.ts.
import type { HubProject, SessionTemplates } from "../protocol/hub-gen";
import type { Templates } from "../api/types";
import { globalTemplate, type SessionTemplateKind } from "../project/templates";

/** Protocolo do hub com `session_templates` (#166). */
export const SESSION_TEMPLATES_PROTOCOL = "1.13";

export const SESSION_TEMPLATE_LABEL: Record<SessionTemplateKind, string> = {
  issue: "Issue e sub-issue",
  epic: "Épico",
  project: "Sessão do projeto (sem issue)",
  release: "Sessão de release",
};

/** Overrides do projeto; `null` = hub anterior à #166 (não edita templates). */
export function sessionTemplatesOf(p: HubProject): SessionTemplates | null {
  return (p as Partial<HubProject>).session_templates ?? null;
}

/** O tipo tem override no projeto (chave não vazia). */
export const hasOverride = (t: SessionTemplates, k: SessionTemplateKind): boolean => t[k].trim() !== "";

/** Ponto de partida do "Personalizar": o template global do tipo no rastreador do projeto. */
export function customizeStart(templates: Templates, p: HubProject, k: SessionTemplateKind): string {
  return globalTemplate(templates, k, p.tracker?.kind ?? "github").text;
}

/**
 * `session_templates` do `hub.projects.update` com a chave `k` trocada (o
 * objeto inteiro: o hub substitui). Vazio = "Voltar ao global". `null` se
 * nada mudou.
 */
export function templatesPatch(cur: SessionTemplates, k: SessionTemplateKind, text: string): SessionTemplates | null {
  const next = text.trim() ? text : "";
  if (next === cur[k]) return null;
  return { ...cur, [k]: next };
}
