// Shell no estilo VS Code (#319): barra de título, barra de atividades,
// painel lateral, área de editor com abas, painel inferior e barra de status.
// O conteúdo de cada tela entra na área de editor; a tela pode fornecer o do
// painel lateral (`SideContent`) e abrir abas próprias (`useNav().openTab`).
import "./shell.css";
import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
  type PointerEvent as ReactPointerEvent,
  type ReactNode,
  type RefObject,
} from "react";
import { closeTargets, hasProjectKey, isPinnable, isShell, useNav, type CloseWhich, type EditorTab, type Nav, type Screen } from "../state/nav";
import { ContextMenu, type MenuItem } from "./ContextMenu";
import { OpenTabsList } from "./OpenTabs";
import { useTower } from "../state/tower";
import { ago } from "../util/time";
import { BottomPanel, useEventLog } from "./BottomPanel";
import { CommandPalette } from "./CommandPalette";
import { Icon, type IconName } from "./icons";
import {
  clampWidth,
  MOBILE_QUERY,
  shellShortcut,
  shortcutLabel,
  SIDE_MAX,
  SIDE_MIN,
  useMediaQuery,
  useShellLayout,
  type PanelTab,
} from "./layout";
import { MobileTabs, WarningsSheet } from "./Mobile";
import { ShortcutsSheet } from "./Shortcuts";
import { isHelpKey } from "./shortcuts";
import { SidePanel, SideGroup, SideRow, SideSlotProvider, useSideSlot } from "./SidePanel";
import { connectedEnvs, interruptedCount, lastSync, plural, syncTrackers, waitingCount, warnings } from "./summary";
import { openScreen } from "./places";
import { SCREEN_LABEL, tabLook } from "./titles";
import { StartSessionHost } from "../project/StartSessionModal";
import { useMaintenancePending } from "../maintenance/pending";
import { maintenanceTab } from "../maintenance/sections";
import { WEB_HIDDEN_SCREENS } from "../web/mode";
import { useWebSession } from "../web/WebGate";

/** Comandos do shell para as telas (e para a paleta, #312). */
export interface ShellApi {
  /** Pedido de abrir a paleta (⌘K ou o campo da barra de título). */
  paletteOpen: boolean;
  openPalette(): void;
  closePalette(): void;
  /** Folha de atalhos e gestos (#27): `?`, a paleta e a barra de status. */
  openShortcuts(): void;
  toggleSide(): void;
  togglePanel(): void;
  /** Abre o painel inferior na aba. */
  showPanel(tab: PanelTab): void;
  /** `sync.now` nos ambientes conectados (botão da barra de título, paleta). */
  syncNow(): Promise<void>;
  syncing: boolean;
  /** A tela Terminal informa a sessão ativa para a barra de status (#316); `null` limpa. */
  setTermStatus(s: TermStatus | null): void;
}

/** Ambiente, worktree (ou pasta) e uso de contexto da sessão ativa, na barra de status. */
export interface TermStatus {
  env: string;
  worktree: string | null;
  /** "Contexto 45% · 90k/200k" (`session.context`); `null` sem dados. */
  context?: string | null;
  /** Link do Remote Control da sessão ativa, quando conectado (#197). */
  remote?: string | null;
}

const ShellCtx = createContext<ShellApi | null>(null);

export function useShell(): ShellApi {
  const s = useContext(ShellCtx);
  if (!s) throw new Error("useShell fora do Shell");
  return s;
}

interface Activity {
  screen: Screen["name"];
  icon: IconName;
}

const ACTIVITIES: Activity[] = [
  { screen: "control", icon: "control" },
  { screen: "terminal", icon: "terminal" },
  { screen: "project", icon: "project" },
  { screen: "review", icon: "review" },
  { screen: "recovery", icon: "recovery" },
  { screen: "hub", icon: "hub" },
  { screen: "maintenance", icon: "maintenance" },
];

const SETTINGS: Activity = { screen: "settings", icon: "settings" };

/** Nome curto sob o ícone da barra inferior do celular (cabe em 60 px). */
const SHORT_LABEL: Partial<Record<Screen["name"], string>> = {
  control: "Sala",
  project: "Projetos",
  review: "Revisão",
  recovery: "Recuperar",
  hub: "Hub",
  settings: "Ajustes",
};

