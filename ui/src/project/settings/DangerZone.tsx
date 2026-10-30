import { useState } from "react";
import { useTower } from "../../state/tower";
import type { DeleteImpact, HubProjectView, HubSession, ProjectDeleted } from "../../protocol/hub-gen";
import { needsConfirm } from "../../hub/calls";
import { keyOf, plural } from "./form";
import type { SettingsRun } from "./run";

/** Exclusão esperando confirmação; `hub` = o hub pediu `confirm`. */
interface Deleting {
  hub: string | null;
  /** `null` = consultando. */
  impact: DeleteImpact | null;
  /** A consulta falhou e o impacto é o que a UI sabe. */
  local: boolean;
}

/**
 * Exclusão do projeto, que mostra o impacto (`DeleteImpact`, do hub ou
 * estimado pela UI com `live` e `secretCount`) e pede a confirmação que o
 * hub pedir. `onDeleted` roda depois de excluir, com a mensagem (o
 * snapshot anterior): quem sai da tela (as Configurações do projeto, que
 * deixa de existir) a leva adiante.
 */
export function DangerZone({
  project: p,
  settings,
  live,
  secretCount,
  onDeleted,
}: {
  project: HubProjectView;
  settings: SettingsRun;
  live: HubSession[];
  /** Secrets e variáveis do próprio projeto. */
  secretCount: number;
  onDeleted?: (message: string) => void;
}) {
  const { api } = useTower();
  const pr = p.project;
  const key = keyOf(pr.gh_owner, pr.gh_repo);
  const [deleting, setDeleting] = useState<Deleting | null>(null);

  const impact: DeleteImpact | null = deleting
    ? (deleting.impact ?? (deleting.local ? { project: pr, assignments: p.assignments, live_sessions: live } : null))
    : null;
  // Com o impacto do hub, atribuições ou sessões vivas já pedem `confirm`: um clique só.
  // Se o projeto mudar entre a consulta e a exclusão, a recusa do hub ainda vale.
  const needs = !!deleting?.impact && (deleting.impact.assignments.length > 0 || deleting.impact.live_sessions.length > 0);

  const remove = (confirm: boolean) =>
    settings.run(
      async () => {
        const r = await api.hubCall<ProjectDeleted>("hub.projects.delete", {
          owner: pr.gh_owner,
          repo: pr.gh_repo,
          ...(confirm ? { confirm: true } : {}),
        });
        setDeleting(null);
        const message = `${key} excluído do hub. Snapshot anterior: ${r.snapshot}`;
        onDeleted?.(message);
        return message;
      },
      (m) => {
        if (confirm || !needsConfirm(m)) return false;
        setDeleting((d) => (d ? { ...d, hub: m } : { hub: m, impact: null, local: true }));
        return true;
      },
    );

  /**
   * Abre a confirmação e consulta o impacto exato no hub. Sem a consulta
   * (hub antigo ou erro), fica o impacto que a UI calcula.
   */
  const askDelete = async () => {
    setDeleting({ hub: null, impact: null, local: false });
    try {
      const i = await api.hubCall<DeleteImpact>("hub.projects.impact", { owner: pr.gh_owner, repo: pr.gh_repo });
      setDeleting((d) => (d ? { ...d, impact: i } : d));
    } catch {
      setDeleting((d) => (d ? { ...d, local: true } : d));
    }
  };

  return (
    <section className="hub-card hub-danger" aria-labelledby={`pc-dng-${pr.id}`}>
      <div className="hub-card-head">
        <h3 id={`pc-dng-${pr.id}`}>Zona de perigo</h3>
      </div>
      {deleting ? (
        <div role="group" aria-label={`Confirmar exclusão de ${pr.name}`} className="confirm-box hub-card-msg">
          {impact ? (
            <DeleteImpactText impact={impact} secrets={secretCount} local={deleting.local && !deleting.impact} />
          ) : (
            <p className="hub-card-sub" role="status">
              Consultando o impacto no hub…
            </p>
          )}
          {deleting.hub ? (
            <p>{deleting.hub}</p>
          ) : (
            <p>
              Excluir {key} do hub? Isso apaga o histórico do projeto no hub (work items, sessões, decisões,
              eventos), que é a única cópia. O hub tira um snapshot antes; para desfazer, restaure-o com{" "}
              <span className="mono">cthubd restore</span>.
            </p>
          )}
          <div className="form-actions">
            <button type="button" className="btn" onClick={() => setDeleting(null)}>
              Cancelar
            </button>
            <button
              type="button"
              className="btn btn-danger"
              disabled={settings.busy || !impact}
              onClick={() => void remove(deleting.hub !== null || needs)}
            >
              {deleting.hub ? "Excluir mesmo assim" : "Excluir projeto"}
            </button>
          </div>
        </div>
      ) : (
        <div className="hub-danger-row">
          <div className="hub-card-text">
            Excluir o projeto do hub
            <span className="hub-card-sub">Mostra o impacto (atribuições, sessões e secrets) antes de confirmar.</span>
          </div>
          <button type="button" className="btn btn-danger" onClick={() => void askDelete()}>
            Excluir projeto… <span className="sr-only">{pr.name}</span>
          </button>
        </div>
      )}
    </section>
  );
}

/**
 * O que a exclusão atinge: o impacto do hub (`hub.projects.impact`) ou, com
 * `local`, o que a UI já sabe; os secrets do projeto vêm do `hub.secrets.list`.
 */
function DeleteImpactText({ impact, secrets, local }: { impact: DeleteImpact; secrets: number; local: boolean }) {
  const envs = impact.assignments.map((a) => a.env_name);
  return (
    <ul className="hub-impact" aria-label="Impacto da exclusão">
      {local && <li>Impacto estimado pela UI (o hub não respondeu à consulta); o hub confere de novo ao excluir.</li>}
      <li>
        {envs.length === 0
          ? "Nenhuma atribuição ativa."
          : `${plural(envs.length, "atribuição ativa", "atribuições ativas")}: ${envs.join(", ")}.`}
      </li>
      <li>
        {impact.live_sessions.length === 0
          ? "Nenhuma sessão viva relatada."
          : `${plural(impact.live_sessions.length, "sessão viva", "sessões vivas")}, que os ambientes não encerram.`}
      </li>
      <li>
        {secrets === 0
          ? "Nenhum secret ou variável do projeto."
          : `${plural(secrets, "secret ou variável do projeto", "secrets e variáveis do projeto")}.`}
      </li>
    </ul>
  );
}
