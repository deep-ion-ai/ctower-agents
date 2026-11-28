// "Precisa de você" compacta (#357): uma linha por item (tipo, projeto, item
// e detalhe, há quanto tempo, ação), com "Recolher" lembrado neste
// navegador. Some quando não há nada: "vazia = tudo andando" (spec §10).
import { useCallback, useId, useState } from "react";
import type { Alert } from "../state/tower";
import { ago } from "../util/time";
import { DISCARD_LABEL, type DiscardTarget } from "../recovery/discard";
import type { AttentionAction, AttentionItem } from "./attention";

const TONE: Record<AttentionItem["kind"], string> = {
  waiting: "attention",
  failed: "error",
  interrupted: "error",
  ci_failed: "error",
  pilot_blocked: "error",
  env_down: "neutral",
  docs: "attention",
  hub: "attention",
  alert: "attention",
  sync_failing: "error",
  sync_late: "neutral",
};

const KEY = "ctower.control.attention";

export function loadCollapsed(): boolean {
  try {
    return JSON.parse(localStorage.getItem(KEY) ?? "null")?.collapsed === true;
  } catch {
    return false;
  }
}

/** "Recolher" da lista, guardado no localStorage como a grade (`grid.ts`). */
export function useCollapsed(): [boolean, (v: boolean) => void] {
  const [collapsed, setState] = useState(loadCollapsed);
  const set = useCallback((v: boolean) => {
    setState(v);
    try {
      localStorage.setItem(KEY, JSON.stringify({ collapsed: v }));
    } catch {
      // Sem armazenamento: vale só nesta execução.
    }
  }, []);
  return [collapsed, set];
}

/** "3 itens · mais antigo há 1 h". */
export function attentionSummary(items: AttentionItem[], now: number): string {
  const n = items.length;
  const oldest = Math.min(...items.map((i) => i.since));
  return `${n} ${n === 1 ? "item" : "itens"} · mais antigo ${ago(oldest, now)}`;
}

export function AttentionList({
  items,
  now,
  onAction,
  onDismiss,
  onDiscard,
  collapsible = true,
}: {
  items: AttentionItem[];
  now: number;
  onAction: (a: AttentionAction) => void;
  onDismiss: (a: Alert) => void;
  /** Interrompida: descarta sem retomar (#138); ausente, o botão não aparece. */
  onDiscard?: (t: DiscardTarget, origin: Element) => void;
  /** Na visão "Precisa de você", a lista fica sempre aberta. */
  collapsible?: boolean;
}) {
  const [stored, setCollapsed] = useCollapsed();
  const id = useId();
  if (items.length === 0) return null;
  const collapsed = collapsible && stored;
  return (
    <section aria-labelledby={`${id}-t`} className="attention">
      <div className="attention-bar">
        <h2 id={`${id}-t`} className="section-title attention-title">
          Precisa de você
        </h2>
        <span className="muted">{attentionSummary(items, now)}</span>
        <span className="control-spacer" />
        {collapsible && (
          <button
            type="button"
            className="btn btn-sm"
            aria-expanded={!collapsed}
            aria-controls={`${id}-l`}
            onClick={() => setCollapsed(!collapsed)}
          >
            {collapsed ? "Mostrar" : "Recolher"}
          </button>
        )}
      </div>
      <ul id={`${id}-l`} className="attention-rows" hidden={collapsed}>
        {items.map((it) => (
          <li key={it.key} className="attention-row" data-tone={TONE[it.kind]}>
            <span className="attention-kind">{it.label}</span>
            <span className="attention-project">{it.project ?? ""}</span>
            <span className="attention-text">
              <span className="attention-item-title">{it.item ?? it.title}</span>
              <span className="attention-detail">{it.detail}</span>
            </span>
            <span className="muted attention-since">{ago(it.since, now)}</span>
            <span className="attention-actions">
              <button type="button" className="btn btn-sm btn-primary" onClick={() => onAction(it.action)}>
                {it.actionLabel}
              </button>
              {it.alert && (
                <button type="button" className="btn btn-sm" onClick={() => onDismiss(it.alert!)}>
                  Dispensar
                </button>
              )}
              {it.discard && onDiscard && (
                <button
                  type="button"
                  className="btn btn-sm"
                  aria-label={`${DISCARD_LABEL}: ${it.title}`}
                  onClick={(e) => onDiscard(it.discard!, e.currentTarget)}
                >
                  {DISCARD_LABEL}
                </button>
              )}
            </span>
          </li>
        ))}
      </ul>
    </section>
  );
}
