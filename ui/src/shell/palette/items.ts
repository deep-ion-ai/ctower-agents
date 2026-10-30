// Itens da paleta de comandos (#312), tirados do `useTower()`. Função pura:
// cada item leva a ação como dado; quem executa é a paleta.
//
// Issues e projetos vêm do dashboard do hub (se no ar) e dos dashboards dos
// ambientes, sem repetir: com o hub fora ou sem ambiente, a paleta segue com
// o que houver.
import type { EnvStatus, EnvView, UiUpdate } from "../../api/types";
import { gestorLabel, lookup } from "../../control-room/attention";
import type { LiveGestor } from "../../hub/HubGestor";
import { STATUS } from "../../project/status";
import type { Dashboard, Session, WorkItemKind, WorkItemStatus } from "../../protocol/gen";
import type { HubView } from "../../state/tower";
import type { OpenTab, Screen } from "../../state/nav";
import { itemTab } from "../../project/tabs";
import { itemTitle, PROJECT_ITEM, trackersText, type ProviderProject } from "../../util/item";
import { liveGestorLabel } from "../../hub/HubGestor";
import { PLACES, type Place } from "../places";
import { maintenanceTab } from "../../maintenance/sections";
import type { StartTarget } from "../start";
import type { SessionAction, SessionActionId } from "../../terminal/actions";
import { placeOnWeb, webPlaceLabel } from "../../web/mode";

export type PaletteKind = "session" | "issue" | "epic" | "project" | "place" | "command";

export type PaletteAction =
  | { type: "screen"; screen: Screen }
  | { type: "place"; place: Place }
  /** Aba própria (ex.: um item do projeto, `itemTab`). */
  | { type: "tab"; tab: OpenTab }
  | { type: "terminal"; env: string; session: string }
  | { type: "start"; target: StartTarget }
  /** Shell novo no worktree do work item (#423). */
  | { type: "shell"; env: string; workItemId: number }
  /** Ação da sessão ativa do Terminal (#424), executada pelo cabeçalho dela. */
  | { type: "session-action"; id: SessionActionId }
  | { type: "sync" };

export interface PaletteItem {
  /** Estável entre aberturas: vai para os recentes. */
  id: string;
  kind: PaletteKind;
  label: string;
  /** Linha de apoio (projeto, ambiente, estado); a busca também olha aqui. */
  detail?: string;
  keywords?: string;
  /** Número da issue: busca exata ("297", "#297") vai para o topo. */
  num?: number;
  /** Issue concluída: fica depois das abertas. */
  closed?: boolean;
  /** `false` = só aparece buscando (listas longas fora da lista inicial). */
  browse?: boolean;
  action: PaletteAction;
}

export interface PaletteSources {
  envs: EnvView[];
  status: Record<string, EnvStatus>;
  dashboards: Record<string, Dashboard>;
  hub: Pick<HubView, "up" | "dashboard" | "connected">;
  uiUpdate: UiUpdate | null;
  liveGestor: LiveGestor[];
  /** Modo web (#201): sem os lugares e comandos exclusivos do desktop. */
  web?: boolean;
  /** Work item da aba ativa do Terminal onde abrir um shell (#423; só no desktop). */
  shellTarget?: { env: string; workItemId: number } | null;
  /** Ações da sessão ativa do Terminal (#424): "Sessão: Retomar", "Sessão: Reiniciar"… */
  sessionActions?: { title: string; actions: SessionAction[] } | null;
}

export const KIND_LABEL: Record<PaletteKind, string> = {
  session: "Sessão",
  issue: "Issue",
  epic: "Épico",
  project: "Projeto",
  place: "Tela",
  command: "Comando",
};

const key = (owner: string, repo: string) => `${owner}/${repo}`.toLowerCase();

interface ProjectRef {
  owner: string;
  repo: string;
  name: string;
  /** Rastreador e forja: chave do Jira no rótulo da issue (#352). */
  provider: ProviderProject;
  env?: string;
  projectId?: number;
}

interface IssueRef {
  project: ProjectRef;
  gh_number: number;
  title: string | null;
  kind: WorkItemKind;
  status: WorkItemStatus;
  closed: boolean;
}

