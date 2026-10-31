import "./project.css";
import {
  useEffect,
  useRef,
  useState,
  type MouseEvent as ReactMouseEvent,
  type ReactNode,
} from "react";
import { useNow } from "../App";
import { ContextMenu } from "../shell/ContextMenu";
import { Icon } from "../shell/icons";
import { shortcutLabel } from "../shell/layout";
import { SideContent, SideGroup, SidePanel } from "../shell/SidePanel";
import { useStartSession, type StartTarget } from "../shell/start";
import { useNav, type ItemIntent, type Screen } from "../state/nav";
import { useTower } from "../state/tower";
import { changeLabel, changeRef, itemShort, trackerLabel, type ProviderProject } from "../util/item";
import { ago } from "../util/time";
import { useMobile } from "../web/mode";
import { Board, type BoardNode } from "./Board";
import { STATUS } from "./status";
import { itemTab } from "./tabs";
import { PROJECT_PAGES, PROJECT_PAGE_LABEL, projectAt, type ProjectPage, type ProjectSettingsSection } from "./sections";
import { TreeControls } from "./TreeControls";
import { ProviderIcon } from "./ProviderIcon";
import { decisionsTitle, mergedOpen, mergedOpenText, rowLabel } from "./TreeRow";
import {
  ACTIVE_SPRINT,
  ALL_SPRINTS,
  currentSprint,
  effectiveSprint,
  projectSprints,
  readSprintChoice,
  saveSprintChoice,
  sprintLabel,
} from "./sprints";
import { EMPTY_ACTIVE_SPRINT, EMPTY_FILTERED, EMPTY_OPEN, SPRINTS_UNREAD, buildView, isExpanded, narrowing, priorityOf, statusKey } from "./tree-view";
import { useProjectUi } from "./ui-store";
import { docsFolderState, type DocsCandidate } from "./docs";
import type { DocsDir } from "../protocol/gen";
import { useViewChoice } from "./view-choice";
import { PlayIcon, WorkItemPanel, type LiveRef, type PanelNode } from "./WorkItemPanel";

/** Nó da árvore (local ou do hub) no formato que a tela usa. */
export type ProjectNode = BoardNode & PanelNode & { children: ProjectNode[] };

type ProjectAddr = Extract<Screen, { name: "project" }>;

/** As abas do projeto além de Itens, que cada visão (local ou do hub) monta. */
export type ProjectPages = Record<Exclude<ProjectPage, "items">, () => ReactNode>;

/**
 * Vai para outra aba (ou seção das Configurações) do projeto `addr` (#17).
 * Na própria aba de editor quando ela mostra o projeto sem item (troca só o
 * endereço, como as seções do Hub); de uma aba de item, na aba do projeto.
 */
export function useGoPage(addr: ProjectAddr): (page: ProjectPage, section?: ProjectSettingsSection) => void {
  const nav = useNav();
  return (page, section) => {
    const screen = projectAt(addr, page, section);
    if (nav.screen.name === "project" && !nav.screen.item) nav.openTab({ id: nav.current, screen });
    else nav.go(screen);
  };
}

/** Dados que cada tela (local ou do hub) entrega à visão comum. */
export interface ProjectViewProps<T extends ProjectNode> {
  /** Endereço do projeto (sem item nem aba): base das abas dos itens e das do projeto. */
  addr: ProjectAddr;
  /** Aba do projeto mostrada sem item (#17); sem ela, Itens. */
  page?: ProjectPage;
  /** Conteúdo das outras abas (Itens é a árvore e o quadro desta visão). */
  pages: ProjectPages;
  /** Chave do estado da árvore (`projectKey`). */
  uiKey: string;
  name: string;
  /** Com rastreador e forja, para as referências (#352); sem eles, GitHub. */
  project: (ProviderProject & { base_branch: string; protected_branches: string[] }) | null;
  /** Árvore do projeto atual (`null` enquanto carrega ou na troca de projeto). */
  roots: T[] | null;
  syncedAt: number | null;
  error: string | null;
  item?: ItemIntent;
  /** Sessões vivas do item que a UI conhece (0 = sessão do projeto). */
  liveOf: (gh: number) => LiveRef[];
  /** Ambientes com sessão viva, para o ponto da linha; null = nenhum. */
  liveText: (n: T) => string | null;
  /** Ambientes com sessão viva segundo o hub (inclusive os que a UI não conhece). */
  whereOf?: (n: T) => string[];
  /** Há ambiente onde iniciar (sem ele, só acompanha). */
  canStart: boolean;
  /** Projeto da modal de início (as chaves do projeto; o número vem do item). */
  startBase: StartTarget;
  /** Seletor de projeto (cada visão troca de um jeito). */
  picker: ReactNode;
  headExtra?: ReactNode;
  banner?: ReactNode;
  /** Projeto sem nenhuma issue no espelho. */
  emptyText: string;
  /** Revisão das decisões do item, se houver work item num ambiente conhecido. */
  reviewOf?: (gh: number) => Screen | null;
  /** Ambientes conectados com o projeto, para "Abrir pasta de docs" (#444, só no desktop). */
  docs?: DocsCandidate[];
  onReload: () => void;
}

