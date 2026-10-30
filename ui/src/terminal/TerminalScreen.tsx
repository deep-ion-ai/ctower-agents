// Tela Terminal no shell (#316): árvore de sessões no painel lateral, cada
// sessão em uma aba de editor, até duas lado a lado ("Dividir à direita"),
// cabeçalho por painel e a faixa do pedido pendente abaixo do console. O
// shell livre no worktree (#423) é uma aba do mesmo jeito, com cabeçalho
// enxuto (`ShellBar`).
import "./terminal.css";
import { itemTitle, PROJECT_ITEM } from "../util/item";
import { useEffect, useRef, useState } from "react";
import { useNow } from "../App";
import { ASK_FALLBACK, gestorLabel, lookup } from "../control-room/attention";
import { tileState } from "../control-room/TileGrid";
import type { Dashboard, Project, RemoteControlEnable, RemoteControlStatus, Review, Session, Shell } from "../protocol/gen";
import { ContextMenu, type MenuItem } from "../shell/ContextMenu";
import { RESUME_PROMPT } from "../recovery/RecoveryScreen";
import { Icon } from "../shell/icons";
import { openScreen } from "../shell/places";
import { useShell } from "../shell/Shell";
import { SideContent } from "../shell/SidePanel";
import { useStartSession } from "../shell/start";
import { closeTargets, isShell, paneTabId, ptyOf, useNav, type Pane, type ShellTab, type Tab, type TabMeta } from "../state/nav";
import { contextShortcut, newShellShortcut } from "../shell/layout";
import { isMac } from "./links";
import { agentLabel } from "../settings/session-options";
import { askKey, useTower, type PermissionDecision } from "../state/tower";
import { ago, duration } from "../util/time";
import type { TermState } from "./adapter";
import {
  actionUrl,
  contextAction,
  prUrl,
  promptActions,
  promptIdOf,
  publishSessionActions,
  restartTarget,
  resumePrompt,
  sessionActions,
  unpublishSessionActions,
  vscodePath,
  type SessionActionId,
} from "./actions";
import { contextText, contextWindow, useContextUsage } from "./context";
import { ContextPanel, loadContextOpen, saveContextOpen } from "./ContextPanel";
import { permissionText, remaining } from "./permission";
import { findPrompt, PERMISSION_PENDING, promptBlocked, renderPrompt, sendReadyPrompt } from "./prompts";
import { rcView, type RcView } from "./remote";
import { RemotePanel } from "./RemoteControl";
import {
  dashboardSessions,
  groupSessions,
  isEnded,
  keyOf,
  rowState,
  SessionTree,
  type InfoOf,
  type TabInfo,
  type TreeGroup,
} from "./SessionTree";
import { projectKey, projectSessionTitle } from "./tabmeta";
import {
  closeCheck,
  dropShell,
  listShells,
  programName,
  setShellError,
  setShells,
  shellItem,
  shellTarget,
  shellTitle,
  useOpenShell,
  useShellStore,
} from "./shells";
import { TerminalView } from "./TerminalView";

function findSession(d: Dashboard | undefined, id: string): Session | undefined {
  return dashboardSessions(d).find((s) => s.id === id);
}

// `prUrl` mora em ./actions (o painel de contexto também usa); fica exportado daqui.
export { prUrl } from "./actions";

const same = (a: Pane | null, b: Pane | null) => !!a && !!b && a.env === b.env && ptyOf(a) === ptyOf(b);
const sessionOf = (p: Pane | null): Tab | null => (p && !isShell(p) ? p : null);


