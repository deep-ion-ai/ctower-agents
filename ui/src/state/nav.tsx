import { createContext, useCallback, useContext, useMemo, useReducer, useRef, useState, type ReactNode } from "react";
import type { Shell } from "../protocol/gen";
import { paneProject, type TabProject } from "../terminal/tabmeta";
import { projectKey } from "../project/tabs";
import { useTower } from "./tower";
import type { HubSection, ProjectRef, RegisterIntent } from "../hub/sections";
import type { MaintenanceSection } from "../maintenance/sections";
import type { SettingsScope, SettingsSection, TemplateKey } from "../settings/sections";
import { projectsPlace, type ProjectPage, type ProjectSettingsSection } from "../project/sections";

/**
 * Item do projeto mostrado na aba (detalhe no editor, linha selecionada na
 * árvore). Cada pedido (objeto novo) abre o épico pai na árvore uma vez.
 */
export interface ItemIntent {
  ghNumber: number;
}

export type Screen =
  | { name: "control" }
  | { name: "terminal" }
  | {
      name: "project";
      env?: string;
      projectId?: number;
      /** Visão do hub: o projeto pela chave natural (vale com o hub no ar). */
      owner?: string;
      repo?: string;
      /** Item aberto nesta aba (aba própria por item: `itemTab` em project/tabs.ts). */
      item?: ItemIntent;
      /**
       * Aba do projeto (#17): Visão geral, Itens, Sessões ou Configurações.
       * Sem ela, Itens: é o que o endereço sem `view` mostrava antes (árvore
       * e quadro), e os links de fora (Sala de controle, Hub, paleta, PWA)
       * seguem caindo no mesmo lugar. Com `item`, vale Itens. Monte com
       * `projectAt` (project/sections.ts), que tira `view` em Itens.
       */
      view?: ProjectPage;
      /** Seção das Configurações do projeto (só com `view: "settings"`); sem ela, Geral. */
      section?: ProjectSettingsSection;
      /**
       * Pedido de atribuição (#18, o `assign` do antigo Hub › Projetos):
       * com `view: "settings"` e `section: "envs"`, abre o formulário de
       * atribuição do projeto. Atendido, sai do endereço; a web (só
       * leitura) ignora. Monte com `projectsPlace` (project/sections.ts).
       */
      assign?: true;
      /**
       * Pedido de cadastro (#18), só no índice (sem chave): abre o
       * `NewProjectDialog` com a URL (ex.: a partir da Quarentena) ou vazio
       * (o "+" da Sala de controle, a paleta). Atendido, sai do endereço.
       */
      register?: RegisterIntent;
      /**
       * Aviso de uma ação que terminou no índice (sem chave): a exclusão do
       * projeto, com o snapshot anterior (#18). Mostrado uma vez; sai do
       * endereço como o `register`.
       */
      notice?: string;
    }
  /** Recuperação (#318): a sessão interrompida mostrada (sem ela, a primeira). */
  | { name: "recovery"; env?: string; session?: string }
  /**
   * Hub (`cthubd`), por seção (#313): sem `section`, a Visão geral. Cada
   * seção tem aba própria (`hubTab` em hub/sections.ts).
   */
  | {
      name: "hub";
      section?: HubSection;
      /**
       * `register`, `project` e `assign` são do antigo Hub › Projetos e
       * atribuições (#314, #7). Desde a #18 a seção não existe mais: um
       * endereço com `section: "projects"` (deep link, aba de uma versão
       * antiga, código que o monte à mão) vira o lugar novo em
       * `screenTab`, por `projectsPlace` — ninguém renderiza estes campos.
       */
      register?: RegisterIntent;
      project?: ProjectRef;
      assign?: true;
    }
  /**
   * Manutenção (#335), por seção: sem `section`, a Visão geral. Cada seção
   * tem aba própria (`maintenanceTab`); `env` = o ambiente das seções por
   * ambiente (sem ele, o mesmo critério das Configurações).
   */
  | {
      name: "maintenance";
      section?: MaintenanceSection;
      env?: string;
      /**
       * Filtro por projeto da Limpeza (#22): o plano, as sessões abertas e
       * o que ficou fora do hub só desse projeto. Monte com `maintenanceTab`.
       */
      project?: ProjectRef;
    }
  | { name: "review"; env?: string; workItemId?: number }
  /**
   * Configurações (#315): escopo (Este app ou o ambiente `env`), a seção a
   * mostrar (cada pedido, objeto novo, rola até ela) e, com `template`, o
   * editor de um template em aba própria (`templateTab`).
   */
  | { name: "settings"; scope?: SettingsScope; env?: string; section?: SettingsSection; template?: TemplateKey };

