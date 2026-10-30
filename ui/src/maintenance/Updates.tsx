// Atualizações na Manutenção (#336): o app, o daemon de cada ambiente e o
// hub (#421).
import { useEffect, useState } from "react";
import type { EnvView, UiUpdateProgress } from "../api/types";
import type { Available, Phase, UpdateState, UpdateStatus } from "../protocol/gen";
import type { HubUpdateStatus } from "../protocol/hub-gen";
import { errText } from "../hub/calls";
import { useTower } from "../state/tower";
import { useNow } from "../App";
import { ago } from "../util/time";
import { versionNewer } from "../util/version";

const PHASE: Record<Phase, string> = {
  starting: "iniciando",
  fetch: "buscando o código",
  build: "compilando",
  backup: "guardando a versão atual",
  install: "instalando",
  restart: "reiniciando o daemon",
  verify: "conferindo a versão nova",
  done: "concluída",
  failed: "falhou",
  rolled_back: "falhou e voltou à versão anterior",
};

const terminal = (p: Phase) => p === "done" || p === "failed" || p === "rolled_back";

const short = (commit: string) => commit.slice(0, 7);

function build(b: { version: string; commit: string }): string {
  return b.commit ? `${b.version} · ${short(b.commit)}` : b.version;
}

function availableText(u: UpdateStatus | HubUpdateStatus): string {
  const a = u.available;
  if (!a) return u.check_error ? "verificação falhou" : "ainda não verificado";
  switch (a.relation) {
    case "same":
      return `em dia (${u.ref})`;
    case "ahead":
      return `à frente de ${u.ref}`;
    case "behind":
      return `${build(a.build)} · ${a.behind ?? a.commits.length} commit(s) novo(s)`;
    case "diverged":
      return `${build(a.build)} · outro histórico (${u.ref})`;
    case "unknown":
      return `${build(a.build)} · commit instalado desconhecido`;
  }
}

/** Atualização disparada por esta tela e ainda acompanhada. */
interface Track {
  /** `started_at` do estado anterior ao disparo: estados mais novos são desta atualização. */
  prevStarted: number;
  /** Commit esperado depois do restart. */
  target: string | null;
  error?: string;
}

type Row = { text: string; tone: "on" | "off" | "busy" | "muted"; log?: string[] };

/** O que se sabe do serviço: o build no ar e o `state.json` do atualizador. */
interface Live {
  version: string;
  commit: string;
  state: UpdateState | null;
}

/** Restart do serviço (`restart`): o texto diz qual. */
function phaseText(p: Phase, service: string): string {
  return p === "restart" ? `reiniciando o ${service}` : PHASE[p];
}

function rowState(
  live: Live | undefined,
  connected: boolean,
  track: Track | undefined,
  now: number,
  service = "daemon",
): Row | null {
  const s = live?.state ?? null;
  const info = live;
  if (track) {
    if (track.error) return { text: track.error, tone: "off" };
    const mine = s && s.started_at > track.prevStarted ? s : null;
    if (mine && (mine.phase === "failed" || mine.phase === "rolled_back")) {
      return { text: `${PHASE[mine.phase]}: ${mine.error ?? ""}`, tone: "off", log: mine.log_tail };
    }
    if (mine?.kind === "rollback" && mine.phase === "done") {
      return { text: `voltou para ${build(mine.to ?? { version: info?.version ?? "", commit: "" })}`, tone: "on" };
    }
    if (!connected) return { text: "reconectando…", tone: "busy" };
    if (track.target && info?.commit === track.target) {
      return { text: `atualizado para ${info.version} · ${short(track.target)}`, tone: "on" };
    }
    if (mine?.phase === "done") return { text: mine.message ?? "concluída", tone: "on" };
    return { text: mine ? `${phaseText(mine.phase, service)}…` : "iniciando…", tone: "busy" };
  }
  if (!s) return null;
  if (!terminal(s.phase)) return { text: `em andamento: ${phaseText(s.phase, service)}…`, tone: "busy" };
  const when = s.finished_at ? ` ${ago(s.finished_at, now)}` : "";
  const what = s.kind === "rollback" ? "rollback" : "atualização";
  return {
    text: `última ${what}: ${PHASE[s.phase]}${when}`,
    tone: s.phase === "done" ? "muted" : "off",
    log: s.phase === "done" ? undefined : s.log_tail,
  };
}

