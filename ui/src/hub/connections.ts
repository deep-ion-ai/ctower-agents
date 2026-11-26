// Conexões do hub (#238, épico #224): cadastro das conexões com GitHub, GitLab
// e Jira (`hub.connections.*`, #235), formulário e textos. Funções puras,
// testadas em connections.test.ts. A credencial nunca passa pela UI como
// valor: o hub guarda o nome do secret, o comando que imprime o token ou o
// caminho do arquivo, e é isso que a tela mostra e edita.
// O que é de cada tipo (rótulo, formas de credencial aceitas, exemplos e
// opções do formulário) vem do registro `providers.ts` (#278).
import type { Connection, CredentialSource, ProviderKind, Readiness } from "../protocol/gen";
import type { HubProject } from "../protocol/hub-gen";
import { acceptsCredential, providerOf, refusedCredentialText, usesConnection } from "./providers";

export type CredKind = CredentialSource["kind"];

/** Dica do campo da credencial, pela forma escolhida. */
export const CRED_HINT: Partial<Record<CredKind, string>> = {
  command: "O comando roda no ambiente: a senha e o token nunca passam pelo hub.",
  secret:
    "Secret do hub, lido pelo ambiente no projeto em uso: o do projeto vale sobre o global. Para uma conexão de vários projetos, grave o secret como global.",
  file: "Caminho no ambiente (~ é a pasta pessoal); o token é a primeira linha não vazia do arquivo.",
};

/** Formulário de uma conexão. `cred_value` guarda o nome, o comando ou o caminho, conforme `cred`. */
export interface ConnectionForm {
  name: string;
  kind: ProviderKind;
  endpoint: string;
  cred: CredKind;
  cred_value: string;
  /** Opções do tipo (`providerOf(kind).options`), por chave; vazio não vai ao hub. */
  options: Record<string, string>;
}

export const EMPTY_CONNECTION_FORM: ConnectionForm = {
  name: "",
  kind: "jira",
  endpoint: "",
  // O secret do hub é a fonte padrão (épico #224).
  cred: "secret",
  cred_value: "",
  options: {},
};

/** Os campos de `c` no formulário (edição). */
export function connectionFormOf(c: Connection): ConnectionForm {
  const cr = c.credential;
  const cred_value =
    cr.kind === "secret" ? cr.name : cr.kind === "command" ? cr.cmd : cr.kind === "file" ? cr.path : "";
  const options: Record<string, string> = {};
  for (const { key } of providerOf(c.kind).options) {
    const v = c.options?.[key];
    options[key] = typeof v === "string" ? v : "";
  }
  return {
    name: c.name,
    kind: c.kind,
    endpoint: c.endpoint,
    cred: cr.kind,
    cred_value,
    options,
  };
}

/** A credencial do formulário, ou o erro de quem falta. */
export function credentialOf(f: ConnectionForm): { credential: CredentialSource } | { error: string } {
  if (!acceptsCredential(f.kind, f.cred)) return { error: refusedCredentialText(f.kind) };
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
  const options: Record<string, string> = {};
  for (const { key } of providerOf(f.kind).options) {
    const v = (f.options[key] ?? "").trim();
    if (v) options[key] = v;
  }
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
 * Projetos (`owner/repo`, em ordem) que usam a conexão, como
 * `tracker.connection` ou `forge.connection`: o espelho na UI do que o hub
 * confere em `connection_users`. A conexão padrão do tipo (a `github`)
 * também é usada pelos projetos do tipo sem conexão (#275).
 */
export function connectionUsers(
  c: Pick<Connection, "id" | "name" | "kind">,
  projects: readonly { project: Pick<HubProject, "gh_owner" | "gh_repo" | "tracker" | "forge"> }[],
): string[] {
  return projects
    .filter(({ project: p }) => usesConnection(c, p))
    .map(({ project: p }) => `${p.gh_owner}/${p.gh_repo}`)
    .sort();
}

/**
 * Por que "Excluir" fica desabilitado numa conexão em uso (#281): o hub
 * recusa apagar conexão que algum projeto referencia.
 */
export function deleteLockedText(users: readonly string[]): string {
  return `Em uso por ${users.join(", ")}: troque a conexão desses projetos antes de excluir.`;
}

/**
 * Aviso do campo de nome de uma conexão em uso (#271): o hub recusa
 * renomear, porque as sobrescritas `[connections."nome"]` dos ambientes são
 * indexadas pelo nome e deixariam de valer.
 */
export function renameLockedText(name: string, users: readonly string[]): string {
  return `Em uso por ${users.join(", ")}: o nome não muda, porque renomear desligaria as sobrescritas [connections."${name}"] dos ambientes. Para trocar o nome, crie uma conexão nova e troque a conexão dos projetos.`;
}