/** Aba de terminal: uma sessão do agente de um ambiente. */
export interface Tab {
  env: string;
  session: string;
}

/**
 * Shell livre no worktree do work item (#423): um PTY do daemon, sem sessão
 * do agente. `shell` é o id do PTY (attach, input, resize e frames).
 */
export interface ShellTab {
  env: string;
  shell: string;
  workItemId: number;
}

/** O que um console da tela Terminal mostra: a sessão do agente ou um shell. */
export type Pane = Tab | ShellTab;

export const isShell = (p: Pane): p is ShellTab => "shell" in p;

/** Id do PTY no daemon: o da sessão ou o do shell. */
export const ptyOf = (p: Pane): string => (isShell(p) ? p.shell : p.session);

/**
 * Aba da área de editor (shell estilo VS Code, #319). Fica aberta ao trocar
 * de tela. `screen` = uma tela (ou um item dela: seção do Hub, item do
 * projeto), com `id` próprio para ter várias abas da mesma tela; `terminal`
 * = uma sessão, mostrada pela tela Terminal.
 */
export type EditorTab =
  | {
      kind: "screen";
      id: string;
      screen: Screen;
      title?: string;
      /**
       * Aba provisória (como a "preview" do VS Code): a de um item aberto por
       * clique, que o próximo item aberto substitui. No máximo uma. As abas
       * principais das telas (`id` = `screen.name`) nunca são provisórias.
       */
      preview?: boolean;
    }
  | ({ kind: "terminal"; id: string } & Tab)
  /** Shell livre (#423), também mostrado pela tela Terminal. */
  | ({ kind: "shell"; id: string } & ShellTab);

/** Abrir (ou focar) uma aba de tela. Sem `id`, a aba é a da tela (`screen.name`). */
export interface OpenTab {
  id?: string;
  title?: string;
  screen: Screen;
}

export const terminalTabId = (t: Tab) => `terminal:${t.env}:${t.session}`;
export const shellTabId = (t: ShellTab) => `shell:${t.env}:${t.shell}`;
export const paneTabId = (p: Pane) => (isShell(p) ? shellTabId(p) : terminalTabId(p));

/**
 * O que a tela Terminal sabe de uma aba de sessão ou de shell (#427), pelo
 * id da aba: guardado aqui porque a faixa de abas e o "Fechar encerradas"
 * valem com a tela Terminal desmontada, e a sessão encerrada sai do
 * dashboard (o título seguia "Sessão").
 */
export interface TabMeta {
  title: string;
  project: TabProject | null;
  /** Nome do ambiente. */
  env: string;
  /** Sessão encerrada sem volta ou shell encerrado (a interrompida recuperável não). */
  ended: boolean;
}

/** Quais abas fechar pelo menu de contexto da aba ou da árvore (#427). */
export type CloseWhich =
  | { how: "others" | "right"; id: string }
  | { how: "ended" }
  | { how: "project"; project: string };

/**
 * Pedido de fechar várias abas com shells vivos ocupados (ou sem saber):
 * uma confirmação só, com quantos estão ocupados.
 */
export interface CloseAsk {
  ids: string[];
  keep?: string;
  busy: number;
  /** O `shell.list` de algum ambiente falhou. */
  unknown: boolean;
}

