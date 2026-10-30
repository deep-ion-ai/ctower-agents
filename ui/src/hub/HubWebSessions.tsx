// Hub › Sessões web (épico #205, spec §3.1 e §4.2): os navegadores com
// sessão web aberta (`hub.web.sessions.list`) e a revogação, de uma ou de
// todas (`hub.web.sessions.revoke`). Revogar só reduz acesso: a seção existe
// no app desktop e na web. Na web, revogar a própria sessão faz o hub fechar
// o `/web/ws` (4401) e a PWA volta para a tela de login.
import { useCallback, useEffect, useState } from "react";
import type { HubWebSession, WebSessionsRevoked } from "../protocol/hub-gen";
import { useTower } from "../state/tower";
import { ago } from "../util/time";
import { errText } from "./calls";

/** "Safari · iPhone" a partir do User-Agent (o bastante para reconhecer o aparelho). */
export function browserLabel(ua: string | null): string {
  if (!ua) return "navegador desconhecido";
  const device = /iPhone/.test(ua)
    ? "iPhone"
    : /iPad/.test(ua)
      ? "iPad"
      : /Android/.test(ua)
        ? "Android"
        : /Mac OS X|Macintosh/.test(ua)
          ? "macOS"
          : /Windows/.test(ua)
            ? "Windows"
            : /Linux/.test(ua)
              ? "Linux"
              : null;
  const browser = /Edg\//.test(ua)
    ? "Edge"
    : /Firefox\//.test(ua)
      ? "Firefox"
      : /Chrome\//.test(ua) || /CriOS\//.test(ua)
        ? "Chrome"
        : /Safari\//.test(ua)
          ? "Safari"
          : null;
  return [browser, device].filter(Boolean).join(" · ") || ua.slice(0, 60);
}

export function HubWebSessions({ now }: { now: number }) {
  const { api, web } = useTower();
  const [list, setList] = useState<HubWebSession[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [done, setDone] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [confirmAll, setConfirmAll] = useState(false);

  const load = useCallback(async () => {
    try {
      setList(await api.hubCall<HubWebSession[]>("hub.web.sessions.list"));
      setError(null);
    } catch (e) {
      setError(errText(e));
    }
  }, [api]);

  useEffect(() => {
    void load();
  }, [load]);

  const revoke = async (params: { id: string } | { all: true }) => {
    setBusy(true);
    setError(null);
    setDone(null);
    try {
      const r = await api.hubCall<WebSessionsRevoked>("hub.web.sessions.revoke", params);
      setDone(r.revoked === 0 ? "Nada a revogar: a sessão já tinha acabado." : `${r.revoked} sessão(ões) revogada(s).`);
      setConfirmAll(false);
      await load();
    } catch (e) {
      setError(errText(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <section aria-labelledby="hub-web-sessoes" className="panel settings-section">
      <h2 id="hub-web-sessoes">Sessões web · {list?.length ?? "…"}</h2>
      <p className="muted hint">
        Navegadores que entraram pelo acesso web (PWA) com o token de UI do hub. Cada sessão vale 30 dias, ou 7 sem uso.
        Trocar o token de UI não derruba as sessões: revogue aqui ou com{" "}
        <span className="mono">cthubd web revoke-all</span>.
      </p>
      {!list ? (
        !error && <p className="muted">Carregando…</p>
      ) : list.length === 0 ? (
        <p className="muted">Nenhuma sessão web aberta.</p>
      ) : (
        <div className="table-wrap">
          <table>
            <thead>
              <tr>
                <th>Navegador</th>
                <th>Identidade</th>
                <th>Entrou</th>
                <th>Último uso</th>
                <th>Vence</th>
                <th>
                  <span className="sr-only">Ações</span>
                </th>
              </tr>
            </thead>
            <tbody>
              {list.map((s) => (
                <tr key={s.id}>
                  <td>
                    <span className="strong" title={s.user_agent ?? undefined}>
                      {browserLabel(s.user_agent)}
                    </span>
                    {s.current && (
                      <span className="badge" data-tone="running">
                        {" "}
                        este navegador
                      </span>
                    )}
                  </td>
                  <td>{s.identity ?? <span className="muted">sem proxy</span>}</td>
                  <td>{ago(s.created_at, now)}</td>
                  <td>{ago(s.last_used_at, now)}</td>
                  <td>{new Date(s.expires_at).toLocaleDateString("pt-BR")}</td>
                  <td className="hub-card-action">
                    <button
                      type="button"
                      className="btn btn-sm"
                      disabled={busy}
                      onClick={() => void revoke({ id: s.id })}
                    >
                      {s.current ? "Revogar e sair" : "Revogar"}
                      <span className="sr-only"> sessão {browserLabel(s.user_agent)}</span>
                    </button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
      {list && list.length > 0 && (
        <div className="form-actions">
          {confirmAll ? (
            <div role="group" aria-label="Confirmar revogação de todas" className="confirm-box">
              <p>
                Revogar todas as sessões web? Todo navegador conectado volta para a tela de login
                {web ? ", inclusive este" : ""}.
              </p>
              <div className="form-actions">
                <button type="button" className="btn" onClick={() => setConfirmAll(false)}>
                  Cancelar
                </button>
                <button
                  type="button"
                  className="btn btn-danger"
                  disabled={busy}
                  onClick={() => void revoke({ all: true })}
                >
                  Revogar todas
                </button>
              </div>
            </div>
          ) : (
            <button type="button" className="btn btn-danger" onClick={() => setConfirmAll(true)}>
              Revogar todas…
            </button>
          )}
        </div>
      )}
      {error && (
        <p role="alert" className="start-error">
          {error}
        </p>
      )}
      {done && (
        <p role="status" className="state-on">
          {done}
        </p>
      )}
    </section>
  );
}
