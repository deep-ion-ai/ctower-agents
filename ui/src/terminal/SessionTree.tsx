import { useRef, useState, type KeyboardEvent } from "react";
import { GESTOR_LABEL, gestorLabel } from "../control-room/attention";
import { tileState } from "../control-room/TileGrid";
import { ContextMenu, type MenuItem } from "../shell/ContextMenu";
import { Icon } from "../shell/icons";
import { SidePanel } from "../shell/SidePanel";
import type { EnvView } from "../api/types";
import type { Dashboard, DashboardWorkItem, GestorScope, Project, Session, Shell } from "../protocol/gen";
import { ptyOf, type Pane, type ShellTab, type Tab } from "../state/nav";
import { itemShort, itemTitle, PROJECT_ITEM } from "../util/item";
import type { TermState } from "./adapter";
import { rcOn } from "./remote";
import { shellItem, shellLabel } from "./shells";

export const keyOf = (t: Pane) => `${t.env}|${ptyOf(t)}`;

/** Sessão, work item e projeto de uma aba. */
export interface TabInfo {
  s?: Session;
  wi?: DashboardWorkItem;
  project?: Project;
  /** Sessão do gestor de projetos, com o escopo dela se conhecido. */
  gestor?: boolean;
  scope?: GestorScope;
}

export type InfoOf = (t: Tab) => TabInfo;

/** Shell vivo (#423) na árvore, filho da linha do item. */
export interface TreeShell {
  tab: ShellTab;
  /** "zsh · #87". */
  label: string;
}

export interface TreeSession {
  /**
   * Sessão da linha; `null` = linha do item sem sessão (só para os shells
   * dele, #423).
   */
  tab: Tab | null;
  /** Id da linha (único na árvore). */
  key: string;
  workItemId: number | null;
  title: string;
  branch: string | null;
  ghNumber: number | null;
  epic: boolean;
  /** Épico pai da issue (`parent_number`), com o título se o dashboard o conhece. */
  parent: { number: number; title: string } | null;
  state: { text: string; tone: string };
  /** Remote Control conectado (sessão viva com `rc_session_id`, #197). */
  remote: boolean;
  /** Shells vivos no worktree do item (#423). */
  shells: TreeShell[];
}

export interface TreeGroup {
  key: string;
  name: string;
  sessions: TreeSession[];
}

/**
 * Nó abaixo do projeto: uma sessão, ou um épico com a sessão dele (se houver)
 * e as das sub-issues.
 */
export type TreeNode =
  | { kind: "session"; s: TreeSession }
  | { kind: "epic"; key: string; number: number; title: string; sessions: TreeSession[] };

/**
 * Nível de épico (#365): sessões de sub-issues ficam sob o épico pai, junto
 * da sessão do próprio épico. Épico sem sessão de sub-issue fica solto, como
 * as demais. A ordem é a da primeira sessão de cada nó.
 */
export function nestEpics(groupKey: string, sessions: TreeSession[]): TreeNode[] {
  const parents = new Map<number, string>();
  for (const s of sessions) if (s.parent) parents.set(s.parent.number, s.parent.title);
  const nodes: TreeNode[] = [];
  const epics = new Map<number, Extract<TreeNode, { kind: "epic" }>>();
  for (const s of sessions) {
    const n = s.parent?.number ?? (s.epic && s.ghNumber !== null && parents.has(s.ghNumber) ? s.ghNumber : null);
    if (n === null) {
      nodes.push({ kind: "session", s });
      continue;
    }
    let e = epics.get(n);
    if (!e) {
      e = { kind: "epic", key: `${groupKey}|e${n}`, number: n, title: parents.get(n)!, sessions: [] };
      epics.set(n, e);
      nodes.push(e);
    }
    // A sessão do próprio épico vem primeiro.
    if (s.parent) e.sessions.push(s);
    else e.sessions.unshift(s);
  }
  return nodes;
}

/** Sessões que o dashboard de um ambiente conhece, sem repetir. */
export function dashboardSessions(d: Dashboard | undefined): Session[] {
  const seen = new Set<string>();
  return [...(d?.live_sessions ?? []), ...(d?.interrupted_sessions ?? []), ...(d?.failed_sessions ?? [])].filter(
    (s) => !seen.has(s.id) && !!seen.add(s.id),
  );
}