export interface Nav {
  /** Tela da aba ativa. */
  screen: Screen;
  /** Abre (ou foca) a aba da tela, trocando o endereço dela por `s`. */
  go(s: Screen): void;
  /** Abas da área de editor, na ordem da faixa de abas. */
  editors: EditorTab[];
  /** `id` da aba ativa. */
  current: string;
  openTab(t: OpenTab): void;
  activate(id: string): void;
  /** Fixa (ou desafixa: volta a provisória) a aba de um item. */
  pin(id: string, pinned?: boolean): void;
  /**
   * Fecha a aba; sem nenhuma aberta, volta para a Sala de controle. A aba de
   * shell não fecha direto: ela fica ativa e é marcada em `closing`, e o
   * cabeçalho do shell confere se há comando rodando (`shell.list`) antes de
   * `shell.close`; com `force`, fecha já.
   */
  closeEditor(id: string, force?: boolean): void;
  /** Aba de shell com o fechamento pedido, à espera do cabeçalho dela. */
  closing: string | null;
  cancelClose(): void;
  /** Abas da tela Terminal (sessões e shells, na ordem). */
  tabs: Pane[];
  /** Sessão (ou shell) mostrada pela tela Terminal. */
  active: Pane | null;
  /** Abre (ou foca) a aba da sessão (ou do shell) e vai para o terminal. */
  openTerminal(t: Pane): void;
  /**
   * Troca a sessão `old` pela `next` na mesma aba (Retomar e Reiniciar,
   * #424): mesma posição na faixa, mesma coluna da divisão e, se a antiga
   * era a aba ativa, a nova fica ativa. Sem a aba antiga, abre a nova.
   */
  replaceTerminal(old: Tab, next: Tab): void;
  selectTab(t: Pane): void;
  closeTab(t: Pane): void;
  /**
   * Sessão (ou shell) mostrada à direita da ativa ("Dividir à direita",
   * #316); `null` = sem divisão. Ativar a da direita troca os lados.
   */
  split: Pane | null;
  setSplit(t: Pane | null): void;
  /** O que a tela Terminal publicou de cada aba de sessão ou shell (#427). */
  meta: Record<string, TabMeta>;
  /** Junta ao `meta` (só muda o estado se algo mudou). */
  setMeta(m: Record<string, TabMeta>): void;
  /**
   * Fecha várias abas (Fechar outras, à direita, encerradas, do projeto):
   * as sessões do agente seguem vivas; os shells vivos recebem
   * `shell.close`, com uma confirmação única (`closeAsk`) se algum tem um
   * comando rodando. Com `keep`, essa aba fica ativa se a ativa fechou.
   */
  closeTabs(ids: string[], keep?: string): void;
  closeAsk: CloseAsk | null;
  confirmCloseTabs(): void;
  cancelCloseTabs(): void;
  /** Grupos recolhidos da árvore do Terminal (sobrevivem a sair da tela). */
  collapsed: string[];
  setCollapsed(keys: string[]): void;
}

const same = (a: Pane, b: Pane) => a.env === b.env && ptyOf(a) === ptyOf(b);

const HOME: Screen = { name: "control" };

export interface NavState {
  editors: EditorTab[];
  current: string;
  active: Pane | null;
  /** Sessão no painel da direita (só vale se for outra aba de sessão aberta). */
  split: Pane | null;
  /** Aba de shell com o fechamento pedido (`Nav.closing`). */
  closing: string | null;
}

export type NavAction =
  | { type: "open"; tab: OpenTab }
  /**
   * `groups`: o projeto (chave) de cada aba de sessão ou shell, a nova
   * inclusa; a nova entra logo depois da última do mesmo projeto (#427).
   */
  | { type: "terminal"; tab: Pane; groups?: Record<string, string> }
  /** Fecha estas abas sem a confirmação do shell (#427); `keep` fica ativa se a ativa fechou. */
  | { type: "closeMany"; ids: string[]; keep?: string }
  | { type: "replace"; old: Tab; tab: Tab }
  | { type: "activate"; id: string }
  | { type: "pin"; id: string; pinned: boolean }
  | { type: "close"; id: string; force?: boolean }
  | { type: "cancelClose" }
  | { type: "split"; tab: Pane | null };

type ScreenTab = Extract<EditorTab, { kind: "screen" }>;

/** O endereço de projeto tem chave (`owner/repo` ou `env` + `projectId`)? Sem ela, é o índice (#16). */
export const hasProjectKey = (s: Extract<Screen, { name: "project" }>): boolean =>
  !!(s.owner && s.repo) || (s.env !== undefined && s.projectId !== undefined);

/**
 * Id da aba de uma tela aberta sem `id`: o nome da tela, menos no projeto
 * com chave (#16), que tem aba própria (`project:<chave>`, a mesma base das
 * abas de item em project/tabs.ts). Assim a aba "project" (a da barra de
 * atividades) fica com o índice, e abrir um projeto (pelo índice, pela
 * paleta, pela Sala de controle...) não troca o endereço dela: o último
 * projeto aberto segue na sua aba ao voltar ao índice.
 */
