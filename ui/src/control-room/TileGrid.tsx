import { useEffect, useState } from "react";
import type { DaemonApi } from "../api/daemon";
import type { Peek, Session, WorkItemStatus } from "../protocol/gen";
import { agentLabel } from "../settings/session-options";
import { useNav } from "../state/nav";
import { useTower } from "../state/tower";
import { itemRef } from "../util/item";
import { screenTail } from "../util/screen";
import { duration } from "../util/time";
import { GESTOR_LABEL, itemLabel, lookup } from "./attention";
import type { Density } from "./grid";

/** Consulta do fim do terminal enquanto a Sala de controle está na tela. */
export const PEEK_INTERVAL_MS = 2_000;
const TAIL_LINES = 4;

interface Key {
  env: string;
  session: string;
}

const keyOf = (k: Key) => `${k.env}|${k.session}`;

/** Últimas linhas de cada sessão via `session.peek` (sem anexar). */
export function usePeek(api: DaemonApi, keys: Key[]): Record<string, string> {
  const [tails, setTails] = useState<Record<string, string>>({});
  const sig = keys.map(keyOf).join(",");
  useEffect(() => {
    if (keys.length === 0) return;
    let alive = true;
    const tick = async () => {
      if (document.visibilityState === "hidden") return;
      const got = await Promise.all(
        keys.map(async (k) => {
          try {
            const r = await api.call<Peek>(k.env, "session.peek", { session_id: k.session });
            return [keyOf(k), await screenTail(r.data, TAIL_LINES)] as const;
          } catch {
            return null;
          }
        }),
      );
      if (alive) {
        setTails((prev) => ({ ...prev, ...Object.fromEntries(got.filter((x) => x !== null)) }));
      }
    };
    void tick();
    const id = setInterval(tick, PEEK_INTERVAL_MS);
    return () => {
      alive = false;
      clearInterval(id);
    };
    // `sig` resume `keys`: só refaz o polling quando o conjunto de sessões muda.
  }, [api, sig]);
  return tails;
}

/** Estado escrito e tom do selo do tile. */
export function tileState(s: Session, status?: WorkItemStatus): { text: string; tone: string } {
  if (status === "aguardando_voce") return { text: "aguardando você", tone: "attention" };
  switch (s.state) {
    case "running":
      return { text: "rodando", tone: "running" };
    case "idle":
      return { text: "parada no turno", tone: "neutral" };
    default:
      return { text: "iniciando", tone: "running" };
  }
}

/** Sessões vivas a mostrar: todas, só as que esperam você ou só as que andam sozinhas. */
export type TileFilter = "all" | "attention" | "running";

const FILTER_TITLE: Record<TileFilter, string> = {
  all: "Sessões vivas",
  attention: "Aguardando você",
  running: "Rodando",
};

export function TileGrid({
  now,
  filter = "all",
  group = false,
  density = "auto",
}: {
  now: number;
  filter?: TileFilter;
  /** Agrupa os tiles por projeto (subtítulo por grupo). */
  group?: boolean;
  density?: Density;
}) {
  const { envs, dashboards, api, web } = useTower();
  // Com mais de um ambiente, cada tile diz onde a sessão roda.
  const envName = (id: string) => (envs.length > 1 ? `${envs.find((e) => e.id === id)?.name ?? id} · ` : "");
  const nav = useNav();
  const live = envs
    .flatMap((e) =>
      (dashboards[e.id]?.live_sessions ?? []).map((s) => ({
        env: e.id,
        s,
        ...lookup(dashboards[e.id], s),
      })),
    )
    .filter(({ wi }) => {
      const waiting = wi?.status === "aguardando_voce";
      return filter === "all" || (filter === "attention" ? waiting : !waiting);
    });
  // Web: sem terminal, sem o fim da tela (`session.peek` é do ambiente).
  const tails = usePeek(api, web ? [] : live.map(({ env, s }) => ({ env, session: s.id })));

  type Row = (typeof live)[number];
  const tile = ({ env, s, wi, project, gestor, scope }: Row) => {
    const st = tileState(s, wi?.status);
    return (
      <li key={`${env}:${s.id}`}>
        <button type="button" className="tile" onClick={() => nav.openTerminal({ env, session: s.id })}>
          <span className="tile-head">
            <span className="mono muted">
              {gestor ? "hub inteiro" : `${project?.name ?? ""} ${wi ? itemRef(wi.gh_number, project) : ""}`}
            </span>
            <span className="badge" data-tone={st.tone}>
              {st.text}
            </span>
          </span>
          <span className="tile-title">{wi?.title ?? itemLabel(project, wi, gestor, scope)}</span>
          <span className="tile-meta">
            <span className="mono">{wi?.branch}</span>
            <span>
              {envName(env)}
              {s.tool} · {agentLabel(s)} · {duration(now - s.started_at)}
            </span>
          </span>
          {!web && (
            <pre className="tile-tail" aria-label="Últimas linhas do terminal">
              <span>{tails[keyOf({ env, session: s.id })] ?? ""}</span>
            </pre>
          )}
        </button>
      </li>
    );
  };

  // Agrupado: um grupo por projeto (o gestor e as sessões sem projeto em grupos próprios).
  const groups = new Map<string, { name: string; rows: Row[] }>();
  for (const r of live) {
    const [k, name] = r.project
      ? [`${r.env}|p${r.project.id}`, r.project.name]
      : r.gestor
        ? [`${r.env}|gestor`, GESTOR_LABEL]
        : [`${r.env}|outras`, "Sem projeto"];
    const g = groups.get(k) ?? {
      name: envs.length > 1 ? `${name} · ${envName(r.env).slice(0, -3)}` : name,
      rows: [],
    };
    g.rows.push(r);
    groups.set(k, g);
  }

  return (
    <section aria-labelledby="rodando" className="tiles-section">
      <h2 id="rodando" className="section-title">
        {FILTER_TITLE[filter]} · {live.length}
      </h2>
      {live.length === 0 ? (
        <p className="empty">
          {filter === "attention"
            ? "Nenhuma sessão esperando você."
            : web
              ? "Nenhuma sessão rodando."
              : "Nenhuma sessão rodando. Inicie uma pela tela Projeto."}
        </p>
      ) : group ? (
        [...groups.entries()]
          .sort((a, b) => a[1].name.localeCompare(b[1].name))
          .map(([k, g]) => (
            <section key={k} className="tile-group" aria-label={`Projeto ${g.name}`}>
              <h3 className="tile-group-title">
                {g.name} <span className="muted">· {g.rows.length}</span>
              </h3>
              <ul className="tile-grid" data-density={density}>
                {g.rows.map(tile)}
              </ul>
            </section>
          ))
      ) : (
        <ul className="tile-grid" data-density={density}>
          {live.map(tile)}
        </ul>
      )}
    </section>
  );
}
