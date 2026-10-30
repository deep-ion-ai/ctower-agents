// Referências de work item e de mudança (PR/MR) neutras de provedor (spec do
// Jira e GitLab §3.3 e §11, issue #352). Espelho de `ct_core::tracker`
// (`item_ref`, `item_short`, `item_key`, `item_url`, `change_label`,
// `change_short`, `change_ref`, `change_url`): mesmo texto nos dois lados.
//
// O projeto é opcional em tudo: sem ele (tela que não achou o projeto, hub
// anterior à #347) vale o GitHub, como antes da #352.
import type { ForgeConfig, TrackerConfig } from "../protocol/gen";

// Rótulo de um work item. O #0 é o das sessões do projeto (sem issue).
export const PROJECT_ITEM = 0;

/**
 * O que as referências precisam do projeto: `owner`/`repo` da forja (no
 * GitLab, `owner` é o caminho sem o último segmento), rastreador e forja.
 * `Project` (ambiente) e `HubProject` (hub) servem os dois.
 */
export interface ProviderProject {
  gh_owner: string;
  gh_repo: string;
  /** Ausente em hub/ambiente anterior à #347: GitHub. */
  tracker?: TrackerConfig;
  forge?: ForgeConfig;
}

type MaybeProject = ProviderProject | null | undefined;
type Jira = Extract<TrackerConfig, { kind: "jira" }>;
type Gitlab = Extract<ForgeConfig, { kind: "gitlab" }>;

function jiraOf(p: MaybeProject): Jira | null {
  return p?.tracker?.kind === "jira" ? (p.tracker as Jira) : null;
}

function gitlabOf(p: MaybeProject): Gitlab | null {
  return p?.forge?.kind === "gitlab" ? (p.forge as Gitlab) : null;
}

/** O projeto tem as issues no Jira. */
export const isJiraProject = (p: MaybeProject) => jiraOf(p) !== null;
/** O projeto tem o repositório no GitLab. */
export const isGitlabProject = (p: MaybeProject) => gitlabOf(p) !== null;

const jiraKey = (j: Jira, n: number) => `${j.project_key}-${n}`;

/** Referência completa: `owner/repo#12` no GitHub, `PROJ-5450` no Jira (`item_ref`). */
export function itemFullRef(p: MaybeProject, n: number): string {
  const j = jiraOf(p);
  if (j) return jiraKey(j, n);
  return p ? `${p.gh_owner}/${p.gh_repo}#${n}` : `#${n}`;
}

/** Referência curta, dentro do projeto: `#12` ou `PROJ-5450` (`item_short`). */
export function itemShort(p: MaybeProject, n: number): string {
  const j = jiraOf(p);
  return j ? jiraKey(j, n) : `#${n}`;
}

/** O `{key}` do padrão de branch: `12` ou `PROJ-5450` (`item_key`). */
export function itemKey(p: MaybeProject, n: number): string {
  const j = jiraOf(p);
  return j ? jiraKey(j, n) : String(n);
}

/** Link da issue: `https://github.com/owner/repo/issues/12` ou `<jira>/browse/PROJ-5450` (`item_url`). */
export function itemUrl(p: ProviderProject, n: number): string {
  const j = jiraOf(p);
  if (j) return `${j.url.replace(/\/+$/, "")}/browse/${jiraKey(j, n)}`;
  return `https://github.com/${p.gh_owner}/${p.gh_repo}/issues/${n}`;
}

/** Nome do rastreador nas telas: `GitHub` ou `Jira`. */
export const trackerLabel = (p: MaybeProject) => (jiraOf(p) ? "Jira" : "GitHub");

/**
 * Rastreadores de um conjunto de projetos, para o que vale para todos (o
 * sync): `GitHub`, `Jira` ou `GitHub e Jira`; sem projeto, `GitHub`.
 */
export function trackersText(projects: readonly MaybeProject[]): string {
  const kinds = new Set(projects.filter(Boolean).map(trackerLabel));
  const list = (["GitHub", "Jira"] as const).filter((k) => kinds.has(k));
  return list.length ? list.join(" e ") : "GitHub";
}

/** Nome da forja nas telas: `GitHub` ou `GitLab` (`ForgeKind::label`). */
export const forgeLabel = (p: MaybeProject) => (gitlabOf(p) ? "GitLab" : "GitHub");

