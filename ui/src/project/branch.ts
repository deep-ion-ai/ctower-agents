// Mesma regra de crates/ct-core/src/branch.rs (prévia antes de iniciar a
// sessão; quem cria a branch é o daemon). Os testes repetem os casos do Rust.
import type { WorkItemKind } from "../protocol/gen";

const FOLD: Record<string, string> = {};
for (const [to, from] of [
  ["a", "áàâãäå"],
  ["e", "éèêë"],
  ["i", "íìîï"],
  ["o", "óòôõö"],
  ["u", "úùûü"],
  ["c", "ç"],
  ["n", "ñ"],
  ["y", "ýÿ"],
] as const) {
  for (const c of from) FOLD[c] = to;
}

/** Slug ASCII (`[a-z0-9-]`), sem hífens repetidos nem nas pontas, até `maxLen` caracteres. */
export function slugify(title: string, maxLen: number): string {
  let out = "";
  let pendingDash = false;
  for (const raw of title.toLowerCase()) {
    const c = FOLD[raw] ?? raw;
    if (c >= "̀" && c <= "ͯ") continue; // acento combinante (NFD)
    if (/^[a-z0-9]$/.test(c)) {
      if (pendingDash && out) out += "-";
      pendingDash = false;
      out += c;
    } else {
      pendingDash = true;
    }
  }
  return out.slice(0, maxLen).replace(/-+$/, "");
}

export function branchFor(kind: WorkItemKind, number: number, title: string, labels: string[]): string {
  const has = (name: string) => labels.some((l) => l.toLowerCase() === name);
  const prefix = kind === "epic" ? "wip" : has("bug") ? "fix" : has("chore") ? "chore" : "feat";
  const slug = slugify(title, 40);
  return slug ? `${prefix}/${number}-${slug}` : `${prefix}/${number}`;
}

/**
 * Branch pelo padrão do projeto (`branch_from_pattern` do Rust, spec do Jira
 * e GitLab §4): `{prefix}`, `{n}`, `{key}` (`12` ou `PROJ-5450`) e
 * `{slug}`; com slug vazio, o separador antes dele cai. `null` se o padrão
 * é inválido (marcador desconhecido, chave sem par ou sem `{n}`/`{key}`).
 */
export function branchFromPattern(
  pattern: string,
  kind: WorkItemKind,
  number: number,
  key: string,
  title: string,
  labels: string[],
): string | null {
  const parts = pattern.split(/(\{[^{}]*\})/).filter((x) => x !== "");
  const isMarker = (x: string) => /^\{[^{}]*\}$/.test(x);
  if (parts.some((x) => !isMarker(x) && /[{}]/.test(x))) return null;
  const markers = parts.filter(isMarker).map((x) => x.slice(1, -1));
  if (markers.some((m) => !["prefix", "n", "key", "slug"].includes(m))) return null;
  if (!markers.includes("n") && !markers.includes("key")) return null;
  const has = (name: string) => labels.some((l) => l.toLowerCase() === name);
  const prefix = kind === "epic" ? "wip" : has("bug") ? "fix" : has("chore") ? "chore" : "feat";
  const slug = slugify(title, 40);
  let out = "";
  for (const x of parts) {
    if (x === "{prefix}") out += prefix;
    else if (x === "{n}") out += String(number);
    else if (x === "{key}") out += key;
    else if (x === "{slug}") {
      if (slug) out += slug;
      else if (/[-_./]$/.test(out)) out = out.slice(0, -1);
    } else out += x;
  }
  return out;
}

/** Padrão do projeto ou o do rastreador (`{prefix}/{n}-{slug}` no GitHub, `{prefix}/{key}-{slug}` no Jira). */
export function patternOf(p: { branch_pattern?: string | null; tracker?: { kind: string } }): string {
  return p.branch_pattern || (p.tracker?.kind === "jira" ? "{prefix}/{key}-{slug}" : "{prefix}/{n}-{slug}");
}

export function worktreeDirName(branch: string): string {
  return branch.replaceAll("/", "__");
}
