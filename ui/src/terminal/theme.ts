// Tema do console: a paleta ANSI de 16 cores sai dos tokens de theme.css
// (o teste confere que continuam iguais).
import type { ITheme } from "@xterm/xterm";

export const TERM_THEME: ITheme = {
  background: "#0a0c10", // --term-bg
  foreground: "#c9d1dc", // --term-fg
  cursor: "#f2a33a", // --attention
  cursorAccent: "#0a0c10",
  selectionBackground: "#2a3446", // --term-selection
  black: "#1b1f28", // --surface-3
  red: "#f0705a", // --error
  green: "#5fc9a8", // --ok
  yellow: "#f2a33a", // --attention
  blue: "#8ab8ff", // --running
  magenta: "#c7a4f5", // --epic
  cyan: "#6cc7d9",
  white: "#aeb6c4", // --text-2
  brightBlack: "#6f7787", // --faint
  brightRed: "#f7a796", // --error-soft
  brightGreen: "#7fd4bc", // --done
  brightYellow: "#f7c46f",
  brightBlue: "#b3d0ff",
  brightMagenta: "#ddc6fa",
  brightCyan: "#9adbe8",
  brightWhite: "#e6e8ec", // --text
};

/** Destaque das ocorrências da busca (addon `search`). */
export const SEARCH_DECORATIONS = {
  matchBackground: "#3a3220",
  matchOverviewRuler: "#8a6d2e",
  activeMatchBackground: "#f2a33a", // --attention
  activeMatchColorOverviewRuler: "#f2a33a",
};
