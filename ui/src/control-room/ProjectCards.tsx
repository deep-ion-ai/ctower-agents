// Cards por projeto da Sala de controle (#358): o centro da tela inicial,
// entre "Precisa de você" e o bloco do piloto. Os dados vêm de
// `projectCards` (função pura); aqui só a apresentação, os filtros e a
// navegação. O piloto automático não entra no card: tem o bloco próprio (#359).
import { useState } from "react";
import { epicRunTab } from "../hub/sections";
import type { ProjectRow } from "../project/rows";
import { projectAt } from "../project/sections";
import { useNav } from "../state/nav";
import { useTower } from "../state/tower";
import type { AttentionItem, HubRoom } from "./attention";
import { useHubReleases } from "./hubReleases";
import {
  CARD_FILTERS,
  filterCards,
  projectCards,
  SESSION_STATE_TEXT,
  type CardFilter,
  type CardFlow,
  type ProjectCard,
} from "./cards";

const FLOW: { key: keyof CardFlow; label: string }[] = [
  { key: "backlog", label: "Backlog" },
  { key: "doing", label: "Em andamento" },
  { key: "review", label: "Em revisão" },
  { key: "done", label: "Feitas 7 dias" },
];

export function ProjectCards({
  rows,
  items,
  room,
  now,
}: {
  rows: ProjectRow[];
  items: AttentionItem[];
  room?: HubRoom;
  now: number;
}) {
  const t = useTower();
  const releases = useHubReleases();
  const [filter, setFilter] = useState<CardFilter>("all");
  if (rows.length === 0) return null;
  const cards = projectCards({
    rows,
    items,
    dashboards: t.dashboards,
    hubDashboard: t.hub.dashboard ?? null,
    permissions: t.permissions,
    room,
    releases,
    now,
  });
  const shown = filterCards(cards, filter);

  return (
    <section className="cr-projects" aria-labelledby="cr-projects-title">
      <div className="cr-projects-head">
        <h2 id="cr-projects-title" className="cr-projects-title">
          Projetos
        </h2>
        <div role="group" aria-label="Filtrar projetos" className="control-seg">
          {CARD_FILTERS.map((f) => (
            <button
              key={f.id}
              type="button"
              className="btn btn-sm"
              aria-pressed={filter === f.id}
              onClick={() => setFilter(f.id)}
            >
              {f.label} · {filterCards(cards, f.id).length}
            </button>
          ))}
        </div>
        <span className="control-spacer" />
        <span className="muted small">Ordem: o que pede você primeiro</span>
      </div>
      {shown.length === 0 ? (
        <p className="muted small">Nenhum projeto neste filtro.</p>
      ) : (
        <ul className="cr-cards" aria-label="Cards dos projetos">
          {shown.map((c) => (
            <Card key={c.key} card={c} />
          ))}
        </ul>
      )}
    </section>
  );
}

