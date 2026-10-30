import "../hub/hub.css";
import { useNav } from "../state/nav";
import { itemTitle, type ProviderProject } from "../util/item";
import { ago } from "../util/time";
import type { EnvSessions, ProjectSession, SessionKind } from "./sessions";

const KIND_LABEL: Record<SessionKind, string> = {
  live: "Vivas",
  interrupted: "Interrompidas",
  ended: "Encerradas com erro (últimas 24 h)",
};

/**
 * Aba Sessões do projeto (#17): por ambiente, as vivas (abrem o terminal),
 * as interrompidas (abrem na Recuperação, onde se retoma ou descarta) e as
 * que saíram com erro (abrem o terminal com a saída). Ambiente que só o hub
 * conhece fica só leitura. Os dados vêm de `projectSessions`.
 */
export function ProjectSessions({
  groups,
  project,
  now,
  emptyText,
}: {
  groups: EnvSessions[];
  project: ProviderProject | null;
  now: number;
  /** Sem nenhum ambiente com o projeto (cada visão explica o porquê). */
  emptyText: string;
}) {
  const nav = useNav();
  if (groups.length === 0) return <p className="muted">{emptyText}</p>;

  const action = (g: EnvSessions, s: ProjectSession, label: string) => {
    const env = g.env;
    if (!env) return null;
    if (s.kind === "interrupted") {
      return (
        <button
          type="button"
          className="btn btn-sm"
          aria-label={`Abrir na Recuperação: ${label}`}
          onClick={() => nav.go({ name: "recovery", env, session: s.id })}
        >
          Abrir na Recuperação
        </button>
      );
    }
    return (
      <button
        type="button"
        className="btn btn-sm"
        aria-label={`Abrir terminal: ${label}`}
        onClick={() => nav.openTerminal({ env, session: s.id })}
      >
        Abrir terminal
      </button>
    );
  };

  const when = (s: ProjectSession): string => {
    if (s.kind === "live") return `iniciada ${ago(s.started_at, now)} · último evento ${ago(s.last_event_at, now)}`;
    // O mesmo motivo da Recuperação (`reason`): sem código de saída, o processo se perdeu.
    const why = s.exit_code === null ? "processo perdido" : `saiu com código ${s.exit_code}`;
    if (s.kind === "interrupted") return `${why} · último evento ${ago(s.last_event_at, now)}`;
    return `saiu com código ${s.exit_code ?? "?"} ${ago(s.ended_at ?? s.last_event_at, now)}`;
  };

  return (
    <div className="pj-sessions">
      {groups.map((g) => {
        const total = g.live.length + g.interrupted.length + g.ended.length;
        return (
          <section key={g.key} className="hub-card" aria-labelledby={`pj-ses-${g.key}`}>
            <div className="hub-card-head">
              <h2 id={`pj-ses-${g.key}`}>{g.name}</h2>
              {!g.env && <span className="muted small">só pelo hub: esta UI não tem conexão com o ambiente</span>}
            </div>
            {total === 0 ? (
              <p className="muted hub-card-empty">Nenhuma sessão viva, interrompida ou encerrada com erro.</p>
            ) : (
              (["live", "interrupted", "ended"] as SessionKind[])
                .filter((k) => g[k].length > 0)
                .map((k) => (
                  <div key={k}>
                    <h3 className="pj-ses-kind">
                      {KIND_LABEL[k]} · {g[k].length}
                    </h3>
                    <ul className="hub-card-list" aria-label={`${KIND_LABEL[k]} em ${g.name}`}>
                      {g[k].map((s) => {
                        const label = itemTitle({ gh_number: s.gh, title: s.title }, project);
                        return (
                          <li key={s.id}>
                            <span className="hub-card-text">
                              {label}
                              <span className="hub-card-sub">{when(s)}</span>
                            </span>
                            {action(g, s, label)}
                          </li>
                        );
                      })}
                    </ul>
                  </div>
                ))
            )}
          </section>
        );
      })}
    </div>
  );
}
