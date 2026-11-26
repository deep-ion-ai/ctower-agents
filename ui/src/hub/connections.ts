// Conexões do hub (#238, épico #224): cadastro das conexões com GitHub, GitLab
// e Jira (`hub.connections.*`, #235), formulário e textos. Funções puras,
// testadas em connections.test.ts. A credencial nunca passa pela UI como
// valor: o hub guarda o nome do secret, o comando que imprime o token ou o
// caminho do arquivo, e é isso que a tela mostra e edita.
import type { Connection, CredentialSource, ProviderKind, Readiness } from "../protocol/gen";
import type { HubProject } from "../protocol/hub-gen";
import { gitlabOf, jiraOf } from "./tracker";

export const KIND_LABEL: Record<ProviderKind, string> = {
  github: "GitHub",
  gitlab: "GitLab",
  jira: "Jira",
};

/** Tipos de conexão na ordem do formulário. */
export const CONNECTION_KINDS: ProviderKind[] = ["jira", "gitlab", "github"];

export type CredKind = CredentialSource["kind"];

/**
 * Formas de credencial que cada tipo aceita. Jira e GitLab só o comando: o
 * ambiente ainda não entrega secret, arquivo ou CLI à sessão (o hub recusa
 * os outros, `ct-hub-store`).
 */
export const CRED_KINDS: Record<ProviderKind, CredKind[]> = {
  jira: ["command"],
  gitlab: ["command"],
  github: ["cli", "file", "command", "secret", "none"],
};

/** Exemplo de endpoint por tipo (placeholder, sem dado real). */
export const ENDPOINT_HINT: Record<ProviderKind, string> = {
  jira: "https://jira.example.com",
  gitlab: "gitlab.example.com",
  github: "vazio = api.github.com",
};

/** Exemplo de comando de credencial por tipo (placeholder). */
export const CMD_HINT: Record<ProviderKind, string> = {
  jira: "pass show jira/token | head -1",
  gitlab: "pass show gitlab/token | head -1",
  github: "pass show github/token | head -1",
};

/** Formulário de uma conexão. `cred_value` guarda o nome, o comando ou o caminho, conforme `cred`. */
export interface ConnectionForm {
  name: string;
  kind: ProviderKind;
  endpoint: string;
  cred: CredKind;
  cred_value: string;
  /** Jira: arquivo de configuração do jira-cli; vazio = o padrão. */
  config_file: string;
}

export const EMPTY_CONNECTION_FORM: ConnectionForm = {
  name: "",
  kind: "jira",
  endpoint: "",
  cred: "command",
  cred_value: "",
  config_file: "",
};

/** Os campos de `c` no formulário (edição). */
export function connectionFormOf(c: Connection): ConnectionForm {
  const cr = c.credential;
  const cred_value =
    cr.kind === "secret" ? cr.name : cr.kind === "command" ? cr.cmd : cr.kind === "file" ? cr.path : "";
  const cfg = c.options?.config_file;
  return {
    name: c.name,
    kind: c.kind,
    endpoint: c.endpoint,
    cred: cr.kind,
    cred_value,
    config_file: typeof cfg === "string" ? cfg : "",
  };
}

/** A credencial do formulário, ou o erro de quem falta. */
export function credentialOf(f: ConnectionForm): { credential: CredentialSource } | { error: string } {
  if (!CRED_KINDS[f.kind].includes(f.cred)) {
    return {
      error: `${KIND_LABEL[f.kind]}: a credencial precisa ser um comando que imprime o token; secret, arquivo e CLI ainda não chegam ao ambiente`,
    };
  }
  const v = f.cred_value.trim();
  switch (f.cred) {
    case "command":
      return v ? { credential: { kind: "command", cmd: v } } : { error: "informe o comando que imprime o token" };
    case "secret":
      return v ? { credential: { kind: "secret", name: v } } : { error: "informe o nome do secret do hub" };
    case "file":
      return v ? { credential: { kind: "file", path: v } } : { error: "informe o caminho do arquivo com o token" };
    case "cli":
      return { credential: { kind: "cli" } };
    case "none":
      return { credential: { kind: "none" } };
  }
}

/**
 * Parâmetros de `hub.connections.add` (sem `id`) ou `update` (com `id`, e
 * sem o tipo, que não muda). Erro com a primeira coisa a corrigir.
 */
export function connectionParams(
  f: ConnectionForm,
  id?: number,
): { params: Record<string, unknown> } | { error: string } {
  const name = f.name.trim();
  if (!name) return { error: "informe o nome da conexão (ex.: jira-empresa)" };
  const cred = credentialOf(f);
  if ("error" in cred) return cred;
  const options = f.kind === "jira" && f.config_file.trim() ? { config_file: f.config_file.trim() } : {};
  const common = { name, endpoint: f.endpoint.trim(), credential: cred.credential, options };
  if (id === undefined) return { params: { ...common, kind: f.kind } };
  return { params: { id, ...common } };
}

/** O que a credencial é, em texto, sem valor de secret. */
export function credentialText(c: CredentialSource): string {
  switch (c.kind) {
    case "command":
      return `comando: ${c.cmd}`;
    case "secret":
      return `secret do hub: ${c.name}`;
    case "file":
      return `arquivo: ${c.path}`;
    case "cli":
      return "login da CLI";
    case "none":
      return "sem autenticação";
  }
}

/** Texto da prontidão de `hub.connections.test` (a checagem é sem rede). */
export function readinessText(r: Readiness): { ok: boolean; text: string } {
  return r.state === "ready" ? { ok: true, text: "pronta" } : { ok: false, text: r.reason };
}

/** Resultado de `hub.connections.test`: o tipo gerado não cobre o formato (`unknown` no registro). */
export interface ConnectionTest {
  id: number;
  name: string;
  readiness: Readiness;
}

/** Conexões de um tipo, por nome. */
export const connectionsOfKind = (list: readonly Connection[], kind: ProviderKind): Connection[] =>
  list.filter((c) => c.kind === kind).sort((a, b) => a.name.localeCompare(b.name));

/** Rótulo de uma conexão no seletor e nos cartões: `jira-empresa (https://jira.example.com)`. */
export function connectionLabel(c: Connection): string {
  return c.endpoint ? `${c.name} (${c.endpoint})` : c.name;
}

/**
 * Projetos (`owner/repo`, em ordem) que usam a conexão `id`, como
 * `tracker.connection` ou `forge.connection`: o espelho na UI do que o hub
 * confere em `connection_users`.
 */
export function connectionUsers(id: number, projects: readonly { project: Pick<HubProject, "gh_owner" | "gh_repo" | "tracker" | "forge"> }[]): string[] {
  return projects
    .filter(({ project: p }) => jiraOf(p)?.connection === id || gitlabOf(p)?.connection === id)
    .map(({ project: p }) => `${p.gh_owner}/${p.gh_repo}`)
    .sort();
}

/**
 * Aviso do campo de nome de uma conexão em uso (#271): o hub recusa
 * renomear, porque as sobrescritas `[connections."nome"]` dos ambientes são
 * indexadas pelo nome e deixariam de valer.
 */
export function renameLockedText(name: string, users: readonly string[]): string {
  return `Em uso por ${users.join(", ")}: o nome não muda, porque renomear desligaria as sobrescritas [connections."${name}"] dos ambientes. Para trocar o nome, crie uma conexão nova e troque a conexão dos projetos.`;
}