/**
 * Estado em texto da sessão na árvore: o fim visto pelo terminal vence; senão
 * a sessão viva segue `tileState` (âmbar só quando espera pelo usuário).
 */
export function rowState(s: Session | undefined, wi: DashboardWorkItem | undefined, term?: TermState) {
  if (term?.kind === "ended")
    return term.recoverable
      ? { text: "interrompida · recuperável", tone: "error" }
      : { text: "encerrada", tone: "neutral" };
  if (!s) return { text: "—", tone: "neutral" };
  if (s.state === "interrupted") return { text: "interrompida · recuperável", tone: "error" };
  if (s.state === "exited" || s.state === "archived") return { text: "encerrada", tone: "neutral" };
  return tileState(s, wi?.status);
}

/** Sessão encerrada sem volta: sai da árvore (a interrompida recuperável fica). */
export const isEnded = (st: { text: string }) => st.text === "encerrada";

/** Linhas de sessão (as do item sem sessão, só com shells, não contam). */
export const sessionCount = (rows: TreeSession[]) => rows.filter((r) => r.tab).length;

/**
 * Agrupa as sessões por projeto (`Project` do dashboard). A sessão do gestor
 * vai para o projeto do escopo dela; sem projeto no escopo, para o grupo do
 * gestor. Com mais de um ambiente, o nome do grupo diz o ambiente.
 *
 * Os shells vivos (`shell.list`, #423) entram como filhos da linha da sessão
 * do item; sem sessão do item na árvore, de uma linha do próprio item.
 */
export function groupSessions(
  envs: Pick<EnvView, "id" | "name">[],
  dashboards: Record<string, Dashboard | undefined>,
  tabs: Pane[],
  info: InfoOf,
  states: Record<string, TermState>,
  shells: Record<string, Shell[] | undefined> = {},
): TreeGroup[] {
  const all: Tab[] = envs.flatMap((e) => dashboardSessions(dashboards[e.id]).map((s) => ({ env: e.id, session: s.id })));
  const listed = new Set(all.map(keyOf));
  for (const t of tabs) if (!("shell" in t) && !listed.has(keyOf(t))) all.push(t);

  const groups = new Map<string, TreeGroup>();
  const envName = (id: string) => (envs.length > 1 ? ` · ${envs.find((e) => e.id === id)?.name ?? id}` : "");
  const groupOf = (env: string, p: Project | undefined, gestor = false) => {
    const [key, name] = p
      ? [`${env}|p${p.id}`, p.name]
      : gestor
        ? [`${env}|gestor`, GESTOR_LABEL]
        : [`${env}|outras`, "Sem projeto"];
    const g = groups.get(key) ?? { key, name: name + envName(env), sessions: [] };
    groups.set(key, g);
    return g;
  };
  const parentOf = (env: string, wi: DashboardWorkItem | undefined, project: Project | undefined) => {
    const pn = wi?.parent_number ?? null;
    if (pn === null) return null;
    const epicWi = dashboards[env]?.projects
      .find((x) => x.project.id === wi?.project_id)
      ?.work_items.find((w) => w.gh_number === pn);
    return { number: pn, title: epicWi ? itemTitle(epicWi, project) : `Épico ${itemShort(project, pn)}` };
  };
  // Linha de cada (ambiente, work item), para pendurar os shells.
  const rows = new Map<string, TreeSession>();
  for (const tab of all) {
    const { s, wi, project, gestor, scope } = info(tab);
    const state = rowState(s, wi, states[keyOf(tab)]);
    if (isEnded(state)) continue;
    let p = project;
    if (gestor && scope?.kind === "project")
      p = dashboards[tab.env]?.projects.find(
        (x) => x.project.gh_owner === scope.owner && x.project.gh_repo === scope.repo,
      )?.project;
    const row: TreeSession = {
      tab,
      key: `s:${keyOf(tab)}`,
      workItemId: wi?.id ?? null,
      title: gestor ? gestorLabel(scope) : wi ? itemTitle(wi, project) : `Sessão ${tab.session.slice(0, 8)}`,
      branch: wi?.branch ?? null,
      ghNumber: wi?.gh_number ?? null,
      epic: wi?.kind === "epic",
      parent: parentOf(tab.env, wi, project),
      state,
      remote: rcOn(s),
      shells: [],
    };
    groupOf(tab.env, p, gestor).sessions.push(row);
    if (wi && !rows.has(`${tab.env}|${wi.id}`)) rows.set(`${tab.env}|${wi.id}`, row);
  }
  for (const e of envs) {
    for (const sh of shells[e.id] ?? []) {
      const { wi, project } = shellItem(dashboards[e.id], sh.work_item_id);
      const k = `${e.id}|${sh.work_item_id}`;
      let row = rows.get(k);
      if (!row) {
        row = {
          tab: null,
          key: `i:${k}`,
          workItemId: sh.work_item_id,
          title: wi ? itemTitle(wi, project) : `Item ${sh.work_item_id}`,
          branch: wi?.branch ?? null,
          ghNumber: wi?.gh_number ?? null,
          epic: wi?.kind === "epic",
          parent: parentOf(e.id, wi, project),
          state: { text: "sem sessão", tone: "neutral" },
          remote: false,
          shells: [],
        };
        rows.set(k, row);
        groupOf(e.id, project).sessions.push(row);
      }
      row.shells.push({
        tab: { env: e.id, shell: sh.id, workItemId: sh.work_item_id },
        label: shellLabel(sh, wi, project),
      });
    }
  }
  // A sessão do projeto (#0) fica no topo do grupo, antes das issues (§6).
  for (const g of groups.values()) g.sessions.sort((a, b) => Number(b.ghNumber === PROJECT_ITEM) - Number(a.ghNumber === PROJECT_ITEM));
  return [...groups.values()].sort((a, b) => a.name.localeCompare(b.name));
}