/** Projetos e issues do hub e dos ambientes, sem repetir (o hub primeiro). */
function catalog(src: PaletteSources): { projects: ProjectRef[]; issues: IssueRef[] } {
  const projects = new Map<string, ProjectRef>();
  const issues = new Map<string, IssueRef>();
  const add = (p: ProjectRef, items: { gh_number: number; title: string | null; kind: WorkItemKind; status: WorkItemStatus; closed_at: number | null }[]) => {
    const k = key(p.owner, p.repo);
    const known = projects.get(k);
    // O primeiro ambiente com o projeto completa a referência do hub (vale com o hub fora).
    if (known) {
      if (known.env === undefined && p.env !== undefined) Object.assign(known, { env: p.env, projectId: p.projectId });
    } else projects.set(k, p);
    const ref = projects.get(k)!;
    for (const w of items) {
      if (w.gh_number === PROJECT_ITEM) continue;
      const ik = `${k}#${w.gh_number}`;
      if (issues.has(ik)) continue;
      issues.set(ik, {
        project: ref,
        gh_number: w.gh_number,
        title: w.title,
        kind: w.kind,
        status: w.status,
        closed: w.status === "concluido" || w.closed_at !== null,
      });
    }
  };
  if (src.hub.up && src.hub.dashboard) {
    for (const p of src.hub.dashboard.projects) {
      add({ owner: p.project.gh_owner, repo: p.project.gh_repo, name: p.project.name, provider: p.project }, p.work_items);
    }
  }
  for (const e of src.envs) {
    for (const p of src.dashboards[e.id]?.projects ?? []) {
      if (p.project.kind === "gestor") continue;
      add(
        {
          owner: p.project.gh_owner,
          repo: p.project.gh_repo,
          name: p.project.name,
          provider: p.project,
          env: e.id,
          projectId: p.project.id,
        },
        p.work_items,
      );
    }
  }
  return { projects: [...projects.values()], issues: [...issues.values()] };
}

const projectScreen = (p: ProjectRef): Extract<Screen, { name: "project" }> => ({
  name: "project",
  env: p.env,
  projectId: p.projectId,
  owner: p.owner,
  repo: p.repo,
});

const SESSION_STATE: Partial<Record<Session["state"], string>> = {
  starting: "iniciando",
  running: "em execução",
  idle: "ociosa",
  interrupted: "interrompida",
};

function sessions(src: PaletteSources): PaletteItem[] {
  return src.envs.flatMap((e) => {
    const d = src.dashboards[e.id];
    if (!d) return [];
    const one = (s: Session, interrupted: boolean): PaletteItem => {
      const { wi, project, gestor, scope } = lookup(d, s);
      const label = gestor ? gestorLabel(scope) : wi ? itemTitle(wi, project) : "Sessão";
      const state = wi?.status === "aguardando_voce" ? "aguardando você" : (SESSION_STATE[s.state] ?? s.state);
      return {
        id: `session:${e.id}:${s.id}`,
        kind: "session",
        label,
        detail: [project?.name, e.name, interrupted ? "interrompida" : state].filter(Boolean).join(" · "),
        keywords: "sessão",
        num: wi && wi.gh_number !== PROJECT_ITEM ? wi.gh_number : undefined,
        // A interrompida se resolve na Recuperação (retomar ou arquivar).
        action: interrupted
          ? { type: "screen", screen: { name: "recovery", env: e.id, session: s.id } }
          : { type: "terminal", env: e.id, session: s.id },
      };
    };
    return [...d.live_sessions.map((s) => one(s, false)), ...d.interrupted_sessions.map((s) => one(s, true))];
  });
}