export function TerminalScreen() {
  const nav = useNav();
  const { tabs, active } = nav;
  const shell = useShell();
  const startSession = useStartSession();
  const [states, setStates] = useState<Record<string, TermState>>({});
  const { envs, dashboards, api, infos, refresh, status } = useTower();
  const shellStore = useShellStore();
  const openShell = useOpenShell();
  // Pedidos de busca e de foco por console (botões do cabeçalho e da faixa).
  const [search, setSearch] = useState<Record<string, number>>({});
  const [focus, setFocus] = useState<Record<string, number>>({});
  const bump = (set: typeof setSearch, t: Pane) => set((all) => ({ ...all, [keyOf(t)]: (all[keyOf(t)] ?? 0) + 1 }));
  // A sessão encerrada sai do dashboard; a aba guarda o último contexto conhecido.
  const known = useRef(new Map<string, TabInfo>());
  // Aba de uma sessão que já não estava no dashboard (aberta depois do fim,
  // ou a UI recarregou): a sessão vem do ledger por `session.get` (#283).
  const [fetched, setFetched] = useState<Record<string, Session | null>>({});
  const info: InfoOf = (t) => {
    const d = dashboards[t.env];
    const s = findSession(d, t.session);
    if (!s) {
      const cached = known.current.get(keyOf(t));
      if (cached) return cached;
      const f = fetched[keyOf(t)];
      return f ? { s: f, ...lookup(d, f) } : {};
    }
    const r = { s, ...lookup(d, s) };
    known.current.set(keyOf(t), r);
    return r;
  };

  // Remote Control (#197): pedidos de ligar em curso (aba → quando) e a
  // sessão cujo link e QR code estão abertos.
  const [rcAsked, setRcAsked] = useState<Record<string, { tab: Tab; at: number }>>({});
  const [rcShown, setRcShown] = useState<Tab | null>(null);
  const rcNow = useNow(Object.keys(rcAsked).length > 0 ? 1000 : 60_000);
  const rcOf = (t: Tab): RcView => rcView(info(t).s, rcAsked[keyOf(t)]?.at, rcNow);
  /** `env.info.remote_control`; `undefined` = daemon anterior ao protocolo 2.3. */
  const rcStatus = (t: Tab): RemoteControlStatus | undefined => infos[t.env]?.remote_control;
  const forgetAsk = (t: Tab) =>
    setRcAsked((all) => {
      const { [keyOf(t)]: _omit, ...rest } = all;
      return rest;
    });
  const enableRc = async (t: Tab) => {
    setRcAsked((all) => ({ ...all, [keyOf(t)]: { tab: t, at: Date.now() } }));
    try {
      const r = await api.call<RemoteControlEnable>(t.env, "session.remote_control.enable", { session_id: t.session });
      if (!r.requested) {
        // Já estava ligado: o dashboard traz o id.
        forgetAsk(t);
        void refresh(t.env);
        setRcShown(t);
      }
    } catch (e) {
      forgetAsk(t);
      throw e;
    }
  };
  // O id chegou: o pedido termina e o link com o QR code se abre.
  useEffect(() => {
    for (const { tab } of Object.values(rcAsked)) {
      if (rcView(info(tab).s, undefined, 0).kind !== "on") continue;
      forgetAsk(tab);
      setRcShown(tab);
    }
  });

  // Shells vivos (#423): `shell.list` de cada ambiente conectado (o app pode
  // ter recarregado com shells no daemon) e o push `shell_exited`.
  const connected = envs
    .filter((e) => status[e.id]?.connected)
    .map((e) => e.id)
    .join("|");
  useEffect(() => {
    for (const env of connected ? connected.split("|") : []) {
      void listShells(api, env).then((list) => setShells(env, list ?? []));
    }
  }, [api, connected]);
  useEffect(
    () =>
      api.onPush((e) => {
        if (e.event === "shell_exited" && typeof e.data?.shell_id === "string") dropShell(e.env, e.data.shell_id);
      }),
    [api],
  );
  // O erro de abrir fica até sair da tela.
  useEffect(() => () => setShellError(null), []);

  useEffect(() => {
    for (const t of tabs) {
      if (isShell(t)) continue;
      const k = keyOf(t);
      const d = dashboards[t.env];
      if (k in fetched || known.current.has(k) || !d || findSession(d, t.session)) continue;
      setFetched((all) => ({ ...all, [k]: null }));
      api
        .call<Session>(t.env, "session.get", { session_id: t.session })
        .then((s) => setFetched((all) => ({ ...all, [k]: s })))
        .catch(() => {});
    }
  }, [tabs, dashboards, api, fetched]);

  // Barra de status: ambiente e worktree (ou pasta, no gestor) da sessão ativa;
  // no shell, a pasta dele.
  const activeSession = sessionOf(active);
  const activeInfo = activeSession ? info(activeSession) : null;
  const statusEnv = active ? (envs.find((e) => e.id === active.env)?.name ?? active.env) : null;
  const statusPath =
    active && isShell(active)
      ? (shellStore.seen[active.shell]?.cwd ?? null)
      : activeInfo
        ? activeInfo.gestor
          ? activeInfo.s?.cwd
          : activeInfo.wi?.worktree_path
        : null;
  // Uso de contexto: só da sessão viva (a encerrada não muda mais).
  const activeLive =
    !!activeSession && !!dashboards[activeSession.env]?.live_sessions.some((x) => x.id === activeSession.session);
  const usage = useContextUsage(api, activeLive ? activeSession.env : null, activeLive ? activeSession.session : null);
  const context = usage ? contextText(usage.tokens, contextWindow(activeInfo?.s?.model ?? usage.model)) : null;
  const activeRc = activeSession ? rcView(activeInfo?.s, undefined, 0) : null;
  const remote = activeRc?.kind === "on" ? activeRc.url : null;
  const { setTermStatus } = shell;
  useEffect(() => {
    setTermStatus(statusEnv ? { env: statusEnv, worktree: statusPath ?? null, context, remote } : null);
  }, [setTermStatus, statusEnv, statusPath, context, remote]);
  useEffect(() => () => setTermStatus(null), [setTermStatus]);

  const groups = groupSessions(envs, dashboards, tabs, info, states, shellStore.live);

  // O que a faixa de abas e o "Fechar encerradas" precisam de cada aba
  // (#427): título, projeto, ambiente e se encerrou. Publicado no nav
  // porque a sessão encerrada sai do dashboard e a tela pode desmontar.
  const metaOf = (t: Pane): TabMeta => {
    const env = envs.find((e) => e.id === t.env)?.name ?? t.env;
    const of = (p: Project | undefined) => (p ? { key: projectKey(t.env, p), name: p.name } : null);
    if (isShell(t)) {
      const { wi, project } = shellItem(dashboards[t.env], t.workItemId);
      const list = shellStore.live[t.env];
      const ended = states[keyOf(t)]?.kind === "ended" || (!!list && !list.some((x) => x.id === t.shell));
      return { title: shellTitle(wi, project), project: of(project), env, ended };
    }
    const { s, wi, project, gestor, scope } = info(t);
    const p =
      gestor && scope?.kind === "project"
        ? dashboards[t.env]?.projects.find((x) => x.project.gh_owner === scope.owner && x.project.gh_repo === scope.repo)
            ?.project
        : project;
    const title = gestor
      ? gestorLabel(scope)
      : wi
        ? (projectSessionTitle(wi.gh_number, project) ?? itemTitle(wi, project))
        : "Sessão";
    return { title, project: gestor && scope?.kind !== "project" ? null : of(p), env, ended: isEnded(rowState(s, wi, states[keyOf(t)])) };
  };
  const { setMeta } = nav;
  useEffect(() => {
    setMeta(Object.fromEntries(tabs.map((t) => [paneTabId(t), metaOf(t)])));
  });
  // "Fechar encerradas" (cabeçalho da árvore) e "Fechar as abas deste projeto" (grupo).
  const endedIds = closeTargets(nav.editors, { how: "ended" }, nav.meta);
  const groupMenu = (g: TreeGroup): MenuItem[] => {
    if (!g.key.includes("|p")) return [];
    const ids = closeTargets(nav.editors, { how: "project", project: g.key }, nav.meta);
    return [
      {
        label: "Fechar as abas deste projeto",
        run: () => nav.closeTabs(ids),
        disabled: ids.length === 0,
        title: ids.length === 0 ? "Nenhuma aba aberta deste projeto" : undefined,
      },
    ];
  };
  // Só divide com outra sessão aberta (o reducer já garante; aqui, o que mostrar).
  const right = nav.split && tabs.some((t) => same(t, nav.split)) && !same(nav.split, active) ? nav.split : null;
  // "Dividir à direita": a próxima sessão aberta depois da ativa (ou a anterior).
  const idx = active ? tabs.findIndex((t) => same(t, active)) : -1;
  const other = tabs.length > 1 && idx >= 0 ? tabs[(idx + 1) % tabs.length] : null;

  // "Nova sessão": a modal de início no projeto da sessão ativa (sessão de
  // planejamento; os atalhos da modal não trocam de projeto). Sem sessão
  // ativa, a tela Projetos para escolher o item.
  const newSession = () => {
    const p = active && isShell(active) ? shellItem(dashboards[active.env], active.workItemId).project : activeInfo?.project;
    if (active && p) startSession({ env: active.env, projectId: p.id, owner: p.gh_owner, repo: p.gh_repo });
    else openScreen(nav, "project");
  };

  // Ctrl+Shift+` (como "novo terminal" no VS Code): shell novo no worktree
  // do item da aba ativa. Fase de captura: vale com o foco no console.
  const shellKey = useRef<() => void>(() => {});
  shellKey.current = () => {
    const t = shellTarget(active, info);
    if (t) void openShell(t.env, t.workItemId);
  };
  useEffect(() => {
    const on = (e: KeyboardEvent) => {
      if (!newShellShortcut(e)) return;
      e.preventDefault();
      e.stopPropagation();
      shellKey.current();
    };
    window.addEventListener("keydown", on, true);
    return () => window.removeEventListener("keydown", on, true);
  }, []);

  // Painel de contexto (#426; spec da #87, §3.1): aberto/fechado por
  // máquina; mostra a sessão da coluna com o foco (a 1 sem split). Na coluna
  // de um shell, não aparece.
  const [ctxOpen, setCtxOpenState] = useState(loadContextOpen);
  const setCtxOpen = (open: boolean) => {
    setCtxOpenState(open);
    saveContextOpen(open);
  };
  const [focusCol, setFocusCol] = useState<1 | 2>(1);
  const ctxCol: 1 | 2 = right && focusCol === 2 ? 2 : 1;
  const ctxTab = sessionOf(ctxCol === 2 ? right : active);
  // O foco vem do console (ou da faixa do pedido) que o usuário clicou; os
  // botões do cabeçalho escolhem a coluna pelo próprio "Contexto".
  const trackCol = (e: { target: EventTarget }) => {
    const el = e.target instanceof Element ? e.target.closest(".term-pane[data-col], .term-ask[data-col]") : null;
    const c = el?.getAttribute("data-col");
    if (c === "1" || c === "2") setFocusCol(c === "2" ? 2 : 1);
  };
  /** "Contexto" no cabeçalho da coluna: abre nela; aberto em outra coluna, passa para esta. */
  const contextFrom = (col: 1 | 2) => {
    if (ctxOpen && ctxCol === col) return setCtxOpen(false);
    setFocusCol(col);
    setCtxOpen(true);
  };
  // Ctrl+Alt+B (⌘⌥B): abre e fecha, na coluna com o foco. Captura: vale no console.
  const ctxKey = useRef<() => void>(() => {});
  ctxKey.current = () => setCtxOpen(!ctxOpen);
  useEffect(() => {
    const on = (e: KeyboardEvent) => {
      if (!contextShortcut(e)) return;
      e.preventDefault();
      e.stopPropagation();
      ctxKey.current();
    };
    window.addEventListener("keydown", on, true);
    return () => window.removeEventListener("keydown", on, true);
  }, []);

  // Menu de contexto de uma sessão na árvore: abrir e o Remote Control.
  const menuOf = (t: Tab): MenuItem[] => {
    const items: MenuItem[] = [{ label: "Abrir", run: () => nav.openTerminal(t) }];
    const live = !!dashboards[t.env]?.live_sessions.some((x) => x.id === t.session);
    const rc = rcOf(t);
    if (rc.kind === "on") {
      items.push(
        { label: "Mostrar link e QR code do Remote Control", run: () => setRcShown(t) },
        { label: "Abrir no Remote Control", run: () => void api.openUrl(rc.url) },
        { label: "Copiar link do Remote Control", run: () => void api.writeClipboard(rc.url) },
      );
    } else if (live && rc.kind !== "connecting" && rcStatus(t)?.available) {
      items.push({ label: "Ligar Remote Control", run: () => void enableRc(t).catch(() => {}) });
    }
    return items;
  };

  const side = (
    <SideContent>
      <SessionTree
        groups={groups}
        active={active}
        collapsed={nav.collapsed}
        onCollapse={nav.setCollapsed}
        onSelect={nav.openTerminal}
        onNew={newSession}
        menu={menuOf}
        groupMenu={groupMenu}
        onCloseEnded={endedIds.length > 0 ? () => nav.closeTabs(endedIds) : null}
      />
    </SideContent>
  );
  const shownRc = rcShown ? rcOf(rcShown) : null;
  const shownInfo = rcShown ? info(rcShown) : null;

  const splitOf = (col: 1 | 2): SplitAction | null =>
    col === 2
      ? {
          label: "Fechar divisão",
          icon: "close",
          run: () => nav.setSplit(null),
        }
      : right
        ? null
        : {
            label: "Dividir à direita",
            icon: "split",
            run: () => other && nav.setSplit(other),
            disabled: !other,
          };

  const pane = (t: Pane, col: 1 | 2) =>
    isShell(t) ? (
      <ShellBar
        key={`bar:${col}:${keyOf(t)}`}
        tab={t}
        col={col}
        state={states[keyOf(t)]}
        shell={shellStore.seen[t.shell]}
        onSearch={() => bump(setSearch, t)}
        onOpenShell={() => void openShell(t.env, t.workItemId)}
        split={splitOf(col)}
      />
    ) : (
    <PaneBar
      key={`bar:${col}:${keyOf(t)}`}
      tab={t}
      col={col}
      state={states[keyOf(t)]}
      info={info}
      rc={rcOf(t)}
      rcStatus={rcStatus(t)}
      onEnableRc={() => enableRc(t)}
      onShowRc={() => setRcShown(t)}
      onSearch={() => bump(setSearch, t)}
      onOpenShell={() => {
        const target = shellTarget(t, info);
        if (target) void openShell(target.env, target.workItemId);
      }}
      onPromptSent={() => bump(setFocus, t)}
      context={{ open: ctxOpen, here: ctxOpen && ctxCol === col, fromBar: () => contextFrom(col), toggle: () => setCtxOpen(!ctxOpen) }}
      split={splitOf(col)}
    />
  );

  return (
    <main className="terminal-screen">
      <h1 className="sr-only">Terminal</h1>
      {side}
      {!active && (
        <p className="empty term-pick">
          {groups.length === 0
            ? "Abra uma sessão pela Sala de controle ou pela tela Projetos."
            : "Selecione uma sessão na árvore do painel lateral."}
        </p>
      )}
      {shellStore.error && (
        <p role="alert" className="session-error term-pick">
          {shellStore.error}{" "}
          <button type="button" className="btn btn-sm" onClick={() => setShellError(null)}>
            Fechar
          </button>
        </p>
      )}
      {rcShown && shownRc?.kind === "on" && (
        <RemotePanel
          url={shownRc.url}
          title={
            shownInfo?.gestor
              ? gestorLabel(shownInfo.scope)
              : shownInfo?.wi
                ? itemTitle(shownInfo.wi)
                : "Sessão"
          }
          api={api}
          onClose={() => setRcShown(null)}
        />
      )}
      {active && (
        <div className="term-body" data-split={right ? "true" : undefined}>
        <div
          className="term-split"
          data-split={right ? "true" : undefined}
          onFocusCapture={trackCol}
          onPointerDownCapture={trackCol}
        >
          {pane(active, 1)}
          {right && pane(right, 2)}
          {/* Todos os consoles são filhos diretos daqui, na ordem das abas: ao
            trocar de aba ou dividir, só mudam de coluna ou se escondem. */}
          {tabs.map((t) => {
            const col = same(t, active) ? 1 : same(t, right) ? 2 : undefined;
            return (
              <TerminalView
                key={keyOf(t)}
                env={t.env}
                pty={ptyOf(t)}
                kind={isShell(t) ? "shell" : "session"}
                visible={col !== undefined}
                column={col}
                searchSignal={search[keyOf(t)]}
                focusSignal={focus[keyOf(t)]}
                onState={(s) => setStates((all) => ({ ...all, [keyOf(t)]: s }))}
                onClose={isShell(t) ? undefined : () => nav.closeTab(t)}
              />
            );
          })}
          {activeSession && (
            <AskStrip
              tab={activeSession}
              col={1}
              state={states[keyOf(activeSession)]}
              info={info}
              onAnswer={() => bump(setFocus, activeSession)}
            />
          )}
          {right && !isShell(right) && (
            <AskStrip
              tab={right}
              col={2}
              state={states[keyOf(right)]}
              info={info}
              onAnswer={() => bump(setFocus, right)}
            />
          )}
        </div>
        {ctxOpen && ctxTab && (
          <ContextPanel key={keyOf(ctxTab)} tab={ctxTab} info={info(ctxTab)} onClose={() => setCtxOpen(false)} />
        )}
        </div>
      )}
    </main>
  );
}