export function mainTabId(s: Screen): string {
  if (s.name === "project" && hasProjectKey(s)) return `project:${projectKey(s)}`;
  return s.name;
}

function screenTab(t: OpenTab): ScreenTab {
  // Antigo Hub › Projetos e atribuições (#18): vira o projeto (Configurações)
  // ou o índice de Projetos, já com o `id` do lugar novo. Aqui, e não num
  // redirecionamento na tela do Hub, porque todo caminho passa por aqui
  // (abrir, `go`, a aba inicial) e a aba nasce certa: sem uma aba
  // "hub:projects" que pisca e fica para trás, e sem `id` velho (o
  // `nav.current` de quem trocava só o endereço) prendendo o projeto na aba
  // de outra tela.
  if (t.screen.name === "hub" && t.screen.section === "projects") {
    const { project, assign, register } = t.screen;
    const screen = projectsPlace({ project, assign, register });
    return { kind: "screen", id: mainTabId(screen), screen };
  }
  return { kind: "screen", id: t.id ?? mainTabId(t.screen), screen: t.screen, title: t.title };
}

/**
 * Aba principal de uma tela (a da barra de atividades, ou a de um projeto):
 * nunca provisória nem fixável. A do projeto não é provisória porque o item
 * aberto na árvore dela (aba provisória) a substituiria.
 */
export const isMainTab = (e: EditorTab): boolean => e.kind === "screen" && e.id === mainTabId(e.screen);

/** Aba que pode ser fixada: a de um item (seção, item do projeto, template...). */
export const isPinnable = (e: EditorTab): boolean => e.kind === "screen" && !isMainTab(e);

const isPreview = (e: EditorTab): boolean => e.kind === "screen" && !!e.preview;

export function initialNav(screen: Screen): NavState {
  const tab = screenTab({ screen });
  return { editors: [tab], current: tab.id, active: null, split: null, closing: null };
}

/** Sessão ou shell de uma aba (as de tela não têm). */
export function paneOf(e: EditorTab): Pane | null {
  if (e.kind === "terminal") return { env: e.env, session: e.session };
  if (e.kind === "shell") return { env: e.env, shell: e.shell, workItemId: e.workItemId };
  return null;
}

const terminals = (editors: EditorTab[]): Pane[] => editors.flatMap((e) => paneOf(e) ?? []);

/**
 * A divisão depois de a sessão `next` ficar ativa: se ela era a da direita,
 * os lados trocam (a ativa anterior vai para a direita).
 */
function swapSplit(s: NavState, next: Pane | null): Pane | null {
  if (!s.split || !next || !same(s.split, next)) return s.split;
  return s.active && !same(s.active, next) ? s.active : null;
}

