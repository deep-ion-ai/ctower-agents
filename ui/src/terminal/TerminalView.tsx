import { useEffect, useRef, useState } from "react";
import { useTower } from "../state/tower";
import { shellEndText, TerminalAdapter, type PtyKind, type TermState } from "./adapter";
import { effectiveSize, termPrefs, useTermPrefs, zoom } from "./prefs";
import { SearchBar } from "./SearchBar";
import { rememberTermSize } from "./size";
import { mountTerminal, type TermFont } from "./xterm";

/** Espera antes de fechar a aba da sessão que saiu sem erro (#432). */
export const CLOSE_DELAY_MS = 1000;
/** Interagir com o console nesse intervalo cancela o fechamento. */
const CANCEL_CLOSE = ["keydown", "pointerdown", "wheel"] as const;

/**
 * Um xterm ligado a um PTY do daemon: uma sessão do agente ou um shell
 * (`kind`, #423; muda só o fim, pelo push `shell_exited`). Só anexa quando a aba aparece pela primeira
 * vez (escondida, o xterm não mede a fonte e o replay sairia no tamanho
 * errado); depois fica montado enquanto a aba existe (as abas escondidas
 * mantêm o histórico) e reage ao fim da sessão e à perda do attach.
 *
 * Com a divisão (#316) duas ficam visíveis ao mesmo tempo: `column` diz em
 * que coluna da grade o console aparece (o mesmo elemento muda de coluna,
 * sem remontar). `searchSignal`/`focusSignal`: cada valor novo abre a busca
 * ou põe o foco no console (botões do cabeçalho e da faixa do pedido).
 *
 * A sessão que sai sem erro (`/exit`, código 0) fecha a aba com `onClose`
 * depois de `CLOSE_DELAY_MS`, se a preferência estiver ligada (#432); teclar,
 * clicar ou rolar o console nesse meio-tempo deixa a aba aberta.
 */
export function TerminalView({
  env,
  pty,
  kind = "session",
  visible,
  onState,
  onClose,
  column,
  searchSignal = 0,
  focusSignal = 0,
}: {
  env: string;
  /** Id do PTY: o da sessão ou o do shell. */
  pty: string;
  kind?: PtyKind;
  visible: boolean;
  onState: (s: TermState) => void;
  onClose?: () => void;
  column?: 1 | 2;
  searchSignal?: number;
  focusSignal?: number;
}) {
  const { api } = useTower();
  const pane = useRef<HTMLDivElement>(null);
  const el = useRef<HTMLDivElement>(null);
  const mounted = useRef<ReturnType<typeof mountTerminal> | null>(null);
  const adapter = useRef<TerminalAdapter | null>(null);
  const started = useRef(false);
  const [searching, setSearching] = useState(false);
  const prefs = useTermPrefs();
  const font: TermFont = { family: prefs.fontFamily, size: effectiveSize(prefs) };
  const fontRef = useRef(font);
  fontRef.current = font;
  const applied = useRef<TermFont | null>(null);
  const visibleRef = useRef(visible);
  visibleRef.current = visible;
  const stateCb = useRef(onState);
  stateCb.current = onState;
  const closeCb = useRef(onClose);
  closeCb.current = onClose;

  useEffect(() => {
    const m = mountTerminal(el.current!, {
      font: fontRef.current,
      openUrl: (url) => api.openUrl(url),
      clipboard: { read: () => api.readClipboard(), write: (text) => api.writeClipboard(text) },
      onUiAction: (a) => {
        if (a === "search") setSearching(true);
        // O zoom vale para todas as abas (preferência comum).
        else zoom(a === "zoomIn" ? 1 : a === "zoomOut" ? -1 : 0);
      },
    });
    mounted.current = m;
    applied.current = fontRef.current;
    let closeTimer: ReturnType<typeof setTimeout> | null = null;
    const cancelClose = () => {
      if (closeTimer) clearTimeout(closeTimer);
      closeTimer = null;
      for (const ev of CANCEL_CLOSE) pane.current?.removeEventListener(ev, cancelClose, true);
    };
    const a = new TerminalAdapter(
      api,
      env,
      pty,
      m.term,
      (s) => {
        stateCb.current(s);
        // Só a sessão do agente marca `clean`: o shell encerrado fica com
        // "shell encerrado (código n)" e as ações (spec do épico #429, §4.3).
        if (s.kind !== "ended" || !s.clean || !termPrefs().closeOnExit) return;
        for (const ev of CANCEL_CLOSE) pane.current?.addEventListener(ev, cancelClose, true);
        closeTimer = setTimeout(() => {
          cancelClose();
          closeCb.current?.();
        }, CLOSE_DELAY_MS);
      },
      kind,
    );
    adapter.current = a;
    started.current = false;
    const off = api.onPush((e) => {
      if (e.env !== env) return;
      if (kind === "shell") {
        if (e.event === "shell_exited" && e.data?.shell_id === pty) a.exited(e.data.code ?? null);
        // Depois de reconectar o shell não estava mais no daemon (saiu enquanto isso).
        if (e.event === "attach_lost" && e.data?.session_id === pty) a.end(shellEndText());
        return;
      }
      if (e.data?.session_id !== pty) return;
      if (e.event === "session_exited") a.exited(e.data.exit_code ?? null, e.data.state);
      if (e.event === "attach_lost") {
        a.end(`sessão indisponível após reconectar: ${e.data.reason ?? "conexão perdida"}. Veja a Recuperação`, true);
      }
    });
    const ro = new ResizeObserver(() => {
      if (!started.current || el.current?.offsetParent === null) return; // aba escondida
      const size = m.fit();
      rememberTermSize(size);
      a.resize(size.cols, size.rows);
    });
    ro.observe(el.current!);
    return () => {
      cancelClose();
      ro.disconnect();
      off();
      a.dispose();
      m.dispose();
      mounted.current = null;
      adapter.current = null;
    };
  }, [api, env, pty, kind]);

  useEffect(() => {
    const m = mounted.current;
    const a = adapter.current;
    if (!visible || !m || !a) return;
    const size = m.fit();
    rememberTermSize(size);
    if (!started.current) {
      started.current = true;
      void a.start(size);
    } else {
      a.resize(size.cols, size.rows);
    }
    m.focus();
  }, [visible, api, env, pty]);

  // Fonte ou zoom mudou: a aba visível se reajusta já; as escondidas, ao aparecer.
  useEffect(() => {
    const m = mounted.current;
    const a = adapter.current;
    const last = applied.current;
    if (!m || !a || (last && last.family === font.family && last.size === font.size)) return;
    m.setFont(font);
    applied.current = font;
    if (!started.current || !visibleRef.current) return;
    const size = m.fit();
    rememberTermSize(size);
    a.resize(size.cols, size.rows);
  }, [font.family, font.size]);

  useEffect(() => {
    if (searchSignal > 0) setSearching(true);
  }, [searchSignal]);

  useEffect(() => {
    if (focusSignal > 0) mounted.current?.focus();
  }, [focusSignal]);

  const closeSearch = () => {
    setSearching(false);
    mounted.current?.focus();
  };

  return (
    <div
      ref={pane}
      className="term-pane"
      hidden={!visible}
      data-col={visible ? column : undefined}
      style={visible && column ? { gridColumn: column } : undefined}
    >
      {searching && mounted.current && <SearchBar search={mounted.current.search} onClose={closeSearch} />}
      <div ref={el} className="term-host" role="region" aria-label={kind === "shell" ? "Terminal do shell" : "Terminal da sessão"} />
    </div>
  );
}
