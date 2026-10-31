import { useEffect, useRef, useState, type KeyboardEvent as ReactKeyboardEvent } from "react";
import { Icon, type IconName } from "./icons";
import type { TabKind } from "./tabColors";

/** Uma aba na lista de abas abertas. */
export interface OpenTabItem {
  id: string;
  title: string;
  /** Projeto da aba de sessão ou shell. */
  project?: string;
  tone?: string;
  icon?: IconName;
  /** Tipo da aba (#73): a lista usa só a cor do texto dele. */
  kind?: TabKind;
}

/** Filtro da lista: título ou projeto, sem diferenciar maiúsculas. */
export function filterTabs(items: OpenTabItem[], q: string): OpenTabItem[] {
  const f = q.trim().toLowerCase();
  if (!f) return items;
  return items.filter((t) => t.title.toLowerCase().includes(f) || !!t.project?.toLowerCase().includes(f));
}

/**
 * Lista de abas abertas (#427; como o "Show Opened Editors" do VS Code):
 * aberta pelo ⌄ da faixa de abas quando elas não cabem. Campo de filtro com
 * o foco (`role="combobox"`), setas movem a seleção, Enter ativa a aba e
 * Esc fecha devolvendo o foco (`onClose(true)`); clicar fora fecha.
 */
export function OpenTabsList({
  items,
  current,
  onPick,
  onClose,
}: {
  items: OpenTabItem[];
  current: string;
  onPick: (id: string) => void;
  onClose: (restore: boolean) => void;
}) {
  const [q, setQ] = useState("");
  const shown = filterTabs(items, q);
  const [sel, setSel] = useState(() => Math.max(0, items.findIndex((t) => t.id === current)));
  const at = Math.min(sel, shown.length - 1);
  const ref = useRef<HTMLDivElement>(null);
  const close = useRef(onClose);
  close.current = onClose;
  useEffect(() => {
    const away = (e: MouseEvent) => {
      if (!ref.current?.contains(e.target as Node)) close.current(false);
    };
    document.addEventListener("mousedown", away);
    return () => document.removeEventListener("mousedown", away);
  }, []);
  useEffect(() => {
    if (at >= 0) document.getElementById(`otab-${shown[at].id}`)?.scrollIntoView?.({ block: "nearest" });
  });

  const pick = (id: string) => {
    onClose(false);
    onPick(id);
  };
  const onKeyDown = (e: ReactKeyboardEvent) => {
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      if (shown.length === 0) return;
      const d = e.key === "ArrowDown" ? 1 : -1;
      setSel((Math.max(0, at) + d + shown.length) % shown.length);
    } else if (e.key === "Enter") {
      e.preventDefault();
      if (at >= 0) pick(shown[at].id);
    } else if (e.key === "Escape" || e.key === "Tab") {
      e.preventDefault();
      e.stopPropagation();
      onClose(true);
    }
  };

  return (
    <div ref={ref} className="otabs" role="dialog" aria-label="Abas abertas" onKeyDown={onKeyDown}>
      <input
        type="search"
        role="combobox"
        aria-label="Filtrar abas abertas"
        aria-expanded="true"
        aria-controls="otabs-list"
        aria-activedescendant={at >= 0 ? `otab-${shown[at].id}` : undefined}
        placeholder="Filtrar por título ou projeto"
        autoFocus
        value={q}
        onChange={(e) => {
          setQ(e.target.value);
          setSel(0);
        }}
      />
      <ul id="otabs-list" role="listbox" aria-label="Abas" className="otabs-list">
        {shown.map((t, i) => (
          <li
            key={t.id}
            id={`otab-${t.id}`}
            role="option"
            aria-selected={i === at}
            data-current={t.id === current ? "true" : undefined}
            data-kind={t.kind}
            onMouseDown={(e) => e.preventDefault()}
            onMouseEnter={() => setSel(i)}
            onClick={() => pick(t.id)}
          >
            {t.tone && <span className="sp-dot" data-tone={t.tone} aria-hidden="true" />}
            {t.icon && <Icon name={t.icon} size={13} />}
            <span className="otabs-title">{t.title}</span>
            {t.project && <span className="otabs-project">{t.project}</span>}
          </li>
        ))}
      </ul>
      {shown.length === 0 && <p className="otabs-empty muted">Nenhuma aba encontrada.</p>}
    </div>
  );
}
