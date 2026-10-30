// Docs do projeto no formulário de Hub › Projetos (#444, épico #445): liga e
// desliga a pasta de docs e edita os modelos de pastas. Funções puras,
// testadas em docs.test.ts.
import type { DocsConfig, HubProject } from "../protocol/hub-gen";

/** Campos do formulário: as pastas uma por linha. */
export interface DocsForm {
  enabled: boolean;
  project_dirs: string;
  epic_dirs: string;
}

/** Linhas não vazias, aparadas e sem barras nas pontas (`analises/` = `analises`). */
export const dirLines = (s: string): string[] =>
  s
    .split("\n")
    .map((l) => l.trim().replace(/^\/+|\/+$/g, ""))
    .filter(Boolean);

/** Formulário do projeto; `null` = hub anterior à #440 (sem `docs`), que não edita docs. */
export function docsFormOf(p: HubProject): DocsForm | null {
  const d = (p as Partial<HubProject>).docs;
  if (!d) return null;
  return { enabled: d.enabled, project_dirs: d.project_dirs.join("\n"), epic_dirs: d.epic_dirs.join("\n") };
}

/**
 * `docs` do `hub.projects.update` (o objeto inteiro: o hub substitui), ou
 * `null` se nada mudou ou o hub não conhece `docs`.
 */
export function docsPatch(f: DocsForm | null, p: HubProject): DocsConfig | null {
  const was = (p as Partial<HubProject>).docs;
  if (!f || !was) return null;
  const next: DocsConfig = { enabled: f.enabled, project_dirs: dirLines(f.project_dirs), epic_dirs: dirLines(f.epic_dirs) };
  const same =
    next.enabled === was.enabled &&
    next.project_dirs.join("\n") === was.project_dirs.join("\n") &&
    next.epic_dirs.join("\n") === was.epic_dirs.join("\n");
  return same ? null : next;
}

/** Resumo de uma linha para o detalhe do projeto. */
export function docsText(d: DocsConfig): string {
  if (!d.enabled) return "desligados";
  const list = (xs: string[]) => (xs.length > 0 ? xs.join(", ") : "nenhuma pasta");
  return `ligados · _projeto: ${list(d.project_dirs)} · épico: ${list(d.epic_dirs)}`;
}

/** Pasta do projeto em `docs_dir`: `<owner>__<repo>` (as barras do grupo do GitLab também viram `__`). */
export const docsDirName = (owner: string, repo: string) => `${owner.replaceAll("/", "__")}__${repo}`;