/** Transições das abas (função pura, testada à parte). */
export function navReducer(s: NavState, a: NavAction): NavState {
  switch (a.type) {
    case "open": {
      // A tela Terminal com sessões abertas é a aba da sessão ativa.
      if (a.tab.screen.name === "terminal" && !a.tab.id) {
        const all = terminals(s.editors);
        const t = (s.active && all.find((x) => same(x, s.active!))) ?? all[all.length - 1];
        if (t) return { ...s, current: paneTabId(t), active: t, split: swapSplit(s, t) };
      }
      const tab = screenTab(a.tab);
      const at = s.editors.findIndex((e) => e.id === tab.id);
      if (at >= 0) {
        // Já aberta (fixada ou não): ativa, trocando o endereço e mantendo o estado de fixada.
        const prev = s.editors[at];
        const next = isPreview(prev) ? { ...tab, preview: true } : tab;
        return { ...s, editors: s.editors.map((e, i) => (i === at ? next : e)), current: tab.id };
      }
      if (isMainTab(tab)) return { ...s, editors: [...s.editors, tab], current: tab.id };
      // Item novo: nasce provisório, no lugar da provisória que houver.
      const fresh: EditorTab = { ...tab, preview: true };
      const old = s.editors.findIndex(isPreview);
      const editors = old >= 0 ? s.editors.map((e, i) => (i === old ? fresh : e)) : [...s.editors, fresh];
      return { ...s, editors, current: tab.id };
    }
    case "pin": {
      const e = s.editors.find((x) => x.id === a.id);
      if (!e || e.kind !== "screen" || !isPinnable(e)) return s;
      if (a.pinned) {
        return e.preview ? { ...s, editors: s.editors.map((x) => (x.id === a.id ? { ...e, preview: false } : x)) } : s;
      }
      if (e.preview) return s;
      // Desafixar: ela vira a provisória (a que havia antes sai, para seguir só uma).
      const other = s.editors.find((x) => isPreview(x) && x.id !== a.id);
      const base = other ? navReducer(s, { type: "close", id: other.id }) : s;
      return { ...base, editors: base.editors.map((x) => (x.id === a.id ? { ...e, preview: true } : x)) };
    }
    case "terminal": {
      const id = paneTabId(a.tab);
      const tab: EditorTab = isShell(a.tab) ? { kind: "shell", id, ...a.tab } : { kind: "terminal", id, ...a.tab };
      let editors = s.editors;
      if (!editors.some((e) => e.id === id)) {
        // A aba vazia da tela Terminal dá lugar à primeira sessão.
        const empty = editors.findIndex((e) => e.kind === "screen" && e.id === "terminal");
        // Abas do mesmo projeto lado a lado: depois da última dele, ou no fim.
        const g = a.groups?.[id];
        let at = -1;
        if (g) editors.forEach((e, i) => a.groups![e.id] === g && (at = i));
        if (empty >= 0) editors = editors.map((e, i) => (i === empty ? tab : e));
        else if (at >= 0) editors = [...editors.slice(0, at + 1), tab, ...editors.slice(at + 1)];
        else editors = [...editors, tab];
      }
      return { ...s, editors, current: id, active: a.tab, split: swapSplit(s, a.tab) };
    }
    case "replace": {
      const oldId = terminalTabId(a.old);
      const id = terminalTabId(a.tab);
      const at = s.editors.findIndex((e) => e.id === oldId);
      if (at < 0) return navReducer(s, { type: "terminal", tab: a.tab });
      // A nova já tinha aba (não deveria): a antiga fecha e a nova fica ativa.
      if (s.editors.some((e) => e.id === id)) {
        return navReducer(navReducer(s, { type: "close", id: oldId }), { type: "terminal", tab: a.tab });
      }
      const tab: EditorTab = { kind: "terminal", id, ...a.tab };
      return {
        ...s,
        editors: s.editors.map((e, i) => (i === at ? tab : e)),
        current: s.current === oldId ? id : s.current,
        active: s.active && same(s.active, a.old) ? a.tab : s.active,
        split: s.split && same(s.split, a.old) ? a.tab : s.split,
      };
    }
    case "activate": {
      const e = s.editors.find((x) => x.id === a.id);
      if (!e) return s;
      const t = paneOf(e);
      if (!t) return { ...s, current: e.id };
      return { ...s, current: e.id, active: t, split: swapSplit(s, t) };
    }
    case "close": {
      const i = s.editors.findIndex((x) => x.id === a.id);
      if (i < 0) return s;
      const closed = s.editors[i];
      // Shell: fechar encerra o processo; quem confere e confirma é o cabeçalho dele.
      if (closed.kind === "shell" && !a.force) return { ...navReducer(s, { type: "activate", id: a.id }), closing: a.id };
      const closedPane = paneOf(closed);
      let editors = s.editors.filter((x) => x.id !== a.id);
      if (editors.length === 0) editors = [screenTab({ screen: HOME })];
      let active = s.active;
      if (closedPane && active && same(active, closedPane)) {
        const rest = terminals(editors);
        active = rest[rest.length - 1] ?? null;
      }
      let current = s.current;
      if (current === closed.id) {
        // A vizinha da esquerda (ou a nova primeira) fica ativa.
        const next = editors[Math.max(0, Math.min(i - 1, editors.length - 1))];
        current = next.id;
        active = paneOf(next) ?? active;
      }
      // A sessão fechada sai da divisão; a que ficou ativa também (não divide consigo).
      let split = s.split && closedPane && same(s.split, closedPane) ? null : s.split;
      if (split && active && same(split, active)) split = null;
      return { editors, current, active, split, closing: s.closing === a.id ? null : s.closing };
    }
    case "closeMany": {
      let n = s;
      for (const id of a.ids) n = navReducer(n, { type: "close", id, force: true });
      if (a.keep && a.ids.includes(s.current) && n.editors.some((e) => e.id === a.keep))
        n = navReducer(n, { type: "activate", id: a.keep });
      return n;
    }
    case "cancelClose": {
      return s.closing ? { ...s, closing: null } : s;
    }
    case "split": {
      // Só divide com outra sessão aberta.
      const t = a.tab;
      const ok = t && (!s.active || !same(t, s.active)) && terminals(s.editors).some((x) => same(x, t));
      return { ...s, split: ok ? t : null };
    }
  }
}

