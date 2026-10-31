// Tema da UI (#290): `data-theme` no <html> escolhe os tokens de theme.css.
// O public/theme-boot.js põe o tema antes do primeiro quadro (com o que
// ficou gravado aqui ou o que o app nativo leu do ui.json); daqui em diante
// vale a preferência do ui.json, seguindo o sistema quando for `system`.
import { useEffect } from "react";
import type { TabColors, ThemePref } from "../api/types";
import { applyTabColors } from "../shell/tabColors";

export type Theme = "light" | "dark";

/** Última preferência aplicada, lida pelo theme-boot.js na próxima abertura. */
export const THEME_KEY = "ct.theme";

const DARK_QUERY = "(prefers-color-scheme: dark)";

const systemTheme = (): Theme =>
  typeof matchMedia === "function" && !matchMedia(DARK_QUERY).matches ? "light" : "dark";

export const resolveTheme = (pref: ThemePref): Theme => (pref === "system" ? systemTheme() : pref);

export function applyTheme(pref: ThemePref): Theme {
  const theme = resolveTheme(pref);
  document.documentElement.dataset.theme = theme;
  try {
    localStorage.setItem(THEME_KEY, pref);
  } catch {
    // Sem localStorage: a próxima abertura usa o que o app nativo leu do ui.json.
  }
  return theme;
}

/**
 * Aplica `pref` (`null` = ui.json ainda não carregou: fica o tema do boot) e
 * as cores das abas do tema em uso (#73), e acompanha o sistema.
 */
export function useTheme(pref: ThemePref | null, tabs?: TabColors) {
  // A configuração chega como objeto novo a cada gravação: compara pelo conteúdo.
  const tabsKey = JSON.stringify(tabs ?? null);
  useEffect(() => {
    if (!pref) return;
    const cfg = JSON.parse(tabsKey) as TabColors | null;
    const apply = () => applyTabColors(cfg ?? undefined, applyTheme(pref));
    apply();
    if (pref !== "system" || typeof matchMedia !== "function") return;
    const mq = matchMedia(DARK_QUERY);
    mq.addEventListener("change", apply);
    return () => mq.removeEventListener("change", apply);
  }, [pref, tabsKey]);
}
