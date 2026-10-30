// Peças do shell no celular (modo web abaixo de 768 px, spec do épico #205
// §10.3): a lista de abas no cabeçalho (uma aba visível por vez) e a folha de
// avisos, no lugar do painel inferior e da barra de status.
import { useCallback, useEffect, useRef, useState, type RefObject } from "react";
import type { AttentionItem } from "../control-room/attention";
import { useNav } from "../state/nav";
import { useTower } from "../state/tower";
import { Warnings } from "./BottomPanel";
import { Icon } from "./icons";
import { tabLook } from "./titles";

/** Fecha com Escape e com toque fora de `ref`. */
function useDismiss(open: boolean, ref: RefObject<HTMLElement>, onClose: () => void) {
  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    const onDown = (e: MouseEvent) => {
      if (ref.current && e.target instanceof Node && !ref.current.contains(e.target)) onClose();
    };
    window.addEventListener("keydown", onKey);
    window.addEventListener("mousedown", onDown);
    return () => {
      window.removeEventListener("keydown", onKey);
      window.removeEventListener("mousedown", onDown);
    };
  }, [open, ref, onClose]);
}

/** As abas abertas numa lista no cabeçalho; a área de editor mostra só a ativa. */
export function MobileTabs() {
  const nav = useNav();
  const { dashboards } = useTower();
  const [open, setOpen] = useState(false);
  const box = useRef<HTMLDivElement>(null);
  const close = useCallback(() => setOpen(false), []);
  useDismiss(open, box, close);

  const current = nav.editors.find((t) => t.id === nav.current);
  const title = current ? tabLook(current, dashboards, nav.meta[current.id]).title : "";

  return (
    <div className="mtabs" ref={box}>
      <button
        type="button"
        className="mtabs-trigger"
        aria-label={`Abas abertas (${nav.editors.length}): ${title}`}
        aria-expanded={open}
        aria-controls="mtabs-list"
        onClick={() => setOpen((o) => !o)}
      >
        <span className="mtabs-title">{title}</span>
        {nav.editors.length > 1 && <span className="mtabs-count">{nav.editors.length}</span>}
        <Icon name="chevron" size={14} />
      </button>
      {open && (
        <ul id="mtabs-list" className="mtabs-list" aria-label="Abas abertas">
          {nav.editors.map((t) => {
            const look = tabLook(t, dashboards, nav.meta[t.id]);
            const on = t.id === nav.current;
            return (
              <li key={t.id} className="mtabs-row" data-on={on ? "true" : undefined}>
                <button
                  type="button"
                  className="mtabs-open"
                  aria-current={on ? "page" : undefined}
                  onClick={() => {
                    nav.activate(t.id);
                    close();
                  }}
                >
                  {look.tone && <span className="sp-dot" data-tone={look.tone} aria-hidden="true" />}
                  <span className="mtabs-title">{look.title}</span>
                </button>
                <button
                  type="button"
                  className="ibtn"
                  aria-label={`Fechar aba ${look.title}`}
                  onClick={() => nav.closeEditor(t.id)}
                >
                  <Icon name="close" size={14} />
                </button>
              </li>
            );
          })}
        </ul>
      )}
    </div>
  );
}

/** Avisos (`hub.status`, ambientes, alertas) numa folha, aberta pelo sino do cabeçalho. */
export function WarningsSheet({ items, now, onClose }: { items: AttentionItem[]; now: number; onClose(): void }) {
  const { envs } = useTower();
  const box = useRef<HTMLDivElement>(null);
  const closeBtn = useRef<HTMLButtonElement>(null);
  useDismiss(true, box, onClose);
  useEffect(() => closeBtn.current?.focus(), []);
  const envName = (id: string) => (id === "hub" ? "hub" : (envs.find((e) => e.id === id)?.name ?? id));
  return (
    <div className="msheet-scrim">
      <div ref={box} className="msheet" role="dialog" aria-modal="true" aria-labelledby="msheet-h">
        <div className="msheet-head">
          <h2 id="msheet-h" className="msheet-title">
            Avisos{items.length > 0 ? ` · ${items.length}` : ""}
          </h2>
          <button ref={closeBtn} type="button" className="ibtn" aria-label="Fechar avisos" onClick={onClose}>
            <Icon name="close" size={16} />
          </button>
        </div>
        <div className="msheet-body">
          <Warnings items={items} envName={envName} now={now} onGo={onClose} />
        </div>
      </div>
    </div>
  );
}