function Logo() {
  return (
    <svg className="brand-logo" width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" aria-hidden="true">
      <path d="M12 2v6M12 22v-6M2 12h6M22 12h-6" />
      <circle cx="12" cy="12" r="4" />
    </svg>
  );
}

export function Shell({ banners, now, children }: { banners: ReactNode; now: number; children: ReactNode }) {
  const nav = useNav();
  const tower = useTower();
  const { envs, status, api, dashboards, hub, web } = tower;
  const webSession = useWebSession();
  const [logoutError, setLogoutError] = useState<string | null>(null);
  const [layout, setLayout] = useShellLayout();
  const [paletteOpen, setPaletteOpen] = useState(false);
  const [keysOpen, setKeysOpen] = useState(false);
  const side = useSideSlot();
  const events = useEventLog(api, dashboards);
  const warns = warnings(tower, now);
  // Celular (§10.3 da spec do épico #205): só no modo web, abaixo de 768 px.
  const mobile = useMediaQuery(MOBILE_QUERY) && web;
  const [sheet, setSheet] = useState(false);

  const [syncing, setSyncing] = useState(false);
  const [termStatus, setTermStatus] = useState<TermStatus | null>(null);
  const syncNow = useCallback(async () => {
    // Web: `sync.now` fica fora do canal reverso (spec do épico #205 §5.2).
    if (web) return;
    setSyncing(true);
    try {
      await Promise.allSettled(
        envs.filter((e) => status[e.id]?.connected).map((e) => api.call(e.id, "sync.now")),
      );
    } finally {
      setSyncing(false);
    }
  }, [api, envs, status, web]);

  const shell = useMemo<ShellApi>(
    () => ({
      paletteOpen,
      openPalette: () => setPaletteOpen(true),
      closePalette: () => setPaletteOpen(false),
      openShortcuts: () => setKeysOpen(true),
      toggleSide: () => setLayout((l) => ({ sideOpen: !l.sideOpen })),
      togglePanel: () => setLayout((l) => ({ panelOpen: !l.panelOpen })),
      showPanel: (panelTab) => setLayout({ panelOpen: true, panelTab }),
      syncNow,
      syncing,
      setTermStatus,
    }),
    [paletteOpen, setLayout, syncNow, syncing],
  );

  // Celular: a gaveta fecha ao trocar de tela ou de aba (a escolha foi feita nela).
  const where = `${nav.current}|${JSON.stringify(nav.screen)}`;
  const lastWhere = useRef(where);
  useEffect(() => {
    if (lastWhere.current === where) return;
    lastWhere.current = where;
    if (mobile) setLayout({ sideOpen: false });
  }, [where, mobile, setLayout]);
  useEffect(() => {
    if (!mobile || !layout.sideOpen) return;
    const on = (e: KeyboardEvent) => e.key === "Escape" && setLayout({ sideOpen: false });
    window.addEventListener("keydown", on);
    return () => window.removeEventListener("keydown", on);
  }, [mobile, layout.sideOpen, setLayout]);

  // ⌘B, ⌘J e ⌘K valem em qualquer tela (fase de captura: antes do xterm).
  const shellRef = useRef(shell);
  shellRef.current = shell;
  useEffect(() => {
    const on = (e: KeyboardEvent) => {
      const inTerminal = e.target instanceof Element && !!e.target.closest(".xterm");
      const s = shellShortcut(e, inTerminal);
      if (!s) return;
      e.preventDefault();
      e.stopPropagation();
      const sh = shellRef.current;
      if (s === "side") sh.toggleSide();
      else if (s === "panel") sh.togglePanel();
      else sh.openPalette();
    };
    window.addEventListener("keydown", on, true);
    return () => window.removeEventListener("keydown", on, true);
  }, []);

  // `?` abre e fecha a folha de atalhos, fora de campos de texto e do console.
  useEffect(() => {
    const on = (e: KeyboardEvent) => {
      if (e.defaultPrevented || !isHelpKey(e)) return;
      e.preventDefault();
      setKeysOpen((o) => !o);
    };
    window.addEventListener("keydown", on);
    return () => window.removeEventListener("keydown", on);
  }, []);

  const maintenancePending = useMaintenancePending();
  const counts = {
    waiting: waitingCount(dashboards),
    interrupted: interruptedCount(dashboards),
    hubWarnings: hub.health?.warnings.length ?? 0,
    maintenance: maintenancePending,
    sessions: nav.tabs.filter((t) => !isShell(t)).length,
  };

  return (
    <ShellCtx.Provider value={shell}>
      <SideSlotProvider slot={side.slot}>
        <StartSessionHost>
          <div className="shell" data-mobile={mobile ? "true" : undefined}>
            <header className="titlebar">
              {mobile ? (
                <>
                  <button
                    type="button"
                    className="ibtn"
                    aria-label={layout.sideOpen ? "Fechar painel lateral" : "Abrir painel lateral"}
                    aria-expanded={layout.sideOpen}
                    onClick={shell.toggleSide}
                  >
                    <Icon name="menu" size={20} />
                  </button>
                  <MobileTabs />
                </>
              ) : (
                <div className="brand">
                  <Logo />
                  Control Tower
                </div>
              )}
              <button
                type="button"
                className="cmd-trigger"
                aria-label="Abrir paleta de comandos"
                aria-keyshortcuts="Meta+K Control+K"
                onClick={shell.openPalette}
              >
                <Icon name="search" size={14} />
                <span className="cmd-text">Buscar sessões, issues, projetos e comandos</span>
                <kbd className="kbd">{shortcutLabel("K")}</kbd>
              </button>
              <div className="titlebar-actions">
                {!web && (
                  <button
                    type="button"
                    className="ibtn"
                    aria-label={syncing ? "Sincronizando…" : "Sincronizar agora"}
                    title={`Sincronizar com o ${syncTrackers(dashboards)} agora`}
                    aria-busy={syncing}
                    disabled={syncing}
                    onClick={() => void syncNow()}
                  >
                    <Icon name="sync" />
                  </button>
                )}
                <button
                  type="button"
                  className="ibtn"
                  aria-label={warns.length > 0 ? `Notificações, ${plural(warns.length, "aviso", "avisos")}` : "Notificações"}
                  title="Avisos"
                  aria-haspopup={mobile ? "dialog" : undefined}
                  onClick={() => (mobile ? setSheet(true) : shell.showPanel("warnings"))}
                >
                  <Icon name="bell" />
                  {warns.length > 0 && <span className="ibtn-dot" aria-hidden="true" />}
                </button>
                {webSession && (
                  <button
                    type="button"
                    className="ibtn"
                    aria-label="Sair"
                    title={`Sair da sessão web${webSession.identity ? ` (${webSession.identity})` : ""}`}
                    onClick={() => {
                      setLogoutError(null);
                      webSession.logout().catch((e) => setLogoutError(e instanceof Error ? e.message : String(e)));
                    }}
                  >
                    <Icon name="logout" />
                  </button>
                )}
              </div>
            </header>

            <div className="shell-work">
              {!mobile && <ActivityBar counts={counts} onToggleSide={shell.toggleSide} />}

              <div
                className="sidepanel"
                role="region"
                aria-label="Painel lateral"
                hidden={!layout.sideOpen}
                style={mobile ? undefined : { width: layout.sideWidth }}
              >
                <div ref={side.ref} className="sidepanel-slot" />
                {!side.filled && <DefaultSide onCollapse={shell.toggleSide} />}
              </div>
              {layout.sideOpen && (
                <>
                  {!mobile && <Resizer width={layout.sideWidth} onWidth={(w) => setLayout({ sideWidth: w })} />}
                  {/* Gaveta (tela estreita): tocar fora fecha. */}
                  <div className="sidepanel-scrim" aria-hidden="true" onClick={shell.toggleSide} />
                </>
              )}

              <div className="shell-main">
                {logoutError && (
                  <div className="banner" role="alert">
                    {logoutError}
                  </div>
                )}
                {banners}
                {!mobile && <EditorTabs />}
                <div className="editor" role="tabpanel" id="editor-area" aria-label="Área de editor">
                  {children}
                </div>
                {layout.panelOpen && !mobile && (
                  <BottomPanel
                    tab={layout.panelTab}
                    onTab={(panelTab) => setLayout({ panelTab })}
                    onClose={shell.togglePanel}
                    events={events}
                    warnings={warns}
                    now={now}
                  />
                )}
              </div>
            </div>

            {/* Celular: a barra de atividades vira barra inferior; sem barra de status. */}
            {mobile && <ActivityBar bottom counts={counts} onToggleSide={shell.toggleSide} />}

            {paletteOpen && <CommandPalette onClose={shell.closePalette} />}
            {sheet && mobile && <WarningsSheet items={warns} now={now} onClose={() => setSheet(false)} />}
            {keysOpen && <ShortcutsSheet web={web} mobile={mobile} onClose={() => setKeysOpen(false)} />}

            {!mobile && (
              <StatusBar
                now={now}
                term={nav.screen.name === "terminal" ? termStatus : null}
                warnings={warns.length}
                onWarnings={() => shell.showPanel("warnings")}
                onSync={() => shell.showPanel("sync")}
                onShortcuts={shell.openShortcuts}
              />
            )}
          </div>
        </StartSessionHost>
      </SideSlotProvider>
    </ShellCtx.Provider>
  );
}

