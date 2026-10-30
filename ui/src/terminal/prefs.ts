// Fonte, zoom e fechamento da aba do console. Ficam por máquina (dependem
// das fontes instaladas), em localStorage, e valem para todas as abas.
import { useSyncExternalStore } from "react";

export interface TermPrefs {
  fontFamily: string;
  fontSize: number;
  /** Passos de Ctrl+= / Ctrl+-, somados ao tamanho; Ctrl+0 zera. */
  zoom: number;
  /** Fecha a aba quando a sessão sai sem erro (`/exit`, código 0; #432). */
  closeOnExit: boolean;
}

/** Fallbacks com os símbolos que o Plex Mono não tem (ex.: ⎿ do Claude Code). */
export const DEFAULT_FONT = "'IBM Plex Mono', 'Cascadia Mono', Consolas, 'Segoe UI Symbol', 'DejaVu Sans Mono', monospace";
export const DEFAULT_PREFS: TermPrefs = { fontFamily: DEFAULT_FONT, fontSize: 13, zoom: 0, closeOnExit: true };
export const MIN_FONT = 8;
export const MAX_FONT = 32;

const KEY = "ctower.terminal.prefs";
const clamp = (n: number) => Math.min(MAX_FONT, Math.max(MIN_FONT, Math.round(n)));

let current: TermPrefs = load();
const listeners = new Set<() => void>();

function load(): TermPrefs {
  try {
    const v = JSON.parse(localStorage.getItem(KEY) ?? "null");
    if (!v || typeof v !== "object") return DEFAULT_PREFS;
    return normalize({
      fontFamily: typeof v.fontFamily === "string" ? v.fontFamily : DEFAULT_PREFS.fontFamily,
      fontSize: Number.isFinite(v.fontSize) ? v.fontSize : DEFAULT_PREFS.fontSize,
      zoom: Number.isFinite(v.zoom) ? v.zoom : 0,
      closeOnExit: typeof v.closeOnExit === "boolean" ? v.closeOnExit : DEFAULT_PREFS.closeOnExit,
    });
  } catch {
    return DEFAULT_PREFS;
  }
}

function normalize(p: TermPrefs): TermPrefs {
  const fontSize = clamp(p.fontSize);
  // O zoom não leva o tamanho efetivo para fora dos limites.
  const zoom = clamp(fontSize + Math.round(p.zoom)) - fontSize;
  return { fontFamily: p.fontFamily.trim() || DEFAULT_FONT, fontSize, zoom, closeOnExit: p.closeOnExit };
}

export function termPrefs(): TermPrefs {
  return current;
}

/** Tamanho com o zoom aplicado. */
export function effectiveSize(p: TermPrefs): number {
  return p.fontSize + p.zoom;
}

export function setTermPrefs(patch: Partial<TermPrefs>) {
  current = normalize({ ...current, ...patch });
  try {
    localStorage.setItem(KEY, JSON.stringify(current));
  } catch {
    // Sem armazenamento: vale só nesta execução.
  }
  for (const l of listeners) l();
}

export function zoom(step: 1 | -1 | 0) {
  setTermPrefs({ zoom: step === 0 ? 0 : current.zoom + step });
}

export function useTermPrefs(): TermPrefs {
  return useSyncExternalStore(
    (cb) => {
      listeners.add(cb);
      return () => listeners.delete(cb);
    },
    () => current,
  );
}

/** Só para os testes. */
export function resetTermPrefs() {
  try {
    localStorage.removeItem(KEY);
  } catch {
    // idem
  }
  current = DEFAULT_PREFS;
  for (const l of listeners) l();
}