const isClosed = (n: ProjectNode) => n.issue.state === "closed" || n.work_item?.status === "concluido";

/**
 * Tela Projeto no shell (#317), comum à visão local e à do hub. Painel
 * lateral: seletor de projeto, Árvore | Quadro, filtro e a árvore (épicos com
 * progresso e sub-issues, depois as avulsas), com ▶ inline e menu de contexto.
 * Área de editor: sem item, o projeto (cabeçalho e, no Quadro, o Kanban); com
 * item (cada um em aba própria), o detalhe do item. Iniciar sessão abre a
 * modal de início (`useStartSession`).
 *
 * No celular (#25), sem item, a árvore (ou o quadro) vem para o corpo, na
 * aba Itens, e o painel lateral (a gaveta) fica com o conteúdo do shell: os
 * itens aparecem sem abrir a gaveta. Com item, a árvore segue na gaveta,
 * para pular de um item a outro.
 */
export function ProjectView<T extends ProjectNode>(p: ProjectViewProps<T>) {
  const nav = useNav();
  const goPage = useGoPage(p.addr);
  const page = p.page ?? "items";
  const { api, envs, infos, web } = useTower();
  const mobile = useMobile();
  const now = useNow();
  const startSession = useStartSession();
  const [docsError, setDocsError] = useState<string | null>(null);
  const [ui, setUi] = useProjectUi(p.uiKey);
  const [viewMode, setViewMode] = useViewChoice();
  const [menu, setMenu] = useState<{ x: number; y: number; n: T; origin: HTMLElement } | null>(null);
  const roots = p.roots;
  const all = (roots ?? []).flatMap((r) => [r, ...(r.children as T[])]);
  // Sprints só no projeto Jira: as conhecidas vêm das issues do espelho, e
  // a sprint guardada que não existe mais vale como "todas".
  const jira = p.project?.tracker?.kind === "jira";
  const sprints = jira ? projectSprints(all.map((x) => x.issue)) : null;
  // Espelho carregado sem nenhuma sprint: o sync ainda não as leu (ou o
  // projeto não usa sprints).
  const sprintsUnread = !!roots && sprints?.length === 0;
  // A escolha fica guardada por projeto (`owner/repo`); sem escolha nesta
  // execução, vale a guardada.
  const sprintKey = p.project ? `${p.project.gh_owner}/${p.project.gh_repo}`.toLowerCase() : null;
  const chosen = ui.filters.sprint ?? (sprintKey ? readSprintChoice(sprintKey) : ALL_SPRINTS);
  const filters = { ...ui.filters, sprint: sprints ? effectiveSprint(chosen, sprints) : ALL_SPRINTS };
  const focus = p.item?.ghNumber ?? null;
  const node = focus !== null ? all.find((x) => x.issue.gh_number === focus) : undefined;
  const parent = node ? roots?.find((r) => r.children.some((c) => c.issue.gh_number === focus)) : undefined;

  // Item pedido (aba do item, paleta): abre o épico pai uma vez por pedido.
  const shown = useRef<ItemIntent | undefined>(undefined);
  useEffect(() => {
    const item = p.item;
    if (!roots || !item || shown.current === item) return;
    const root = roots.find(
      (r) => r.issue.gh_number === item.ghNumber || r.children.some((c) => c.issue.gh_number === item.ghNumber),
    );
    if (!root) return;
    shown.current = item;
    if (root.issue.gh_number !== item.ghNumber) setUi((u) => ({ ...u, open: { ...u.open, [root.issue.gh_number]: true } }));
  }, [roots, p.item, setUi]);

  const isLive = (n: T) => p.liveText(n) !== null || (p.whereOf?.(n).length ?? 0) > 0;
  const familyText = (e: T) => {
    const envs = [e, ...(e.children as T[])].flatMap((x) => p.liveText(x)?.split(", ") ?? p.whereOf?.(x) ?? []);
    return envs.length > 0 ? [...new Set(envs)].join(", ") : null;
  };
  const canStartNode = (n: ProjectNode) => p.canStart && !isClosed(n);
  const start = (n: number | undefined, origin: HTMLElement) =>
    startSession({ ...p.startBase, ...(n !== undefined ? { ghNumber: n } : {}) }, origin);
  const openItem = (n: number) => {
    const x = all.find((y) => y.issue.gh_number === n);
    nav.openTab(itemTab(p.addr, n, x?.issue.title ?? "", prov));
  };
  const openUrl = (url: string) => void api.openUrl(url).catch(() => {});
  // "Abrir pasta de docs" (#444): o ambiente cria o que faltar e devolve o
  // caminho (a pasta `<n>-*` existente do épico); o app abre no sistema.
  const docs = !web && p.docs ? docsFolderState(p.docs, envs, infos) : null;
  const openDocs = (epic?: number) => {
    if (!docs?.enabled) return;
    setDocsError(null);
    void api
      .call<DocsDir>(docs.env, "docs.dir", { project_id: docs.projectId, ...(epic !== undefined ? { epic } : {}) })
      .then((d) => api.openFolder(docs.env, d.path))
      .catch((e) => setDocsError(`Não foi possível abrir a pasta de docs: ${e instanceof Error ? e.message : String(e)}`));
  };
  const baseBranch = p.project?.base_branch;
  // Provedor do projeto: chave do Jira e MR do GitLab no lugar de #n e PR (#352).
  const prov = p.project;
  const ref = (n: number) => itemShort(prov, n);

  const onMenu = (e: ReactMouseEvent, n: T) => {
    e.preventDefault();
    const origin = (e.currentTarget as HTMLElement).querySelector<HTMLElement>(".sp-row-main") ?? (e.currentTarget as HTMLElement);
    setMenu({ x: e.clientX, y: e.clientY, n, origin });
  };

  const row = (n: T, depth: number, epic?: { expanded: boolean; done: number }) => {
    const gh = n.issue.gh_number;
    const st = STATUS[statusKey(n)];
    const live = epic ? familyText(n) : (p.liveText(n) ?? (p.whereOf?.(n).join(", ") || null));
    const prio = priorityOf(n);
    const sprint = jira ? currentSprint(n.issue) : null;
    const warn = mergedOpen(n, baseBranch);
    const change = changeRef(prov, n.summary.pr?.number ?? 0);
    const base = epic
      ? [
          `Épico ${ref(gh)} ${n.issue.title}`,
          st.text,
          `${epic.done} de ${n.children.length} concluídas`,
          ...(n.summary.decisions.total > 0 ? [decisionsTitle(n.summary.decisions)] : []),
          ...(warn ? [mergedOpenText(prov)] : []),
        ].join(", ")
      : rowLabel(n, now, baseBranch, prov);
    const label = live ? `${base}, sessão viva em ${live}` : base;
    const selected = focus === gh;
    return (
      <li
        key={gh}
        className={`sp-row pj-row${epic ? " is-epic" : ""}${n.issue.state === "closed" ? " closed" : ""}`}
        data-selected={selected ? "true" : undefined}
        style={{ paddingLeft: 4 + depth * 16 }}
        onContextMenu={(e) => onMenu(e, n)}
      >
        {epic ? (
          <button
            type="button"
            className="pj-toggle"
            aria-expanded={epic.expanded}
            aria-label={`${epic.expanded ? "Recolher" : "Expandir"} épico ${ref(gh)}`}
            onClick={() => setUi((u) => ({ ...u, open: { ...u.open, [gh]: !epic.expanded } }))}
          >
            <Icon name={epic.expanded ? "chevronDown" : "chevronRight"} size={14} />
          </button>
        ) : (
          <span className="pj-indent" aria-hidden="true" />
        )}
        <button
          type="button"
          className="sp-row-main"
          aria-label={label}
          aria-current={selected ? "true" : undefined}
          title={n.issue.title}
          onClick={() => openItem(gh)}
        >
          <span className="sp-dot" data-tone={st.tone} aria-hidden="true" />
          <span className="mono pj-num">{ref(gh)}</span>
          <span className="sp-row-label">{n.issue.title}</span>
          {sprint && (
            <span className="pj-sprint" data-state={sprint.state} aria-hidden="true" title={sprintLabel(sprint)}>
              {sprint.name}
            </span>
          )}
          {warn && (
            <span
              className="pj-warn"
              aria-hidden="true"
              title={`${mergedOpenText(prov)}: o ${change} não fechou a issue (faltou Closes ${ref(gh)})`}
            >
              !
            </span>
          )}
          {live && <span className="pj-live" aria-hidden="true" title={`sessão viva em ${live}`} />}
          <span className="sp-row-trailing" aria-hidden="true">
            {epic ? `${epic.done}/${n.children.length}` : (prio ?? "")}
          </span>
        </button>
        {canStartNode(n) && (
          <span className="sp-row-actions">
            <button
              type="button"
              className="ibtn pj-play"
              aria-label={`Iniciar sessão para ${ref(gh)}`}
              title="Iniciar sessão"
              onClick={(e) => start(gh, e.currentTarget)}
            >
              <PlayIcon />
            </button>
          </span>
        )}
      </li>
    );
  };

  const view = buildView(roots ?? [], filters);
  const nEpics = view.epics.length;
  const empty =
    roots && roots.length === 0
      ? p.emptyText
      : roots && nEpics === 0 && view.loose.length === 0
        ? filters.sprint === ACTIVE_SPRINT
          ? sprintsUnread
            ? `${EMPTY_ACTIVE_SPRINT} (${SPRINTS_UNREAD})`
            : EMPTY_ACTIVE_SPRINT
          : narrowing(filters)
            ? EMPTY_FILTERED
            : EMPTY_OPEN
        : null;

  // Peças da árvore, no painel lateral (desktop, ou com item) ou no corpo
  // (celular, aba Itens): a mesma árvore, os mesmos filtros.
  const filtersButton = (
    <button
      type="button"
      className="ibtn"
      aria-label="Filtros"
      title="Filtros: status, ordem e concluídos"
      aria-expanded={ui.showFilters}
      aria-controls="tree-more"
      onClick={() => setUi((u) => ({ ...u, showFilters: !u.showFilters }))}
    >
      <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" aria-hidden="true">
        <path d="M4 6h16M7 12h10M10 18h4" />
      </svg>
    </button>
  );
  const views = (
    <div className="views" role="group" aria-label="Visão">
      <button
        type="button"
        aria-pressed={viewMode === "tree"}
        onClick={() => {
          setViewMode("tree");
          // Árvore e Quadro são da aba Itens (#17).
          if (focus === null && page !== "items") goPage("items");
        }}
      >
        Árvore
      </button>
      <button
        type="button"
        aria-pressed={viewMode === "board"}
        onClick={() => {
          setViewMode("board");
          // O Quadro fica na aba do projeto, em Itens (#17).
          if (focus !== null) nav.go(p.addr);
          else if (page !== "items") goPage("items");
        }}
      >
        Quadro
      </button>
    </div>
  );
  const controls = (
    <>
      <TreeControls
        value={filters}
        onChange={(f) => {
          if (sprintKey && f.sprint !== filters.sprint) saveSprintChoice(sprintKey, f.sprint ?? ALL_SPRINTS);
          setUi((u) => ({ ...u, filters: f }));
        }}
        more={ui.showFilters}
        sprints={sprints}
        sprintsUnread={sprintsUnread}
      />
      {docsError && (
        <p role="alert" className="pj-empty start-error">
          {docsError}
        </p>
      )}
      {!roots && !p.error && (
        <p className="pj-empty" role="status">
          Carregando…
        </p>
      )}
      {empty && <p className="pj-empty">{empty}</p>}
    </>
  );
  const tree = (
    <>
      {nEpics > 0 && (
        <SideGroup title="Épicos" count={nEpics}>
          {view.epics.flatMap(({ epic: e, shown: kids }) => {
            const done = e.children.filter((c) => statusKey(c) === "concluido").length;
            const expanded = isExpanded(e, filters, ui.open[e.issue.gh_number], now, isLive);
            return [row(e, 0, { expanded, done }), ...(expanded ? kids.map((c) => row(c, 1)) : [])];
          })}
        </SideGroup>
      )}
      {view.loose.length > 0 && (
        <SideGroup title="Issues avulsas" count={view.loose.length}>
          {view.loose.map((n) => row(n, 0))}
        </SideGroup>
      )}
    </>
  );
  // O menu de contexto vale nas linhas onde estiverem (painel ou corpo).
  const contextMenu = menu && (
    <ContextMenu
      label="Ações do item"
      x={menu.x}
      y={menu.y}
      items={[
        { label: "Abrir", run: () => openItem(menu.n.issue.gh_number) },
        ...(canStartNode(menu.n)
          ? [{ label: "Iniciar sessão", run: () => start(menu.n.issue.gh_number, menu.origin) }]
          : []),
        { label: `Abrir no ${trackerLabel(prov)}`, run: () => openUrl(menu.n.issue.url) },
        ...(docs && menu.n.children.length > 0
          ? [
              {
                label: "Abrir pasta de docs do épico",
                run: () => openDocs(menu.n.issue.gh_number),
                ...(docs.enabled ? {} : { disabled: true, title: docs.reason }),
              },
            ]
          : []),
        ...(mergedOpen(menu.n, baseBranch)
          ? [
              {
                label: `Fechar a issue no ${trackerLabel(prov)} (${changeLabel(prov)} mesclado)`,
                run: () => openUrl(menu.n.issue.url),
              },
            ]
          : []),
      ]}
      onClose={(restore) => {
        const o = menu.origin;
        setMenu(null);
        if (restore) o.focus();
      }}
    />
  );

  // No celular sem item, a árvore está no corpo: o painel fica com o do shell.
  const treeInBody = mobile && focus === null;
  const side = treeInBody ? (
    contextMenu
  ) : (
    <SideContent>
      <SidePanel
        title="Projeto"
        label="Árvore do projeto"
        actions={
          <>
            {p.canStart && (
              <button
                type="button"
                className="ibtn pj-play"
                aria-label="Iniciar sessão de planejamento"
                title="Sessão de planejamento do projeto"
                onClick={(e) => start(undefined, e.currentTarget)}
              >
                <PlayIcon />
              </button>
            )}
            {filtersButton}
          </>
        }
      >
        <div className="pj-picker">
          {prov && <ProviderIcon project={prov} />}
          {p.picker}
        </div>
        {views}
        {controls}
        {tree}
      </SidePanel>
      {contextMenu}
    </SideContent>
  );

  if (focus !== null) {
    const review = node ? p.reviewOf?.(focus) : null;
    return (
      <main className="project-screen">
        {side}
        {node && p.project ? (
          <WorkItemPanel
            key={focus}
            node={node}
            project={p.project}
            baseBranch={p.project.base_branch}
            now={now}
            live={p.liveOf(focus)}
            where={p.whereOf?.(node)}
            crumbs={
              <ol className="iv-crumbs" aria-label="Trilha">
                <li>
                  <button type="button" className="linklike" onClick={() => nav.go(p.addr)}>
                    {p.name}
                  </button>
                </li>
                <li>
                  {parent ? (
                    <button type="button" className="linklike" onClick={() => openItem(parent.issue.gh_number)}>
                      Épico {ref(parent.issue.gh_number)}
                    </button>
                  ) : node.children.length > 0 ? (
                    "Épicos"
                  ) : (
                    "Issues avulsas"
                  )}
                </li>
                <li aria-current="page">{ref(focus)}</li>
              </ol>
            }
            subs={
              node.children.length > 0
                ? (node.children as T[]).map((c) => ({ node: c, live: p.liveOf(c.issue.gh_number), canStart: canStartNode(c) }))
                : undefined
            }
            onStart={canStartNode(node) ? (origin) => start(focus, origin) : undefined}
            onOpenItem={openItem}
            onStartItem={(n, origin) => start(n, origin)}
            onReview={review ? () => nav.go(review) : undefined}
            onStatusChanged={p.onReload}
          />
        ) : (
          <div className="pj-overview">
            {p.error ? (
              <p role="alert" className="banner">
                {p.error}
              </p>
            ) : (
              <p className="muted" role="status">
                {roots ? `${ref(focus)} não está na árvore deste projeto.` : "Carregando…"}
              </p>
            )}
          </div>
        )}
      </main>
    );
  }

  const openCount = all.filter((x) => !isClosed(x)).length;
  const liveCount = all.filter((x) => p.liveOf(x.issue.gh_number).length > 0).length;
  const board = (
    <Board
      view={view}
      filters={filters}
      now={now}
      focus={null}
      project={prov}
      liveLabel={p.liveText}
      onSelect={openItem}
    />
  );
  return (
    <main className="project-screen">
      {side}
      <div className="pj-overview">
        {/* Celular: a trilha "Projetos › X", que no desktop abre o painel. */}
        {mobile && <div className="pj-picker">{p.picker}</div>}
        <div className="project-head">
          <h1>{p.name}</h1>
          {p.project && <ProviderIcon project={p.project} size={16} />}
          {p.project && (
            <span className="mono muted">
              {p.project.gh_owner}/{p.project.gh_repo} · base {p.project.base_branch} · protegidas{" "}
              {p.project.protected_branches.join(", ")}
            </span>
          )}
          {p.headExtra}
          <span className="muted project-sync">
            {p.syncedAt ? `sincronizado ${ago(p.syncedAt, now)}` : "ainda não sincronizado"}
          </span>
          {docs && (
            <button
              type="button"
              className="btn"
              disabled={!docs.enabled}
              title={docs.enabled ? "Abre a pasta de docs do projeto no gerenciador de arquivos" : docs.reason}
              onClick={() => openDocs()}
            >
              Abrir pasta de docs
            </button>
          )}
          {p.canStart && (
            <button type="button" className="btn" onClick={(e) => start(undefined, e.currentTarget)}>
              <PlayIcon /> Sessão de planejamento
            </button>
          )}
        </div>
        {/* Abas do projeto (#17): cada uma com endereço próprio (`projectAt`).
            Navegação, e não `tablist`: trocar de aba troca o endereço da aba
            de editor, e "tab" já é o papel das abas do editor. */}
        <nav className="pj-tabs" aria-label="Abas do projeto" data-segmented={mobile ? "true" : undefined}>
          {PROJECT_PAGES.map((pg) => (
            <button
              key={pg}
              type="button"
              aria-current={pg === page ? "page" : undefined}
              onClick={() => goPage(pg)}
            >
              {PROJECT_PAGE_LABEL[pg]}
            </button>
          ))}
        </nav>
        {p.banner}
        {p.error && (
          <p role="alert" className="banner">
            {p.error}
          </p>
        )}
        {page !== "items" ? (
          p.pages[page]()
        ) : treeInBody ? (
          // Celular (#25): Árvore | Quadro, filtros e os itens no corpo. Sem
          // dica de atalho de teclado: as linhas abrem e o ▶ inicia.
          <section className="pj-mitems" aria-label="Itens do projeto">
            <div className="pj-mitems-bar">
              {views}
              {filtersButton}
            </div>
            {controls}
            {viewMode === "board" ? board : tree}
          </section>
        ) : viewMode === "board" ? (
          board
        ) : (
          <>
            {roots && (
              <ul className="pj-stats" aria-label="Resumo do projeto">
                <li>
                  <strong>{(roots ?? []).filter((r) => r.children.length > 0 && !isClosed(r)).length}</strong>
                  <span className="muted">épicos abertos</span>
                </li>
                <li>
                  <strong>{openCount}</strong>
                  <span className="muted">itens em aberto</span>
                </li>
                <li>
                  <strong>{liveCount}</strong>
                  <span className="muted">com sessão viva</span>
                </li>
              </ul>
            )}
            <p className="muted">
              Escolha um item na árvore do painel lateral para abri-lo numa aba. O ▶ de cada linha (ou o menu de
              contexto, ou a paleta, {shortcutLabel("K")}) inicia uma sessão.
            </p>
          </>
        )}
      </div>
    </main>
  );
}
