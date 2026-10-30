// Painel da sessão no modo web (épico #205, spec §10.2): no lugar do
// terminal, o estado da sessão (pelo último envio ao hub), modelo, ambiente,
// há quanto tempo espera você e, com o Remote Control ligado, o botão
// **Abrir no Remote Control** (`claude.ai/code/<rc_session_id>`, #197).
// Fase 2 (#343), com o ambiente online: **Ligar Remote Control** (e abrir o
// link quando o id chegar), **Encerrar** e **Retomar**, pelo canal reverso.
import "./web.css";
import { useEffect, useRef, useState } from "react";
import { useNow } from "../App";
import { itemLabel, lookup } from "../control-room/attention";
import { tileState } from "../control-room/TileGrid";
import type { Session } from "../protocol/gen";
import { agentLabel } from "../settings/session-options";
import { SideContent, SideGroup, SidePanel, SideRow } from "../shell/SidePanel";
import { isShell, useNav, type Tab } from "../state/nav";
import { useTower } from "../state/tower";
import { RC_CONNECT_MS, rcView } from "../terminal/remote";
import { ago, duration } from "../util/time";
import { RESUME_PROMPT } from "../recovery/RecoveryScreen";
import { OFFLINE_ACTIONS, rcDone, rcRequested, useEnvOnline, useEnvSession, useRcRequest } from "./envActions";

const ENDED: Partial<Record<Session["state"], string>> = {
  exited: "encerrada",
  interrupted: "interrompida",
  archived: "arquivada",
};

/** A sessão da aba em qualquer lista do dashboard do ambiente (viva, interrompida ou com erro). */
function findSession(t: Tab, dashboards: ReturnType<typeof useTower>["dashboards"]): Session | undefined {
  const d = dashboards[t.env];
  if (!d) return undefined;
  return [...d.live_sessions, ...d.interrupted_sessions, ...d.failed_sessions].find((s) => s.id === t.session);
}

/** A "tela Terminal" da web: o painel da sessão ativa e, ao lado, as sessões vivas. */
export function SessionScreen() {
  const nav = useNav();
  const { envs, dashboards } = useTower();
  const live = envs.flatMap((e) =>
    (dashboards[e.id]?.live_sessions ?? []).map((s) => ({
      env: e.id,
      envName: e.name,
      s,
      ...lookup(dashboards[e.id], s),
    })),
  );
  // Shell (#423) só existe no desktop: na web não há aba dele.
  const active = nav.active && !isShell(nav.active) ? nav.active : null;
  return (
    <>
      <SideContent>
        <SidePanel title="Sessões" label="Sessões vivas">
          <SideGroup title="Vivas" count={live.length}>
            {live.map((r) => {
              const st = tileState(r.s, r.wi?.status);
              return (
                <SideRow
                  key={`${r.env}:${r.s.id}`}
                  label={itemLabel(r.project, r.wi, r.gestor, r.scope)}
                  sub={envs.length > 1 ? `${st.text} · ${r.envName}` : st.text}
                  tone={st.tone === "attention" ? "attention" : undefined}
                  selected={!!active && active.env === r.env && active.session === r.s.id}
                  onClick={() => nav.openTerminal({ env: r.env, session: r.s.id })}
                />
              );
            })}
          </SideGroup>
        </SidePanel>
      </SideContent>
      {active ? (
        <SessionPanel tab={active} />
      ) : (
        <main className="web-session">
          <h1 className="web-session-title">Sessões</h1>
          <p className="muted">Escolha uma sessão na Sala de controle ou no painel lateral.</p>
        </main>
      )}
    </>
  );
}

