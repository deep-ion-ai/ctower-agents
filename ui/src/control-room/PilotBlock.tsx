// Bloco do piloto automático da Sala de controle (#359, épico #353): o estado
// global, as contas e uma linha por projeto autônomo, no fim da página. Sem
// backend novo: lê `hub.autopilot.status` e `hub.autopilot.digest` (relidos
// pelo push `hub.autopilot.changed`) e reaproveita os textos de
// `hub/autopilot.ts`. Sem hub, ou com hub anterior ao status, não aparece.
import { useCallback, useEffect, useState } from "react";
import { errText } from "../hub/calls";
import {
  PILOT_STATE_HINT,
  PILOT_STATE_TEXT,
  aheadText,
  ciText,
  digestCounts,
  localTime,
  nextBandText,
  pilotState,
  reasonText,
  weeklyHeadline,
} from "../hub/autopilot";
import { hubTab } from "../hub/sections";
import type {
  AutonomyWip,
  AutopilotSettings,
  AutopilotStatus,
  DigestProject,
  MorningDigest,
  ProjectPlan,
} from "../protocol/hub-gen";
import { ExternalLink } from "../shell/ExternalLink";
import { useNav } from "../state/nav";
import { useTower } from "../state/tower";
import { ago } from "../util/time";

const WINDOW_TEXT: Record<string, string> = { five_hour: "janela de 5h", seven_day: "semanal" };
const localNow = (ms: number) => localTime(ms, -new Date(ms).getTimezoneOffset());

/** "Agora: #120 (running)" das sessões autônomas vivas do projeto; `null` sem nenhuma. */
export function nowText(sessions: AutopilotStatus["sessions"], p: Pick<ProjectPlan, "owner" | "repo">): string | null {
  const mine = sessions.filter((s) => s.owner === p.owner && s.repo === p.repo);
  return mine.length ? `Agora: ${mine.map((s) => `#${s.gh_number} (${s.state})`).join(", ")}` : null;
}

/** "Próxima: #123" ou "Nada inicia agora: motivo", com o tamanho da fila. */
export function nextText(p: Pick<ProjectPlan, "next" | "probe" | "blocked_message" | "queue">): string {
  const size = `fila: ${p.queue.length}`;
  if (p.next != null) return `Próxima: #${p.next}${p.probe ? " (sonda)" : ""} · ${size}`;
  return `${p.blocked_message ? `Nada inicia agora: ${p.blocked_message}` : "Nada inicia agora"} · ${size}`;
}

/** Contagens do resumo sem o que a linha da wip já mostra (à frente da base, esperando bloqueante). */
export const sinceDigestText = (d: DigestProject) => digestCounts({ ...d, ahead: 0, waiting_blocker: 0 });

