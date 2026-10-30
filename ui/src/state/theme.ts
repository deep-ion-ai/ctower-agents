// Tema da UI (#290): `data-theme` no <html> escolhe os tokens de theme.css.
// O public/theme-boot.js põe o tema antes do primeiro quadro (com o que
// ficou gravado aqui ou o que o app nativo leu do ui.json); daqui em diante
// vale a preferência do ui.json, seguindo o sistema quando for `system`.
import { useEffect } from "react";
import type { ThemePref } from "../api/types";

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

/** Aplica `pref` (`null` = ui.json ainda não carregou: fica o tema do boot) e acompanha o sistema. */
export function useTheme(pref: ThemePref | null) {
  useEffect(() => {
    if (!pref) return;
    applyTheme(pref);
    if (pref !== "system" || typeof matchMedia !== "function") return;
    const mq = matchMedia(DARK_QUERY);
    const onChange = () => applyTheme(pref);
    mq.addEventListener("change", onChange);
    return () => mq.removeEventListener("change", onChange);
  }, [pref]);
}