/** Estado do painel: o fim visto pelo console, o ambiente fora ou o da sessão. */
function paneState(state: TermState | undefined, s: Session | undefined, info: TabInfo, offline: boolean) {
  if (state?.kind === "ended") return { text: state.message, tone: "neutral" };
  if (offline) return { text: "ambiente desconectado · reconectando…", tone: "error" };
  return s ? tileState(s, info.wi?.status) : { text: "—", tone: "neutral" };
}

/**
 * Pedido pendente da sessão, logo abaixo do console. Pedido de permissão
 * segurado pelo daemon (#365): a ferramenta e o argumento, quanto falta para
 * voltar ao menu do terminal e Negar / Sempre nesta sessão / Permitir;
 * "Responder no terminal" devolve o pedido ao menu (`ask`) e foca o console.
 * Sem pedido de permissão (pergunta, ociosidade), aparece enquanto o work
 * item espera você, com o texto que o agente mandou (push `needs_human`).
 */
function AskStrip({
  tab,
  col,
  state,
  info,
  onAnswer,
}: {
  tab: Tab;
  col: 1 | 2;
  state?: TermState;
  info: InfoOf;
  onAnswer: () => void;
}) {
  const { asks, dashboards, permissions, respondPermission } = useTower();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const pending = (permissions[tab.env] ?? []).filter((p) => p.session_id === tab.session);
  const req = pending[0];
  // A contagem regressiva só anda com um pedido na faixa.
  const now = useNow(req ? 1000 : 30_000);
  const { s, wi } = info(tab);
  const live = dashboards[tab.env]?.live_sessions.some((x) => x.id === tab.session);
  if (!s || !live || state?.kind === "ended") return null;

  if (req) {
    const answer = async (decision: PermissionDecision) => {
      setBusy(true);
      setError(null);
      try {
        await respondPermission(tab.env, req.request_id, decision);
        if (decision === "ask") onAnswer();
      } catch (e) {
        setError(String(e));
      } finally {
        setBusy(false);
      }
    };
    const more = pending.length - 1;
    return (
      <div className="term-ask" role="group" aria-label="Pedido de permissão" data-col={col} style={{ gridColumn: col }}>
        <span className="term-ask-kind">Permissão</span>
        <span className="term-ask-text mono" title={permissionText(req, Infinity)}>
          {permissionText(req)}
        </span>
        {more > 0 && <span className="muted">{`+${more} na fila`}</span>}
        <span className="muted" title="Sem resposta, o pedido volta ao menu do terminal">
          {`terminal em ${remaining(req.expires_at, now)}`}
        </span>
        <span className="term-ask-actions">
          <button type="button" className="btn btn-sm" disabled={busy} onClick={() => answer("deny")}>
            Negar
          </button>
          <button type="button" className="btn btn-sm" disabled={busy} onClick={() => answer("allow_session")}>
            Sempre nesta sessão
          </button>
          <button type="button" className="btn btn-primary btn-sm" disabled={busy} onClick={() => answer("allow")}>
            Permitir
          </button>
          <button type="button" className="btn btn-sm" disabled={busy} onClick={() => answer("ask")}>
            Responder no terminal
          </button>
        </span>
        {error && (
          <p role="alert" className="session-error">
            {error}
          </p>
        )}
      </div>
    );
  }

  if (wi?.status !== "aguardando_voce") return null;
  const ask = asks[askKey(tab.env, tab.session)];
  return (
    <div className="term-ask" role="group" aria-label="Pedido pendente" data-col={col} style={{ gridColumn: col }}>
      <span className="term-ask-kind">Aguardando você</span>
      <span className="term-ask-text">{ask?.message || ASK_FALLBACK}</span>
      <span className="muted">{ago(ask?.at ?? s.last_event_at, now)}</span>
      <button type="button" className="btn btn-primary btn-sm" onClick={onAnswer}>
        Responder no terminal
      </button>
    </div>
  );
}

