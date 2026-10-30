// Muitas abas (#427; spec da #87, §5 e §6): o projeto de uma aba de sessão
// ou de shell, pelo dashboard do ambiente, e o título da aba da sessão do
// projeto (#0). Só tipos de fora: o reducer de nav importa daqui.
import type { Dashboard, Project } from "../protocol/gen";
import type { Pane } from "../state/nav";
import { PROJECT_ITEM } from "../util/item";

/** Projeto de uma aba: `key` é a mesma do grupo na árvore (`env|p<id>`). */
export interface TabProject {
  key: string;
  name: string;
}

export const projectKey = (env: string, p: Pick<Project, "id">) => `${env}|p${p.id}`;

/**
 * Projeto da aba pelo dashboard: o do work item da sessão (o gestor, o do
 * escopo `project`) ou o do work item do shell. `null` sem dashboard, sem a
 * sessão nele ou no gestor de portfólio.
 */
export function paneProject(d: Dashboard | undefined, p: Pane): TabProject | null {
  if (!d) return null;
  const of = (pr: Project | undefined) => (pr ? { key: projectKey(p.env, pr), name: pr.name } : null);
  const byItem = (id: number) => of(d.projects.find((x) => x.work_items.some((w) => w.id === id))?.project);
  if ("shell" in p) return byItem(p.workItemId);
  const s = [...d.live_sessions, ...d.interrupted_sessions, ...d.failed_sessions].find((x) => x.id === p.session);
  if (!s) return null;
  if (d.gestor_work_item_id !== null && s.work_item_id === d.gestor_work_item_id) {
    const scope = d.gestor_scopes?.[s.id];
    if (scope?.kind !== "project") return null;
    return of(d.projects.find((x) => x.project.gh_owner === scope.owner && x.project.gh_repo === scope.repo)?.project);
  }
  return byItem(s.work_item_id);
}

/**
 * Título da aba da sessão: "Sessão do projeto · <projeto>" no #0 (a árvore
 * já mostra o projeto no grupo, a faixa de abas não); nos demais, o de
 * `itemTitle`.
 */
export function projectSessionTitle(ghNumber: number, project: Pick<Project, "name"> | undefined): string | null {
  if (ghNumber !== PROJECT_ITEM) return null;
  return project ? `Sessão do projeto · ${project.name}` : "Sessão do projeto";
}