function ActivityBar({
  counts,
  onToggleSide,
  bottom = false,
}: {
  counts: { waiting: number; interrupted: number; hubWarnings: number; maintenance: number; sessions: number };
  onToggleSide(): void;
  /** Celular: barra inferior, com o nome curto de cada tela sob o ícone. */
  bottom?: boolean;
}) {
  const nav = useNav();
  const { web } = useTower();
  const activities = web ? ACTIVITIES.filter((a) => !WEB_HIDDEN_SCREENS.has(a.screen)) : ACTIVITIES;

  /** Contador no ícone (e no nome acessível). */
  const badge = (a: Activity): { n: number; text: string } | null => {
    switch (a.screen) {
      case "control":
        return counts.waiting > 0 ? { n: counts.waiting, text: `${counts.waiting} aguardando você` } : null;
      case "terminal":
        // Sessões abertas só no nome: não é algo que peça ação.
        return counts.sessions > 0 ? { n: 0, text: plural(counts.sessions, "sessão", "sessões") } : null;
      case "recovery":
        return counts.interrupted > 0
          ? { n: counts.interrupted, text: plural(counts.interrupted, "interrompida", "interrompidas") }
          : null;
      case "hub":
        return counts.hubWarnings > 0 ? { n: counts.hubWarnings, text: plural(counts.hubWarnings, "aviso", "avisos") } : null;
      case "maintenance":
        return counts.maintenance > 0
          ? { n: counts.maintenance, text: plural(counts.maintenance, "pendência", "pendências") }
          : null;
      default:
        return null;
    }
  };

  const open = (name: Screen["name"]) => {
    // Clicar na tela atual recolhe ou mostra o painel lateral (como no VS Code).
    // Na aba de um projeto, Projetos não é a "tela atual": vai ao índice (#16).
    const here = nav.screen.name === name && !(nav.screen.name === "project" && hasProjectKey(nav.screen));
    if (here) {
      onToggleSide();
      return;
    }
    // Volta à aba já aberta da tela (com o endereço dela); senão, abre.
    openScreen(nav, name);
  };

  const item = (a: Activity) => {
    const b = badge(a);
    const label = SCREEN_LABEL[a.screen];
    const desc = `ab-desc-${a.screen}`;
    // O nome é o da tela; o contador vai na descrição (lida depois do nome).
    return (
      <button
        key={a.screen}
        type="button"
        className="ab-item"
        aria-label={label}
        aria-describedby={b ? desc : undefined}
        aria-current={nav.screen.name === a.screen ? "page" : undefined}
        data-tip={bottom ? undefined : b ? `${label} · ${b.text}` : label}
        onClick={() => open(a.screen)}
      >
        <Icon name={a.icon} size={22} />
        {bottom && (
          <span className="ab-name" aria-hidden="true">
            {SHORT_LABEL[a.screen] ?? label}
          </span>
        )}
        {b && b.n > 0 && (
          <span className="ab-badge" aria-hidden="true">
            {b.n}
          </span>
        )}
        {b && (
          <span id={desc} hidden>
            {b.text}
          </span>
        )}
      </button>
    );
  };

  return (
    <nav className={bottom ? "activitybar bottombar" : "activitybar"} aria-label="Telas">
      {activities.map(item)}
      {!bottom && <div className="ab-spacer" />}
      {item(SETTINGS)}
    </nav>
  );
}