interface SplitAction {
  label: string;
  icon: "split" | "close";
  run(): void;
  disabled?: boolean;
}

/**
 * Cabeçalho do painel de uma sessão: estado, título, ambiente, branch,
 * ferramenta e modelo. Botões das ações frequentes (Buscar, Enviar prompt
 * pronto (#425, §2.4), Contexto (#426, §3), Abrir shell no worktree, Remote Control, Retomar,
 * dividir e Encerrar) e o menu ⋯ com as
 * demais (#424; spec da #87, §2.1): Abrir issue (no `#0`, o repositório),
 * Abrir PR (ou Revisar decisões do gestor), Abrir item no projeto, Abrir no
 * VS Code e Reiniciar. O da coluna 1 (a sessão ativa) publica as ações para
 * a paleta de comandos.
 */
function PaneBar({
  tab,
  col,
  state,
  info,
  rc,
  rcStatus,
  onEnableRc,
  onShowRc,
  onSearch,
  onOpenShell,
  onPromptSent,
  context,
  split,
}: {
  tab: Tab;
  col: 1 | 2;
  state?: TermState;
  info: InfoOf;
  onOpenShell: () => void;
  /** O prompt pronto foi enviado: o foco volta ao console. */
  onPromptSent: () => void;
  rc: RcView;
  rcStatus?: RemoteControlStatus;
  onEnableRc: () => Promise<void>;
  onShowRc: () => void;
  onSearch: () => void;
  /**
   * Painel de contexto (#426): aberto (em alguma coluna), aberto nesta;
   * `fromBar` = o botão desta coluna; `toggle` = a paleta.
   */
  context: { open: boolean; here: boolean; fromBar: () => void; toggle: () => void };
  split: SplitAction | null;
}) {
  const { envs, api, status, dashboards, config, permissions } = useTower();
  const nav = useNav();
  const startSession = useStartSession();
  const now = useNow();
  const [confirm, setConfirm] = useState<"kill" | "restart" | null>(null);
  const [error, setError] = useState<string | null>(null);
  // Erro do `session.resume` (ex.: transcript perdido): vem com "Abrir Recuperação".
  const [resumeError, setResumeError] = useState<string | null>(null);
  const [resuming, setResuming] = useState(false);
  const [menu, setMenu] = useState<{ x: number; y: number } | null>(null);
  const moreRef = useRef<HTMLButtonElement>(null);
  // Menu "Enviar prompt pronto" (#425).
  const [promptMenu, setPromptMenu] = useState<{ x: number; y: number } | null>(null);
  const promptRef = useRef<HTMLButtonElement>(null);
  const ti = info(tab);
  const { s, wi, project, gestor, scope } = ti;
  const env = envs.find((e) => e.id === tab.env);
  const ended = state?.kind === "ended";
  const live = !ended && !!dashboards[tab.env]?.live_sessions.some((x) => x.id === tab.session);
  const st = paneState(state, s, ti, status[tab.env]?.connected === false);
  const actions = sessionActions({ info: ti, ended, vscode: env?.kind === "wsl" });
  // Prompts prontos (§2.4): desligados com a sessão encerrada; com um pedido
  // de permissão pendente o menu avisa e não envia.
  const permissionPending = (permissions[tab.env] ?? []).some((p) => p.session_id === tab.session);
  const guard = { live, permissionPending };
  const prompts = promptActions(config.prompts, promptBlocked(guard));
  const resumable = actions.some((a) => a.id === "resume");
  const title = gestor ? gestorLabel(scope) : wi ? itemTitle(wi, project) : "Sessão";

  const run = async (f: () => Promise<unknown>) => {
    setError(null);
    try {
      await f();
    } catch (e) {
      setError(String(e));
    }
  };

  const openPr = () =>
    run(async () => {
      if (!wi || !project) return;
      const review = await api.call<Review>(tab.env, "review.get", {
        work_item_id: wi.id,
      });
      const url = prUrl(review, project, project.base_branch);
      if (!url) throw new Error("o work item ainda não tem branch");
      await api.openUrl(url);
    });

  // Retomar (§2.2): `session.resume` direto, com o modelo e o modo da sessão
  // (o daemon os herda); a nova ocupa a aba e a coluna da antiga.
  const resume = async () => {
    setError(null);
    setResumeError(null);
    setResuming(true);
    try {
      const n = await api.call<Session>(tab.env, "session.resume", {
        session_id: tab.session,
        prompt: resumePrompt(!!(ended && state.recoverable), RESUME_PROMPT),
      });
      nav.replaceTerminal(tab, { env: tab.env, session: n.id });
    } catch (e) {
      setResumeError(String(e));
      setResuming(false);
    }
  };

  // Reiniciar (§2.2): a modal de início pré-preenchida; com a sessão viva,
  // confirma antes (a modal encerra a antiga ao iniciar a nova).
  const openRestart = () => {
    setConfirm(null);
    const t = restartTarget(tab, ti, live);
    if (t) startSession(t, moreRef.current);
  };
  const restart = () => (live ? setConfirm("restart") : openRestart());

  const sendPrompt = (id: string) => {
    const p = findPrompt(config.prompts, id);
    if (!p) return;
    void run(async () => {
      await sendReadyPrompt(api, tab.env, tab.session, renderPrompt(p.text, ti, env?.name ?? tab.env), guard);
      onPromptSent();
    });
  };

  const runAction = (id: SessionActionId) => {
    const pid = promptIdOf(id);
    if (pid !== null) return sendPrompt(pid);
    switch (id) {
      case "resume":
        return void resume();
      case "open-issue":
      case "open-repo": {
        const url = actionUrl(id, ti);
        return void (url && run(() => api.openUrl(url)));
      }
      case "open-pr":
        return void openPr();
      case "review":
        return s && nav.go({ name: "review", env: tab.env, workItemId: s.work_item_id });
      case "open-item":
        return (
          wi &&
          project &&
          nav.go({ name: "project", env: tab.env, projectId: project.id, item: { ghNumber: wi.gh_number } })
        );
      case "vscode": {
        const path = vscodePath(ti);
        return void (path && run(() => api.openVsCode(tab.env, path)));
      }
      case "restart":
        return restart();
      case "context":
        return context.toggle();
    }
  };

  // Paleta de comandos: as ações da sessão ativa (coluna 1), pelo store de módulo.
  const runRef = useRef(runAction);
  runRef.current = runAction;
  const all = [...actions, contextAction(context.open), ...prompts];
  const actionsRef = useRef(all);
  actionsRef.current = all;
  const sig = all.map((a) => `${a.id}:${a.label}:${a.disabled ? 1 : 0}`).join("|");
  useEffect(() => {
    if (col !== 1) return;
    const p = { title, actions: actionsRef.current, run: (id: SessionActionId) => runRef.current(id) };
    publishSessionActions(p);
    return () => unpublishSessionActions(p);
  }, [col, sig, title]);

  const btn = (label: string, icon: Parameters<typeof Icon>[0]["name"], onClick: () => void, disabled = false) => (
    <button type="button" className="ibtn" aria-label={label} title={label} disabled={disabled} onClick={onClick}>
      <Icon name={icon} size={15} />
    </button>
  );

  const menuItems: MenuItem[] = actions
    .filter((a) => a.place === "menu")
    .map((a) => ({ label: a.label, disabled: a.disabled, title: a.title, run: () => runAction(a.id) }));
  // Alinhado à direita do botão, logo abaixo (o menu tem 200 px no mínimo).
  const below = (b: HTMLButtonElement | null) => {
    const r = b?.getBoundingClientRect();
    return r ? { x: Math.max(4, r.right - 220), y: r.bottom + 2 } : null;
  };
  const openMenu = () => setMenu(below(moreRef.current));
  const promptItems: MenuItem[] = prompts.map((a) => ({
    label: a.label.replace(/^Enviar prompt — /, ""),
    disabled: a.disabled,
    title: a.title,
    run: () => runAction(a.id),
  }));
  const promptOff = !live ? "A sessão está encerrada" : prompts.length === 0 ? "Nenhum prompt pronto nas Configurações" : null;

  return (
    <section
      aria-label={col === 1 ? "Sessão" : "Sessão à direita"}
      className="pane-bar"
      data-col={col}
      style={{ gridColumn: col }}
    >
      <div className="pane-id">
        <span className="badge session-state" data-tone={st.tone}>
          {st.text}
        </span>
        <span className="session-title">{title}</span>
        <span className="pane-meta">
          <span>{gestor ? `hub inteiro · ${env?.name ?? tab.env}` : (env?.name ?? tab.env)}</span>
          {!gestor && project && (
            <span>
              {project.name}
              {wi?.kind === "epic" ? " · épico" : ""}
            </span>
          )}
          {wi?.branch && <span className="mono">{wi.branch}</span>}
          {s && <span>{`${s.tool} · ${duration((s.ended_at ?? now) - s.started_at)}`}</span>}
          {s && <span>{agentLabel(s)}</span>}
        </span>
      </div>
      <div className="pane-actions">
        {btn("Buscar no terminal", "search", onSearch)}
        <button
          ref={promptRef}
          type="button"
          className="ibtn"
          aria-label="Enviar prompt pronto"
          title={promptOff ?? "Enviar prompt pronto"}
          aria-haspopup="menu"
          aria-expanded={promptMenu ? true : undefined}
          disabled={!!promptOff}
          onClick={() => setPromptMenu(promptMenu ? null : below(promptRef.current))}
        >
          <Icon name="send" size={15} />
        </button>
        <button
          type="button"
          className="ibtn"
          aria-label="Contexto"
          title={`Contexto (${isMac() ? "⌘⌥B" : "Ctrl+Alt+B"})`}
          aria-pressed={context.here}
          onClick={context.fromBar}
        >
          <Icon name="context" size={15} />
        </button>
        {/* Sem pasta de item no gestor; o #0 abre no clone principal. */}
        {!gestor &&
          btn(
            "Abrir shell no worktree",
            "prompt",
            onOpenShell,
            !wi || (!wi.worktree_path && wi.gh_number !== PROJECT_ITEM),
          )}
        <RcControl
          rc={rc}
          status={rcStatus}
          live={live}
          onEnable={() => run(onEnableRc)}
          onShow={onShowRc}
        />
        {resumable ? (
          <button type="button" className="btn btn-primary btn-sm" disabled={resuming} onClick={() => void resume()}>
            {resuming ? "Retomando…" : "Retomar"}
          </button>
        ) : (
          ended &&
          state.recoverable && (
            <button type="button" className="btn btn-primary btn-sm" onClick={() => nav.go({ name: "recovery", env: tab.env, session: tab.session })}>
              Abrir Recuperação
            </button>
          )
        )}
        <button
          ref={moreRef}
          type="button"
          className="ibtn"
          aria-label="Mais ações"
          title="Mais ações"
          aria-haspopup="menu"
          aria-expanded={menu ? true : undefined}
          disabled={menuItems.length === 0}
          onClick={() => (menu ? setMenu(null) : openMenu())}
        >
          <Icon name="more" size={15} />
        </button>
        {split && btn(split.label, split.icon, split.run, split.disabled)}
        {btn("Encerrar sessão", "stop", () => setConfirm("kill"), ended || !!confirm)}
      </div>
      {menu && (
        <ContextMenu
          x={menu.x}
          y={menu.y}
          label="Ações da sessão"
          items={menuItems}
          onClose={(restore) => {
            setMenu(null);
            if (restore) moreRef.current?.focus();
          }}
        />
      )}
      {promptMenu && !promptOff && (
        <ContextMenu
          x={promptMenu.x}
          y={promptMenu.y}
          label="Prompts prontos"
          items={promptItems}
          note={permissionPending ? PERMISSION_PENDING : undefined}
          onClose={(restore) => {
            setPromptMenu(null);
            if (restore) promptRef.current?.focus();
          }}
        />
      )}
      {confirm === "kill" && (
        <span role="group" aria-label="Confirmar encerramento" className="confirm">
          <span>Encerrar mata o processo do agente.</span>
          <button
            type="button"
            className="btn btn-danger btn-sm"
            onClick={() =>
              run(async () => {
                setConfirm(null);
                await api.call(tab.env, "session.kill", {
                  session_id: tab.session,
                });
              })
            }
          >
            Confirmar
          </button>
          <button type="button" className="btn btn-sm" onClick={() => setConfirm(null)}>
            Cancelar
          </button>
        </span>
      )}
      {confirm === "restart" && (
        <span role="group" aria-label="Confirmar reinício" className="confirm">
          <span>Reiniciar encerra o processo atual do agente e começa uma conversa nova.</span>
          <button type="button" className="btn btn-danger btn-sm" onClick={openRestart}>
            Continuar
          </button>
          <button type="button" className="btn btn-sm" onClick={() => setConfirm(null)}>
            Cancelar
          </button>
        </span>
      )}
      {resumeError && (
        <p role="alert" className="session-error">
          {`Não foi possível retomar: ${resumeError} `}
          <button
            type="button"
            className="btn btn-sm"
            onClick={() => nav.go({ name: "recovery", env: tab.env, session: tab.session })}
          >
            Abrir Recuperação
          </button>
        </p>
      )}
      {error && (
        <p role="alert" className="session-error">
          {error}
        </p>
      )}
    </section>
  );
}

