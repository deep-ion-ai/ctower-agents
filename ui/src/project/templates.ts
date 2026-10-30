// Prompt inicial a partir do template do tipo (spec §7.1) e do rastreador
// do projeto (spec do Jira e GitLab §9, #351).
import type { ForgeConfig, GitlabConfig, JiraConfig, TrackerConfig, WorkItemKind } from "../protocol/gen";
import type { Templates } from "../api/types";
import type { TemplateKey } from "../settings/sections";

export interface TemplateVars {
  kind: WorkItemKind;
  number: number;
  owner: string;
  repo: string;
  branch: string;
  protected: string[];
  /** Título da issue no espelho; vazio na sessão do projeto. */
  title?: string;
  /** `base_branch` do projeto. */
  base?: string;
  /** Rastreador e forja do projeto; ausentes = GitHub. */
  tracker?: TrackerConfig;
  forge?: ForgeConfig;
}

type Jira = Extract<TrackerConfig, { kind: "jira" }>;
type Gitlab = Extract<ForgeConfig, { kind: "gitlab" }>;

/** Palavra do shell com aspas simples só quando precisa (espelho de `ct_core`). */
function shellWord(s: string): string {
  return /^[A-Za-z0-9/._\-~+=:,@%]+$/.test(s) ? s : `'${s.replaceAll("'", "'\\''")}'`;
}

/**
 * Prefixo do jira-cli nas sessões (`{{jira}}`, espelho de
 * `ct_core::jira_cli_prefix`): `JIRA_API_TOKEN="$(<comando>)" jira`, com
 * `JIRA_CONFIG_FILE` antes quando configurado. Leva o comando de credencial
 * do cadastro, nunca a senha.
 */
export function jiraCliPrefix(j: Pick<JiraConfig, "credential_cmd" | "config_file">): string {
  const parts: string[] = [];
  if (j.config_file.trim()) parts.push(`JIRA_CONFIG_FILE=${shellWord(j.config_file.trim())}`);
  if (j.credential_cmd.trim()) parts.push(`JIRA_API_TOKEN="$(${j.credential_cmd.trim()})"`);
  parts.push("jira");
  return parts.join(" ");
}

/** Prefixo do glab (`{{glab}}`, espelho de `ct_core::glab_cli_prefix`): `GITLAB_HOST=<host> GITLAB_TOKEN="$(<comando>)" glab`. */
export function glabCliPrefix(g: Pick<GitlabConfig, "host" | "credential_cmd">): string {
  const parts: string[] = [];
  if (g.host.trim()) parts.push(`GITLAB_HOST=${shellWord(g.host.trim())}`);
  if (g.credential_cmd.trim()) parts.push(`GITLAB_TOKEN="$(${g.credential_cmd.trim()})"`);
  parts.push("glab");
  return parts.join(" ");
}

/**
 * Troca `{{var}}` pelos valores; variável desconhecida fica como está. Além
 * das de sempre, `{{key}}` (`#12` ou `PROJ-5450`), `{{title}}`, `{{base}}`,
 * `{{jira_url}}`, `{{jira_project}}`, `{{jira}}`, `{{glab}}` e
 * `{{forge_path}}` (spec do Jira e GitLab §9.2).
 */
export function renderTemplate(template: string, v: TemplateVars): string {
  const jira = v.tracker?.kind === "jira" ? (v.tracker as Jira) : null;
  const gitlab = v.forge?.kind === "gitlab" ? (v.forge as Gitlab) : null;
  const values: Record<string, string> = {
    kind: v.kind === "epic" ? "épico" : "issue",
    number: String(v.number),
    owner: v.owner,
    repo: v.repo,
    branch: v.branch,
    protected: v.protected.join(", "),
    key: jira ? `${jira.project_key}-${v.number}` : `#${v.number}`,
    title: v.title ?? "",
    base: v.base ?? "",
    jira_url: jira ? jira.url.replace(/\/+$/, "") : "",
    jira_project: jira?.project_key ?? "",
    jira: jira ? jiraCliPrefix(jira) : "jira",
    glab: gitlab ? glabCliPrefix(gitlab) : "glab",
    forge_path: gitlab?.path.replace(/^\/+|\/+$/g, "") || `${v.owner}/${v.repo}`,
  };
  return template.replace(/\{\{\s*(\w+)\s*\}\}/g, (all, name: string) => values[name] ?? all);
}

/**
 * Template do tipo de sessão pelo rastreador do projeto (spec do Jira e
 * GitLab §9.1): os `jira_*` nos projetos Jira; os de hoje no GitHub.
 */
export function templateFor(t: Templates, kind: WorkItemKind | "project", tracker: TrackerConfig["kind"] = "github"): string {
  return t[templateKeyFor(kind, tracker)];
}

/** Chave do template de `templateFor` (o "Editar template" da modal de início abre a aba dela). */
export function templateKeyFor(kind: WorkItemKind | "project", tracker: TrackerConfig["kind"] = "github"): TemplateKey {
  if (tracker === "jira") return kind === "epic" ? "jira_epic" : kind === "project" ? "jira_project" : "jira_issue";
  return kind === "epic" ? "epic" : kind === "project" ? "project" : "issue";
}

/** Nota acrescentada quando a sessão interrompida recomeça do zero (sem o transcript, ou por escolha). */
export function contextNote(sessionId: string, lost = true): string {
  return [
    "",
    lost
      ? `Contexto: a sessão anterior (${sessionId}) foi interrompida e o transcript se perdeu.`
      : `Contexto: a sessão anterior (${sessionId}) foi interrompida e esta recomeça sem a conversa dela.`,
    "Antes de continuar, revise o estado do worktree, os commits da branch e as decisões já registradas.",
  ].join("\n");
}

/** Escopo do gestor (espelha `GestorScope` do daemon). */
export type GestorScopeChoice = { kind: "portfolio" } | { kind: "project"; owner: string; repo: string } | { kind: "health" };

/** Linha de foco que entra em `{{scope}}` (spec do gestor §10.1). */
export function scopeLine(s: GestorScopeChoice): string {
  switch (s.kind) {
    case "portfolio":
      return "Foco: o portfólio inteiro. Comece por portfolio_overview e portfolio_ranking.";
    case "project":
      return `Foco: ${s.owner}/${s.repo}. Comece por portfolio_project e portfolio_ranking com owner e repo.`;
    case "health":
      return "Foco: saúde e riscos. Comece por portfolio_health e aprofunde com portfolio_work_item.";
  }
}

/** Escopo legível nos rótulos das sessões do gestor: "Portfólio", "owner/repo", "Saúde e riscos". */
export function scopeText(s: GestorScopeChoice): string {
  switch (s.kind) {
    case "portfolio":
      return "Portfólio";
    case "project":
      return `${s.owner}/${s.repo}`;
    case "health":
      return "Saúde e riscos";
  }
}

/** Prompt inicial do gestor: `{{env}}`, `{{scope}}`, `{{owner}}`, `{{repo}}`. */
export function renderGestorTemplate(template: string, env: string, s: GestorScopeChoice): string {
  const values: Record<string, string> = {
    env,
    scope: scopeLine(s),
    owner: s.kind === "project" ? s.owner : "",
    repo: s.kind === "project" ? s.repo : "",
  };
  return template.replace(/\{\{\s*(\w+)\s*\}\}/g, (all, name: string) => values[name] ?? all);
}