/**
 * Ids das abas a fechar (#427): "outras" = todas menos a `id`; "à direita" =
 * as depois dela; "encerradas" = sessões encerradas sem volta e shells
 * encerrados, pelo `meta` (a interrompida recuperável fica); "do projeto" =
 * as de sessão e de shell do projeto (chave do grupo da árvore).
 */
export function closeTargets(editors: EditorTab[], which: CloseWhich, meta: Record<string, TabMeta>): string[] {
  switch (which.how) {
    case "others":
      return editors.some((e) => e.id === which.id) ? editors.filter((e) => e.id !== which.id).map((e) => e.id) : [];
    case "right": {
      const i = editors.findIndex((e) => e.id === which.id);
      return i < 0 ? [] : editors.slice(i + 1).map((e) => e.id);
    }
    case "ended":
      return editors.filter((e) => e.kind !== "screen" && meta[e.id]?.ended).map((e) => e.id);
    case "project":
      return editors.filter((e) => e.kind !== "screen" && meta[e.id]?.project?.key === which.project).map((e) => e.id);
  }
}

const sameMeta = (a: TabMeta | undefined, b: TabMeta) =>
  !!a &&
  a.title === b.title &&
  a.env === b.env &&
  a.ended === b.ended &&
  a.project?.key === b.project?.key &&
  a.project?.name === b.project?.name;

/** Tela mostrada pela aba. */
export function tabScreen(e: EditorTab | undefined): Screen {
  if (!e) return HOME;
  return e.kind === "screen" ? e.screen : { name: "terminal" };
}

/** Shells ainda vivos entre as abas `ids` (os encerrados fecham sem `shell.close`). */
function liveShells(editors: EditorTab[], meta: Record<string, TabMeta>, ids: string[]): ShellTab[] {
  return editors.flatMap((e) =>
    e.kind === "shell" && ids.includes(e.id) && !meta[e.id]?.ended
      ? [{ env: e.env, shell: e.shell, workItemId: e.workItemId }]
      : [],
  );
}

const Ctx = createContext<Nav | null>(null);

export function useNav(): Nav {
  const n = useContext(Ctx);
  if (!n) throw new Error("useNav fora do NavProvider");
  return n;
}

