import { useEffect, useRef, type KeyboardEvent as ReactKeyboardEvent } from "react";

export interface MenuItem {
  label: string;
  run: () => void;
  /** Visível, mas sem ação agora (ex.: sem worktree); `title` diz o motivo. */
  disabled?: boolean;
  title?: string;
}

/**
 * Menu de contexto (`role="menu"`) na posição do clique: o foco vai para a
 * primeira ação, setas movem, Esc e Tab fecham devolvendo o foco
 * (`onClose(true)`), clicar fora fecha sem devolver.
 */
export function ContextMenu({
  x,
  y,
  label,
  items,
  note,
  onClose,
}: {
  x: number;
  y: number;
  label: string;
  items: MenuItem[];
  /** Aviso no topo do menu (ex.: por que as ações estão desligadas). */
  note?: string;
  onClose: (restore: boolean) => void;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const close = useRef(onClose);
  close.current = onClose;
  useEffect(() => {
    // Sem ação habilitada, o foco fica no próprio menu (Esc ainda fecha).
    (ref.current?.querySelector<HTMLButtonElement>("button:not(:disabled)") ?? ref.current)?.focus();
    const away = (e: MouseEvent) => {
      if (!ref.current?.contains(e.target as Node)) close.current(false);
    };
    document.addEventListener("mousedown", away);
    return () => document.removeEventListener("mousedown", away);
  }, []);
  const onKeyDown = (e: ReactKeyboardEvent) => {
    const list = [...(ref.current?.querySelectorAll<HTMLButtonElement>("button:not(:disabled)") ?? [])];
    const i = list.indexOf(document.activeElement as HTMLButtonElement);
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      const d = e.key === "ArrowDown" ? 1 : -1;
      list[(i + d + list.length) % list.length]?.focus();
    } else if (e.key === "Escape" || e.key === "Tab") {
      e.preventDefault();
      e.stopPropagation();
      onClose(true);
    }
  };
  return (
    <div ref={ref} className="ctx-menu" role="menu" aria-label={label} tabIndex={-1} style={{ left: x, top: y }} onKeyDown={onKeyDown}>
      {note && (
        <p role="note" className="ctx-note">
          {note}
        </p>
      )}
      {items.map((it, i) => (
        <button
          key={`${i}:${it.label}`}
          type="button"
          role="menuitem"
          disabled={it.disabled}
          title={it.title}
          onClick={() => {
            if (it.disabled) return;
            onClose(false);
            it.run();
          }}
        >
          {it.label}
        </button>
      ))}
    </div>
  );
}