/**
 * Menu de contexto de uma aba (#427; spec da #87, §5): Fechar, Fechar
 * outras, Fechar à direita, Fechar encerradas e Fechar as do projeto (só nas
 * abas de sessão e shell com projeto). Item sem aba a fechar fica desabilitado.
 */
export function tabMenu(nav: Pick<Nav, "editors" | "meta" | "closeEditor" | "closeTabs">, id: string): MenuItem[] {
  const many = (label: string, which: CloseWhich, keep?: string): MenuItem => {
    const ids = closeTargets(nav.editors, which, nav.meta);
    return { label, run: () => nav.closeTabs(ids, keep), disabled: ids.length === 0 };
  };
  const project = nav.meta[id]?.project;
  return [
    { label: "Fechar", run: () => nav.closeEditor(id) },
    many("Fechar outras", { how: "others", id }, id),
    many("Fechar à direita", { how: "right", id }, id),
    many("Fechar encerradas", { how: "ended" }),
    project
      ? many(`Fechar as do projeto ${project.name}`, { how: "project", project: project.key })
      : { label: "Fechar as do projeto", run: () => {}, disabled: true, title: "A aba não é de um projeto" },
  ];
}

/** A faixa não mostra todas as abas (rola na horizontal). */
function useOverflow(ref: RefObject<HTMLElement | null>, key: unknown): boolean {
  const [over, setOver] = useState(false);
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const check = () => setOver(el.scrollWidth > el.clientWidth + 1);
    check();
    const ro = new ResizeObserver(check);
    ro.observe(el);
    return () => ro.disconnect();
  }, [ref, key]);
  return over;
}

