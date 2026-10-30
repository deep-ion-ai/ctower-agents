// Abas da tela Projeto (#317): cada item aberto é uma aba de editor própria.
import type { OpenTab, Screen } from "../state/nav";
import { itemShort, type ProviderProject } from "../util/item";

type ProjectScreen = Extract<Screen, { name: "project" }>;

/** Chave estável do projeto: `owner/repo` (minúsculas) ou, sem ela, `env:id`. */
export function projectKey(s: Pick<ProjectScreen, "owner" | "repo" | "env" | "projectId">): string {
  if (s.owner && s.repo) return `${s.owner}/${s.repo}`.toLowerCase();
  return `${s.env ?? "?"}:${s.projectId ?? "?"}`;
}

/** Aba do item `n` do projeto `base` (endereço do projeto, sem item). */
export function itemTab(base: ProjectScreen, n: number, title: string, project?: ProviderProject | null): OpenTab {
  // A aba do item é sempre Itens: a aba e a seção do projeto não vão junto (#17).
  const { item: _omit, view: _view, section: _section, ...addr } = base;
  return {
    id: `project:${projectKey(addr)}#${n}`,
    title: `${itemShort(project, n)} ${title}`,
    screen: { ...addr, item: { ghNumber: n } },
  };
}