function commands(src: PaletteSources, interrupted: number, projects: ProjectRef[]): PaletteItem[] {
  if (src.web) {
    // Sem sync nem atualizador; iniciar sessão fica na tela Projeto (só com o ambiente online): abrir o gestor vivo e a Recuperação.
    return [
      ...src.liveGestor.map((l) => ({
        id: `cmd:gestor:${l.env}:${l.session}`,
        kind: "command" as const,
        label: liveGestorLabel(l),
        keywords: "gestor de projetos",
        action: { type: "terminal" as const, env: l.env, session: l.session },
      })),
      {
        id: "cmd:resume",
        kind: "command",
        label: "Ver interrompidas",
        detail: interrupted > 0 ? `${interrupted} na Recuperação` : "Nenhuma interrompida",
        keywords: "recuperação retomar",
        action: { type: "screen", screen: { name: "recovery" } },
      },
    ];
  }
  const out: PaletteItem[] = [
    {
      id: "cmd:sync",
      kind: "command",
      label: "Sincronizar agora",
      detail: `${trackersText(projects.map((p) => p.provider))}, nos ambientes conectados`,
      keywords: "sync github jira",
      action: { type: "sync" },
    },
  ];
  // Gestor vivo: abrir a sessão; senão, a tela Hub (onde ele começa).
  if (src.liveGestor.length > 0) {
    for (const l of src.liveGestor) {
      out.push({
        id: `cmd:gestor:${l.env}:${l.session}`,
        kind: "command",
        label: liveGestorLabel(l),
        keywords: "gestor de projetos",
        action: { type: "terminal", env: l.env, session: l.session },
      });
    }
  } else {
    out.push({
      id: "cmd:gestor",
      kind: "command",
      label: "Abrir gestor",
      detail: src.hub.connected ? "Iniciar o gestor de projetos na tela Hub" : "Gestor de projetos (precisa do hub)",
      keywords: "gestor de projetos",
      action: { type: "screen", screen: { name: "hub" } },
    });
  }
  out.push(
    {
      id: "cmd:update",
      kind: "command",
      label: "Atualizar app",
      detail: src.uiUpdate?.available
        ? `Versão ${src.uiUpdate.available.version} disponível`
        : "Manutenção › Atualizações › Este app",
      keywords: "atualização versão",
      action: { type: "tab", tab: maintenanceTab("app-updates") },
    },
    {
      id: "cmd:resume",
      kind: "command",
      label: "Retomar interrompidas",
      detail: interrupted > 0 ? `${interrupted} na Recuperação` : "Nenhuma interrompida",
      keywords: "recuperação",
      action: { type: "screen", screen: { name: "recovery" } },
    },
  );
  // Ações da sessão ativa (#424): só as que dá para executar agora.
  for (const a of src.sessionActions?.actions ?? []) {
    if (a.disabled) continue;
    out.push({
      id: `cmd:session:${a.id}`,
      kind: "command",
      label: `Sessão: ${a.label}`,
      detail: src.sessionActions!.title,
      keywords: "sessão ativa",
      action: { type: "session-action", id: a.id },
    });
  }
  // Shell livre (#423): só no desktop (`shell.*` fica fora da allowlist web).
  const t = src.shellTarget;
  if (t) {
    out.push({
      id: "cmd:shell-new",
      kind: "command",
      label: "Shell: novo no worktree da sessão ativa",
      keywords: "terminal zsh bash prompt",
      action: { type: "shell", env: t.env, workItemId: t.workItemId },
    });
  }
  return out;
}

export function paletteItems(src: PaletteSources): PaletteItem[] {
  const { projects, issues } = catalog(src);
  const interrupted = src.envs.reduce((n, e) => n + (src.dashboards[e.id]?.interrupted_sessions.length ?? 0), 0);
  // "Acesso web" só tem conteúdo na web (a sessão deste navegador).
  const shown = src.web ? PLACES.filter(placeOnWeb) : PLACES.filter((p) => p.id !== "settings:web-access");
  const places: PaletteItem[] = shown.map((p) => ({
    id: p.id,
    kind: "place",
    label: src.web ? webPlaceLabel(p.label) : p.label,
    keywords: p.keywords,
    action: { type: "place", place: p },
  }));
  const projectItems: PaletteItem[] = projects.map((p) => ({
    id: `project:${key(p.owner, p.repo)}`,
    kind: "project",
    label: p.name,
    detail: `${p.owner}/${p.repo}`,
    action: { type: "screen", screen: projectScreen(p) },
  }));
  const issueItems: PaletteItem[] = issues.map((i) => ({
    id: `issue:${key(i.project.owner, i.project.repo)}#${i.gh_number}`,
    kind: i.kind === "epic" ? "epic" : "issue",
    label: itemTitle(i, i.project.provider),
    detail: `${i.project.name} · ${STATUS[i.status]?.text ?? i.status}`,
    keywords: i.kind === "epic" ? "épico" : undefined,
    num: i.gh_number,
    closed: i.closed,
    browse: false,
    action: { type: "tab", tab: itemTab(projectScreen(i.project), i.gh_number, i.title ?? "", i.project.provider) },
  }));
  // "Iniciar sessão: #n" para cada issue aberta (só buscando).
  const starts: PaletteItem[] = issues
    .filter((i) => !i.closed)
    .map((i) => ({
      id: `cmd:start:${key(i.project.owner, i.project.repo)}#${i.gh_number}`,
      kind: "command",
      label: `Iniciar sessão: ${itemTitle(i, i.project.provider)}`,
      detail: i.project.name,
      keywords: "nova sessão começar",
      num: i.gh_number,
      browse: false,
      action: {
        type: "start",
        target: {
          ghNumber: i.gh_number,
          owner: i.project.owner,
          repo: i.project.repo,
          env: i.project.env,
          projectId: i.project.projectId,
        },
      },
    }));
  return [
    ...sessions(src),
    ...places,
    ...commands(src, interrupted, projects),
    ...projectItems,
    ...issueItems,
    ...(src.web ? [] : starts),
  ];
}
