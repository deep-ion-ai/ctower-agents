// Substitui ../terminal/xterm em todos os testes (xterm não roda no jsdom):
// cada terminal montado vira um registro do que foi escrito.
import type { UiAction } from "../terminal/keys";
import type { MountedTerm, MountOptions, TermFont } from "../terminal/xterm";

export interface FakeScreen {
  screen: string;
  typed: (d: string) => void;
  /** Simula um atalho de tela (ex.: Ctrl+F). */
  uiAction: (a: UiAction) => void;
  /** Chamadas à busca: `next|previous|clear` com o texto e as opções. */
  searches: string[];
  /** Simula o addon de busca informando o resultado. */
  results: (index: number, count: number) => void;
  focused: number;
  /** Fonte atual (começa com a das preferências). */
  font: TermFont;
}

export const terms: FakeScreen[] = [];

export function mountTerminal(_el: HTMLElement, opts: MountOptions): MountedTerm {
  let cb: (d: string) => void = () => {};
  let onResults: (index: number, count: number) => void = () => {};
  const t: FakeScreen = {
    screen: "",
    typed: (d) => cb(d),
    uiAction: (a) => opts.onUiAction(a),
    searches: [],
    results: (i, c) => onResults(i, c),
    focused: 0,
    font: opts.font,
  };
  terms.push(t);
  return {
    term: {
      write: (d) => {
        t.screen += typeof d === "string" ? d : new TextDecoder().decode(d);
      },
      reset: () => {
        t.screen = "";
      },
      onData: (f) => {
        cb = f;
        return { dispose: () => {} };
      },
    },
    fit: () => ({ cols: 100, rows: 30 }),
    focus: () => {
      t.focused++;
    },
    setFont: (f) => {
      t.font = f;
    },
    search: {
      next: (q, cs, inc) => t.searches.push(`next ${q}${cs ? " Aa" : ""}${inc ? " inc" : ""}`),
      previous: (q, cs) => t.searches.push(`previous ${q}${cs ? " Aa" : ""}`),
      clear: () => t.searches.push("clear"),
      onResults: (f) => {
        onResults = f;
        return { dispose: () => (onResults = () => {}) };
      },
    },
    dispose: () => {},
  };
}