/**
 * Manutenção › Atualizações › Ambientes (#336, antes nas Configurações, #90):
 * todos os ambientes numa tabela, com versão instalada e disponível, sessões
 * vivas e o disparo do `env.update`. O daemon reinicia; a conexão cai e volta
 * sozinha, e a linha confirma o commit novo. Canal e intervalo ficam nas
 * Configurações (`onSettings`).
 */
export function EnvUpdates({ onSettings, onHub }: { onSettings: () => void; onHub?: () => void }) {
  const { envs, status, infos, dashboards, api, refresh, hub } = useTower();
  const now = useNow();
  const [track, setTrack] = useState<Record<string, Track>>({});
  const [checking, setChecking] = useState(false);
  const [confirm, setConfirm] = useState<string[] | null>(null);

  const live = (env: string) => dashboards[env]?.live_sessions.length ?? 0;
  const liveOf = (env: string): Live | undefined => {
    const i = infos[env];
    return i && { version: i.version, commit: i.commit ?? "", state: i.update?.state ?? null };
  };
  const running = (env: string) => {
    const r = rowState(liveOf(env), !!status[env]?.connected, track[env], now);
    return r?.tone === "busy";
  };
  const canUpdate = (e: EnvView) =>
    !!status[e.id]?.connected && !!infos[e.id]?.update?.available?.update_available && !running(e.id);
  // O hub (cthubd) não é um ambiente: atualizar os ambientes na ordem da lista.
  const updatable = envs.filter(canUpdate);
  const connected = envs.filter((e) => status[e.id]?.connected);

  // Verifica todos os conectados de uma vez; o erro de cada um volta em `update.check_error`.
  const check = async () => {
    setChecking(true);
    await Promise.all(
      connected.map((e) =>
        api
          .call<UpdateStatus>(e.id, "env.check_update")
          .then(() => refresh(e.id))
          .catch(() => {}),
      ),
    );
    setChecking(false);
  };

  const start = async (ids: string[]) => {
    setConfirm(null);
    for (const id of ids) {
      const info = infos[id];
      const prev: Track = {
        prevStarted: info?.update?.state?.started_at ?? 0,
        target: info?.update?.available?.build.commit ?? null,
      };
      setTrack((all) => ({ ...all, [id]: prev }));
      try {
        await api.call(id, "env.update", { force: live(id) > 0 });
      } catch (err) {
        setTrack((all) => ({ ...all, [id]: { ...prev, error: String(err) } }));
      }
    }
  };

  const name = (id: string) => envs.find((e) => e.id === id)?.name ?? id;
  const confirmLive = confirm?.reduce((n, id) => n + live(id), 0) ?? 0;
  // O hub vai antes dos ambientes (#421): um ambiente novo pode exigir um
  // protocolo que o hub ainda não tem.
  const hubNext = hub.update?.available?.update_available ? hub.update.available : null;

  return (
    <section className="panel settings-section set-stack" aria-labelledby="maint-env-updates">
      <div className="settings-head">
        <h2 id="maint-env-updates">Daemons · {envs.length}</h2>
        <div className="env-actions">
          <button type="button" className="btn" disabled={checking || connected.length === 0} onClick={check}>
            {checking ? "Verificando…" : "Verificar agora"}
          </button>
          {envs.length > 1 && (
            <button
              type="button"
              className="btn"
              disabled={updatable.length === 0}
              onClick={() => setConfirm(updatable.map((e) => e.id))}
            >
              Atualizar todos
            </button>
          )}
        </div>
      </div>
      <div className="table-wrap update-table">
        <table aria-label="Atualização dos ambientes">
          <thead>
            <tr>
              <th>Ambiente</th>
              <th>Instalado</th>
              <th>Disponível</th>
              <th>Sessões vivas</th>
              <th>
                <span className="sr-only">Ações</span>
              </th>
            </tr>
          </thead>
          {envs.map((e) => {
            const info = infos[e.id];
            const u = info?.update;
            const on = !!status[e.id]?.connected;
            const row = rowState(liveOf(e.id), on, track[e.id], now);
            const a: Available | null | undefined = u?.available;
            const commits = a && a.commits.length > 0;
            const log = row?.log && row.log.length > 0;
            const checkError = u?.check_error && on;
            // Uma linha por ambiente; o estado, os commits e o log vão na linha de baixo.
            return (
              <tbody key={e.id} aria-label={`Atualização de ${e.name}`}>
                <tr>
                  <td className="strong">{e.name}</td>
                  <td className="mono">{info ? build({ version: info.version, commit: info.commit ?? "" }) : "—"}</td>
                  <td>
                    {!on ? (
                      <span className="muted">sem conexão</span>
                    ) : !u ? (
                      <span className="muted">daemon sem suporte a atualização</span>
                    ) : (
                      <span className={a?.update_available ? "strong" : "muted"} title={u.check_error ?? undefined}>
                        {availableText(u)}
                      </span>
                    )}
                  </td>
                  <td>{on ? live(e.id) : "—"}</td>
                  <td>
                    <div className="row-actions">
                      <button type="button" className="btn" disabled={!canUpdate(e)} onClick={() => setConfirm([e.id])}>
                        Atualizar <span className="sr-only">{e.name}</span>
                      </button>
                    </div>
                  </td>
                </tr>
                {(row || checkError || commits || log) && (
                  <tr className="update-detail">
                    <td colSpan={5}>
                      {row && (
                        <div className={`update-state state-${row.tone}`} role="status">
                          {row.text}
                        </div>
                      )}
                      {checkError && <div className="update-state muted">erro na verificação: {u?.check_error}</div>}
                      {commits && (
                        <details>
                          <summary>Commits novos em {u?.ref}</summary>
                          <ul className="mono commit-list">
                            {a.commits.map((c) => (
                              <li key={c}>{c}</li>
                            ))}
                          </ul>
                        </details>
                      )}
                      {log && (
                        <details>
                          <summary>Fim do log</summary>
                          <pre className="update-log">{row!.log!.join("\n")}</pre>
                        </details>
                      )}
                    </td>
                  </tr>
                )}
              </tbody>
            );
          })}
        </table>
      </div>
      {confirm && (
        <div role="group" aria-label="Confirmar atualização" className="confirm-box">
          <p>
            Atualizar {confirm.map(name).join(", ")}? O daemon compila a versão nova, reinicia e a conexão cai por alguns
            segundos. Se a versão nova não subir, a anterior volta sozinha.
          </p>
          {hubNext && (
            <p className="state-off" role="alert">
              O hub também tem atualização ({build(hubNext.build)}). Atualize o hub antes dos ambientes: a versão nova
              do ambiente pode exigir um protocolo que o hub ainda não tem.
              {onHub && (
                <>
                  {" "}
                  <button type="button" className="link-btn" onClick={onHub}>
                    Atualizar o hub primeiro ›
                  </button>
                </>
              )}
            </p>
          )}
          {confirmLive > 0 && (
            <p>
              <strong>{confirmLive} sessão(ões) viva(s)</strong> serão encerradas pelo restart e voltarão como
              interrompidas na Recuperação.
            </p>
          )}
          <div className="form-actions">
            <button type="button" className="btn" onClick={() => setConfirm(null)}>
              Cancelar
            </button>
            <button
              type="button"
              className={confirmLive > 0 ? "btn btn-danger" : "btn btn-primary"}
              onClick={() => start(confirm)}
            >
              {confirmLive > 0 ? "Atualizar e encerrar sessões" : "Atualizar"}
            </button>
          </div>
        </div>
      )}
      <p className="muted hint">
        Cada ambiente verifica sozinho no intervalo dos ajustes (padrão 6 h). Canal e Reverter (volta à versão
        anterior) ficam nos ajustes; no terminal do ambiente também há <span className="mono">ctower update --rollback</span>.
      </p>
      <div>
        <button type="button" className="link-btn" onClick={onSettings}>
          Ajustes de atualização ›
        </button>
      </div>
    </section>
  );
}