export function SessionPanel({ tab }: { tab: Tab }) {
  const { envs, dashboards, status, api } = useTower();
  const nav = useNav();

  const [error, setError] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);
  const [busy, setBusy] = useState(false);
  const [confirmKill, setConfirmKill] = useState(false);
  const fromHub = findSession(tab, dashboards);
  const envName = envs.find((e) => e.id === tab.env)?.name ?? tab.env;
  const st = status[tab.env];
  const online = useEnvOnline(tab.env);
  const requested = useRcRequest(tab.session);
  // Esperando o Remote Control: o relógio anda por segundo (o prazo de 30 s).
  const now = useNow(requested !== undefined ? 1_000 : 30_000);
  const waitingRc = requested !== undefined && now - requested < RC_CONNECT_MS && !fromHub?.rc_session_id;
  // Fora da visão do hub (acabou de nascer) ou esperando o id do Remote
  // Control: a sessão como o ambiente a vê.
  const fromEnv = useEnvSession(api, tab.env, tab.session, online && (!fromHub || waitingRc), waitingRc);
  const s =
    fromEnv && (!fromHub || fromEnv.last_event_at >= fromHub.last_event_at || fromEnv.rc_session_id)
      ? fromEnv
      : fromHub;
  const rc = rcView(s, requested, now);

  // O id chegou depois do pedido: abre o link uma vez (no celular, o sistema
  // entrega ao app Claude) e mantém o botão, caso o navegador barre a janela.
  const opened = useRef<string | null>(null);
  useEffect(() => {
    if (rc.kind !== "on" || requested === undefined || opened.current === tab.session) return;
    opened.current = tab.session;
    rcDone(tab.session);
    void api.openUrl(rc.url).catch(() => {});
  }, [rc, requested, api, tab.session]);

  if (!s) {
    return (
      <main className="web-session">
        <h1 className="web-session-title">Sessão</h1>
        <p className="muted">
          {online
            ? "Carregando a sessão do ambiente…"
            : "Esta sessão não está mais na visão do hub (encerrou há mais de 24 h ou foi arquivada)."}
        </p>
      </main>
    );
  }

  const { wi, project, gestor, scope } = lookup(dashboards[tab.env], s);
  const title = itemLabel(project, wi, gestor, scope);
  const ended = ENDED[s.state];
  const state = ended
    ? { text: ended, tone: s.state === "interrupted" ? "attention" : "neutral" }
    : tileState(s, wi?.status);
  const waiting = !ended && wi?.status === "aguardando_voce";
  const resumable = s.state === "interrupted" || s.state === "exited";

  const run = (f: () => Promise<void>) => {
    setError(null);
    setBusy(true);
    f()
      .catch((e) => setError(e instanceof Error ? e.message : String(e)))
      .finally(() => setBusy(false));
  };

  const enableRc = () =>
    run(async () => {
      await api.call(tab.env, "session.remote_control.enable", {
        session_id: s.id,
      });
      opened.current = null;
      rcRequested(s.id);
    });
  const kill = () =>
    run(async () => {
      setConfirmKill(false);
      await api.call(tab.env, "session.kill", { session_id: s.id });
    });
  const resume = () =>
    run(async () => {
      // Pelo celular não há outro jeito de conversar com a sessão: sempre com o Remote Control.
      const n = await api.call<Session>(tab.env, "session.resume", {
        session_id: s.id,
        prompt: RESUME_PROMPT,
        remote_control: true,
      });
      rcRequested(n.id);
      nav.openTerminal({ env: tab.env, session: n.id });
    });

  return (
    <main className="web-session" aria-labelledby="web-session-h">
      <div className="web-session-head">
        <h1 id="web-session-h" className="web-session-title">
          {title}
        </h1>
        <span className="badge" data-tone={state.tone}>
          {state.text}
        </span>
      </div>

      <dl className="web-facts" aria-label="Estado da sessão">
        <div>
          <dt>Ambiente</dt>
          <dd>
            {envName}
            {st && !st.connected && <span className="muted"> · {st.error ?? "sem envio ao hub"}</span>}
          </dd>
        </div>
        <div>
          <dt>Modelo e modo</dt>
          <dd>{agentLabel(s)}</dd>
        </div>
        {waiting ? (
          <div>
            <dt>Aguardando você</dt>
            <dd>há {duration(now - s.last_event_at)}</dd>
          </div>
        ) : (
          <div>
            <dt>Última atividade</dt>
            <dd>{ago(s.last_event_at, now)}</dd>
          </div>
        )}
        <div>
          <dt>{ended ? "Duração" : "Rodando há"}</dt>
          <dd>{duration((s.ended_at ?? now) - s.started_at)}</dd>
        </div>
        {wi?.branch && (
          <div>
            <dt>Branch</dt>
            <dd className="mono">{wi.branch}</dd>
          </div>
        )}
      </dl>

      {rc.kind === "on" && (
        <section className="web-actions" aria-label="Remote Control">
          <button type="button" className="btn btn-primary" onClick={() => run(() => api.openUrl(rc.url))}>
            Abrir no Remote Control
          </button>
          <button
            type="button"
            className="btn"
            onClick={() =>
              run(async () => {
                await api.writeClipboard(rc.url);
                setCopied(true);
              })
            }
          >
            {copied ? "Link copiado" : "Copiar link"}
          </button>
          <span className="mono muted small">{rc.url}</span>
        </section>
      )}
      {rc.kind === "connecting" && (
        <p role="status" className="muted">
          Conectando ao Remote Control… o link abre assim que a sessão o receber.
        </p>
      )}
      {rc.kind === "failed" && !ended && (
        <p role="status" className="web-notice">
          O Remote Control não conectou em 30 s. Confira o login do Claude Code (claude.ai) no ambiente, ou tente de
          novo.
        </p>
      )}

      {online ? (
        <section className="web-actions" aria-label="Ações da sessão">
          {!ended && rc.kind !== "on" && rc.kind !== "connecting" && (
            <button type="button" className="btn btn-primary" disabled={busy} onClick={enableRc}>
              Ligar Remote Control
            </button>
          )}
          {resumable && (
            <button type="button" className="btn btn-primary" disabled={busy} onClick={resume}>
              Retomar
            </button>
          )}
          {!ended &&
            (confirmKill ? (
              <>
                <button type="button" className="btn btn-danger" disabled={busy} onClick={kill}>
                  Confirmar encerramento
                </button>
                <button type="button" className="btn" onClick={() => setConfirmKill(false)}>
                  Cancelar
                </button>
              </>
            ) : (
              <button type="button" className="btn" disabled={busy} onClick={() => setConfirmKill(true)}>
                Encerrar
              </button>
            ))}
        </section>
      ) : (
        <p className="muted">
          {ended ? "A sessão não está rodando. " : ""}
          {OFFLINE_ACTIONS}
        </p>
      )}
      {online && rc.kind === "off" && !ended && (
        <p className="muted">
          {waiting
            ? "Para responder daqui, ligue o Remote Control e continue no app Claude."
            : "Remote Control desligado nesta sessão."}
        </p>
      )}
      {error && (
        <p role="alert" className="web-error">
          {error}
        </p>
      )}
    </main>
  );
}
