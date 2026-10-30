// Abas do projeto aberto (#17): Visão geral, Itens, Sessões e
// Configurações, e as seções das Configurações. Cada aba e seção tem
// endereço próprio (`Screen` project com `view` e `section`), que a paleta,
// os deep links da PWA e os botões das telas montam por `projectAt`.
// Funções puras, testadas em sections.test.ts.
import type { Screen } from "../state/nav";
import type { RegisterIntent } from "./NewProjectDialog";

export type ProjectAddr = Extract<Screen, { name: "project" }>;

/** Aba do projeto. Sem `view` no endereço, Itens (os endereços de antes da #17). */
export type ProjectPage = "overview" | "items" | "sessions" | "settings";

/**
 * Seção das Configurações do projeto. Cada uma compõe um cartão de
 * `project/settings` (épico #12): Geral = `RepositoryCard`; Rastreador e
 * sync = `SyncEnvCard` (Jira) ou o sync do hub (GitHub); Ambientes =
 * `AssignmentsCard`; Regras = `AgentRules`; Secrets = `ProjectSecretsCard`;
 * Docs = `DocsForm`; Zona de perigo = `DangerZone`.
 */
export type ProjectSettingsSection = "general" | "tracker" | "envs" | "rules" | "secrets" | "docs" | "danger";

/** Na ordem da faixa de abas. */
export const PROJECT_PAGES: ProjectPage[] = ["overview", "items", "sessions", "settings"];

export const PROJECT_PAGE_LABEL: Record<ProjectPage, string> = {
  overview: "Visão geral",
  items: "Itens",
  sessions: "Sessões",
  settings: "Configurações",
};

/** Na ordem do índice das Configurações. */
export const PROJECT_SECTIONS: ProjectSettingsSection[] = ["general", "tracker", "envs", "rules", "secrets", "docs", "danger"];

export const PROJECT_SECTION_LABEL: Record<ProjectSettingsSection, string> = {
  general: "Geral",
  tracker: "Repositório e rastreador",
  envs: "Ambientes e atribuições",
  rules: "Regras do agente",
  secrets: "Secrets e variáveis",
  docs: "Docs",
  danger: "Zona de perigo",
};

/**
 * Seções que a web (só leitura, épico #205) não mostra: os secrets ficam
 * fora do /web/ws (`hub.secrets.list` devolve valores de variáveis) e
 * excluir é do desktop. O mesmo corte que o Hub › Projetos faz.
 */
export const DESKTOP_ONLY_SECTIONS: ReadonlySet<ProjectSettingsSection> = new Set(["secrets", "danger"]);

export const isPage = (v: unknown): v is ProjectPage => typeof v === "string" && (PROJECT_PAGES as string[]).includes(v);
export const isSection = (v: unknown): v is ProjectSettingsSection =>
  typeof v === "string" && (PROJECT_SECTIONS as string[]).includes(v);

/** Aba mostrada pelo endereço: com item (aba do item), sempre Itens. */
export const pageOf = (s: ProjectAddr): ProjectPage => (s.item ? "items" : (s.view ?? "items"));

/**
 * Seção das Configurações mostrada: a pedida, se existe nesta visão (na web
 * as do desktop caem em Geral), senão Geral.
 */
export function sectionOf(s: ProjectAddr, web = false): ProjectSettingsSection {
  const want = s.section;
  if (!want || (web && DESKTOP_ONLY_SECTIONS.has(want))) return "general";
  return want;
}

/**
 * Endereço do projeto `base` na aba `page` (e na `section`, nas
 * Configurações). Sem item; Itens sai sem `view`, para o endereço seguir
 * igual ao de antes da #17 (abas, recentes e testes comparam o objeto).
 */
export function projectAt(base: ProjectAddr, page: ProjectPage = "items", section?: ProjectSettingsSection): ProjectAddr {
  // Os pedidos (`assign`, `register`) também saem: valem uma vez, no endereço que os fez.
  const {
    item: _item,
    view: _view,
    section: _section,
    assign: _assign,
    register: _register,
    notice: _notice,
    ...addr
  } = base;
  return {
    ...addr,
    ...(page !== "items" ? { view: page } : {}),
    ...(page === "settings" && section ? { section } : {}),
  };
}

/** Sufixo do título da aba do projeto fora de Itens: "ctower-agents · Sessões". */
export function pageSuffix(s: ProjectAddr): string {
  const page = pageOf(s);
  if (page === "items") return "";
  if (page === "settings" && s.section) return ` · ${PROJECT_PAGE_LABEL.settings} › ${PROJECT_SECTION_LABEL[s.section]}`;
  return ` · ${PROJECT_PAGE_LABEL[page]}`;
}

/**
 * O lugar novo do antigo Hub › Projetos e atribuições (#18): com projeto,
 * as Configurações dele (com `assign`, a seção Ambientes e atribuições com
 * o formulário de atribuição aberto); sem projeto, o índice de Projetos
 * (com `register`, o cadastro aberto). Uma tela só para configurar cada
 * projeto: `hubTab("projects", ...)` e os endereços antigos passam por aqui.
 */
export function projectsPlace({
  project,
  assign,
  register,
}: {
  project?: { owner: string; repo: string };
  assign?: true;
  register?: RegisterIntent;
}): ProjectAddr {
  if (!project) return { name: "project", ...(register ? { register } : {}) };
  const base: ProjectAddr = { name: "project", owner: project.owner, repo: project.repo };
  return assign ? { ...projectAt(base, "settings", "envs"), assign } : projectAt(base, "settings");
}