/**
 * Resumo da atualização do hub numa linha (dados do hub no painel do Hub,
 * também na web): em andamento, disponível ou em dia.
 */
export function hubUpdateText(u: HubUpdateStatus): string {
  const s = u.state;
  if (s && !terminal(s.phase)) return `em andamento: ${phaseText(s.phase, "hub")}…`;
  if (u.available?.update_available) return `${build(u.available.build)} disponível em ${u.ref}`;
  return availableText(u);
}

/** Recusa do `hub.update.rollback` sem `force`: o `hub.db` voltaria do snapshot. */
const needsDbForce = (msg: string) => msg.includes("Confirme com force");

/**
 * Manutenção › Atualizações › Hub (#421): o próprio cthubd. Versão,
 * protocolo e commit no ar, a disponível no `update_ref`, e o disparo do
 * `hub.update.start` (ou `hub.update.rollback`). O atualizador roda fora do
 * serviço; durante o restart a conexão com o hub cai e volta sozinha, e o
 * resultado vem do `state.json` (`hub.update.get` ao reconectar e o push
 * `hub.update.status`). Na web, só leitura.
 */
export function HubUpdate({ onHubSettings }: { onHubSettings?: () => void }) {
  const { api, hub, web, envs, infos, status } = useTower();
  const now = useNow();
  const u = hub.update;
  const info = hub.info;
  const [checking, setChecking] = useState(false);
  const [track, setTrack] = useState<Track | undefined>(undefined);
  const [confirm, setConfirm] = useState<"update" | "rollback" | null>(null);
  // Rollback que troca o hub.db: a explicação do hub, para confirmar de novo.
  const [dbLoss, setDbLoss] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const live: Live | undefined = u
    ? { version: u.installed.version, commit: u.installed.commit, state: u.state }
    : undefined;
  const row = rowState(live, hub.connected, track, now, "hub");
  const busy = row?.tone === "busy";
  const a = u?.available ?? null;
  const canUpdate = !web && hub.connected && !!a?.update_available && !busy;
  const canRollback = !web && hub.connected && !!u?.previous && !busy;
  // Ambientes (conectados) com versão à frente do hub: o hub vai primeiro.
  const ahead = info
    ? envs.filter((e) => status[e.id]?.connected && infos[e.id] && versionNewer(infos[e.id].version, info.version))
    : [];

  const check = async () => {
    setChecking(true);
    setError(null);
    try {
      await api.hubCall<HubUpdateStatus>("hub.update.check");
    } catch (e) {
      setError(errText(e));
    } finally {
      setChecking(false);
    }
  };

  const start = async (kind: "update" | "rollback", force = false) => {
    setConfirm(null);
    setDbLoss(null);
    setError(null);
    const next: Track = {
      prevStarted: u?.state?.started_at ?? 0,
      target: (kind === "rollback" ? u?.previous?.commit : a?.build.commit) || null,
    };
    try {
      if (kind === "rollback") await api.hubCall("hub.update.rollback", { force });
      else await api.hubCall("hub.update.start");
      setTrack(next);
    } catch (e) {
      const msg = errText(e);
      if (kind === "rollback" && !force && needsDbForce(msg)) setDbLoss(msg);
      else setError(msg);
    }
  };

  let body;
  if (!hub.conn) {
    body = (
      <p className="muted">
        Sem hub configurado.{" "}
        {onHubSettings && (
          <button type="button" className="link-btn" onClick={onHubSettings}>
            Configurar o hub ›
          </button>
        )}
      </p>
    );
  } else if (!hub.connected && !track) {
    body = <p className="muted">Hub indisponível: {hub.status?.error ?? "reconectando…"}</p>;
  } else {
    body = (
      <>
        <div className="env-row update-row" role="group" aria-label="Atualização do hub">
          <div className="fact">
            <span className="label">Hub</span>
            <span className="strong">{info?.name ?? "hub"}</span>
          </div>
          <div className="fact">
            <span className="label">No ar</span>
            <span className="mono">
              {info ? `${build({ version: info.version, commit: info.commit })} · protocolo ${info.protocol}` : "—"}
            </span>
          </div>
          <div className="fact">
            <span className="label">Disponível</span>
            {!hub.connected ? (
              <span className="muted">sem conexão</span>
            ) : !u ? (
              <span className="muted">hub sem atualização pela UI</span>
            ) : (
              <span className={a?.update_available ? "strong" : "muted"} title={u.check_error ?? undefined}>
                {availableText(u)}
              </span>
            )}
          </div>
          <div className="fact" />
          {!web && u && (
            <div className="env-actions">
              <button type="button" className="btn" disabled={checking || !hub.connected} onClick={check}>
                {checking ? "Verificando…" : "Verificar agora"}
              </button>
              <button type="button" className="btn" disabled={!canRollback} onClick={() => setConfirm("rollback")}>
                Voltar à versão anterior
              </button>
              <button
                type="button"
                className="btn btn-primary"
                disabled={!canUpdate}
                onClick={() => setConfirm("update")}
              >
                Atualizar o hub
              </button>
            </div>
          )}
          {row && (
            <div className={`update-state wide state-${row.tone}`} role="status">
              {row.text}
            </div>
          )}
          {error && (
            <div className="update-state wide state-off" role="alert">
              {error}
            </div>
          )}
          {u?.check_error && hub.connected && (
            <div className="update-state wide muted">erro na verificação: {u.check_error}</div>
          )}
          {a && a.commits.length > 0 && (
            <details className="wide">
              <summary>Commits novos em {u?.ref}</summary>
              <ul className="mono commit-list">
                {a.commits.map((c) => (
                  <li key={c}>{c}</li>
                ))}
              </ul>
            </details>
          )}
          {row?.log && row.log.length > 0 && (
            <details className="wide">
              <summary>Fim do log</summary>
              <pre className="update-log">{row.log.join("\n")}</pre>
            </details>
          )}
        </div>
        {hub.connected && !u && info && (
          <p className="muted hint">
            Este hub (cthubd {info.version}, protocolo {info.protocol}) ainda não atualiza pela UI: atualize-o uma vez
            no terminal da máquina do hub com <span className="mono">cthubd update</span>.
          </p>
        )}
        {ahead.length > 0 && (
          <p className="update-state state-off" role="alert">
            {ahead.map((e) => `${e.name} (${infos[e.id].version})`).join(", ")}{" "}
            {ahead.length === 1 ? "está" : "estão"} à frente do hub ({info?.version}): atualize o hub primeiro.
          </p>
        )}
      </>
    );
  }

  return (
    <section className="panel settings-section set-stack" aria-labelledby="maint-hub-update">
      <div className="settings-head">
        <h2 id="maint-hub-update">Hub (cthubd)</h2>
      </div>
      {body}
      {confirm === "update" && a && (
        <div role="group" aria-label="Confirmar atualização do hub" className="confirm-box">
          <p>
            Atualizar o hub para {build(a.build)}? O hub compila a versão nova fora do serviço (falha no build não toca
            no hub em uso), guarda a atual e um snapshot do hub.db e reinicia: a conexão com o hub cai por alguns
            segundos. Se a versão nova não ficar saudável em 60 s, a anterior volta sozinha. As sessões seguem nos
            ambientes.
          </p>
          <div className="form-actions">
            <button type="button" className="btn" onClick={() => setConfirm(null)}>
              Cancelar
            </button>
            <button type="button" className="btn btn-primary" onClick={() => start("update")}>
              Atualizar o hub
            </button>
          </div>
        </div>
      )}
      {confirm === "rollback" && u?.previous && (
        <div role="group" aria-label="Confirmar rollback do hub" className="confirm-box">
          <p>
            Voltar o hub para {build(u.previous)}, a versão guardada pela última atualização? O hub reinicia e a
            conexão cai por alguns segundos.
          </p>
          <div className="form-actions">
            <button type="button" className="btn" onClick={() => setConfirm(null)}>
              Cancelar
            </button>
            <button type="button" className="btn btn-primary" onClick={() => start("rollback")}>
              Voltar à versão anterior
            </button>
          </div>
        </div>
      )}
      {dbLoss && (
        <div role="group" aria-label="Confirmar a troca do hub.db" className="confirm-box">
          <p>
            <strong>O rollback troca o banco do hub.</strong> {dbLoss.replace(/\. Confirme com force.*$/, ".")}
          </p>
          <p>
            Depois, em cada ambiente, rode <span className="mono">ctower hub resync</span> e{" "}
            <span className="mono">ctower hub sync</span> para reenviar o que ainda não foi podado.
          </p>
          <div className="form-actions">
            <button type="button" className="btn" onClick={() => setDbLoss(null)}>
              Cancelar
            </button>
            <button type="button" className="btn btn-danger" onClick={() => start("rollback", true)}>
              Voltar e trocar o banco
            </button>
          </div>
        </div>
      )}
      <p className="muted hint">
        {web
          ? "Na web, só o estado: atualize o hub pelo app desktop."
          : "Canal: update_ref no config.toml do hub (padrão main); o hub verifica sozinho a cada update_check_interval_hours (6 h). No terminal da máquina do hub também há cthubd update e cthubd update --rollback."}
      </p>
    </section>
  );
}

