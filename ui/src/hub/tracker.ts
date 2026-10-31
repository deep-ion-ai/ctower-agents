// Projetos Jira + GitLab no Hub (#348, spec do Jira e GitLab §5, §8.4 e §11):
// formulário, prévia do padrão de branch e o que a prontidão do ambiente de
// sync pede para configurar. Funções puras, testadas em tracker.test.ts.
import type {
  GitlabConfig,
  HubProject,
  HubProjectView,
  JiraConfig,
  JiraFieldInfo,
  JiraTypeRole,
  TrackerReadiness,
} from "../protocol/hub-gen";

/** Tipo do projeto no cadastro: GitHub, ou issues no Jira e repositório no GitLab. */
export type ProjectKindForm = "github" | "jira";

/** Repositório do código-fonte, escolhido no cadastro. */
export type CodeHostForm = "github" | "gitlab";
/** Repositório de demandas (issues), escolhido no cadastro. */
export type IssueHostForm = "github" | "jira";

/** Os dois campos do cadastro a partir do tipo (edição, quarentena do GitLab). */
export const hostsOf = (k: ProjectKindForm): { code: CodeHostForm; issues: IssueHostForm } =>
  k === "jira" ? { code: "gitlab", issues: "jira" } : { code: "github", issues: "github" };

/**
 * Tipo do projeto pelos dois campos; `null` = combinação que o hub ainda não
 * aceita (`ProjectProvider::validate`: só GitHub + GitHub ou GitLab + Jira).
 */
export function kindOf(code: CodeHostForm, issues: IssueHostForm): ProjectKindForm | null {
  if (code === "github" && issues === "github") return "github";
  if (code === "gitlab" && issues === "jira") return "jira";
  return null;
}

/**
 * Protocolo do hub a partir do qual `hub.projects.add` aceita `tracker` e
 * `forge` (#348): o 1.5, do épico #353 (o 1.3 e o 1.4 da develop são os do
 * épico #205, sem Jira/GitLab).
 */
export const JIRA_PROTOCOL = "1.5";

/**
 * O hub aceita projeto Jira + GitLab? Hub anterior ao protocolo 1.5 ignora
 * `tracker` e `forge` e recusa o cadastro pedindo `url`. Sem `hub.info`
 * (ainda carregando), não bloqueia: o hub confere de novo.
 */
export function hubSupportsJira(protocol: string | null | undefined): boolean {
  if (!protocol) return true;
  const [major, minor] = protocol.split(".").map((n) => Number.parseInt(n, 10));
  const [wantMajor, wantMinor] = JIRA_PROTOCOL.split(".").map(Number);
  if (Number.isNaN(major)) return true;
  return major > wantMajor || (major === wantMajor && (Number.isNaN(minor) ? 0 : minor) >= wantMinor);
}

/** Campos do Jira + GitLab no formulário de Hub › Projetos. */
export interface JiraForm {
  jira_url: string;
  jira_key: string;
  /** Sync só da sprint ativa (`sprint in openSprints()`), sem JQL nem limite. */
  active_sprint_only: boolean;
  /** JQL do sync; obrigatório sem `active_sprint_only`. */
  jql: string;
  /** Limite de issues sincronizadas (as mais recentes do JQL), sem `active_sprint_only`. */
  max_issues: string;
  jira_cmd: string;
  /** URL de clone do GitLab (HTTPS ou SSH); host e caminho saem dela. */
  gitlab_url: string;
  gitlab_cmd: string;
  /** Vazio = o padrão do Jira (`{prefix}/{key}-{slug}`). */
  branch_pattern: string;
  attribution: boolean;
  /** `Tipo=papel`, separados por vírgula. */
  type_map: string;
  epic_link_field: string;
  epic_name_field: string;
  /** Campo da sprint; vazio = o ambiente detecta em cada issue. */
  sprint_field: string;
}

export const JIRA_BRANCH_PATTERN = "{prefix}/{key}-{slug}";
export const GITHUB_BRANCH_PATTERN = "{prefix}/{n}-{slug}";

/** O mapa padrão do hub (`ct_core::default_type_map`). */
export const DEFAULT_TYPE_MAP = "Epic=epic, Épico=epic, Bug=bug, Erro=bug";

/** Limite padrão e teto do limite de issues (`ct_core::DEFAULT_MAX_ISSUES`, `MAX_ISSUES_CEILING`). */
export const DEFAULT_MAX_ISSUES = 100;
export const MAX_ISSUES_CEILING = 5000;

export const EMPTY_JIRA: JiraForm = {
  jira_url: "",
  jira_key: "",
  // Projeto novo: só a sprint ativa, o recorte que não precisa de JQL.
  active_sprint_only: true,
  jql: "",
  max_issues: String(DEFAULT_MAX_ISSUES),
  jira_cmd: "",
  gitlab_url: "",
  gitlab_cmd: "",
  branch_pattern: "",
  attribution: true,
  type_map: DEFAULT_TYPE_MAP,
  epic_link_field: "",
  epic_name_field: "",
  sprint_field: "",
};