export function NavProvider({
  initial = HOME,
  children,
}: {
  initial?: Screen;
  children: ReactNode;
}) {
  const [state, dispatch] = useReducer(navReducer, initial, initialNav);
  const [collapsed, setCollapsed] = useState<string[]>([]);
  const { editors, current, active, split, closing } = state;
  const { api, dashboards } = useTower();
  const [meta, setMetaState] = useState<Record<string, TabMeta>>({});
  const [closeAsk, setCloseAsk] = useState<CloseAsk | null>(null);
  // Para os callbacks estáveis (abrir e fechar) lerem o estado de agora.
  const now = useRef({ editors, meta, dashboards, closeAsk });
  now.current = { editors, meta, dashboards, closeAsk };

  const setMeta = useCallback(
    (m: Record<string, TabMeta>) =>
      setMetaState((all) => (Object.entries(m).every(([k, v]) => sameMeta(all[k], v)) ? all : { ...all, ...m })),
    [],
  );
  /** Projeto de cada aba de sessão ou shell: o publicado pela tela Terminal ou o do dashboard. */
  const groupOf = useCallback((id: string, p: Pane) => {
    const { meta: m, dashboards: d } = now.current;
    return m[id]?.project?.key ?? paneProject(d[p.env], p)?.key;
  }, []);

  /**
   * Endereço local de projeto (`env` + `projectId`) ganha a chave natural
   * (`owner/repo`) do dashboard: a aba do projeto (`mainTabId`) é a mesma
   * por qualquer caminho, e a árvore volta a ela pela trilha do item (o
   * `addr` da visão leva `owner/repo`), sem abrir uma segunda.
   */
  const withKey = useCallback((s: Screen): Screen => {
    if (s.name !== "project" || (s.owner && s.repo) || s.env === undefined || s.projectId === undefined) return s;
    const p = now.current.dashboards[s.env]?.projects.find((x) => x.project.id === s.projectId)?.project;
    return p ? { ...s, owner: p.gh_owner, repo: p.gh_repo } : s;
  }, []);
  const go = useCallback((s: Screen) => dispatch({ type: "open", tab: { screen: withKey(s) } }), [withKey]);
  const openTab = useCallback(
    (tab: OpenTab) => dispatch({ type: "open", tab: tab.id ? tab : { ...tab, screen: withKey(tab.screen) } }),
    [withKey],
  );
  const activate = useCallback((id: string) => dispatch({ type: "activate", id }), []);
  const closeEditor = useCallback((id: string, force?: boolean) => dispatch({ type: "close", id, force }), []);
  const cancelClose = useCallback(() => dispatch({ type: "cancelClose" }), []);
  const pin = useCallback((id: string, pinned = true) => dispatch({ type: "pin", id, pinned }), []);
  const openTerminal = useCallback(
    (tab: Pane) => {
      const groups: Record<string, string> = {};
      for (const e of now.current.editors) {
        const p = paneOf(e);
        const g = p && groupOf(e.id, p);
        if (g) groups[e.id] = g;
      }
      const g = groupOf(paneTabId(tab), tab);
      if (g) groups[paneTabId(tab)] = g;
      dispatch({ type: "terminal", tab, groups });
    },
    [groupOf],
  );
  // Fechar várias (#427): os shells vivos entre elas passam pelo `shell.list`.
  const shutdown = useCallback(
    async (ids: string[], keep?: string) => {
      const live = liveShells(now.current.editors, now.current.meta, ids);
      await Promise.all(live.map((t) => api.call(t.env, "shell.close", { shell_id: t.shell }).catch(() => {})));
      dispatch({ type: "closeMany", ids, keep });
    },
    [api],
  );
  const closeTabs = useCallback(
    async (ids: string[], keep?: string) => {
      if (ids.length === 0) return;
      const live = liveShells(now.current.editors, now.current.meta, ids);
      let busy = 0;
      let unknown = false;
      for (const env of new Set(live.map((t) => t.env))) {
        const list = await api.call<Shell[]>(env, "shell.list").catch(() => null);
        if (!list) unknown = true;
        else busy += live.filter((t) => t.env === env && list.some((x) => x.id === t.shell && x.busy)).length;
      }
      if (busy > 0 || unknown) setCloseAsk({ ids, keep, busy, unknown });
      else await shutdown(ids, keep);
    },
    [api, shutdown],
  );
  const confirmCloseTabs = useCallback(() => {
    const ask = now.current.closeAsk;
    setCloseAsk(null);
    if (ask) void shutdown(ask.ids, ask.keep);
  }, [shutdown]);
  const cancelCloseTabs = useCallback(() => setCloseAsk(null), []);
  const replaceTerminal = useCallback((old: Tab, tab: Tab) => dispatch({ type: "replace", old, tab }), []);
  const closeTab = useCallback((t: Pane) => dispatch({ type: "close", id: paneTabId(t) }), []);
  const setSplit = useCallback((tab: Pane | null) => dispatch({ type: "split", tab }), []);

  const value = useMemo<Nav>(() => {
    const tabs = terminals(editors);
    return {
      screen: tabScreen(editors.find((e) => e.id === current)),
      go,
      editors,
      current,
      openTab,
      activate,
      pin,
      closeEditor,
      closing,
      cancelClose,
      tabs,
      active,
      openTerminal,
      replaceTerminal,
      selectTab: openTerminal,
      closeTab,
      split,
      setSplit,
      meta,
      setMeta,
      closeTabs: (ids, keep) => void closeTabs(ids, keep),
      closeAsk,
      confirmCloseTabs,
      cancelCloseTabs,
      collapsed,
      setCollapsed,
    };
  }, [
    meta,
    setMeta,
    closeTabs,
    closeAsk,
    confirmCloseTabs,
    cancelCloseTabs,
    editors,
    current,
    active,
    split,
    closing,
    go,
    openTab,
    activate,
    pin,
    closeEditor,
    cancelClose,
    openTerminal,
    replaceTerminal,
    closeTab,
    setSplit,
    collapsed,
  ]);
  return <Ctx.Provider value={value}>{children}</Ctx.Provider>;
}
