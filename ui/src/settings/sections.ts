// Configurações como editor de settings (#315): escopos, seções do índice,
// endereço das abas (template em aba própria) e a busca. Funções puras,
// testadas em sections.test.ts.
import type { Templates } from "../api/types";
import type { Place } from "../shell/places";
import type { OpenTab, Screen } from "../state/nav";

/** "Este app" (ui.json, nesta máquina) ou o daemon de um ambiente. */
export type SettingsScope = "app" | "env";

export type SettingsSection =
  // Este app
  | "connections"
  | "sessions"
  | "appearance"
  | "terminal"
  | "notifications"
  /** Só na web (#201): a sessão web deste navegador e o botão de sair. */
  | "web-access"
  // Ambiente
  | "github"
  | "daemon-update"
  | "local-projects"
  | "env-notifications";

export type TemplateKey = keyof Templates;

export const SECTION_LABEL: Record<SettingsSection, string> = {
  connections: "Conexões",
  sessions: "Sessões",
  appearance: "Aparência",
  terminal: "Terminal",
  notifications: "Notificações",
  "web-access": "Acesso web",
  github: "GitHub",
  "daemon-update": "Atualização do daemon",
  "local-projects": "Projetos locais",
  "env-notifications": "Notificações",
};

/** Seções de cada escopo, na ordem do índice. */
export const SCOPE_SECTIONS: Record<SettingsScope, SettingsSection[]> = {
  // As atualizações do app ficam em Manutenção › Atualizações › Este app (#336).
  app: ["connections", "sessions", "appearance", "terminal", "notifications", "web-access"],
  // A Limpeza fica só em Manutenção › Limpeza (#22): aqui, um atalho em Projetos locais.
  env: ["github", "daemon-update", "local-projects", "env-notifications"],
};

export const scopeOf = (s: SettingsSection): SettingsScope => (SCOPE_SECTIONS.app.includes(s) ? "app" : "env");

export const TEMPLATE_LABEL: Record<TemplateKey, string> = {
  issue: "Issue avulsa e sub-issue",
  epic: "Épico",
  project: "Sessão do projeto (sem issue)",
  gestor: "Gestor de projetos",
  jira_issue: "Issue do Jira (e sub-tarefa)",
  jira_epic: "Épico do Jira",
  jira_project: "Sessão do projeto Jira (sem issue)",
};

export const TEMPLATE_KEYS = Object.keys(TEMPLATE_LABEL) as TemplateKey[];

export type SettingsScreen = Extract<Screen, { name: "settings" }>;

/** Aba própria do editor de um template. */
export function templateTab(k: TemplateKey): OpenTab {
  return { id: `settings:template:${k}`, title: `Template · ${TEMPLATE_LABEL[k]}`, screen: { name: "settings", template: k } };
}

/** A aba das Configurações (id `settings`) num escopo/seção. */
export function settingsTab(to: { scope?: SettingsScope; section?: SettingsSection; env?: string } = {}): OpenTab {
  const scope = to.scope ?? (to.section ? scopeOf(to.section) : undefined);
  return { id: "settings", screen: { name: "settings", ...to, scope } };
}

/** Outros nomes pelos quais a paleta acha a seção. */
const KEYWORDS: Record<SettingsSection, string> = {
  connections: "hub url token ambientes daemon ws distro wsl",
  sessions: "modelo modo padrão templates prompt",
  appearance: "tema claro escuro sistema cores dark light",
  terminal: "fonte tamanho zoom console",
  notifications: "avisos aguardando erro desconectado",
  "web-access": "sair logout sessão web navegador cookie revogar",
  github: "token gh sync sincronização intervalo",
  "daemon-update": "update_ref canal intervalo update_check_interval_hours versão atualizar rollback",
  "local-projects": "clones worktrees repos_root",
  "env-notifications": "silenciar avisos ambiente",
};

/** Escopos e seções das Configurações na paleta (`PLACES`). */
export const SETTINGS_PLACES: Place[] = [
  {
    id: "settings:scope:app",
    label: "Configurações › Este app",
    keywords: "preferências app ui.json",
    open: { tab: settingsTab({ scope: "app" }) },
  },
  {
    id: "settings:scope:env",
    label: "Configurações › Ambiente",
    keywords: "daemon ctowerd config.toml",
    open: { tab: settingsTab({ scope: "env" }) },
  },
  ...(Object.keys(SECTION_LABEL) as SettingsSection[]).map((s) => ({
    id: `settings:${s}`,
    label: `Configurações › ${scopeOf(s) === "app" ? "Este app" : "Ambiente"} › ${SECTION_LABEL[s]}`,
    keywords: KEYWORDS[s],
    open: { tab: settingsTab({ section: s }) },
  })),
];

/** O que a busca olha de uma configuração. */
export interface Searchable {
  /** "Início de sessão" em "Início de sessão: **Modelo padrão**". */
  category: string;
  name: string;
  description: string;
  /** Chaves do `config.toml` (ambiente) ou do `ui.json` (app), ex. `update_ref`. */
  keys?: string[];
  /** Outros termos que acham a configuração. */
  keywords?: string;
}

const fold = (s: string) =>
  s
    .toLowerCase()
    .normalize("NFD")
    .replace(/[̀-ͯ]/g, "");

/**
 * Busca: cada palavra (sem acento nem caixa) precisa aparecer no título,
 * na descrição, numa chave ou nos termos extras. Vazia = tudo.
 */
export function matchesSetting(s: Searchable, query: string): boolean {
  const words = fold(query).split(/\s+/).filter(Boolean);
  if (words.length === 0) return true;
  const hay = fold([s.category, s.name, s.description, ...(s.keys ?? []), s.keywords ?? ""].join(" "));
  return words.every((w) => hay.includes(w));
}
