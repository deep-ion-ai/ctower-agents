// Título e estado das abas da área de editor.
import { GESTOR_LABEL, gestorLabel, lookup } from "../control-room/attention";
import { tileState } from "../control-room/TileGrid";
import { hubTitle } from "../hub/sections";
import { maintenanceTitle } from "../maintenance/sections";
import type { Dashboard, Project } from "../protocol/gen";
import { shellItem, shellTitle } from "../terminal/shells";
import type { EditorTab, Screen, TabMeta } from "../state/nav";
import { paneProject, projectSessionTitle } from "../terminal/tabmeta";
import { dashboardSessions } from "../terminal/SessionTree";
import { itemShort, itemTitle, PROJECT_ITEM } from "../util/item";
import type { IconName } from "./icons";

/** Nome de cada tela (barra de atividades, abas, cabeçalho do painel lateral). */
export const SCREEN_LABEL: Record<Screen["name"], string> = {
  control: "Sala de controle",
  terminal: "Terminal",
  project: "Projetos",
  review: "Revisão",
  recovery: "Recuperação",
  hub: "Hub",
  maintenance: "Manutenção",
  settings: "Configurações",
};

function screenTitle(s: Screen, dashboards: Record<string, Dashboard>): string {
  switch (s.name) {
    case "project": {
      if (s.repo) return s.repo;
      const p =
        s.env !== undefined && s.projectId !== undefined
          ? dashboards[s.env]?.projects.find((x) => x.project.id === s.projectId)?.project
          : undefined;
      return p?.name ?? "Projeto";
    }
    case "review": {
      const dp =
        s.env !== undefined && s.workItemId !== undefined
          ? dashboards[s.env]?.projects.find((p) => p.work_items.some((w) => w.id === s.workItemId))
          : undefined;
      const wi = dp?.work_items.find((w) => w.id === s.workItemId);
      if (s.env !== undefined && s.workItemId !== undefined && dashboards[s.env]?.gestor_work_item_id === s.workItemId)
        return `Revisão · ${GESTOR_LABEL}`;
      if (!wi) return SCREEN_LABEL.review;
      return `Revisão · ${wi.gh_number === PROJECT_ITEM ? "planejamento" : itemShort(dp?.project, wi.gh_number)}`;
    }
    case "recovery": {
      const d = s.env !== undefined ? dashboards[s.env] : undefined;
      const sess = d?.interrupted_sessions.find((x) => x.id === s.session);
      if (!d || !sess) return SCREEN_LABEL.recovery;
      const { wi, project, gestor } = lookup(d, sess);
      if (gestor) return `Recuperação · ${GESTOR_LABEL}`;
      return wi
        ? `Recuperação · ${wi.gh_number === PROJECT_ITEM ? "planejamento" : itemShort(project, wi.gh_number)}`
        : SCREEN_LABEL.recovery;
    }
    case "hub":
      return hubTitle(s.section);
    case "maintenance":
      return maintenanceTitle(s.section);
    default:
      return SCREEN_LABEL[s.name];
  }
}

export interface TabLook {
  title: string;
  /** Tom do ponto de estado (só abas de sessão). */
  tone?: string;
  /** Ícone antes do título (o `>_` das abas de shell, #423). */
  icon?: IconName;
  /** Projeto da aba de sessão ou shell (lista de abas, "Fechar as do projeto"). */
  project?: string;
  /** Tooltip: projeto e ambiente (#427). */
  tip?: string;
}

/** Projeto e ambiente da aba, para o tooltip: "control-tower · WSL". */
function tipOf(project: string | undefined, env: string): string {
  return `${project ?? "Sem projeto"} · ${env}`;
}

/**
 * Título e estado da aba. `meta` é o que a tela Terminal publicou da aba
 * (#427): vale quando a sessão já saiu do dashboard (a aba encerrada segue
 * com o título do item, e não "Sessão") e dá o nome do ambiente.
 */
export function tabLook(t: EditorTab, dashboards: Record<string, Dashboard>, meta?: TabMeta): TabLook {
  if (t.kind === "screen") return { title: t.title ?? screenTitle(t.screen, dashboards) };
  const project = meta?.project?.name ?? paneProject(dashboards[t.env], t)?.name;
  const tip = tipOf(project, meta?.env ?? t.env);
  if (t.kind === "shell") {
    const { wi, project: p } = shellItem(dashboards[t.env], t.workItemId);
    return { title: wi ? shellTitle(wi, p) : (meta?.title ?? shellTitle(wi, p)), icon: "prompt", project, tip };
  }
  const d = dashboards[t.env];
  const s = dashboardSessions(d).find((x) => x.id === t.session);
  if (!s) return { title: meta?.title ?? "Sessão", tone: "neutral", project, tip };
  const { wi, project: p, gestor, scope } = lookup(d, s);
  const title = gestor ? gestorLabel(scope) : wi ? sessionTitle(wi, p) : (meta?.title ?? "Sessão");
  const live = d?.live_sessions.some((x) => x.id === s.id) && !meta?.ended;
  return { title, tone: live ? tileState(s, wi?.status).tone : "neutral", project, tip };
}

/** Título da aba de uma sessão de item: "Sessão do projeto · <projeto>" no #0 (§6). */
export function sessionTitle(wi: { gh_number: number; title?: string | null }, p?: Project) {
  return projectSessionTitle(wi.gh_number, p) ?? itemTitle(wi, p);
}
