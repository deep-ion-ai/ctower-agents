// Hub › Painel do piloto (#185, épico #178): o estado do agendador do
// piloto automático (`hub.autopilot.status`), relido pelo push
// `hub.autopilot.changed`. Mostra se está ligado, pausado ou aguardando
// faixa, "agora pode usar até X% da semanal" e a próxima faixa elegível por
// conta, a fila de cada projeto com o motivo de cada item que ficou de fora,
// a branch de integração, as sessões vivas e as estacionadas, e o botão
// "Pausar tudo" (e "Retomar"). No desktop e na PWA: na web, as contas
// chegam como `conta-N` (o hub anonimiza).
import { useCallback, useEffect, useState } from "react";
import type { AutopilotSettings, AutopilotStatus, ProjectPlan } from "../protocol/hub-gen";
import { useNav } from "../state/nav";
import { useTower } from "../state/tower";
import { ago } from "../util/time";
import {
  AUTOPILOT_STATUS_PROTOCOL,
  PILOT_STATE_HINT,
  PILOT_STATE_TEXT,
  aheadText,
  ciText,
  localTime,
  nextBandText,
  pilotState,
  reasonText,
  skipText,
  weeklyHeadline,
} from "./autopilot";
import { errText } from "./calls";
import { hubTab } from "./sections";

const WINDOW_TEXT: Record<string, string> = { five_hour: "janela de 5h", seven_day: "semanal" };
const localNow = (ms: number) => localTime(ms, -new Date(ms).getTimezoneOffset());

export function HubAutopilotPanel({ now }: { now: number }) {
  const { api, hub, web } = useTower();
  const nav = useNav();
  const [status, setStatus] = useState<AutopilotStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [msg, setMsg] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      setStatus(await api.hubCall<AutopilotStatus>("hub.autopilot.status"));
      setError(null);
    } catch (e) {
      const t = errText(e);
      setError(
        t.includes("método desconhecido")
          ? `Este hub não tem o agendador do piloto automático (anterior ao protocolo ${AUTOPILOT_STATUS_PROTOCOL}): atualize o hub.`
          : t,
      );
    }
  }, [api]);

  useEffect(() => {
    if (!hub.connected) return;
    void load();
    return api.onHubPush((e) => {
      if (e.event === "hub.autopilot.changed") void load();
    });
  }, [api, load, hub.connected]);

  const toggle = async (pause: boolean) => {
    setBusy(true);
    setMsg(null);
    try {
      const s = await api.hubCall<AutopilotSettings>(pause ? "hub.autopilot.pause" : "hub.autopilot.resume");
      setStatus((cur) => (cur ? { ...cur, settings: s } : cur));
      setMsg(
        pause
          ? "Piloto automático pausado: nenhuma sessão nova inicia. As em andamento terminam a issue."
          : "Piloto automático retomado.",
      );
      void load();
    } catch (e) {
      setError(errText(e));
    } finally {
      setBusy(false);
    }
  };

  const state = status ? pilotState(status) : null;

  return (
    <section aria-labelledby="hub-autopilot-panel" className="panel settings-section">
      <div className="hub-page-head">
        <h2 id="hub-autopilot-panel">Painel do piloto automático</h2>
        {status && (
          <button
            type="button"
            className={status.settings.paused ? "btn btn-primary" : "btn btn-danger"}
            disabled={busy}
            onClick={() => void toggle(!status.settings.paused)}
          >
            {status.settings.paused ? "Retomar" : "Pausar tudo"}
          </button>
        )}
        {!web && (
          <button type="button" className="btn btn-sm" onClick={() => nav.openTab(hubTab("autopilot"))}>
            Orçamento do plano
          </button>
        )}
      </div>
      {error && (
        <p role="alert" className="start-error">
          {error}
        </p>
      )}
      {msg && (
        <p role="status" className="state-on">
          {msg}
        </p>
      )}
      {!status && !error && <p className="muted">Lendo o estado do piloto automático…</p>}
      {status && state && (
        <>
          <p role="status" aria-label="Estado do piloto" className={state === "on" ? "state-on" : undefined}>
            <span className="strong">{PILOT_STATE_TEXT[state]}</span>: {PILOT_STATE_HINT[state]}.
            {status.settings.paused && status.settings.paused_at != null && (
              <span className="muted"> Pausado {ago(status.settings.paused_at, now)}.</span>
            )}
          </p>
          <p className="muted small">
            {status.last_tick_at != null
              ? `Último ciclo do agendador ${ago(status.last_tick_at, now)}.`
              : "O agendador ainda não rodou desde que o hub subiu."}{" "}
            Até {status.settings.max_sessions_per_env} sessão(ões) autônoma(s) por ambiente.
          </p>

          <h3>Contas</h3>
          {status.accounts.length === 0 ? (
            <p className="muted">Nenhuma conta com política de orçamento: o piloto automático está desligado.</p>
          ) : (
            <ul className="hub-card-list" aria-label="Contas">
              {status.accounts.map((a) => (
                <li key={a.account} data-account={a.account}>
                  <span className="hub-card-text">
                    <span className="mono">{a.account}</span> · {weeklyHeadline(a.decision)}
                    <span className="hub-card-sub muted">
                      {a.decision.can_start
                        ? "pode iniciar agora"
                        : `não inicia agora: ${a.decision.reasons.map(reasonText).join("; ")}`}{" "}
                      · {nextBandText(a.decision)}
                      {a.exhausted_window &&
                        ` · limite do plano atingido (${WINDOW_TEXT[a.exhausted_window] ?? a.exhausted_window})${
                          a.exhausted_until != null ? ` até ${localNow(a.exhausted_until)}` : ""
                        }`}
                    </span>
                  </span>
                </li>
              ))}
            </ul>
          )}

          <h3>Fila por projeto</h3>
          {status.projects.length === 0 ? (
            <p className="muted">Nenhum projeto no modo autônomo.</p>
          ) : (
            status.projects.map((p) => <ProjectQueue key={`${p.owner}/${p.repo}`} p={p} status={status} />)
          )}

          <h3>Sessões autônomas</h3>
          {status.sessions.length === 0 && status.parked.length === 0 ? (
            <p className="muted">Nenhuma sessão autônoma viva nem estacionada.</p>
          ) : (
            <ul className="hub-card-list" aria-label="Sessões autônomas">
              {status.sessions.map((s) => (
                <li key={s.session_id}>
                  <span className="hub-card-text">
                    {s.owner}/{s.repo}#{s.gh_number} · {s.state}
                    <span className="hub-card-sub muted">
                      {s.env_id} · iniciada {ago(s.started_at, now)}
                    </span>
                  </span>
                </li>
              ))}
              {status.parked.map((s) => (
                <li key={`p:${s.session_id}`} data-parked="true">
                  <span className="hub-card-text">
                    {s.owner}/{s.repo}#{s.gh_number} · estacionada
                    <span className="hub-card-sub muted">
                      limite da {WINDOW_TEXT[s.window] ?? s.window}; retoma {localNow(s.resume_at)}
                    </span>
                  </span>
                </li>
              ))}
            </ul>
          )}

          <h3>Eventos recentes</h3>
          {status.events.length === 0 ? (
            <p className="muted">Nenhum evento do agendador ainda.</p>
          ) : (
            <ul className="hub-card-list" aria-label="Eventos recentes">
              {status.events.slice(0, 10).map((e) => (
                <li key={e.id} data-kind={e.kind}>
                  <span className="hub-card-text">
                    {e.message}
                    <span className="hub-card-sub muted">{ago(e.at, now)}</span>
                  </span>
                </li>
              ))}
            </ul>
          )}
        </>
      )}
    </section>
  );
}

