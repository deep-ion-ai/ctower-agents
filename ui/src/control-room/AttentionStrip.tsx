import type { Alert } from "../state/tower";
import { ago } from "../util/time";
import type { AttentionAction, AttentionItem } from "./attention";

const TONE: Record<AttentionItem["kind"], string> = {
  waiting: "attention",
  failed: "error",
  interrupted: "error",
  env_down: "neutral",
  docs: "attention",
  hub: "attention",
  alert: "attention",
  sync_failing: "error",
  sync_late: "neutral",
};

/** Some quando não há nada: "vazia = tudo andando" (spec §10). */
export function AttentionStrip({
  items,
  now,
  onAction,
  onDismiss,
}: {
  items: AttentionItem[];
  now: number;
  onAction: (a: AttentionAction) => void;
  onDismiss: (a: Alert) => void;
}) {
  if (items.length === 0) return null;
  return (
    <section aria-labelledby="atencao" className="attention">
      <h2 id="atencao" className="section-title attention-title">
        Precisa de você · {items.length}
      </h2>
      <ul className="attention-grid">
        {items.map((it) => (
          <li key={it.key} className="attention-card" data-tone={TONE[it.kind]}>
            <div className="attention-head">
              <span className="attention-kind">{it.label}</span>
              <span className="muted">{ago(it.since, now)}</span>
            </div>
            <div className="attention-item-title">{it.title}</div>
            <div className="attention-detail">{it.detail}</div>
            <div className="attention-actions">
              <button type="button" className="btn btn-primary" onClick={() => onAction(it.action)}>
                {it.actionLabel}
              </button>
              {it.alert && (
                <button type="button" className="btn" onClick={() => onDismiss(it.alert!)}>
                  Dispensar
                </button>
              )}
            </div>
          </li>
        ))}
      </ul>
    </section>
  );
}
