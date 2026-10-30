// "Aguardando você" em destaque no topo da Sala de controle, no modo web
// (spec do épico #205 §10.3): cada sessão que espera o humano vira um cartão
// inteiro tocável. Com o Remote Control ligado, o toque abre direto o
// `claude.ai/code/<id>`; sem ele, o painel da sessão.
import { useState } from "react";
import { useNav } from "../state/nav";
import { useTower } from "../state/tower";
import { ago } from "../util/time";
import type { AttentionItem } from "./attention";
import { goToAction } from "../shell/BottomPanel";

export function WaitingNow({ items, now }: { items: AttentionItem[]; now: number }) {
  const nav = useNav();
  const { api } = useTower();
  const [error, setError] = useState<string | null>(null);
  if (items.length === 0) return null;

  const open = (it: AttentionItem) => {
    setError(null);
    if (it.remote) {
      api.openUrl(it.remote).catch((e) => setError(e instanceof Error ? e.message : String(e)));
      return;
    }
    goToAction(nav, it.action);
  };

  return (
    <section className="waiting-now" aria-labelledby="aguardando-voce">
      <h2 id="aguardando-voce" className="section-title waiting-now-title">
        Aguardando você · {items.length}
      </h2>
      <ul className="waiting-now-list">
        {items.map((it) => (
          <li key={it.key}>
            <button
              type="button"
              className="waiting-now-card"
              aria-label={`${it.actionLabel}: ${it.title}`}
              onClick={() => open(it)}
            >
              <span className="waiting-now-head">
                <span className="attention-kind">{it.label}</span>
                <span className="muted">{ago(it.since, now)}</span>
              </span>
              <span className="attention-item-title">{it.title}</span>
              <span className="attention-detail">{it.detail}</span>
              <span className="waiting-now-go">{it.actionLabel} ›</span>
            </button>
          </li>
        ))}
      </ul>
      {error && (
        <p role="alert" className="web-error">
          {error}
        </p>
      )}
    </section>
  );
}
