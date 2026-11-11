import { useState } from "react";
import { useTower } from "../../state/tower";
import type { HubProjectView } from "../../protocol/hub-gen";
import { APPROVAL_PROTOCOL, DEFAULT_APPROVAL_LABEL } from "../approval";
import type { SettingsRun } from "./run";
import { DesktopOnly } from "../../web/DesktopOnly";

/**
 * Rótulo de aprovação do projeto (#124): o rótulo que marca a issue como
 * aprovada (padrão `aprovada`, vazio desliga) e a opção de exigi-lo para
 * iniciar sessão. Fica no hub e chega aos ambientes no próximo pull; quem
 * recusa o início é o ambiente. Hub anterior à 1.15: só o aviso.
 */
export function ApprovalCard({
  project: p,
  settings,
  readOnly = false,
}: {
  project: HubProjectView;
  settings: SettingsRun;
  /** Web (épico #205): só o resumo. */
  readOnly?: boolean;
}) {
  const { api } = useTower();
  const pr = p.project;
  const cur = pr.approval;
  const [form, setForm] = useState<{ label: string; required: boolean } | null>(null);

  const head = (
    <div className="hub-card-head">
      <h3 id={`pc-appr-${pr.id}`}>Rótulo de aprovação</h3>
      {cur && !readOnly && !form && (
        <button type="button" className="btn btn-sm" onClick={() => setForm({ label: cur.label, required: cur.required })}>
          Editar aprovação
        </button>
      )}
      {cur && readOnly && <DesktopOnly className="btn btn-sm" labels="Editar aprovação" />}
    </div>
  );

  if (!cur) {
    return (
      <section className="hub-card" aria-labelledby={`pc-appr-${pr.id}`}>
        {head}
        <p className="muted hub-card-empty" role="note">
          Este hub não tem rótulo de aprovação por projeto (anterior ao protocolo {APPROVAL_PROTOCOL}): atualize o hub para
          exigir o rótulo ao iniciar sessão.
        </p>
      </section>
    );
  }

  const save = async () => {
    if (!form) return;
    const approval = { label: form.label.trim(), required: form.required };
    if (approval.label === cur.label && approval.required === cur.required) {
      setForm(null);
      return;
    }
    const ok = await settings.run(async () => {
      await api.hubCall("hub.projects.update", { owner: pr.gh_owner, repo: pr.gh_repo, approval });
      return `Rótulo de aprovação de ${pr.gh_owner}/${pr.gh_repo} salvo; os ambientes recebem no próximo pull.`;
    });
    if (ok) setForm(null);
  };

  const label = cur.label.trim();
  const summary = !label
    ? "Desligado: nenhuma issue precisa de rótulo."
    : cur.required
      ? `Exigido para iniciar sessão: issues sem "${label}" ficam aguardando aprovação.`
      : `"${label}", sem exigir para iniciar sessão (o piloto automático só pega issues com ele).`;

  return (
    <section className="hub-card" aria-labelledby={`pc-appr-${pr.id}`}>
      {head}
      {form ? (
        <form
          className="env-form hub-card-form"
          aria-label={`Editar rótulo de aprovação de ${pr.name}`}
          onSubmit={(e) => {
            e.preventDefault();
            void save();
          }}
        >
          <div className="field">
            <label htmlFor={`pa-${pr.id}-label`}>Rótulo de aprovação</label>
            <input
              id={`pa-${pr.id}-label`}
              className="mono"
              value={form.label}
              placeholder={DEFAULT_APPROVAL_LABEL}
              aria-describedby={`pa-${pr.id}-hint`}
              onChange={(e) => setForm({ ...form, label: e.target.value })}
            />
            <span id={`pa-${pr.id}-hint`} className="hint muted">
              O rótulo (label) do GitHub ou do Jira que aprova a issue. Vazio desliga. O épico conta pelo próprio rótulo; a
              sub-issue precisa do dela.
            </span>
          </div>
          <label className="ctl">
            <input
              type="checkbox"
              checked={form.required}
              onChange={(e) => setForm({ ...form, required: e.target.checked })}
            />
            Exigir para iniciar sessão
          </label>
          <div className="form-actions">
            <button type="button" className="btn" onClick={() => setForm(null)}>
              Cancelar
            </button>
            <button type="submit" className="btn btn-primary" disabled={settings.busy}>
              {settings.busy ? "Salvando…" : "Salvar aprovação"}
            </button>
          </div>
        </form>
      ) : (
        <dl className="hub-kv">
          <div>
            <dt>Rótulo</dt>
            <dd>
              {label ? <span className="mono">{label}</span> : "nenhum"}
              <span className="hub-card-sub">{summary}</span>
            </dd>
          </div>
        </dl>
      )}
      <p className="muted hub-card-sub">
        Com a exigência ligada, o ambiente recusa iniciar sessão numa issue sem o rótulo; na modal de início, "Iniciar mesmo
        assim" pede confirmação e registra uma decisão orientada no item. O gestor e o piloto automático nunca iniciam sem o
        rótulo. Sessão de planejamento e do gestor não passam pela regra.
      </p>
    </section>
  );
}