/**
 * Manutenção › Atualizações › Este app (#336, antes nas Configurações): o
 * próprio app (tauri-plugin-updater): versão, verificar, notas e instalar e
 * reiniciar.
 */
export function AppUpdate() {
  const { api, uiUpdate, checkUiUpdate } = useTower();
  const [checking, setChecking] = useState(false);
  const [confirm, setConfirm] = useState(false);
  const [installing, setInstalling] = useState(false);
  const [progress, setProgress] = useState<UiUpdateProgress | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => api.onUiUpdateProgress(setProgress), [api]);
  const next = uiUpdate?.available ?? null;

  const install = async () => {
    setConfirm(false);
    setInstalling(true);
    setError(null);
    try {
      // Em sucesso o app reinicia e esta promessa não volta.
      await api.uiUpdateInstall();
    } catch (e) {
      setError(String(e));
    } finally {
      setInstalling(false);
    }
  };

  const pct = progress?.total ? Math.floor((100 * progress.downloaded) / progress.total) : null;
  return (
    <div className="env-row update-row" role="group" aria-label="Atualização do app">
      <div className="fact">
        <span className="label">App</span>
        <span className="strong">Control Tower (este app)</span>
      </div>
      <div className="fact">
        <span className="label">Instalado</span>
        <span className="mono">{uiUpdate?.current ?? "—"}</span>
      </div>
      <div className="fact">
        <span className="label">Disponível</span>
        {!uiUpdate ? (
          <span className="muted">verificando…</span>
        ) : next ? (
          <span className="strong">{next.version}</span>
        ) : uiUpdate.error ? (
          <span className="muted" title={uiUpdate.error}>
            verificação falhou
          </span>
        ) : (
          <span className="muted">em dia</span>
        )}
      </div>
      <div className="fact" />
      <div className="env-actions">
        <button
          type="button"
          className="btn"
          disabled={checking}
          onClick={async () => {
            setChecking(true);
            try {
              await checkUiUpdate();
            } finally {
              setChecking(false);
            }
          }}
        >
          {checking ? "Verificando…" : "Verificar agora"}
        </button>
        <button type="button" className="btn btn-primary" disabled={!next || installing} onClick={() => setConfirm(true)}>
          Atualizar o app
        </button>
      </div>
      {installing && (
        <div className="update-state wide state-busy" role="status">
          {pct === null ? "baixando a versão nova…" : `baixando a versão nova… ${pct}%`}
        </div>
      )}
      {error && (
        <div className="update-state wide state-off" role="status">
          {error}
        </div>
      )}
      {uiUpdate?.error && <div className="update-state wide muted">erro na verificação: {uiUpdate.error}</div>}
      {next?.notes && (
        <details className="wide">
          <summary>Notas da versão {next.version}</summary>
          <pre className="update-log">{next.notes}</pre>
        </details>
      )}
      {confirm && next && (
        <div role="group" aria-label="Confirmar atualização do app" className="confirm-box wide">
          <p>
            Instalar o Control Tower {next.version} e reiniciar o app? As sessões continuam rodando nos ambientes; só a
            janela fecha e abre de novo.
          </p>
          <div className="form-actions">
            <button type="button" className="btn" onClick={() => setConfirm(false)}>
              Cancelar
            </button>
            <button type="button" className="btn btn-primary" onClick={install}>
              Atualizar e reiniciar
            </button>
          </div>
        </div>
      )}
    </div>
  );
}