const ROLES: JiraTypeRole[] = ["epic", "issue", "bug", "chore", "ignore"];

export const isJira = (p: HubProject) => p.tracker?.kind === "jira";

/** O que as funções de rastreador e forja leem do projeto: `HubProject` ou o `Project` do ambiente. */
type Provider = Pick<HubProject, "tracker" | "forge">;

export function jiraOf(p: Pick<HubProject, "tracker">): JiraConfig | null {
  return p.tracker?.kind === "jira" ? (p.tracker as JiraConfig & { kind: "jira" }) : null;
}

export function gitlabOf(p: Pick<HubProject, "forge">): GitlabConfig | null {
  return p.forge?.kind === "gitlab" ? (p.forge as GitlabConfig & { kind: "gitlab" }) : null;
}

/**
 * Catálogo de campos próprios do Jira do projeto (#72), já ordenado por nome:
 * o que o ambiente de sync leu do `.config.yml` do jira-cli e publicou no
 * `tracker_status`. Vazio sem status, sem a seção ou com ambiente anterior.
 */
export function jiraFieldCatalog(p: Pick<HubProjectView, "tracker_status">): JiraFieldInfo[] {
  return p.tracker_status?.readiness.jira.custom_fields ?? [];
}

/** `Tipo=papel, ...` → mapa; erro com o trecho inválido. */
export function parseTypeMap(s: string): { map: Record<string, JiraTypeRole> } | { error: string } {
  const map: Record<string, JiraTypeRole> = {};
  for (const part of s.split(",").map((p) => p.trim()).filter(Boolean)) {
    const i = part.lastIndexOf("=");
    const name = i > 0 ? part.slice(0, i).trim() : "";
    const role = (i > 0 ? part.slice(i + 1).trim().toLowerCase() : "") as JiraTypeRole;
    if (!name || !ROLES.includes(role)) {
      return { error: `mapa de tipos: "${part}" (use Tipo=papel, com papel ${ROLES.join(", ")})` };
    }
    map[name] = role;
  }
  return { map };
}

export const typeMapText = (m: Record<string, JiraTypeRole | undefined>) =>
  Object.entries(m)
    .map(([k, v]) => `${k}=${v}`)
    .join(", ");

/**
 * Nome de exemplo do padrão de branch (como o `branch_from_pattern` do
 * daemon): `{prefix}` = feat, `{n}` = 5450, `{key}` = a chave e `{slug}` do
 * título de exemplo. `null` se o padrão tem marcador desconhecido ou não tem
 * `{n}` nem `{key}` (o hub recusa).
 */
export function branchPreview(pattern: string, key: string | null): string | null {
  const markers = [...pattern.matchAll(/\{([^}]*)\}/g)].map((m) => m[1]);
  if (markers.some((m) => !["prefix", "n", "key", "slug"].includes(m))) return null;
  if (!markers.includes("n") && !markers.includes("key")) return null;
  return pattern
    .replaceAll("{prefix}", "feat")
    .replaceAll("{n}", "5450")
    .replaceAll("{key}", key ? `${key}-5450` : "5450")
    .replaceAll("{slug}", "portar-o-plugin");
}

/** Parâmetros `tracker` e `forge` de `hub.projects.add|update`. */
export function providerParams(f: JiraForm): { tracker: object; forge: object } | { error: string } {
  const tm = parseTypeMap(f.type_map);
  if ("error" in tm) return tm;
  const max = Number(f.max_issues.trim());
  if (!f.active_sprint_only) {
    if (!f.jql.trim()) return { error: 'informe o JQL do sync ou marque "Sincronizar somente a sprint ativa"' };
    if (!Number.isInteger(max) || max < 1 || max > MAX_ISSUES_CEILING) {
      return { error: `limite de issues sincronizadas: use um número de 1 a ${MAX_ISSUES_CEILING}` };
    }
  }
  return {
    tracker: {
      kind: "jira",
      url: f.jira_url.trim(),
      project_key: f.jira_key.trim(),
      active_sprint_only: f.active_sprint_only,
      // Com a sprint ativa, o JQL e o limite são ignorados pelo sync; vão como estão.
      jql: f.jql.trim(),
      max_issues: Number.isInteger(max) && max >= 1 && max <= MAX_ISSUES_CEILING ? max : DEFAULT_MAX_ISSUES,
      credential_cmd: f.jira_cmd.trim(),
      config_file: "",
      epic_link_field: f.epic_link_field.trim(),
      epic_name_field: f.epic_name_field.trim(),
      sprint_field: f.sprint_field.trim(),
      type_map: tm.map,
    },
    forge: { kind: "gitlab", clone_url: f.gitlab_url.trim(), credential_cmd: f.gitlab_cmd.trim() },
  };
}