function EditorTabs() {
  const nav = useNav();
  const { dashboards } = useTower();
  const strip = useRef<HTMLDivElement>(null);
  const listBtn = useRef<HTMLButtonElement>(null);
  // Recalcula a cada mudança das abas (a faixa só muda de largura por elas ou pelo resize).
  const over = useOverflow(strip, nav.editors);
  const [menu, setMenu] = useState<{ x: number; y: number; id: string; origin: HTMLElement } | null>(null);
  const [list, setList] = useState(false);
  const looks = nav.editors.map((t) => tabLook(t, dashboards, nav.meta[t.id]));

  const focusTab = (id: string) => {
    const el = document.getElementById(`etab-${id}`);
    el?.focus();
    el?.scrollIntoView?.({ block: "nearest", inline: "nearest" });
  };

  const onKey = (e: ReactKeyboardEvent, i: number) => {
    if (e.key === "ContextMenu" || (e.key === "F10" && e.shiftKey)) {
      e.preventDefault();
      const el = e.currentTarget as HTMLElement;
      const r = el.getBoundingClientRect();
      setMenu({ x: r.left + 8, y: r.bottom, id: nav.editors[i].id, origin: el });
      return;
    }
    // Setas trocam de aba (padrão de tablist).
    const d = e.key === "ArrowRight" ? 1 : e.key === "ArrowLeft" ? -1 : 0;
    if (!d) return;
    e.preventDefault();
    const next = nav.editors[(i + d + nav.editors.length) % nav.editors.length];
    nav.activate(next.id);
    focusTab(next.id);
  };

  return (
    <>
      <div className="etabs-bar">
        <div ref={strip} className="etabs" role="tablist" aria-label="Abas abertas">
          {nav.editors.map((t: EditorTab, i) => {
            const look = looks[i];
            const on = t.id === nav.current;
            const pinnable = isPinnable(t);
            const preview = t.kind === "screen" && !!t.preview;
            return (
              <div
                key={t.id}
                className="etab"
                data-on={on ? "true" : undefined}
                data-preview={preview ? "true" : undefined}
                data-pinned={pinnable && !preview ? "true" : undefined}
                onContextMenu={(e) => {
                  e.preventDefault();
                  const el = document.getElementById(`etab-${t.id}`) ?? e.currentTarget;
                  setMenu({ x: e.clientX, y: e.clientY, id: t.id, origin: el });
                }}
              >
                <button
                  type="button"
                  role="tab"
                  id={`etab-${t.id}`}
                  className="etab-main"
                  aria-selected={on}
                  aria-controls="editor-area"
                  tabIndex={on ? 0 : -1}
                  // Projeto e ambiente da aba de sessão ou shell (#427).
                  title={look.tip ? `${look.title}\n${look.tip}` : undefined}
                  onClick={() => nav.activate(t.id)}
                  onKeyDown={(e) => onKey(e, i)}
                  onAuxClick={(e) => e.button === 1 && nav.closeEditor(t.id)}
                  // Duplo clique fixa a provisória, como no VS Code.
                  onDoubleClick={() => preview && nav.pin(t.id)}
                >
                  {look.tone && <span className="sp-dot" data-tone={look.tone} aria-hidden="true" />}
                  {look.icon && <Icon name={look.icon} size={13} />}
                  <span className="etab-title">{look.title}</span>
                </button>
                {pinnable && (
                  <button
                    type="button"
                    className="ibtn etab-pin"
                    aria-label={preview ? "Fixar aba" : "Desafixar aba"}
                    aria-pressed={!preview}
                    title={preview ? `Fixar aba ${look.title}` : `Desafixar aba ${look.title}`}
                    tabIndex={on ? 0 : -1}
                    onClick={() => nav.pin(t.id, preview)}
                  >
                    <Icon name="pin" size={12} />
                  </button>
                )}
                <button
                  type="button"
                  className="ibtn etab-close"
                  aria-label={`Fechar aba ${look.title}`}
                  tabIndex={on ? 0 : -1}
                  onClick={() => nav.closeEditor(t.id)}
                >
                  <Icon name="close" size={12} />
                </button>
              </div>
            );
          })}
        </div>
        {/* Abas que não cabem: a lista filtrável (#427). */}
        {(over || list) && (
          <button
            ref={listBtn}
            type="button"
            className="ibtn etabs-more"
            aria-label={`Lista de abas abertas (${nav.editors.length})`}
            title="Lista de abas abertas"
            aria-haspopup="dialog"
            aria-expanded={list}
            onMouseDown={(e) => list && e.stopPropagation()}
            onClick={() => setList((o) => !o)}
          >
            <Icon name="chevronDown" size={15} />
          </button>
        )}
        {list && (
          <OpenTabsList
            items={nav.editors.map((t, i) => ({ id: t.id, ...looks[i] }))}
            current={nav.current}
            onPick={(id) => {
              nav.activate(id);
              focusTab(id);
            }}
            onClose={(restore) => {
              setList(false);
              if (restore) listBtn.current?.focus();
            }}
          />
        )}
      </div>
      {nav.closeAsk && (
        <div role="group" aria-label="Confirmar fechamento das abas" className="confirm etabs-confirm">
          <span>
            {nav.closeAsk.busy > 0
              ? `${plural(nav.closeAsk.busy, "shell tem um comando rodando", "shells têm um comando rodando")}. Fechar encerra os processos.`
              : "Não deu para saber se há um comando rodando nos shells. Fechar encerra os processos."}
          </span>
          <button type="button" className="btn btn-danger btn-sm" onClick={nav.confirmCloseTabs}>
            {`Fechar ${plural(nav.closeAsk.ids.length, "aba", "abas")}`}
          </button>
          <button type="button" className="btn btn-sm" onClick={nav.cancelCloseTabs}>
            Cancelar
          </button>
        </div>
      )}
      {menu && (
        <ContextMenu
          x={menu.x}
          y={menu.y}
          label="Ações da aba"
          items={tabMenu(nav, menu.id)}
          onClose={(restore) => {
            const el = menu.origin;
            setMenu(null);
            if (restore && el.isConnected) el.focus();
          }}
        />
      )}
    </>
  );
}

