// Disposição do shell (painel lateral, painel inferior). Fica por máquina,
// em localStorage, como as preferências do console.
import { useCallback, useEffect, useState } from "react";
import { isMac } from "../terminal/links";

export type PanelTab = "events" | "warnings" | "sync";

export interface ShellLayout {
  sideOpen: boolean;
  /** Largura do painel lateral (px). */
  sideWidth: number;
  panelOpen: boolean;
  panelTab: PanelTab;
}

export const SIDE_MIN = 180;
export const SIDE_MAX = 520;
export const DEFAULT_LAYOUT: ShellLayout = { sideOpen: true, sideWidth: 268, panelOpen: false, panelTab: "events" };
/** Abaixo disso o painel lateral vira gaveta (por cima da área de editor). */
export const NARROW_PX = 900;

const KEY = "ctower.shell.layout";
const TABS: PanelTab[] = ["events", "warnings", "sync"];

export const clampWidth = (n: number) => Math.min(SIDE_MAX, Math.max(SIDE_MIN, Math.round(n)));

/**
 * Abaixo disso, no modo web, o shell vira o do celular: barra inferior,
 * gaveta, uma aba por vez, sem painel inferior nem barra de status (spec do
 * épico #205 §10.3).
 */
export const MOBILE_PX = 768;
export const MOBILE_QUERY = `(max-width: ${MOBILE_PX - 1}px)`;

function matches(query: string): boolean {
  try {
    return window.matchMedia?.(query).matches ?? false;
  } catch {
    return false;
  }
}

export function isNarrow(): boolean {
  return matches(`(max-width: ${NARROW_PX}px)`);
}

/** A consulta de mídia casa agora; acompanha a mudança (girar o celular, redimensionar). */
export function useMediaQuery(query: string): boolean {
  const [on, setOn] = useState(() => matches(query));
  useEffect(() => {
    let mq: MediaQueryList | undefined;
    try {
      mq = window.matchMedia?.(query);
    } catch {
      return;
    }
    if (!mq) return;
    const update = () => setOn(mq!.matches);
    update();
    mq.addEventListener?.("change", update);
    return () => mq!.removeEventListener?.("change", update);
  }, [query]);
  return on;
}

export function loadLayout(): ShellLayout {
  let v: Partial<ShellLayout> | null = null;
  try {
    v = JSON.parse(localStorage.getItem(KEY) ?? "null");
  } catch {
    // Sem armazenamento: o padrão.
  }
  const d = DEFAULT_LAYOUT;
  const out: ShellLayout = {
    sideOpen: typeof v?.sideOpen === "boolean" ? v.sideOpen : d.sideOpen,
    sideWidth: Number.isFinite(v?.sideWidth) ? clampWidth(v!.sideWidth!) : d.sideWidth,
    panelOpen: typeof v?.panelOpen === "boolean" ? v.panelOpen : d.panelOpen,
    panelTab: TABS.includes(v?.panelTab as PanelTab) ? (v!.panelTab as PanelTab) : d.panelTab,
  };
  // Tela estreita: a gaveta começa fechada para não cobrir a tela.
  if (isNarrow()) out.sideOpen = false;
  return out;
}

function saveLayout(l: ShellLayout) {
  try {
    localStorage.setItem(KEY, JSON.stringify(l));
  } catch {
    // Sem armazenamento: vale só nesta execução.
  }
}

export type LayoutPatch = Partial<ShellLayout> | ((l: ShellLayout) => Partial<ShellLayout>);

export function useShellLayout(): [ShellLayout, (patch: LayoutPatch) => void] {
  const [layout, setLayout] = useState(loadLayout);
  const update = useCallback((p: LayoutPatch) => {
    setLayout((l) => {
      const patch = typeof p === "function" ? p(l) : p;
      const next = { ...l, ...patch };
      if (patch.sideWidth !== undefined) next.sideWidth = clampWidth(patch.sideWidth);
      saveLayout(next);
      return next;
    });
  }, []);
  return [layout, update];
}

export type ShellShortcut = "side" | "panel" | "palette";

export type ShortcutKey = Pick<KeyboardEvent, "key" | "ctrlKey" | "shiftKey" | "altKey" | "metaKey">;

/**
 * Atalhos do shell: ⌘B (painel lateral), ⌘J (painel inferior) e ⌘K
 * (paleta); Ctrl fora do macOS. Fora do macOS, dentro do console, Ctrl+B/J/K
 * são teclas do agente e do shell: `inTerminal` deixa a tecla para o xterm.
 */
export function shellShortcut(e: ShortcutKey, inTerminal: boolean, mac = isMac()): ShellShortcut | null {
  const mod = mac ? e.metaKey && !e.ctrlKey : e.ctrlKey && !e.metaKey;
  if (!mod || e.altKey || e.shiftKey) return null;
  if (inTerminal && !mac) return null;
  switch (e.key.toLowerCase()) {
    case "b":
      return "side";
    case "j":
      return "panel";
    case "k":
      return "palette";
    default:
      return null;
  }
}

/**
 * Ctrl+Shift+` (também no macOS, como "novo terminal" no VS Code): shell
 * novo no worktree da aba ativa do Terminal (#423). Pela tecla física
 * (`code`), porque com Shift o `key` vira "~" no layout americano.
 */
export function newShellShortcut(e: ShortcutKey & { code?: string }): boolean {
  if (!e.ctrlKey || !e.shiftKey || e.altKey || e.metaKey) return false;
  return e.code === "Backquote" || e.key === "`" || e.key === "~";
}

/**
 * Ctrl+Alt+B (⌘⌥B no macOS), como a barra lateral secundária do VS Code:
 * painel de contexto do console (#426). Pela tecla física (`code`), porque
 * com Option o `key` vira "∫" no macOS. Não colide com ⌘B/Ctrl+B (painel
 * lateral), que exige Alt solto.
 */
export function contextShortcut(e: ShortcutKey & { code?: string }, mac = isMac()): boolean {
  const mod = mac ? e.metaKey && !e.ctrlKey : e.ctrlKey && !e.metaKey;
  if (!mod || !e.altKey || e.shiftKey) return false;
  return e.code === "KeyB" || e.key.toLowerCase() === "b";
}

/** Rótulo do atalho para a plataforma ("⌘B" ou "Ctrl+B"). */
export function shortcutLabel(key: string, mac = isMac()): string {
  return mac ? `⌘${key}` : `Ctrl+${key}`;
}
