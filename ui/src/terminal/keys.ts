// Teclado e clique direito do console: uma regra só (modelo do Windows
// Terminal e do VS Code), para os atalhos não brigarem entre si nem com as
// teclas do Claude Code. No macOS, Cmd no lugar de Ctrl.
import { isMac } from "./links";

export type KeyAction = "copy" | "paste" | "newline" | "search" | "zoomIn" | "zoomOut" | "zoomReset";

/** Ações que mexem na tela (não no xterm): quem monta o terminal trata. */
export type UiAction = Extract<KeyAction, "search" | "zoomIn" | "zoomOut" | "zoomReset">;

export type KeyLike = Pick<KeyboardEvent, "key" | "ctrlKey" | "shiftKey" | "altKey" | "metaKey">;

/** Nova linha sem enviar: a mesma sequência do `/terminal-setup` do Claude Code. */
export const NEWLINE = "\x1b\r";

/**
 * Ação do atalho, ou `null` para o xterm tratar a tecla como sempre (Ctrl+C
 * sem seleção continua sendo ^C para o agente).
 */
export function keyAction(e: KeyLike, hasSelection: boolean, mac = isMac()): KeyAction | null {
  const mod = mac ? e.metaKey && !e.ctrlKey : e.ctrlKey && !e.metaKey;
  const key = e.key.toLowerCase();
  if (key === "enter" && !e.altKey && !e.metaKey && (e.ctrlKey || e.shiftKey)) return "newline";
  if (!mod || e.altKey) return null;
  if (key === "c") return e.shiftKey || hasSelection || mac ? "copy" : null;
  if (key === "v") return "paste";
  if (key === "f" && !e.shiftKey) return "search";
  if (key === "=" || key === "+") return "zoomIn";
  if (key === "-" || key === "_") return "zoomOut";
  if (key === "0" && !e.shiftKey) return "zoomReset";
  return null;
}

/** O que as ações usam do xterm (um fake nos testes). */
export interface KeyTarget {
  hasSelection(): boolean;
  getSelection(): string;
  clearSelection(): void;
  /** Cola com bracketed paste, quando o agente pediu. */
  paste(text: string): void;
  /** Manda como se fosse digitado. */
  input(data: string): void;
}

export interface Clipboard {
  read(): Promise<string>;
  write(text: string): Promise<void>;
}

export async function runAction(a: Exclude<KeyAction, UiAction>, t: KeyTarget, cb: Clipboard): Promise<void> {
  switch (a) {
    case "copy": {
      if (!t.hasSelection()) return;
      const text = t.getSelection();
      t.clearSelection();
      await cb.write(text);
      return;
    }
    case "paste": {
      const text = await cb.read();
      if (text) t.paste(text);
      return;
    }
    case "newline":
      t.input(NEWLINE);
  }
}

/** Clique direito: sobre um link, copia o endereço; com seleção, copia; senão, cola. */
export function contextAction(t: KeyTarget, link: string | null, cb: Clipboard): Promise<void> {
  if (link) return cb.write(link);
  return runAction(t.hasSelection() ? "copy" : "paste", t, cb);
}