/** Painel lateral das telas que ainda não fornecem o seu: as abas abertas. */
function DefaultSide({ onCollapse }: { onCollapse(): void }) {
  const nav = useNav();
  const { dashboards } = useTower();
  return (
    <SidePanel
      title={SCREEN_LABEL[nav.screen.name]}
      actions={
        <button
          type="button"
          className="ibtn"
          aria-label={`Recolher painel lateral (${shortcutLabel("B")})`}
          onClick={onCollapse}
        >
          <Icon name="sidebar" size={15} />
        </button>
      }
    >
      <SideGroup title="Abertas" count={nav.editors.length}>
        {nav.editors.map((t) => {
          const look = tabLook(t, dashboards, nav.meta[t.id]);
          return (
            <SideRow
              key={t.id}
              label={look.title}
              ariaLabel={`Aba ${look.title}`}
              tone={look.tone}
              selected={t.id === nav.current}
              onClick={() => nav.activate(t.id)}
            />
          );
        })}
      </SideGroup>
    </SidePanel>
  );
}

/** Divisória arrastável (ou pelas setas) entre o painel lateral e o editor. */
function Resizer({ width, onWidth }: { width: number; onWidth(w: number): void }) {
  const start = useRef<{ x: number; w: number } | null>(null);
  const onPointerDown = (e: ReactPointerEvent) => {
    start.current = { x: e.clientX, w: width };
    (e.target as Element).setPointerCapture?.(e.pointerId);
  };
  const onPointerMove = (e: ReactPointerEvent) => {
    if (start.current) onWidth(start.current.w + e.clientX - start.current.x);
  };
  const stop = () => {
    start.current = null;
  };
  const onKeyDown = (e: ReactKeyboardEvent) => {
    const step = e.shiftKey ? 64 : 16;
    const w =
      e.key === "ArrowLeft"
        ? width - step
        : e.key === "ArrowRight"
          ? width + step
          : e.key === "Home"
            ? SIDE_MIN
            : e.key === "End"
              ? SIDE_MAX
              : null;
    if (w === null) return;
    e.preventDefault();
    onWidth(clampWidth(w));
  };
  return (
    <div
      className="sidepanel-resizer"
      role="separator"
      aria-orientation="vertical"
      aria-label="Largura do painel lateral"
      title="Arraste ou use ←/→ para mudar a largura"
      aria-valuenow={width}
      aria-valuemin={SIDE_MIN}
      aria-valuemax={SIDE_MAX}
      tabIndex={0}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={stop}
      onPointerCancel={stop}
      onKeyDown={onKeyDown}
    />
  );
}