function ProjectQueue({ p, status }: { p: ProjectPlan; status: AutopilotStatus }) {
  const wip = status.wip?.find((w) => w.owner === p.owner && w.repo === p.repo);
  const key = `${p.owner}/${p.repo}`;
  return (
    <section className="hub-card" aria-label={`Fila de ${key}`}>
      <div className="hub-card-head">
        <h4>{key}</h4>
        <span className="muted small">
          {p.env_name ?? p.env_id} · {p.env_online ? "online" : "offline"}
        </span>
      </div>
      <p className={p.blocked ? "muted" : "state-on"}>
        {p.next != null
          ? `Próxima a iniciar: #${p.next}`
          : p.blocked_message
            ? `Nada inicia agora: ${p.blocked_message}`
            : "Nada inicia agora."}
      </p>
      {wip && (
        <p className="muted small">
          <span className="mono">{wip.branch}</span>: {aheadText(wip)}
          {wip.conflict && " · em conflito com a base"}
          {wip.pr_url && (
            <>
              {" · "}
              <a href={wip.pr_url} target="_blank" rel="noreferrer">
                PR {wip.branch} → {wip.base}
              </a>{" "}
              ({ciText(wip.ci)})
            </>
          )}
        </p>
      )}
      {p.queue.length === 0 ? (
        <p className="hub-card-empty muted">Fila vazia: nenhuma issue aberta no ranking do projeto.</p>
      ) : (
        <table className="hub-card-table">
          <thead>
            <tr>
              <th>Issue</th>
              <th>Prioridade</th>
              <th>Modelo</th>
              <th>Situação</th>
            </tr>
          </thead>
          <tbody>
            {p.queue.map((q) => (
              <tr key={q.number} data-skip={q.skip ?? ""}>
                <td>
                  #{q.number}
                  {q.title ? ` ${q.title}` : ""}
                </td>
                <td>{q.priority ?? "—"}</td>
                <td>{q.model ?? "—"}</td>
                <td>{q.skip ? `fora: ${skipText(q.skip)}` : q.number === p.next ? "inicia agora" : "elegível"}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </section>
  );
}