/**
 * Cabeçalho do shell livre (#423; spec da #87, §4.4): enxuto, sem selo de
 * estado, faixa de pedido, contexto nem prompts. "Shell · #87", pasta,
 * programa e duração ("clone principal" no #0); ações Buscar, Abrir no VS
 * Code, dividir e Fechar. Fechar encerra o processo (`shell.close`), com
 * confirmação se há um comando rodando (`busy`, consultado na hora pelo
 * `shell.list`); o × da aba passa por aqui (`nav.closing`). No fim do
 * processo: "Abrir novo shell" e "Fechar".
 */
function ShellBar({
  tab,
  col,
  state,
  shell,
  onSearch,
  onOpenShell,
  split,
}: {
  tab: ShellTab;
  col: 1 | 2;
  state?: TermState;
  /** O shell visto no `shell.open`/`shell.list` (pasta, programa, início). */
  shell?: Shell;
  onSearch: () => void;
  onOpenShell: () => void;
  split: SplitAction | null;
}) {
  const { envs, api, dashboards } = useTower();
  const nav = useNav();
  const now = useNow();
  const [confirm, setConfirm] = useState<"busy" | "unknown" | null>(null);
  const [checking, setChecking] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const env = envs.find((e) => e.id === tab.env);
  const { wi, project } = shellItem(dashboards[tab.env], tab.workItemId);
  const ended = state?.kind === "ended";
  const id = paneTabId(tab);

  const close = async () => {
    setConfirm(null);
    // Já saiu (ou o ambiente caiu): a aba fecha do mesmo jeito.
    await api.call(tab.env, "shell.close", { shell_id: tab.shell }).catch(() => {});
    nav.closeEditor(id, true);
  };
  const askClose = async () => {
    if (ended) {
      nav.closeEditor(id, true);
      return;
    }
    setChecking(true);
    const check = closeCheck(await listShells(api, tab.env), tab.shell);
    setChecking(false);
    if (check === "close") await close();
    else setConfirm(check);
  };
  const askRef = useRef(askClose);
  askRef.current = askClose;
  // Fechar pela aba (× ou clique do meio): o reducer ativa a aba e marca o pedido.
  const asked = nav.closing === id;
  const { cancelClose } = nav;
  useEffect(() => {
    if (!asked) return;
    cancelClose();
    void askRef.current();
  }, [asked, cancelClose]);

  const btn = (label: string, icon: Parameters<typeof Icon>[0]["name"], onClick: () => void, disabled = false) => (
    <button type="button" className="ibtn" aria-label={label} title={label} disabled={disabled} onClick={onClick}>
      <Icon name={icon} size={15} />
    </button>
  );

  return (
    <section
      aria-label={col === 1 ? "Shell" : "Shell à direita"}
      className="pane-bar"
      data-col={col}
      style={{ gridColumn: col }}
    >
      <div className="pane-id">
        {ended && (
          <span className="badge session-state" data-tone="neutral">
            {state.message}
          </span>
        )}
        <span className="session-title pane-shell-title">
          <Icon name="prompt" size={14} />
          {shellTitle(wi, project)}
        </span>
        <span className="pane-meta">
          {shell && <span className="mono">{shell.cwd}</span>}
          {wi?.gh_number === PROJECT_ITEM && (
            <span className="pane-warn" title="O shell roda no clone principal do projeto, não num worktree isolado">
              clone principal
            </span>
          )}
          {shell && <span>{programName(shell.program)}</span>}
          {shell && !ended && <span>{duration(now - shell.started_at)}</span>}
        </span>
      </div>
      <div className="pane-actions">
        {btn("Buscar no terminal", "search", onSearch)}
        {btn(
          "Abrir no VS Code",
          "code",
          () => {
            setError(null);
            api.openVsCode(tab.env, shell!.cwd).catch((e) => setError(String(e)));
          },
          !shell || env?.kind !== "wsl",
        )}
        {split && btn(split.label, split.icon, split.run, split.disabled)}
        {ended ? (
          <>
            <button type="button" className="btn btn-primary btn-sm" onClick={onOpenShell}>
              Abrir novo shell
            </button>
            <button type="button" className="btn btn-sm" onClick={() => nav.closeEditor(id, true)}>
              Fechar
            </button>
          </>
        ) : (
          btn("Fechar shell", "stop", () => void askClose(), checking || !!confirm)
        )}
      </div>
      {confirm && (
        <span role="group" aria-label="Confirmar fechamento" className="confirm">
          <span>
            {confirm === "busy"
              ? "Há um comando rodando no shell. Fechar encerra o processo."
              : "Não deu para saber se há um comando rodando no shell. Fechar encerra o processo."}
          </span>
          <button type="button" className="btn btn-danger btn-sm" onClick={() => void close()}>
            Fechar
          </button>
          <button type="button" className="btn btn-sm" onClick={() => setConfirm(null)}>
            Cancelar
          </button>
        </span>
      )}
      {error && (
        <p role="alert" className="session-error">
          {error}
        </p>
      )}
    </section>
  );
}