function StatusBar({
  now,
  term,
  warnings,
  onWarnings,
  onSync,
  onShortcuts,
}: {
  now: number;
  term: TermStatus | null;
  warnings: number;
  onWarnings(): void;
  onSync(): void;
  onShortcuts(): void;
}) {
  const { envs, status, dashboards, hub, uiUpdate, api, web } = useTower();
  const nav = useNav();
  const sync = lastSync(dashboards);
  const connected = connectedEnvs(envs, status);

  let hubText: string | null = null;
  if (hub.conn) {
    const name = hub.info?.name ?? "hub";
    hubText = !hub.status ? `Hub ${name} · conectando…` : hub.connected ? `Hub ${name}` : `Hub ${name} · indisponível`;
  } else if (hub.unconfigured) hubText = "Hub não configurado";

  return (
    <footer className="statusbar" aria-label="Barra de status">
      {hubText && (
        <button
          type="button"
          className="sb-item"
          onClick={() => nav.go({ name: hub.conn ? "hub" : "settings" })}
        >
          <span className="sb-dot" data-on={hub.connected ? "true" : "false"} aria-hidden="true" />
          {hubText}
        </button>
      )}
      <span className="sb-item">
        Ambientes {connected}/{envs.length}
      </span>
      <button type="button" className="sb-item" onClick={onSync}>
        {sync ? `${syncTrackers(dashboards)} sincronizado ${ago(sync, now)}` : `${syncTrackers(dashboards)} ainda não sincronizado`}
      </button>
      {warnings > 0 && (
        <button type="button" className="sb-item sb-warn" onClick={onWarnings}>
          <Icon name="warning" size={13} />
          {plural(warnings, "aviso", "avisos")}
        </button>
      )}
      {/* Sessão ativa do Terminal. */}
      {term && (
        <span className="sb-item" title="Ambiente da sessão ativa">
          {term.env}
        </span>
      )}
      {term?.worktree && (
        <span className="sb-item mono" title="Worktree da sessão ativa">
          {term.worktree}
        </span>
      )}
      {term?.context && (
        <span className="sb-item" title="Uso de contexto da sessão ativa (última resposta do agente)">
          {term.context}
        </span>
      )}
      {term?.remote && (
        <button
          type="button"
          className="sb-item"
          title={`Remote Control conectado: ${term.remote}`}
          onClick={() => void api.openUrl(term.remote!).catch(() => {})}
        >
          <span className="sb-dot" data-on="true" aria-hidden="true" />
          Remote Control
        </button>
      )}
      <span className="sb-spacer" />
      {!web && uiUpdate?.available && (
        <button type="button" className="sb-item sb-update" onClick={() => nav.openTab(maintenanceTab("app-updates"))}>
          App {uiUpdate.available.version} disponível
        </button>
      )}
      {!web && uiUpdate?.current && <span className="sb-item">v{uiUpdate.current}</span>}
      <button
        type="button"
        className="sb-item"
        aria-label="Atalhos e gestos"
        aria-keyshortcuts="?"
        title="Atalhos e gestos (?)"
        onClick={onShortcuts}
      >
        ?
      </button>
    </footer>
  );
}

