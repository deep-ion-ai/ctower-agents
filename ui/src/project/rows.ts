// Lista de projetos com contadores e o menu ⋯ de cada um (#16). A Sala de
// controle (grupo Projetos do painel lateral, #10) e o índice da atividade
// Projetos mostram a mesma lista, pelo mesmo critério: com o hub no ar, a
// visão consolidada, na ordem de prioridade do hub; sem ele, os projetos
// locais de cada ambiente.
import { lookup } from "../control-room/attention";
import type { HubProjectView } from "../protocol/hub-gen";
import type { SyncState } from "../protocol/gen";
import type { MenuItem } from "../shell/ContextMenu";
import type { StartSession, StartTarget } from "../shell/start";
import type { Nav, Screen } from "../state/nav";
import { useTower } from "../state/tower";
import { forgeLabel, jiraProjectUrl, repoUrl, type ProviderProject } from "../util/item";
import { hubClones, localClone } from "./clones";
import { projectAt, projectsPlace } from "./sections";

export interface ProjectRow {
  key: string;
  name: string;
  running: number;
  waiting: number;
  /** Rastreador e forja, para "Abrir no GitHub/GitLab/Jira". */
  project: ProviderProject;
  /** Alvo da sessão de planejamento; `null` = nenhum ambiente conectado com o projeto. */
  start: StartTarget | null;
  /** Endereço do projeto (aba própria, `mainTabId` em state/nav). */
  screen: Extract<Screen, { name: "project" }>;
  /** Sem hub: o ambiente do projeto local. Com o hub, `null` (o projeto é de todos). */
  env: { id: string; name: string } | null;
  /** Com o hub: o projeto com atribuições e sync (`hub.projects.list`), se já veio. */
  view: HubProjectView | null;
  /** Sem hub: a sincronização relatada pelo ambiente. */
  sync: SyncState | null;
}

const same = (a: string, b: string) => a.toLowerCase() === b.toLowerCase();

/**
 * Projetos com sessões em execução e aguardando; com o hub no ar, a visão
 * consolidada, na ordem de `hub.projects.list` (a prioridade entre
 * projetos, `rank`), que é a que o índice reordena.
 */
export function useProjectRows(): ProjectRow[] {
  const tower = useTower();
  const { envs, dashboards, hub } = tower;
  if (hub.up && hub.dashboard) {
    const hd = hub.dashboard;
    const views = hub.projects;
    const rank = (id: number) => {
      const i = views.findIndex((v) => v.project.id === id);
      return i < 0 ? Number.MAX_SAFE_INTEGER : i;
    };
    return [...hd.projects]
      .sort((a, b) => rank(a.project.id) - rank(b.project.id))
      .map(({ project, work_items }) => {
        let running = 0;
        let waiting = 0;
        for (const s of hd.live_sessions) {
          const wi = work_items.find((w) => w.id === s.work_item_id);
          if (!wi) continue;
          if (wi.status === "aguardando_voce") waiting++;
          else running++;
        }
        return {
          key: `hub:${project.id}`,
          name: project.name,
          running,
          waiting,
          project,
          start:
            hubClones(tower, project.gh_owner, project.gh_repo).length > 0
              ? { owner: project.gh_owner, repo: project.gh_repo }
              : null,
          screen: { name: "project", owner: project.gh_owner, repo: project.gh_repo },
          env: null,
          view:
            views.find((v) => same(v.project.gh_owner, project.gh_owner) && same(v.project.gh_repo, project.gh_repo)) ??
            null,
          sync: null,
        };
      });
  }
  return envs.flatMap((e) => {
    const d = dashboards[e.id];
    if (!d) return [];
    return d.projects.map(({ project }): ProjectRow => {
      let running = 0;
      let waiting = 0;
      for (const s of d.live_sessions) {
        const { wi } = lookup(d, s);
        if (wi?.project_id !== project.id) continue;
        if (wi.status === "aguardando_voce") waiting++;
        else running++;
      }
      return {
        key: `${e.id}:${project.id}`,
        name: project.name,
        running,
        waiting,
        project,
        start: localClone(tower, e.id, project.id) ? { env: e.id, projectId: project.id } : null,
        // Com `owner/repo`: a aba do projeto é a mesma por qualquer caminho.
        screen: { name: "project", env: e.id, projectId: project.id, owner: project.gh_owner, repo: project.gh_repo },
        env: { id: e.id, name: e.name },
        view: null,
        sync: d.sync.find((x) => x.project_id === project.id) ?? null,
      };
    });
  });
}

/**
 * Ações do menu ⋯ (e do botão direito) de um projeto: as mesmas na Sala de
 * controle e no índice. Configurações abre a aba Configurações do projeto
 * (#17); a atribuição é no Hub, só no desktop (`editable`).
 */
export function projectMenuItems(
  p: ProjectRow,
  ctx: {
    nav: Pick<Nav, "go" | "openTab">;
    hubConnected: boolean;
    editable: boolean;
    startSession: StartSession;
    openUrl(url: string): void;
    origin: HTMLElement;
  },
): MenuItem[] {
  const { nav, startSession, openUrl, origin } = ctx;
  const ref = { owner: p.project.gh_owner, repo: p.project.gh_repo };
  const jira = jiraProjectUrl(p.project);
  return [
    { label: "Abrir", run: () => nav.go(projectAt(p.screen)) },
    // A aba Configurações do próprio projeto (#17), que vem do hub.
    ...(ctx.hubConnected ? [{ label: "Configurações", run: () => nav.go(projectAt(p.screen, "settings")) }] : []),
    ...(ctx.editable
      ? // Configurações › Ambientes do próprio projeto, com o formulário aberto (#18).
        [{ label: "Atribuir a ambiente", run: () => nav.go(projectsPlace({ project: ref, assign: true })) }]
      : []),
    ...(p.start ? [{ label: "Sessão de planejamento", run: () => startSession(p.start!, origin) }] : []),
    { label: `Abrir no ${forgeLabel(p.project)}`, run: () => openUrl(repoUrl(p.project)) },
    ...(jira ? [{ label: "Abrir no Jira", run: () => openUrl(jira) }] : []),
  ];
}