/** Filtro por título, branch ou #issue (com ou sem o `#`). */
export function matches(s: TreeSession, q: string): boolean {
  const f = q.trim().toLowerCase();
  if (!f) return true;
  if (s.title.toLowerCase().includes(f) || s.branch?.toLowerCase().includes(f)) return true;
  const n = f.replace(/^#/, "");
  return s.ghNumber !== null && /^\d+$/.test(n) && String(s.ghNumber) === n;
}

const plural = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`;

/** Item da árvore na ordem de leitura, para o teclado. */
interface FlatItem {
  id: string;
  /** Chave de recolher, nos nós (projeto ou épico). */
  key?: string;
  /** Id do nó pai. */
  parent?: string;
  tab?: Pane;
}

/**
 * Árvore projeto → (épico →) sessões (`role="tree"`) no painel lateral do shell (#316),
 * com foco itinerante: setas movem o foco, → e ← expandem e recolhem,
 * Enter/Espaço abrem a sessão. O cabeçalho tem "Nova sessão" e "Recolher
 * tudo" (ou "Expandir tudo", com tudo recolhido). Com `menu`, o botão
 * direito (ou Shift+F10 / tecla de menu) numa sessão abre o menu de contexto;
 * com `groupMenu`, num grupo de projeto também ("Fechar as abas deste
 * projeto", #427). Com `onCloseEnded`, o cabeçalho tem "Fechar encerradas".
 * Os shells (#423) vêm logo abaixo da linha do item, um nível adentro.
 */
export function SessionTree({
  groups,
  active,
  collapsed,
  onCollapse,
  onSelect,
  onNew,
  menu,
  groupMenu,
  onCloseEnded,
}: {
  groups: TreeGroup[];
  active: Pane | null;
  collapsed: string[];
  onCollapse: (keys: string[]) => void;
  onSelect: (t: Pane) => void;
  onNew: () => void;
  /** Ações do menu de contexto de uma sessão. */
  menu?: (t: Tab) => MenuItem[];
  /** Ações do menu de contexto de um grupo (projeto); vazio = sem menu. */
  groupMenu?: (g: TreeGroup) => MenuItem[];
  /** "Fechar encerradas" no cabeçalho; `null` = nenhuma aba encerrada (desabilitado). */
  onCloseEnded?: (() => void) | null;
}) {
  const [filter, setFilter] = useState("");
  const [ctx, setCtx] = useState<{ x: number; y: number; items: MenuItem[]; label: string; origin: HTMLElement } | null>(
    null,
  );
  const openMenu = (tab: Tab, x: number, y: number, origin: HTMLElement) => {
    const items = menu?.(tab) ?? [];
    if (items.length > 0) setCtx({ x, y, items, label: "Ações da sessão", origin });
  };
  const openGroupMenu = (g: TreeGroup, x: number, y: number, origin: HTMLElement) => {
    const items = groupMenu?.(g) ?? [];
    if (items.length > 0) setCtx({ x, y, items, label: `Ações do projeto ${g.name}`, origin });
  };
  const [focus, setFocus] = useState<string | null>(null);
  const refs = useRef(new Map<string, HTMLLIElement>());
  const filtering = filter.trim() !== "";
  const shown = groups
    .map((g) => {
      const sessions = g.sessions.filter((s) => matches(s, filter));
      return { ...g, sessions, nodes: nestEpics(g.key, sessions) };
    })
    .filter((g) => !filtering || g.sessions.length > 0);
  // Filtrando, os grupos com resultado ficam abertos.
  const isOpen = (k: string) => filtering || !collapsed.includes(k);
  const toggle = (k: string) => onCollapse(isOpen(k) ? [...collapsed, k] : collapsed.filter((c) => c !== k));

  // Itens visíveis na ordem da árvore, para o teclado.
  const items: FlatItem[] = shown.flatMap((g) => {
    const gid = `g:${g.key}`;
    const out: FlatItem[] = [{ id: gid, key: g.key }];
    if (!isOpen(g.key)) return out;
    // A linha e, logo abaixo, os shells dela.
    const row = (s: TreeSession, parent: string) => {
      out.push({ id: s.key, parent, tab: s.tab ?? undefined });
      for (const sh of s.shells) out.push({ id: `s:${keyOf(sh.tab)}`, parent: s.key, tab: sh.tab });
    };
    for (const n of g.nodes) {
      if (n.kind === "session") {
        row(n.s, gid);
        continue;
      }
      const eid = `e:${n.key}`;
      out.push({ id: eid, key: n.key, parent: gid });
      if (isOpen(n.key)) for (const s of n.sessions) row(s, eid);
    }
    return out;
  });
  const activeId = active ? `s:${keyOf(active)}` : null;
  const current = items.find((i) => i.id === focus) ?? items.find((i) => i.id === activeId) ?? items[0];

  const move = (id: string | undefined) => {
    if (!id) return;
    setFocus(id);
    refs.current.get(id)?.focus();
  };

  const onKeyDown = (e: KeyboardEvent) => {
    if (!current) return;
    const i = items.indexOf(current);
    const node = current.key;
    if (e.key === "ContextMenu" || (e.key === "F10" && e.shiftKey)) {
      const el = refs.current.get(current.id);
      const r = el?.getBoundingClientRect();
      const g = current.id.startsWith("g:") ? groups.find((x) => `g:${x.key}` === current.id) : undefined;
      if (el && r && current.tab && !("shell" in current.tab)) openMenu(current.tab, r.left + 16, r.bottom, el);
      else if (el && r && g) openGroupMenu(g, r.left + 16, r.top + 22, el);
      e.preventDefault();
      return;
    }
    switch (e.key) {
      case "ArrowDown":
        move(items[i + 1]?.id);
        break;
      case "ArrowUp":
        move(items[i - 1]?.id);
        break;
      case "Home":
        move(items[0]?.id);
        break;
      case "End":
        move(items[items.length - 1]?.id);
        break;
      case "ArrowRight":
        if (!node) return;
        if (!isOpen(node)) toggle(node);
        else if (items[i + 1]?.parent === current.id) move(items[i + 1].id);
        break;
      case "ArrowLeft":
        if (node && isOpen(node) && !filtering) toggle(node);
        else if (current.parent) move(current.parent);
        break;
      case "Enter":
      case " ":
        if (current.tab) onSelect(current.tab);
        else if (node) toggle(node);
        break;
      default:
        return;
    }
    e.preventDefault();
  };

  const total = groups.reduce((n, g) => n + sessionCount(g.sessions), 0);
  const item = (id: string) => ({
    ref: (el: HTMLLIElement | null) => {
      if (el) refs.current.set(id, el);
      else refs.current.delete(id);
    },
    tabIndex: current?.id === id ? 0 : -1,
    onFocus: (e: { target: EventTarget; currentTarget: EventTarget }) => {
      if (e.target === e.currentTarget) setFocus(id);
    },
  });

  const shellRow = (sh: TreeShell, level: number) => {
    const sid = `s:${keyOf(sh.tab)}`;
    const selected = sid === activeId;
    return (
      <li
        key={sid}
        role="treeitem"
        aria-level={level}
        aria-selected={selected}
        aria-current={selected ? "true" : undefined}
        aria-label={`Shell ${sh.label}`}
        className="tree-session tree-shell"
        data-selected={selected}
        {...item(sid)}
        onClick={() => {
          move(sid);
          onSelect(sh.tab);
        }}
      >
        <span className="tree-shell-icon" aria-hidden="true">
          <Icon name="prompt" size={13} />
        </span>
        <span className="tree-text">
          <span className="tree-session-title mono">{sh.label}</span>
        </span>
      </li>
    );
  };

  const sessionRow = (s: TreeSession, level: 2 | 3) => [rowOf(s, level), ...s.shells.map((sh) => shellRow(sh, level + 1))];

  const rowOf = (s: TreeSession, level: 2 | 3) => {
    const sid = s.key;
    const tab = s.tab;
    const selected = sid === activeId;
    return (
      <li
        key={sid}
        role="treeitem"
        aria-level={level}
        aria-selected={selected}
        aria-current={selected ? "true" : undefined}
        className="tree-session"
        data-item={tab ? undefined : "true"}
        data-selected={selected}
        {...item(sid)}
        onClick={() => {
          move(sid);
          if (tab) onSelect(tab);
        }}
        onContextMenu={(e) => {
          if (!menu || !tab) return;
          e.preventDefault();
          e.stopPropagation();
          move(sid);
          openMenu(tab, e.clientX, e.clientY, e.currentTarget);
        }}
      >
        <span className="tree-dot" data-tone={s.state.tone} aria-hidden="true" />
        <span className="tree-text">
          <span className="tree-session-title">{s.title}</span>
          <span className="tree-state" data-tone={s.state.tone}>
            {s.state.text}
          </span>
        </span>
        {s.remote && (
          <span className="tree-rc" role="img" aria-label="Remote Control conectado" title="Remote Control conectado">
            <Icon name="remote" size={13} />
          </span>
        )}
        {s.epic && <span className="tree-epic">épico</span>}
      </li>
    );
  };

  const epicRow = (n: Extract<TreeNode, { kind: "epic" }>) => {
    const open = isOpen(n.key);
    const id = `e:${n.key}`;
    const waiting = n.sessions.filter((s) => s.state.tone === "attention").length;
    return (
      <li
        key={id}
        role="treeitem"
        aria-level={2}
        aria-expanded={open}
        aria-label={`${n.title}, épico, ${plural(sessionCount(n.sessions), "sessão", "sessões")}${waiting ? `, ${waiting} aguardando` : ""}`}
        className="tree-group tree-epic-node"
        {...item(id)}
      >
        <div
          className="tree-group-row"
          onClick={(e) => {
            e.stopPropagation();
            move(id);
            toggle(n.key);
          }}
        >
          <svg className="tree-chevron" data-open={open} width="14" height="14" viewBox="0 0 14 14" aria-hidden="true">
            <path d="M5 3l4 4-4 4" />
          </svg>
          <span className="tree-name">{n.title}</span>
          <span className="tree-epic">épico</span>
          {waiting > 0 && <span className="tree-waiting">{waiting} aguardando</span>}
          <span className="tree-count">{sessionCount(n.sessions)}</span>
        </div>
        {open && (
          <ul role="group" className="tree-sessions">
            {n.sessions.map((s) => sessionRow(s, 3))}
          </ul>
        )}
      </li>
    );
  };

  const allClosed = groups.length > 0 && groups.every((g) => collapsed.includes(g.key));
  return (
    <SidePanel
      title="Sessões"
      label="Sessões por projeto"
      actions={
        <>
          <button type="button" className="ibtn" aria-label="Nova sessão" title="Nova sessão" onClick={onNew}>
            <Icon name="plus" size={15} />
          </button>
          {onCloseEnded !== undefined && (
            <button
              type="button"
              className="ibtn"
              aria-label="Fechar encerradas"
              title={onCloseEnded ? "Fechar as abas encerradas" : "Nenhuma aba encerrada"}
              disabled={!onCloseEnded}
              onClick={() => onCloseEnded?.()}
            >
              <Icon name="closeAll" size={15} />
            </button>
          )}
          <button
            type="button"
            className="ibtn"
            aria-label={allClosed ? "Expandir tudo" : "Recolher tudo"}
            title={allClosed ? "Expandir tudo" : "Recolher tudo"}
            onClick={() => onCollapse(allClosed ? [] : groups.map((g) => g.key))}
          >
            <Icon name={allClosed ? "expandAll" : "collapseAll"} size={15} />
          </button>
        </>
      }
      footer={
        <p className="tree-foot">
          {plural(groups.length, "projeto", "projetos")} · {plural(total, "sessão", "sessões")}
        </p>
      }
    >
      <div className="tree-head">
        <label>
          <span className="sr-only">Filtrar sessões</span>
          <input
            type="search"
            value={filter}
            placeholder="Filtrar por título, branch ou #issue"
            onChange={(e) => setFilter(e.target.value)}
          />
        </label>
      </div>
      <ul role="tree" aria-label="Projetos" className="tree" onKeyDown={onKeyDown}>
        {shown.map((g) => {
          const open = isOpen(g.key);
          const waiting = g.sessions.filter((s) => s.state.tone === "attention").length;
          const id = `g:${g.key}`;
          return (
            <li
              key={g.key}
              role="treeitem"
              aria-level={1}
              aria-expanded={open}
              aria-label={`${g.name}, ${plural(sessionCount(g.sessions), "sessão", "sessões")}${waiting ? `, ${waiting} aguardando` : ""}`}
              className="tree-group"
              {...item(id)}
            >
              <div
                className="tree-group-row"
                onClick={() => {
                  move(id);
                  toggle(g.key);
                }}
                onContextMenu={(e) => {
                  if (!groupMenu) return;
                  e.preventDefault();
                  move(id);
                  openGroupMenu(g, e.clientX, e.clientY, e.currentTarget.parentElement as HTMLElement);
                }}
              >
                <svg className="tree-chevron" data-open={open} width="14" height="14" viewBox="0 0 14 14" aria-hidden="true">
                  <path d="M5 3l4 4-4 4" />
                </svg>
                <span className="tree-name">{g.name}</span>
                {waiting > 0 && <span className="tree-waiting">{waiting} aguardando</span>}
                <span className="tree-count">{sessionCount(g.sessions)}</span>
              </div>
              {open && (
                <ul role="group" className="tree-sessions">
                  {g.nodes.map((n) => (n.kind === "session" ? sessionRow(n.s, 2) : epicRow(n)))}
                </ul>
              )}
            </li>
          );
        })}
      </ul>
      {!filtering && groups.length === 0 && (
        <p className="tree-empty muted">Nenhuma sessão aberta. Abra uma pela Sala de controle.</p>
      )}
      {filtering && shown.length === 0 && <p className="tree-empty muted">Nenhuma sessão encontrada.</p>}
      {ctx && (
        <ContextMenu
          x={ctx.x}
          y={ctx.y}
          label={ctx.label}
          items={ctx.items}
          onClose={(restore) => {
            const el = ctx.origin;
            setCtx(null);
            if (restore && el.isConnected) el.focus();
          }}
        />
      )}
    </SidePanel>
  );
}