/**
 * Remote Control no cabeçalho do painel (#197): ligado, o selo abre o link e
 * o QR code; conectando, o aviso; sem o id em 30 s, "não conectou" e o botão
 * de novo. Indisponível no ambiente, o botão fica desabilitado com o motivo.
 * Daemon anterior ao protocolo 2.3 (sem `remote_control` no `env.info`):
 * nada aparece.
 */
function RcControl({
  rc,
  status,
  live,
  onEnable,
  onShow,
}: {
  rc: RcView;
  status?: RemoteControlStatus;
  live: boolean;
  onEnable: () => void;
  onShow: () => void;
}) {
  if (rc.kind === "on")
    return (
      <button
        type="button"
        className="rc-chip"
        data-state="on"
        aria-label="Remote Control conectado: mostrar link e QR code"
        title={rc.url}
        onClick={onShow}
      >
        <span className="sb-dot" data-on="true" aria-hidden="true" />
        Remote Control
      </button>
    );
  if (rc.kind === "connecting")
    return (
      <span className="rc-chip" data-state="connecting" role="status">
        Remote Control · conectando…
      </span>
    );
  if (!live || !status) return null;
  const label = "Ligar Remote Control";
  return (
    <>
      {rc.kind === "failed" && (
        <span
          className="rc-chip"
          data-state="failed"
          role="status"
          title="O link não chegou em 30 s. Confira no terminal o login do claude.ai."
        >
          Remote Control não conectou
        </span>
      )}
      <button
        type="button"
        className="ibtn"
        aria-label={label}
        title={status.available ? label : (status.reason ?? "Remote Control indisponível neste ambiente")}
        disabled={!status.available}
        onClick={onEnable}
      >
        <Icon name="remote" size={15} />
      </button>
    </>
  );
}
