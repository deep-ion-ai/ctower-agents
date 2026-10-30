import "./control-room.css";
import { useState } from "react";
import { useNow } from "../App";
import { goToAction } from "../shell/BottomPanel";
import { useNav } from "../state/nav";
import { useTower } from "../state/tower";
import { attention } from "./attention";
import { AttentionStrip } from "./AttentionStrip";
import { dayItems, Footer } from "./Footer";
import { DENSITIES, useGridPrefs, type ControlView } from "./grid";
import { Sidebar } from "./Sidebar";
import { TileGrid } from "./TileGrid";
import { WaitingNow } from "./WaitingNow";

const VIEW_TITLE: Record<ControlView, string> = {
  attention: "Precisa de você",
  running: "Rodando",
  done: "Concluídas hoje",
};

/**
 * Sala de controle no shell (#316): o painel lateral escolhe a visão; a área
 * de editor mostra a faixa "Precisa de você", a grade de sessões e o fim do
 * dia. Os eventos ficam no painel inferior do shell (⌘J).
 */
export function ControlRoom() {
  const t = useTower();
  const nav = useNav();
  const now = useNow();
  const [view, setView] = useState<ControlView | null>(null);
  const [grid, setGrid] = useGridPrefs();
  const items = attention({
    web: t.web,
    envs: t.envs,
    status: t.status,
    dashboards: t.dashboards,
    alerts: t.alerts,
    asks: t.asks,
    permissions: t.permissions,
    infos: t.infos,
    now,
    hub: t.hub.conn
      ? { status: t.hub.status, warnings: t.hub.health?.warnings ?? [], update: t.hub.update }
      : undefined,
  });
  const live = t.envs.flatMap((e) => t.dashboards[e.id]?.live_sessions ?? []);
  const waiting = items.filter((i) => i.kind === "waiting").length;
  const counts: Record<ControlView, number> = {
    attention: items.length,
    running: live.length - waiting,
    done: dayItems(t.envs, t.dashboards, now).done.length,
  };
  const showTiles = view !== "done";
  const showAttention = view === null || view === "attention";
  // Web: "Aguardando você" sai da faixa e vai em destaque no topo (§10.3).
  const highlight = t.web ? items.filter((i) => i.kind === "waiting") : [];
  const strip = t.web ? items.filter((i) => i.kind !== "waiting") : items;

  return (
    <main className="control-main">
      <Sidebar now={now} view={view} onView={setView} counts={counts} />
      <header className="control-head">
        <h1 className="control-title">{view ? VIEW_TITLE[view] : "Sala de controle"}</h1>
        {waiting > 0 && (
          <span className="badge" data-tone="attention">
            {waiting} aguardando
          </span>
        )}
        <span className="control-spacer" />
        {showTiles && (
          <>
            <button
              type="button"
              className="btn btn-sm"
              aria-pressed={grid.group}
              onClick={() => setGrid({ group: !grid.group })}
            >
              Agrupar por projeto
            </button>
            <div role="group" aria-label="Densidade da grade" className="control-seg">
              {DENSITIES.map((d) => (
                <button
                  key={d.id}
                  type="button"
                  className="btn btn-sm"
                  aria-pressed={grid.density === d.id}
                  onClick={() => setGrid({ density: d.id })}
                >
                  {d.label}
                </button>
              ))}
            </div>
          </>
        )}
      </header>
      {showAttention && <WaitingNow items={highlight} now={now} />}
      {showAttention && (
        <AttentionStrip items={strip} now={now} onAction={(a) => goToAction(nav, a, (url) => void t.api.openUrl(url).catch(() => {}))} onDismiss={t.dismissAlert} />
      )}
      {showTiles && (
        <TileGrid
          now={now}
          filter={view === "attention" ? "attention" : view === "running" ? "running" : "all"}
          group={grid.group}
          density={grid.density}
        />
      )}
      {(view === null || view === "done") && <Footer now={now} />}
    </main>
  );
}