export function PilotBlock({ now }: { now: number }) {
  const { api, hub } = useTower();
  const nav = useNav();
  const [status, setStatus] = useState<AutopilotStatus | null>(null);
  const [digest, setDigest] = useState<MorningDigest | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      setStatus(await api.hubCall<AutopilotStatus>("hub.autopilot.status"));
    } catch {
      // Hub anterior ao agendador (ou fora do ar): o bloco some.
      setStatus(null);
      return;
    }
    try {
      const d = await api.hubCall<MorningDigest[]>("hub.autopilot.digest", { limit: 1 });
      setDigest(d[0] ?? null);
    } catch {
      setDigest(null);
    }
  }, [api]);

  useEffect(() => {
    if (!hub.connected) {
      setStatus(null);
      setDigest(null);
      return;
    }
    void load();
    return api.onHubPush((e) => {
      if (e.event === "hub.autopilot.changed") void load();
    });
  }, [api, hub.connected, load]);

  if (!hub.connected || !status) return null;

  const toggle = async (pause: boolean) => {
    setBusy(true);
    setError(null);
    try {
      const s = await api.hubCall<AutopilotSettings>(pause ? "hub.autopilot.pause" : "hub.autopilot.resume");
      setStatus((cur) => (cur ? { ...cur, settings: s } : cur));
      void load();
    } catch (e) {
      setError(errText(e));
    } finally {
      setBusy(false);
    }
  };

  const state = pilotState(status);
  const paused = status.settings.paused;
  const wipOf = (p: ProjectPlan): AutonomyWip | undefined =>
    status.wip?.find((w) => w.owner === p.owner && w.repo === p.repo);
  const digestOf = (p: ProjectPlan) => digest?.projects.find((d) => d.owner === p.owner && d.repo === p.repo);
  const lastEvent = status.events[0];

  return (
    <section className="cr-pilot" aria-label="Piloto automático">
      <header className="cr-pilot-head">
        <h2 className="cr-pilot-title">Piloto automático</h2>
        <p role="status" aria-label="Estado do piloto" className="cr-pilot-state" data-state={state}>
          <span className="strong">{PILOT_STATE_TEXT[state]}</span>: {PILOT_STATE_HINT[state]}.
        </p>
        <span className="muted small">
          {status.last_tick_at != null
            ? `Último ciclo do agendador ${ago(status.last_tick_at, now)}.`
            : "O agendador ainda não rodou desde que o hub subiu."}
        </span>
        <span className="control-spacer" />
        <button
          type="button"
          className={paused ? "btn btn-sm btn-primary" : "btn btn-sm btn-danger"}
          disabled={busy}
          onClick={() => void toggle(!paused)}
        >
          {paused ? "Retomar" : "Pausar tudo"}
        </button>
        <button type="button" className="cr-link" onClick={() => nav.openTab(hubTab("autopilot-panel"))}>
          Painel do piloto ›
        </button>
      </header>
      {error && (
        <p role="alert" className="start-error">
          {error}
        </p>
      )}

      {status.accounts.length > 0 && (
        <ul className="cr-pilot-list" aria-label="Contas do piloto">
          {status.accounts.map((a) => {
            const parked = status.parked.filter((s) => s.account_key === a.account);
            return (
              <li key={a.account} data-account={a.account}>
                <span className="mono">{a.account}</span>
                {a.exhausted_window ? (
                  <span data-warn="true">
                    limite do plano atingido ({WINDOW_TEXT[a.exhausted_window] ?? a.exhausted_window})
                    {a.exhausted_until != null && ` até ${localNow(a.exhausted_until)}`}
                    {parked.length > 0 && ` · estacionadas: ${parked.map((s) => `#${s.gh_number}`).join(", ")}`}
                  </span>
                ) : (
                  <span>
                    {weeklyHeadline(a.decision)} ·{" "}
                    {a.decision.can_start
                      ? "pode iniciar agora"
                      : `não inicia agora: ${a.decision.reasons.map(reasonText).join("; ")} · ${nextBandText(a.decision)}`}
                  </span>
                )}
              </li>
            );
          })}
        </ul>
      )}

      {status.projects.length === 0 ? (
        <p className="muted small">Nenhum projeto no modo autônomo.</p>
      ) : (
        <ul className="cr-pilot-list" aria-label="Projetos autônomos">
          {status.projects.map((p) => {
            const key = `${p.owner}/${p.repo}`;
            const wip = wipOf(p);
            const d = digestOf(p);
            const agora = nowText(status.sessions, p);
            return (
              <li key={key} data-project={key}>
                <span className="strong">{p.repo}</span>
                <span className="muted">
                  {p.env_name ?? p.env_id} · {p.env_online ? "online" : "offline"}
                </span>
                <span>{[agora, nextText(p)].filter(Boolean).join(" · ")}</span>
                {wip && (
                  <span className="muted">
                    <span className="mono">{wip.branch}</span>: {aheadText(wip)}
                    {wip.conflict && " · em conflito com a base"}
                    {wip.error && ` · erro na branch: ${wip.error}`}
                    {wip.pr_url && (
                      <>
                        {" · "}
                        <ExternalLink href={wip.pr_url}>PR</ExternalLink> ({ciText(wip.ci)})
                      </>
                    )}
                  </span>
                )}
                {d && <span className="muted">Desde o resumo: {sinceDigestText(d)}</span>}
              </li>
            );
          })}
        </ul>
      )}

      <footer className="cr-pilot-foot muted small">
        <span>{digest ? `Resumo da manhã: ${localNow(digest.at)}` : "Sem resumo da manhã ainda"}</span>
        <span>{lastEvent ? `Último evento: ${lastEvent.message} (${ago(lastEvent.at, now)})` : "Nenhum evento do agendador ainda"}</span>
      </footer>
    </section>
  );
}
