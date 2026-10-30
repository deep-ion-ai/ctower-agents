// Criação do xterm (isolada para os testes trocarem por um fake via vi.mock).
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { SearchAddon } from "@xterm/addon-search";
import { Unicode11Addon } from "@xterm/addon-unicode11";
import { WebglAddon } from "@xterm/addon-webgl";
import { WebLinksAddon } from "@xterm/addon-web-links";
import type { TermLike } from "./adapter";
import { contextAction, keyAction, runAction, type Clipboard, type UiAction } from "./keys";
import { linkActivator } from "./links";
import { loadWebgl } from "./renderer";
import { SEARCH_DECORATIONS, TERM_THEME } from "./theme";

export interface MountedTerm {
  term: TermLike;
  /** Ajusta ao contêiner e devolve o tamanho em células. */
  fit(): { cols: number; rows: number };
  focus(): void;
  /** Troca a fonte; quem chama ajusta o tamanho em seguida. */
  setFont(font: TermFont): void;
  search: TermSearch;
  dispose(): void;
}

/** Busca no buffer (addon `search`), com destaque e contador. */
export interface TermSearch {
  /** `incremental`: ao digitar, expande a ocorrência atual em vez de pular. */
  next(query: string, caseSensitive: boolean, incremental?: boolean): void;
  previous(query: string, caseSensitive: boolean): void;
  clear(): void;
  /** `index` é -1 quando não há ocorrência ativa. */
  onResults(cb: (index: number, count: number) => void): { dispose(): void };
}


export interface TermFont {
  family: string;
  size: number;
}

export interface MountOptions {
  font: TermFont;
  /** Abre um link http(s) no navegador (Ctrl/Cmd+clique). */
  openUrl(url: string): Promise<void>;
  clipboard: Clipboard;
  /** Atalho que mexe na tela, como Ctrl+F e o zoom. */
  onUiAction(a: UiAction): void;
}

export function mountTerminal(el: HTMLElement, opts: MountOptions): MountedTerm {
  const activate = linkActivator(opts.openUrl, (e) => console.error("não foi possível abrir o link", e));
  const failed = (e: unknown) => console.error("área de transferência indisponível", e);
  // Link sob o mouse, para o clique direito copiar o endereço.
  let hovered: string | null = null;
  const hover = (_: MouseEvent, uri: string) => (hovered = uri);
  const leave = () => (hovered = null);
  const term = new Terminal({
    fontFamily: opts.font.family,
    fontSize: opts.font.size,
    lineHeight: 1.25,
    cursorBlink: true,
    scrollback: 10_000,
    theme: TERM_THEME,
    // OSC 8: sem isso o xterm pede confirm() e chama window.open, que o Tauri bloqueia.
    linkHandler: { activate: (e, uri) => activate(e, uri), hover, leave },
    // Shift+arrastar (Option no macOS) seleciona mesmo com o modo mouse do agente.
    macOptionClickForcesSelection: true,
    rightClickSelectsWord: false,
    // Destaques da busca (decorações) e unicode11 usam a API proposta do xterm.
    allowProposedApi: true,
  });
  const fit = new FitAddon();
  term.loadAddon(fit);
  // Largura certa de emojis e símbolos usados pelo Claude Code.
  term.loadAddon(new Unicode11Addon());
  term.unicode.activeVersion = "11";
  const search = new SearchAddon();
  term.loadAddon(search);
  term.loadAddon(new WebLinksAddon((e, uri) => activate(e, uri), { hover, leave }));
  term.attachCustomKeyEventHandler((e) => {
    const a = keyAction(e, term.hasSelection());
    if (!a) return true;
    // keypress/keyup do mesmo atalho também não chegam ao xterm.
    e.preventDefault();
    if (e.type !== "keydown") return false;
    if (a === "copy" || a === "paste" || a === "newline") runAction(a, term, opts.clipboard).catch(failed);
    else opts.onUiAction(a);
    return false;
  });
  const onContext = (e: MouseEvent) => {
    e.preventDefault();
    contextAction(term, hovered, opts.clipboard).catch(failed);
  };
  el.addEventListener("contextmenu", onContext);
  term.open(el);
  // Depois do open: o WebGL precisa do elemento no documento.
  loadWebgl(
    (a) => term.loadAddon(a),
    () => new WebglAddon(),
  );
  return {
    term,
    fit() {
      fit.fit();
      return { cols: term.cols, rows: term.rows };
    },
    focus: () => term.focus(),
    setFont(font) {
      term.options.fontFamily = font.family;
      term.options.fontSize = font.size;
    },
    search: {
      next: (q, caseSensitive, incremental = false) =>
        void search.findNext(q, { caseSensitive, incremental, decorations: SEARCH_DECORATIONS }),
      previous: (q, caseSensitive) => void search.findPrevious(q, { caseSensitive, decorations: SEARCH_DECORATIONS }),
      clear: () => {
        search.clearDecorations();
        term.clearSelection();
      },
      onResults: (cb) => search.onDidChangeResults((r) => cb(r.resultIndex, r.resultCount)),
    },
    dispose: () => {
      el.removeEventListener("contextmenu", onContext);
      term.dispose();
    },
  };
}