/** Formulário preenchido com o cadastro (edição). */
export function jiraFormOf(p: HubProject): JiraForm {
  const j = jiraOf(p);
  const g = gitlabOf(p);
  return {
    jira_url: j?.url ?? "",
    jira_key: j?.project_key ?? "",
    // Cadastro anterior ao modo: sem os campos (JQL com limite 100).
    active_sprint_only: j?.active_sprint_only ?? false,
    jql: j?.jql ?? "",
    max_issues: String(j?.max_issues ?? DEFAULT_MAX_ISSUES),
    jira_cmd: j?.credential_cmd ?? "",
    gitlab_url: g?.clone_url ?? "",
    gitlab_cmd: g?.credential_cmd ?? "",
    branch_pattern: p.branch_pattern ?? "",
    attribution: p.attribution,
    type_map: j ? typeMapText(j.type_map) : DEFAULT_TYPE_MAP,
    epic_link_field: j?.epic_link_field ?? "",
    epic_name_field: j?.epic_name_field ?? "",
    // Hub anterior às sprints: sem o campo.
    sprint_field: j?.sprint_field ?? "",
  };
}

/** Um item conferido pelo ambiente de sync, com a instrução quando falta. */
export interface ReadinessCheck {
  label: string;
  ok: boolean;
  /** O valor conferido (versão), quando há. */
  value?: string;
  /** O que fazer no ambiente, quando não está ok. */
  fix?: string;
}

/**
 * O que o ambiente de sync conferiu (§8.4), na ordem em que uma falha
 * impede a seguinte, com a instrução de configuração de cada item que falta.
 */
export function readinessChecks(r: TrackerReadiness, p: Provider): { jira: ReadinessCheck[]; gitlab: ReadinessCheck[] } {
  const j = jiraOf(p);
  const g = gitlabOf(p);
  const jiraCmd = j?.credential_cmd || "o comando de credencial do Jira";
  const gitlabCmd = g?.credential_cmd || "o comando de credencial do GitLab";
  return {
    jira: [
      {
        label: "jira-cli instalado",
        ok: r.jira.cli !== null,
        value: r.jira.cli ?? undefined,
        fix: "instale o jira-cli no ambiente (ou aponte jira_bin no config.toml do ctowerd)",
      },
      {
        label: ".config.yml do jira-cli presente",
        ok: r.jira.config_present,
        fix: 'rode "jira init" num terminal do ambiente',
      },
      {
        label: `servidor do .config.yml é ${j?.url ?? "o do cadastro"}`,
        ok: r.jira.server_matches,
        fix: `rode "jira init" de novo com o servidor ${j?.url ?? "do cadastro"}`,
      },
      {
        label: "comando de credencial do Jira ok",
        ok: r.jira.credential_ok,
        fix: `rode "${jiraCmd}" num terminal do ambiente: o cache do gpg-agent pode ter expirado`,
      },
      {
        label: "jira me ok",
        ok: r.jira.me_ok,
        value: r.jira.server_version ? `Jira ${r.jira.server_version}` : undefined,
        fix: "confira o usuário do .config.yml e a senha que o comando de credencial imprime",
      },
    ],
    gitlab: [
      {
        label: "glab instalado",
        ok: r.gitlab.cli !== null,
        value: r.gitlab.cli ?? undefined,
        fix: "instale o glab no ambiente (ou aponte glab_bin no config.toml do ctowerd)",
      },
      {
        label: "comando de credencial do GitLab ok",
        ok: r.gitlab.credential_ok,
        fix: `rode "${gitlabCmd}" num terminal do ambiente: o cache do gpg-agent pode ter expirado`,
      },
      {
        label: `API do GitLab ok${g ? ` (${g.host})` : ""}`,
        ok: r.gitlab.api_ok,
        fix: "confira o token de acesso que o comando de credencial imprime e o host do GitLab",
      },
    ],
  };
}

/**
 * Prontidão do ambiente para iniciar uma sessão num projeto Jira + GitLab
 * (spec §9.3, #351), pelo `env.info.trackers` do ambiente escolhido: `null`
 * se o projeto não é Jira ou se está tudo pronto; `unknown` se o ambiente
 * ainda não conferiu (ou é anterior à #349); senão, os itens que faltam. A
 * sessão não é recusada: só avisa.
 */
export function sessionReadiness(
  p: Provider & { gh_owner: string; gh_repo: string },
  trackers: readonly TrackerReadiness[] | undefined,
): { unknown: true } | { unknown: false; missing: ReadinessCheck[]; errors: string[] } | null {
  if (p.tracker?.kind !== "jira") return null;
  const same = (a: string, b: string) => a.toLowerCase() === b.toLowerCase();
  const r = trackers?.find((t) => same(t.owner, p.gh_owner) && same(t.repo, p.gh_repo));
  if (!r) return { unknown: true };
  const c = readinessChecks(r, p);
  const missing = [...c.jira, ...c.gitlab].filter((x) => !x.ok);
  if (missing.length === 0) return null;
  const errors = [r.jira.error, r.gitlab.error].filter((e): e is string => !!e);
  return { unknown: false, missing, errors };
}
