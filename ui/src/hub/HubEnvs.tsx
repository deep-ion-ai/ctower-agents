import { envTab } from "../env/sections";
import { maintenanceTab } from "../maintenance/sections";
import { useNav } from "../state/nav";
import { useTower } from "../state/tower";
import { ago } from "../util/time";
import { assignmentText } from "../util/hub";
import { humanBytes } from "../util/bytes";
import type { HubAssignment } from "../protocol/hub-gen";

/**
 * Ambientes ligados ao hub (`hub.envs.list`) com as atribuições de cada um
 * e o estado delas, e as limpezas pendentes que cada um informou (#287); o
 * plano e a limpeza de cada um ficam em Manutenção › Limpeza (#337), aonde
 * o número leva quando esta UI conhece o ambiente (#339). A
 * conexão da UI com cada ambiente é outra (Configurações → Ambientes): aqui
 * aparece se esta UI o conhece. O nome leva ao detalhe do ambiente (#23),
 * na seção Hub, quando esta UI o conhece.
 */
export function HubEnvs({ now }: { now: number }) {
  const { hub, envs, status, web } = useTower();
  const nav = useNav();
  const byEnv = new Map<string, HubAssignment[]>();
  for (const p of hub.projects) {
    for (const a of p.assignments) byEnv.set(a.env_id, [...(byEnv.get(a.env_id) ?? []), a]);
  }

  return (
    <section aria-labelledby="hub-ambientes" className="panel settings-section">
      <h2 id="hub-ambientes">Ambientes · {hub.envs.length}</h2>
      {hub.envs.length === 0 ? (
        <p className="muted">
          Nenhum ambiente ligado ao hub. Gere o token de sync com <span className="mono">cthubd add-env &lt;nome&gt;</span>{" "}
          e grave-o no ambiente com <span className="mono">ctower hub set-token</span>.
        </p>
      ) : (
        <div className="table-wrap">
          <table>
            <thead>
              <tr>
                <th>Ambiente</th>
                <th>Último envio</th>
                <th>Pendências</th>
                <th>Limpezas</th>
                {/* Web: a UI não conecta aos ambientes (fase 1). */}
                {!web && <th>Nesta UI</th>}
                <th>Atribuições</th>
              </tr>
            </thead>
            <tbody>
              {hub.envs.map((e) => {
                const ui = hub.envByDaemon[e.env_id];
                const uiEnv = envs.find((x) => x.id === ui);
                const list = byEnv.get(e.env_id) ?? [];
                return (
                  <tr key={e.env_id}>
                    <td>
                      {uiEnv ? (
                        <button
                          type="button"
                          className="link-btn strong"
                          title={`Tudo sobre ${uiEnv.name}`}
                          onClick={() => nav.openTab(envTab(uiEnv.id, uiEnv.name, "hub"))}
                        >
                          {e.name}
                        </button>
                      ) : (
                        <span className="strong">{e.name}</span>
                      )}
                      <br />
                      <span className="mono muted small">{e.env_id}</span>
                    </td>
                    <td>{e.last_seen_at ? ago(e.last_seen_at, now) : "nunca"}</td>
                    <td>{e.pending}</td>
                    <td>
                      {!e.cleanup ? (
                        <span className="muted">não informado</span>
                      ) : e.cleanup.items > 0 && uiEnv && !web ? (
                        <button
                          type="button"
                          className="link-btn strong"
                          title={`verificado ${ago(e.cleanup.checked_at, now)}`}
                          onClick={() => nav.openTab(maintenanceTab("cleanup", uiEnv.id))}
                        >
                          {e.cleanup.items} pendente(s) · {humanBytes(e.cleanup.bytes)}
                        </button>
                      ) : (
                        <span
                          className={e.cleanup.items > 0 ? "strong" : "muted"}
                          title={`verificado ${ago(e.cleanup.checked_at, now)}`}
                        >
                          {e.cleanup.items > 0
                            ? `${e.cleanup.items} pendente(s) · ${humanBytes(e.cleanup.bytes)}`
                            : "nenhuma pendente"}
                        </span>
                      )}
                    </td>
                    {!web && (
                      <td>
                        {uiEnv ? (
                          <span className={status[uiEnv.id]?.connected ? "state-on" : "state-off"}>
                            {uiEnv.name} · {status[uiEnv.id]?.connected ? "conectado" : "desconectado"}
                          </span>
                        ) : (
                          <span className="muted">não configurado</span>
                        )}
                      </td>
                    )}
                    <td>
                      {list.length === 0 ? (
                        <span className="muted">nenhuma</span>
                      ) : (
                        <ul className="hub-list">
                          {list.map((a) => (
                            <li key={a.project_id}>
                              <span className="mono">
                                {a.gh_owner}/{a.gh_repo}
                              </span>
                              <span className="hub-state" data-state={a.state}>
                                {assignmentText(a)}
                              </span>
                            </li>
                          ))}
                        </ul>
                      )}
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      )}
    </section>
  );
}
