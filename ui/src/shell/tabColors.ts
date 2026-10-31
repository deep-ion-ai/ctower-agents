// Cores das abas por tipo e por tema (#73). A configuração fica em
// `appearance.tabs` do ui.json; a aba leva `data-kind` e o CSS da faixa lê
// `--tab-<kind>-fg` e `--tab-<kind>-bg`, postas no <html> a partir da
// configuração do tema em uso (sem estilo inline em cada aba).
import type { TabColor, TabColors, TabKindColors } from "../api/types";
import { hasProjectKey, type EditorTab } from "../state/nav";
import type { Theme } from "../state/theme";

export type TabKind = keyof TabKindColors;

export const TAB_KINDS: TabKind[] = ["terminal", "shell", "issue", "screen"];

export const TAB_KIND_LABEL: Record<TabKind, string> = {
  terminal: "Sessão (terminal)",
  shell: "Shell",
  issue: "Issue / épico",
  screen: "Demais telas",
};

export const THEMES: Theme[] = ["dark", "light"];

/** Configuração vazia: tudo na cor do tema. */
export const emptyTabColors = (): TabColors => ({
  dark: { terminal: {}, shell: {}, issue: {}, screen: {} },
  light: { terminal: {}, shell: {}, issue: {}, screen: {} },
});

/** Tipo da aba: sessão, shell, tela de issue/épico ou as demais telas. */
export function tabKind(t: EditorTab): TabKind {
  if (t.kind !== "screen") return t.kind;
  const s = t.screen;
  return s.name === "project" && hasProjectKey(s) && s.item ? "issue" : "screen";
}

/**
 * HEX normalizado (`#rrggbb`, minúsculo) ou `null` se inválido. Aceita
 * `#RGB` e `#RRGGBB`, com ou sem `#`, sem diferenciar maiúsculas.
 */
export function normalizeHex(v: string): string | null {
  const m = /^#?([0-9a-f]{3}|[0-9a-f]{6})$/i.exec(v.trim());
  if (!m) return null;
  const h = m[1].toLowerCase();
  return "#" + (h.length === 3 ? [...h].map((c) => c + c).join("") : h);
}

/** Cor gravada válida, ou `null` (ausente, `null` ou inválida = cor do tema). */
export const colorOf = (c: TabColor | undefined, k: "fg" | "bg"): string | null => {
  const v = c?.[k];
  return typeof v === "string" ? normalizeHex(v) : null;
};

function luminance(hex: string): number {
  const n = parseInt(hex.slice(1), 16);
  const ch = [(n >> 16) & 255, (n >> 8) & 255, n & 255].map((x) => {
    const s = x / 255;
    return s <= 0.03928 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4;
  });
  return 0.2126 * ch[0] + 0.7152 * ch[1] + 0.0722 * ch[2];
}

/** Razão de contraste WCAG entre duas cores HEX (1 a 21). */
export function contrastRatio(a: string, b: string): number {
  const la = luminance(normalizeHex(a) ?? "#000000");
  const lb = luminance(normalizeHex(b) ?? "#000000");
  const [hi, lo] = la > lb ? [la, lb] : [lb, la];
  return (hi + 0.05) / (lo + 0.05);
}

/** Contraste mínimo sem aviso (WCAG AA para texto normal). */
export const MIN_CONTRAST = 4.5;

/**
 * Cores padrão de uma aba inativa em cada tema (`--muted` sobre `--bg-side`
 * de theme.css): as usadas no contraste quando um dos campos está vazio.
 */
export const THEME_TAB_DEFAULTS: Record<Theme, { fg: string; bg: string }> = {
  dark: { fg: "#9aa3b2", bg: "#12151b" },
  light: { fg: "#5a6372", bg: "#f0f2f5" },
};

/** Contraste do tipo no tema, com as cores padrão nos campos vazios. */
export function kindContrast(colors: TabColors | undefined, theme: Theme, kind: TabKind): number {
  const c = colors?.[theme]?.[kind];
  const d = THEME_TAB_DEFAULTS[theme];
  return contrastRatio(colorOf(c, "fg") ?? d.fg, colorOf(c, "bg") ?? d.bg);
}

/** Variáveis CSS do tema em uso: só as cores configuradas e válidas. */
export function tabColorVars(colors: TabColors | undefined, theme: Theme): Record<string, string> {
  const out: Record<string, string> = {};
  for (const kind of TAB_KINDS) {
    const c = colors?.[theme]?.[kind];
    for (const k of ["fg", "bg"] as const) {
      const v = colorOf(c, k);
      if (v) out[`--tab-${kind}-${k}`] = v;
    }
  }
  return out;
}

/** Põe no <html> as variáveis do tema em uso e tira as que não valem mais. */
export function applyTabColors(colors: TabColors | undefined, theme: Theme, el: HTMLElement = document.documentElement) {
  const vars = tabColorVars(colors, theme);
  for (const kind of TAB_KINDS)
    for (const k of ["fg", "bg"]) {
      const name = `--tab-${kind}-${k}`;
      if (vars[name]) el.style.setProperty(name, vars[name]);
      else el.style.removeProperty(name);
    }
}

/** Configuração com as cores de `theme` trocadas (`null` limpa o campo). */
export function withTabColor(
  colors: TabColors | undefined,
  theme: Theme,
  kind: TabKind,
  k: "fg" | "bg",
  value: string | null,
): TabColors {
  const base = colors ?? emptyTabColors();
  const themeCfg = { ...emptyTabColors()[theme], ...base[theme] };
  const cur: TabColor = { ...themeCfg[kind] };
  if (value === null) delete cur[k];
  else cur[k] = value;
  return { ...emptyTabColors(), ...base, [theme]: { ...themeCfg, [kind]: cur } };
}

/** Alguma cor configurada no tema (ou em qualquer tema, sem `theme`)? */
export function hasTabColors(colors: TabColors | undefined, theme?: Theme): boolean {
  return (theme ? [theme] : THEMES).some((t) =>
    TAB_KINDS.some((kind) => colorOf(colors?.[t]?.[kind], "fg") || colorOf(colors?.[t]?.[kind], "bg")),
  );
}