/** Nome da mudança na forja: `PR` ou `MR` (`change_label`). */
export const changeLabel = (p: MaybeProject) => (gitlabOf(p) ? "MR" : "PR");

/**
 * Nome da mudança de um conjunto de projetos (grupos que misturam
 * projetos): `PR`, `MR` ou `PR/MR`; sem projeto, `PR`.
 */
export function changeLabels(projects: readonly MaybeProject[]): string {
  const labels = new Set(projects.map(changeLabel));
  const list = (["PR", "MR"] as const).filter((k) => labels.has(k));
  return list.length ? list.join("/") : "PR";
}

/** Referência curta da mudança: `#30` ou `!30`, o `iid` (`change_short`). */
export const changeShort = (p: MaybeProject, n: number) => (gitlabOf(p) ? `!${n}` : `#${n}`);

/** `PR #30` ou `MR !30` (`change_ref`). */
export const changeRef = (p: MaybeProject, n: number) => `${changeLabel(p)} ${changeShort(p, n)}`;

/** Caminho do projeto no GitLab; o do cadastro ou `owner/repo`. */
function gitlabPath(p: ProviderProject, g: Gitlab): string {
  const path = g.path.replace(/^\/+|\/+$/g, "");
  return path || `${p.gh_owner}/${p.gh_repo}`;
}

/**
 * Link da mudança quando a forja não deu um (o espelho guarda o `html_url`
 * do PR ou o `web_url` do MR): `https://github.com/<owner>/<repo>/pull/<n>`
 * ou `https://<host>/<path>/-/merge_requests/<iid>` (`change_url`).
 */
export function changeUrl(p: ProviderProject, n: number): string {
  const g = gitlabOf(p);
  if (g) return `https://${g.host}/${gitlabPath(p, g)}/-/merge_requests/${n}`;
  return `https://github.com/${p.gh_owner}/${p.gh_repo}/pull/${n}`;
}

/**
 * Link de criação da mudança da branch para a base: o compare do GitHub, ou
 * o `merge_requests/new` do GitLab (spec §11).
 */
export function changeNewUrl(p: ProviderProject, branch: string, base: string): string {
  const g = gitlabOf(p);
  if (g) {
    const q = `merge_request[source_branch]=${encodeURIComponent(branch)}&merge_request[target_branch]=${encodeURIComponent(base)}`;
    return `https://${g.host}/${gitlabPath(p, g)}/-/merge_requests/new?${q}`;
  }
  return `https://github.com/${p.gh_owner}/${p.gh_repo}/compare/${base}...${encodeURIComponent(branch)}?expand=1`;
}

/** Página do repositório na forja: `https://github.com/owner/repo` ou `https://<host>/<path>`. */
export function repoUrl(p: ProviderProject): string {
  const g = gitlabOf(p);
  return g ? `https://${g.host}/${gitlabPath(p, g)}` : `https://github.com/${p.gh_owner}/${p.gh_repo}`;
}

/** "#31", "PROJ-31" ou "sessão do projeto". */
export function itemRef(n: number, p?: MaybeProject): string {
  return n === PROJECT_ITEM ? "sessão do projeto" : itemShort(p, n);
}

/** "#31 Título", "PROJ-31 Título" ou "Sessão do projeto". */
export function itemTitle(wi: { gh_number: number; title?: string | null }, p?: MaybeProject): string {
  return wi.gh_number === PROJECT_ITEM ? "Sessão do projeto" : `${itemShort(p, wi.gh_number)} ${wi.title ?? ""}`.trim();
}

/**
 * Número de uma referência digitada na busca: `43`, `#43` ou, no Jira,
 * `PROJ-43` (o prefixo de outro projeto não casa). `null` se não é número.
 */
export function numberQuery(q: string, p?: MaybeProject): string | null {
  let s = q.trim().toLowerCase().replace(/^#/, "");
  const j = jiraOf(p);
  if (j) {
    const prefix = `${j.project_key.toLowerCase()}-`;
    if (s.startsWith(prefix)) s = s.slice(prefix.length);
  }
  return /^\d+$/.test(s) ? s : null;
}
