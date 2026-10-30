import "../hub/hub.css";
import type { ReactNode } from "react";
import type { Tone } from "./settings";
import type { ProjectPage, ProjectSettingsSection } from "./sections";
import type { SessionKind } from "./sessions";

/** Atribuição a um ambiente, já escrita (`assignmentText`). */
export interface OverviewAssignment {
  env: string;
  text: string;
  tone: Tone;
}

/** Sessão viva na Visão geral: o rótulo e, se a UI alcança o ambiente, como abrir. */
export interface OverviewLive {
  id: string;
  label: string;
  envName: string;
  open?: () => void;
}

export interface ProjectOverviewProps {
  /** Estado em uma linha (atribuições e sync, `projectState`); `null` = visão local, sem o hub. */
  state: { text: string; tone: Tone } | null;
  sync: { text: string; tone: Tone; error?: string | null; detail?: string | null };
  /** Atribuições por ambiente (hub); `null` = visão local: `localEnv` é o ambiente mostrado. */
  assignments: OverviewAssignment[] | null;
  localEnv?: string;
  live: OverviewLive[];
  counts: Record<SessionKind, number>;
  /** Vai para outra aba (ou seção das Configurações) do mesmo projeto. */
  onPage: (page: ProjectPage, section?: ProjectSettingsSection) => void;
  /**
   * Ponto de extensão (#19): saúde do projeto, "Buscar issues agora", gestor e
   * outras ações rápidas entram aqui como cartões `hub-card`, depois dos
   * básicos, sem mexer neles.
   */
  extra?: ReactNode;
}

/**
 * Aba Visão geral do projeto (#17), versão básica: estado, sincronização,
 * atribuições (ou o ambiente local, sem o hub) e sessões vivas, cada cartão
 * com o atalho para a aba que aprofunda. A mesma para a visão do hub e a
 * local: cada tela traduz os seus dados nestas props.
 */
export function ProjectOverview(p: ProjectOverviewProps) {
  const { counts } = p;
  return (
    <div className="hub-cards pj-overview-cards">
      <section className="hub-card" aria-labelledby="pj-ov-state">
        <div className="hub-card-head">
          <h2 id="pj-ov-state">Estado</h2>
        </div>
        <dl className="hub-kv">
          {p.state && (
            <div>
              <dt>Projeto</dt>
              <dd>
                <span className="hub-dot" data-tone={p.state.tone} aria-hidden="true" /> {p.state.text}
              </dd>
            </div>
          )}
          <div>
            <dt>Sincronização</dt>
            <dd>
              <span className="hub-dot" data-tone={p.sync.tone} aria-hidden="true" /> {p.sync.text}
              {p.sync.detail && <span className="muted"> · {p.sync.detail}</span>}
              {p.sync.error && <span className="hub-card-sub">{p.sync.error}</span>}
            </dd>
          </div>
          <div>
            <dt>Sessões</dt>
            <dd>
              {counts.live} viva{counts.live === 1 ? "" : "s"} · {counts.interrupted} interrompida
              {counts.interrupted === 1 ? "" : "s"} · {counts.ended} com erro (24 h)
            </dd>
          </div>
        </dl>
      </section>

      <section className="hub-card" aria-labelledby="pj-ov-envs">
        <div className="hub-card-head">
          <h2 id="pj-ov-envs">{p.assignments ? "Atribuições" : "Ambiente"}</h2>
          {p.assignments && (
            <button type="button" className="btn btn-sm" onClick={() => p.onPage("settings", "envs")}>
              Gerenciar
            </button>
          )}
        </div>
        {p.assignments ? (
          p.assignments.length === 0 ? (
            <p className="muted hub-card-empty">Sem ambiente atribuído: dá para acompanhar, mas não iniciar sessões.</p>
          ) : (
            <ul className="hub-card-list" aria-label="Atribuições do projeto">
              {p.assignments.map((a) => (
                <li key={a.env}>
                  <span className="hub-card-text">
                    <span className="hub-dot" data-tone={a.tone} aria-hidden="true" /> {a.env}
                  </span>
                  <span className="muted small">{a.text}</span>
                </li>
              ))}
            </ul>
          )
        ) : (
          <p className="muted hub-card-empty">
            {p.localEnv ? `Dados locais de ${p.localEnv}` : "Dados locais"}: o hub está fora do ar ou sem conexão, e as
            atribuições ficam nele.
          </p>
        )}
      </section>

      <section className="hub-card" aria-labelledby="pj-ov-live">
        <div className="hub-card-head">
          <h2 id="pj-ov-live">Sessões vivas · {p.live.length}</h2>
          <button type="button" className="btn btn-sm" onClick={() => p.onPage("sessions")}>
            Ver sessões
          </button>
        </div>
        {p.live.length === 0 ? (
          <p className="muted hub-card-empty">Nenhuma sessão viva neste projeto.</p>
        ) : (
          <ul className="hub-card-list" aria-label="Sessões vivas do projeto">
            {p.live.map((l) => (
              <li key={l.id}>
                <span className="hub-card-text">
                  {l.label}
                  <span className="hub-card-sub">{l.envName}</span>
                </span>
                {l.open && (
                  <button type="button" className="btn btn-sm" aria-label={`Abrir terminal: ${l.label}`} onClick={l.open}>
                    Abrir terminal
                  </button>
                )}
              </li>
            ))}
          </ul>
        )}
      </section>

      {p.extra}
    </div>
  );
}