function Card({ card: c }: { card: ProjectCard }) {
  const nav = useNav();
  const open = () => nav.go(projectAt(c.row.screen));
  const titleId = `cr-card-${c.key.replace(/[^a-zA-Z0-9_-]/g, "-")}`;
  const total = c.flow.backlog + c.flow.doing + c.flow.review + c.flow.done;

  return (
    <li className="cr-card" aria-labelledby={titleId} data-tone={c.badge.tone}>
      <div className="cr-card-head">
        <div className="cr-card-name">
          <div className="cr-card-title-line">
            <h3 id={titleId} className="cr-card-title">
              <button type="button" className="cr-link" onClick={open}>
                {c.name}
              </button>
            </h3>
            {c.autonomous && <span className="cr-card-auto">autônomo</span>}
          </div>
          <span className="mono muted small">{c.repo}</span>
        </div>
        <span className="cr-card-badge" data-tone={c.badge.tone}>
          {c.badge.text}
        </span>
      </div>

      <div className="cr-card-block">
        <span className="cr-card-label">
          {c.sessions.length > 0 ? `Agora · ${c.sessions.length === 1 ? "1 sessão" : `${c.sessions.length} sessões`}` : "Agora"}
        </span>
        {c.sessions.length > 0 ? (
          <ul className="cr-card-sessions" aria-label={`Sessões de ${c.name}`}>
            {c.sessions.map((s) => (
              <li key={s.key} data-state={s.state}>
                <button
                  type="button"
                  className="cr-card-session"
                  onClick={() => (s.terminal ? nav.openTerminal(s.terminal) : open())}
                >
                  <span className="cr-card-dot" aria-hidden="true" />
                  <span className="mono muted small">{s.ref}</span>
                  <span className="cr-card-session-title">{s.title}</span>
                  <span className="cr-card-state small">{SESSION_STATE_TEXT[s.state]}</span>
                </button>
              </li>
            ))}
          </ul>
        ) : (
          <span className="muted small">{c.idleText}</span>
        )}
      </div>

      <div className="cr-card-block">
        <div className="cr-flow-bar" aria-hidden="true">
          {total > 0 &&
            FLOW.map((f) => <span key={f.key} data-flow={f.key} style={{ flexGrow: c.flow[f.key] }} />)}
        </div>
        <dl className="cr-flow" aria-label={`Fluxo de ${c.name}`}>
          {FLOW.map((f) => (
            <div key={f.key} data-flow={f.key}>
              <dt>{f.label}</dt>
              <dd>{c.flow[f.key]}</dd>
            </div>
          ))}
        </dl>
      </div>

      <dl className="cr-card-facts">
        <div>
          <dt>Decisões a revisar</dt>
          <dd data-warn={c.decisions > 0}>
            {c.decisions === 0 ? "Nenhuma" : c.decisions === 1 ? "1 autônoma" : `${c.decisions} autônomas`}
          </dd>
        </div>
        {c.prs && (
          <div>
            <dt>{c.prs.label}</dt>
            <dd data-tone={c.prs.tone} title={c.prs.lines.join("\n") || undefined}>
              {c.prs.text}
            </dd>
          </div>
        )}
        {c.release && (
          <div>
            <dt>Release</dt>
            <dd>{c.release.text}</dd>
            {c.release.note && (
              <dd className="small" data-warn={c.release.warn} title={c.release.lines.join("\n") || undefined}>
                {c.release.note}
              </dd>
            )}
          </div>
        )}
      </dl>

      {c.epics && (
        <div className="cr-card-epics">
          <span className="cr-card-label">
            {c.epics.length > 0 ? `Épicos em execução · ${c.epics.length}` : "Nenhum épico em execução"}
          </span>
          {c.epics.length > 0 && (
            <ul aria-label={`Épicos em execução de ${c.name}`}>
              {c.epics.map((e) => (
                <li key={e.runId} data-stopped={e.stopped}>
                  <button
                    type="button"
                    className="cr-card-epic"
                    onClick={() => nav.openTab(epicRunTab({ id: e.runId, epic: e.epic }))}
                  >
                    <span className="mono small cr-card-epic-ref">{e.ref}</span>
                    <span className="cr-card-session-title">{e.title}</span>
                  </button>
                  <div className="cr-epic-progress">
                    <div
                      className="cr-epic-bar"
                      role="progressbar"
                      aria-label={`Progresso de ${e.ref}`}
                      aria-valuemin={0}
                      aria-valuemax={e.total > 0 ? e.total : undefined}
                      aria-valuenow={e.total > 0 ? e.done : undefined}
                      aria-valuetext={e.progress}
                    >
                      <span style={{ width: e.total > 0 ? `${(100 * e.done) / e.total}%` : "0%" }} />
                    </div>
                    <span className="small">{e.progress}</span>
                  </div>
                  <span className="muted small" data-warn={e.stopped}>
                    {e.step}
                  </span>
                </li>
              ))}
            </ul>
          )}
        </div>
      )}

      <div className="cr-card-foot small">
        <span className={c.sync.warn ? undefined : "muted"} data-warn={c.sync.warn}>
          {c.sync.text}
        </span>
        <span className="control-spacer" />
        <button type="button" className="cr-link" onClick={open}>
          Abrir projeto ›
        </button>
      </div>
    </li>
  );
}
