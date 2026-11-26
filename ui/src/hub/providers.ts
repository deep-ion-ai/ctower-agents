// Registro dos tipos de conexão na UI (#278, épico #224): o que cada tipo
// tem de próprio na tela de Conexões, na escolha da conexão do projeto e na
// sobrescrita do ambiente. As telas consultam este registro em vez de
// comparar tipos. Os dados que são regra (rótulo, formas de credencial
// aceitas, endpoint padrão, conexão padrão) vêm do descritor do Rust
// (`ct_core::provider`), gerados em `protocol/gen/providers.ts`; aqui ficam
// só os textos do formulário. Um tipo novo entra no descritor e em `UI`.
import type { CredentialKind, ProviderInfo, ProviderKind } from "../protocol/gen";
import { PROVIDER_INFOS } from "../protocol/gen/providers";

/** Campo de `options` da conexão no formulário (texto; vazio não vai ao hub). */
export interface OptionField {
  key: string;
  label: string;
  placeholder: string;
}

/** O que a UI acrescenta ao descritor: rótulos, exemplos e campos do formulário. */
export interface ProviderUi {
  /** Rótulo do campo `endpoint` (URL, host ou API). */
  endpointLabel: string;
  /** Exemplo de endpoint (placeholder, sem dado real). */
  endpointHint: string;
  /** Exemplo de comando de credencial (placeholder). */
  cmdHint: string;
  /** Comando que imprime o token da CLI do tipo, quando a CLI tem um (sugestão na sobrescrita do ambiente). */
  cliTokenCmd?: string;
  /** Opções próprias do tipo (`options` da conexão). */
  options: OptionField[];
}

const UI: Record<ProviderKind, ProviderUi> = {
  jira: {
    endpointLabel: "URL",
    endpointHint: "https://jira.example.com",
    cmdHint: "pass show jira/token | head -1",
    options: [
      {
        key: "config_file",
        label: "Configuração do jira-cli",
        placeholder: "vazio = ~/.config/.jira/.config.yml",
      },
    ],
  },
  gitlab: {
    endpointLabel: "Host",
    endpointHint: "gitlab.example.com",
    cmdHint: "pass show gitlab/token | head -1",
    options: [],
  },
  github: {
    endpointLabel: "API",
    endpointHint: "vazio = api.github.com",
    cmdHint: "pass show github/token | head -1",
    cliTokenCmd: "gh auth token",
    options: [],
  },
};

/** Um tipo de conexão: o descritor do Rust com os textos da UI. */
export type Provider = ProviderInfo & ProviderUi;

/** Os tipos de conexão, na ordem do registro do Rust (a do formulário). */
export const PROVIDERS: Provider[] = PROVIDER_INFOS.map((info) => ({ ...info, ...UI[info.kind] }));

/** O tipo `kind` no registro. */
export function providerOf(kind: ProviderKind): Provider {
  const p = PROVIDERS.find((x) => x.kind === kind);
  if (!p) throw new Error(`tipo de conexão sem descritor: ${kind}`);
  return p;
}

/** Rótulo do tipo (`Jira`, `GitLab`, `GitHub`). */
export const providerLabel = (kind: ProviderKind): string => providerOf(kind).label;

/** O tipo aceita a forma de credencial (`ProviderDescriptor::credential_kinds`)? */
export const acceptsCredential = (kind: ProviderKind, cred: CredentialKind): boolean =>
  providerOf(kind).credential_kinds.includes(cred);

/** Rótulo de cada forma de credencial no seletor. */
export const CRED_TEXT: Record<CredentialKind, string> = {
  command: "Comando que imprime o token",
  secret: "Nome do secret do hub",
  cli: "Login da CLI",
  file: "Caminho do arquivo com o token",
  none: "Sem autenticação",
};

/** Nome da forma nas mensagens: `um comando, um secret do hub ou um arquivo`. */
const CRED_NOUN: Record<CredentialKind, string> = {
  command: "um comando",
  secret: "um secret do hub",
  cli: "o login da CLI",
  file: "um arquivo",
  none: "nenhuma",
};

/** `a, b ou c`. */
function orList(items: string[]): string {
  if (items.length <= 1) return items.join("");
  return `${items.slice(0, -1).join(", ")} ou ${items[items.length - 1]}`;
}

/**
 * Erro da forma de credencial que o tipo recusa: as que ele aceita e, se ele
 * não tem login da CLI, os tipos que têm.
 */
export function refusedCredentialText(kind: ProviderKind): string {
  const p = providerOf(kind);
  const accepted = orList(p.credential_kinds.map((k) => CRED_NOUN[k]));
  const cli = p.credential_kinds.includes("cli")
    ? []
    : PROVIDERS.filter((x) => x.credential_kinds.includes("cli")).map((x) => x.label);
  const tail = cli.length ? `; o login da CLI é só do ${cli.join(" e do ")}` : "";
  return `${p.label}: a credencial precisa ser ${accepted}${tail}`;
}

/** Lado do projeto (rastreador ou forja) como a UI o recebe: o tipo e a conexão referenciada. */
interface Side {
  kind: ProviderKind;
  connection?: number | null;
}

/**
 * A conexão do tipo `kind` que o projeto referencia: a do primeiro lado do
 * tipo que referencia uma (`ct_core::provider::project_ref`). `undefined` =
 * nenhum lado é do tipo; `null` = usa o tipo sem referenciar (vale a padrão
 * do tipo, se houver).
 */
export function projectRef(
  p: { tracker?: Side | null; forge?: Side | null },
  kind: ProviderKind,
): number | null | undefined {
  const sides = [p.tracker, p.forge].filter((s): s is Side => !!s && s.kind === kind);
  if (!sides.length) return undefined;
  return sides.map((s) => s.connection ?? null).find((c) => c !== null) ?? null;
}

/**
 * O projeto usa a conexão? A referenciada ou, sem referência, a padrão do
 * tipo (`ct_core::provider::uses_connection`).
 */
export function usesConnection(
  c: { id: number; name: string; kind: ProviderKind },
  p: { tracker?: Side | null; forge?: Side | null },
): boolean {
  const ref = projectRef(p, c.kind);
  if (ref === undefined) return false;
  if (ref !== null) return ref === c.id;
  return providerOf(c.kind).default_connection === c.name;
}
