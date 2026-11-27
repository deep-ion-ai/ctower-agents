import "./control-room.css";
import { useState } from "react";
import { useNow } from "../App";
import { hubTab } from "../hub/sections";
import { useProjectRows } from "../project/rows";
import { goToAction } from "../shell/BottomPanel";
import { useDiscard } from "../recovery/discard";
import { useNav } from "../state/nav";
import { useTower } from "../state/tower";
import { attention, type AttentionAction } from "./attention";
import { AttentionList } from "./AttentionList";
import { dayItems, Footer } from "./Footer";
import { DENSITIES, useGridPrefs, type ControlView } from "./grid";
import { Health } from "./Health";
import { useHubCounters } from "./hubCounters";
import { Infra } from "./Infra";
import { SessionUsageList } from "./SessionUsageList";
import { Sidebar } from "./Sidebar";
import { TileGrid } from "./TileGrid";
import { WaitingNow } from "./WaitingNow";

const VIEW_TITLE: Record<ControlView, string> = {
  attention: "Precisa de você",
  running: "Rodando",
  done: "Concluídas hoje",
  usage: "Uso por sessão",
};

const plural = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`;

/** "Sexta, 27 de novembro". */
export function todayText(now: number): string {
  const s = new Date(now).toLocaleDateString("pt-BR", { weekday: "long", day: "numeric", month: "long" });
  // "sexta-feira, 27 de novembro" → "Sexta, 27 de novembro".
  const t = s.replace(/-feira/, "");
  return t.charAt(0).toUpperCase() + t.slice(1);
}

interface Chip {
  key: string;
  label: string;
  tone: string;
  /** Visão que o chip alterna; sem ela, `go` abre outra tela. */
  view?: ControlView;
  go?: () => void;
}

/**
 * Sala de controle (#316, #357). Na tela inicial: o topo (título, data,
 * contadores em chips e o botão de saúde), "Precisa de você" compacta e o
 * rodapé Infraestrutura; o centro fica para os cards por projeto (#358) e o
 * bloco do piloto (#359). Cada chip de contagem abre a visão dele (a grade
 * filtrada, o fim do dia ou o uso por sessão); clicar de novo volta.
 */
export function ControlRoom() {
  const t = useTower();
  const nav = useNav();
  const now = useNow();
  const discard = useDiscard();
  const projects = useProjectRows();
  const hubCounters = useHubCounters();
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
  const running = Math.max(0, live.length - waiting);
  const done = dayItems(t.envs, t.dashboards, now).done.length;
  const onAction = (a: AttentionAction) => goToAction(nav, a, (url) => void t.api.openUrl(url).catch(() => {}));

  const chips: Chip[] = [
    { key: "attention", view: "attention", tone: "attention", label: items.length === 0 ? "Nada pede você" : items.length === 1 ? "1 pede você" : `${items.length} pedem você` },
    { key: "running", view: "running", tone: "running", label: `${running} rodando` },
    { key: "done", view: "done", tone: "done", label: plural(done, "concluída hoje", "concluídas hoje") },
    { key: "usage", view: "usage", tone: "neutral", label: "Uso por sessão" },
  ];
  // Só com hub: somem se ele não responde (fora do ar ou anterior ao recurso).
  if (hubCounters.epics !== null) {
    chips.push({
      key: "epics",
      tone: "running",
      label: plural(hubCounters.epics, "épico em execução", "épicos em execução"),
      go: () => nav.openTab(hubTab("epic-runs")),
    });
  }
  const pilot = hubCounters.pilot;
  if (pilot && pilot.projects > 0) {
    chips.push({
      key: "pilot",
      tone: pilot.paused ? "attention" : "done",
      label: pilot.paused ? "Piloto pausado" : `Piloto ligado · ${plural(pilot.projects, "projeto", "projetos")}`,
      go: () => nav.openTab(hubTab("autopilot-panel")),
    });
  }

  const showTiles = view === "attention" || view === "running";
  const showAttention = view === null || view === "attention";
  // Web: "Aguardando você" sai da lista e vai em destaque no topo (§10.3).
  const highlight = t.web ? items.filter((i) => i.kind === "waiting") : [];
  const list = t.web ? items.filter((i) => i.kind !== "waiting") : items;

  return (
    <main className="control-main">
      <Sidebar now={now} />
      <header className="control-head">
        <div className="control-heading">
          {view && (
            <button type="button" className="cr-link" onClick={() => setView(null)}>
              ‹ Sala de controle
            </button>
          )}
          <h1 className="control-title">{view ? VIEW_TITLE[view] : "Sala de controle"}</h1>
          <span className="muted">
            {todayText(now)} · {plural(projects.length, "projeto", "projetos")}
          </span>
        </div>
        <ul className="cr-chips" aria-label="Contadores">
          {chips.map((c) => (
            <li key={c.key}>
              <button
                type="button"
                className="cr-chip"
                data-tone={c.tone}
                aria-pressed={c.view ? view === c.view : undefined}
                onClick={() => (c.view ? setView(view === c.view ? null : c.view) : c.go?.())}
              >
                {c.label}
              </button>
            </li>
          ))}
        </ul>
        <span className="control-spacer" />
        <Health now={now} items={items} onAction={onAction} />
      </header>
      {showTiles && (
        <div className="control-tools">
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
        </div>
      )}
      {showAttention && <WaitingNow items={highlight} now={now} />}
      {showAttention && (
        <AttentionList
          items={list}
          now={now}
          collapsible={view === null}
          onAction={onAction}
          onDismiss={t.dismissAlert}
          onDiscard={(d, origin) => discard(d, { origin })}
        />
      )}
      {showTiles && (
        <TileGrid
          now={now}
          filter={view === "attention" ? "attention" : "running"}
          group={grid.group}
          density={grid.density}
        />
      )}
      {view === "usage" && <SessionUsageList now={now} />}
      {view === "done" && <Footer now={now} />}
      {view === null && <Infra now={now} />}
    </main>
  );
}
