import { useState } from "react";
import { useTower } from "../../state/tower";
import { assignmentText, repoPathText } from "../../util/hub";
import type { AssignmentAdded, AssignmentRemoved, HubAssignment, HubProjectView, HubSession } from "../../protocol/hub-gen";
import { needsConfirm } from "../../hub/calls";
import { isJira } from "../../hub/tracker";
import { keyOf, plural } from "./form";
import type { SettingsRun } from "./run";

/**
 * Atribuições do projeto a ambientes: estado, clone, sessões vivas (`live`),
 * remover (com a confirmação que o hub pedir) e atribuir.
 */
export function AssignmentsCard({
  project: p,
  settings,
  live,
  readOnly = false,
}: {
  project: HubProjectView;
  settings: SettingsRun;
  live: HubSession[];
  /** Web (épico #205): sem atribuir nem remover. */
  readOnly?: boolean;
}) {
  const { hub, api } = useTower();
  const pr = p.project;
  const jira = isJira(pr);
  const assigned = new Set(p.assignments.map((a) => a.env_id));
  const free = hub.envs.filter((e) => !assigned.has(e.env_id));
  const [assign, setAssign] = useState<{ env: string; repo_path: string } | null>(null);
  /** Remoção que o hub pediu para confirmar (`hub` = a mensagem dele). */
  const [pending, setPending] = useState<{ a: HubAssignment; hub: string } | null>(null);

  const addAssignment = async () => {
    if (!assign) return;
    const key = keyOf(pr.gh_owner, pr.gh_repo);
    const ok = await settings.run(async () => {
      const r = await api.hubCall<AssignmentAdded>("hub.assignments.add", {
        owner: pr.gh_owner,
        repo: pr.gh_repo,
        env: assign.env,
        ...(assign.repo_path.trim() ? { repo_path: assign.repo_path.trim() } : {}),
      });
      const sync = r.sync_env ? ` ${r.assignment.env_name} é o ambiente de sync do projeto.` : "";
      return r.changed
        ? `${key} atribuído a ${r.assignment.env_name}; o ambiente aplica no próximo pull.${sync}`
        : `${key} já estava atribuído a ${r.assignment.env_name} assim.`;
    });
    if (ok) setAssign(null);
  };

  const unassign = (a: HubAssignment, confirm: boolean) =>
    settings.run(
      async () => {
        const r = await api.hubCall<AssignmentRemoved>("hub.assignments.remove", {
          owner: a.gh_owner,
          repo: a.gh_repo,
          env: a.env_id,
          ...(confirm ? { confirm: true } : {}),
        });
        setPending(null);
        const n = r.live_sessions.length;
        return (
          `Atribuição de ${a.gh_owner}/${a.gh_repo} a ${a.env_name} removida. O clone e os worktrees ficam no ambiente` +
          (n > 0 ? `; ${n} sessão(ões) viva(s) seguem até terminarem.` : ".")
        );
      },
      (m) => {
        if (confirm || !needsConfirm(m)) return false;
        setPending({ a, hub: m });
        return true;
      },
    );

  return (
    <section className="hub-card" aria-labelledby={`pc-atr-${pr.id}`}>
      <div className="hub-card-head">
        <h3 id={`pc-atr-${pr.id}`}>Atribuições · {p.assignments.length}</h3>
        {!readOnly && free.length > 0 && !assign && (
          <button type="button" className="btn btn-sm" onClick={() => setAssign({ env: free[0].env_id, repo_path: "" })}>
            Atribuir a ambiente <span className="sr-only">({pr.name})</span>
          </button>
        )}
      </div>
      {assign && (
        <form
          className="env-form hub-card-form"
          aria-label={`Atribuir ${pr.name}`}
          onSubmit={(e) => {
            e.preventDefault();
            void addAssignment();
          }}
        >
          <div className="field">
            <label htmlFor={`as-env-${pr.id}`}>Ambiente</label>
            <select id={`as-env-${pr.id}`} value={assign.env} onChange={(e) => setAssign({ ...assign, env: e.target.value })}>
              {free.map((e) => (
                <option key={e.env_id} value={e.env_id}>
                  {e.name}
                </option>
              ))}
            </select>
          </div>
          <div className="field">
            <label htmlFor={`as-path-${pr.id}`}>Caminho do clone (opcional)</label>
            <input
              id={`as-path-${pr.id}`}
              type="text"
              placeholder={`padrão: <repos_root>/${pr.gh_repo}`}
              value={assign.repo_path}
              onChange={(e) => setAssign({ ...assign, repo_path: e.target.value })}
            />
          </div>
          <div className="form-actions">
            <button type="button" className="btn" onClick={() => setAssign(null)}>
              Cancelar
            </button>
            <button type="submit" className="btn btn-primary" disabled={settings.busy || !assign.env}>
              Atribuir
            </button>
          </div>
        </form>
      )}
      {p.assignments.length === 0 ? (
        <p className="muted hub-card-empty">Nenhum ambiente executa este projeto.</p>
      ) : (
        <div className="table-wrap">
          <table className="hub-card-table" aria-label={`Atribuições de ${pr.name}`}>
            <thead>
              <tr>
                <th scope="col">Ambiente</th>
                <th scope="col">Estado</th>
                <th scope="col">Clone</th>
                <th scope="col">Sessões</th>
                <th scope="col">
                  <span className="sr-only">Ações</span>
                </th>
              </tr>
            </thead>
            <tbody>
              {p.assignments.flatMap((a) => {
                const n = live.filter((s) => s.env_id === a.env_id).length;
                const confirming = pending?.a.env_id === a.env_id;
                const row = (
                  <tr key={a.env_id}>
                    <td className="strong">
                      {a.env_name}
                      {jira && a.env_id === pr.sync_env_id && (
                        <>
                          {" "}
                          <span className="badge" data-tone="done">
                            sync
                          </span>
                        </>
                      )}
                    </td>
                    <td>
                      <span className="hub-state" data-state={a.state}>
                        {assignmentText(a)}
                      </span>
                    </td>
                    <td className="mono small">{a.reported_path ?? repoPathText(a)}</td>
                    <td className="nowrap">{n === 0 ? "nenhuma" : plural(n, "ativa", "ativas")}</td>
                    <td className="hub-card-action">
                      {!readOnly && (
                        <button type="button" className="btn btn-sm" disabled={settings.busy} onClick={() => void unassign(a, false)}>
                          Remover <span className="sr-only">atribuição a {a.env_name}</span>
                        </button>
                      )}
                    </td>
                  </tr>
                );
                if (!confirming) return [row];
                return [
                  row,
                  <tr key={`${a.env_id}-confirm`}>
                    <td colSpan={5}>
                      <div role="group" aria-label={`Confirmar remoção de ${a.env_name}`} className="confirm-box">
                        <p>{pending.hub}</p>
                        <div className="form-actions">
                          <button type="button" className="btn" onClick={() => setPending(null)}>
                            Cancelar
                          </button>
                          <button
                            type="button"
                            className="btn btn-danger"
                            disabled={settings.busy}
                            onClick={() => void unassign(a, true)}
                          >
                            Remover mesmo assim
                          </button>
                        </div>
                      </div>
                    </td>
                  </tr>,
                ];
              })}
            </tbody>
          </table>
        </div>
      )}
    </section>
  );
}
