import { useState } from "react";
import { useTower } from "../../state/tower";
import type { HubProjectView } from "../../protocol/hub-gen";
import { TrackerReadiness } from "../../hub/TrackerReadiness";
import type { SettingsRun } from "./run";

/** Ambiente de sync do projeto Jira (spec §5.1), escolhido entre os atribuídos. */
export function SyncEnvCard({
  project: p,
  settings,
  now,
  readOnly = false,
}: {
  project: HubProjectView;
  settings: SettingsRun;
  now: number;
  /** Web (épico #205): sem trocar o ambiente. */
  readOnly?: boolean;
}) {
  const { api } = useTower();
  const pr = p.project;
  const [env, setEnv] = useState<string | null>(null);
  const syncName = pr.sync_env_id ? (p.assignments.find((a) => a.env_id === pr.sync_env_id)?.env_name ?? pr.sync_env_id) : null;

  const change = async (id: string, name: string) => {
    const ok = await settings.run(async () => {
      await api.hubCall("hub.projects.update", { owner: pr.gh_owner, repo: pr.gh_repo, sync_env: id });
      return `${name} é o ambiente de sync de ${pr.name}; os ambientes recebem no próximo pull.`;
    });
    if (ok) setEnv(null);
  };

  return (
    <section className="hub-card" aria-labelledby={`pc-sync-${pr.id}`}>
      <div className="hub-card-head">
        <h3 id={`pc-sync-${pr.id}`}>Ambiente de sync</h3>
        {!readOnly && p.assignments.length > 0 && env === null && (
          <button
            type="button"
            className="btn btn-sm"
            onClick={() => setEnv(p.assignments.find((a) => a.env_id !== pr.sync_env_id)?.env_id ?? p.assignments[0].env_id)}
          >
            {pr.sync_env_id ? "Trocar ambiente de sync" : "Escolher ambiente de sync"}
          </button>
        )}
      </div>
      {env !== null ? (
        <form
          className="env-form hub-card-form"
          aria-label={`Ambiente de sync de ${pr.name}`}
          onSubmit={(e) => {
            e.preventDefault();
            const a = p.assignments.find((x) => x.env_id === env);
            if (a) void change(a.env_id, a.env_name);
          }}
        >
          <div className="field">
            <label htmlFor={`se-env-${pr.id}`}>Ambiente (entre os atribuídos)</label>
            <select id={`se-env-${pr.id}`} value={env} onChange={(e) => setEnv(e.target.value)}>
              {p.assignments.map((a) => (
                <option key={a.env_id} value={a.env_id}>
                  {a.env_name}
                  {a.env_id === pr.sync_env_id ? " (atual)" : ""}
                </option>
              ))}
            </select>
          </div>
          <div className="form-actions">
            <button type="button" className="btn" onClick={() => setEnv(null)}>
              Cancelar
            </button>
            <button type="submit" className="btn btn-primary" disabled={settings.busy || env === pr.sync_env_id}>
              Usar como ambiente de sync
            </button>
          </div>
        </form>
      ) : (
        <p className="hub-card-msg">
          {syncName ? (
            <>
              <strong>{syncName}</strong> busca o Jira e o GitLab e envia o espelho ao hub. Os outros ambientes
              atribuídos sincronizam só para uso local.
            </>
          ) : (
            <span className="muted">
              Sem ambiente de sync: o hub não recebe o espelho do Jira.{" "}
              {p.assignments.length === 0
                ? "Atribua o projeto a um ambiente que alcance o Jira e o GitLab; a primeira atribuição vira o ambiente de sync."
                : "Escolha um dos ambientes atribuídos."}
            </span>
          )}
        </p>
      )}
      {pr.sync_env_id && <TrackerReadiness project={pr} status={p.tracker_status} now={now} />}
    </section>
  );
}
