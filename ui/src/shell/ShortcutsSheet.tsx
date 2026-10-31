// Folha de atalhos e gestos (#27): `?` fora de campos de texto, a paleta e
// o botão da barra de status. No desktop, uma janela como a da paleta; no
// celular, a folha de baixo (como a dos avisos), só com os gestos.
import { useEffect, useRef, type KeyboardEvent as ReactKeyboardEvent } from "react";
import { Icon } from "./icons";
import { shortcutGroups, type ShortcutGroup } from "./shortcuts";

export function ShortcutsSheet({ web, mobile, onClose }: { web: boolean; mobile: boolean; onClose(): void }) {
  const groups = shortcutGroups({ web, mobile });
  const closeBtn = useRef<HTMLButtonElement>(null);
  // Foco de antes de abrir: volta para lá ao fechar (como na paleta).
  const back = useRef<Element | null>(document.activeElement);

  useEffect(() => {
    closeBtn.current?.focus();
    const el = back.current;
    return () => {
      if (el instanceof HTMLElement && el.isConnected) el.focus();
    };
  }, []);

  const onKeyDown = (e: ReactKeyboardEvent) => {
    if (e.key === "Escape" || e.key === "?") {
      e.preventDefault();
      e.stopPropagation();
      onClose();
    } else if (e.key === "Tab") {
      // Só o botão de fechar é focável: o foco fica na folha.
      e.preventDefault();
    }
  };

  const title = mobile ? "Gestos" : "Atalhos e gestos";
  return (
    <div
      className={mobile ? "msheet-scrim" : "palette-backdrop"}
      onMouseDown={(e) => e.target === e.currentTarget && onClose()}
    >
      <div
        className={mobile ? "msheet" : "palette keys-sheet"}
        role="dialog"
        aria-modal="true"
        aria-labelledby="keys-h"
        onKeyDown={onKeyDown}
      >
        <div className="msheet-head">
          <h2 id="keys-h" className="msheet-title">
            {title}
          </h2>
          <button ref={closeBtn} type="button" className="ibtn" aria-label="Fechar atalhos" onClick={onClose}>
            <Icon name="close" size={16} />
          </button>
        </div>
        <div className="msheet-body keys-body">
          {groups.map((g) => (
            <Group key={g.title} group={g} />
          ))}
        </div>
      </div>
    </div>
  );
}

function Group({ group }: { group: ShortcutGroup }) {
  const id = `keys-${group.title.replace(/\W+/g, "-")}`;
  return (
    <section className="keys-group" aria-labelledby={id}>
      <h3 id={id} className="keys-title">
        {group.title}
      </h3>
      {group.note && <p className="keys-note">{group.note}</p>}
      <dl className="keys-list">
        {group.rows.map((r) => (
          <div key={r.keys.join(" ") + r.text} className="keys-row">
            <dt>
              {r.gesture ? (
                <span className="keys-gesture">{r.keys.join(", ")}</span>
              ) : (
                r.keys.map((k) => (
                  <kbd key={k} className="kbd">
                    {k}
                  </kbd>
                ))
              )}
            </dt>
            <dd>{r.text}</dd>
          </div>
        ))}
      </dl>
    </section>
  );
}
