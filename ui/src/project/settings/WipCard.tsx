import { useState } from "react";
import { useTower } from "../../state/tower";
import type { HubProjectView } from "../../protocol/hub-gen";
import { DesktopOnly } from "../../web/DesktopOnly";
import { WIP_PROTOCOL, parseWipField, wipLimitsOf, wipOf, wipParts, wipReason } from "../wip";
import type { SettingsRun } from "./run";

const fieldText = (n: number | null) => (n === null ? "" : String(n));

/**
 * Limite de trabalho em andamento do projeto (#126), na seção Geral: o
 * máximo de sessões de implementação simultâneas e o de PRs abertos (vazio
 * = sem limite), com o uso agora ("N de M"). Acima do limite, iniciar
 * sessão pede confirmação, o início automático é recusado pelo hub e o
 * gestor não propõe trabalho novo. Hub anterior à 1.15: só o aviso.
 */
export function WipCard({
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
  const [form, setForm] = useState<{ sessions: string; prs: string } | null>(null);
  const id = `pc-wip-${pr.id}`;

  if (pr.wip_limits === undefined) {
    return (
      <section className="hub-card" aria-labelledby={id}>
        <div className="hub-card-head">
          <h3 id={id}>Trabalho em andamento</h3>
        </div>
        <p className="muted hub-card-empty" role="note">
          Este hub não tem limite de trabalho em andamento (anterior ao protocolo {WIP_PROTOCOL}): atualize o hub para
          limitar as sessões simultâneas e os PRs abertos deste projeto.
        </p>
      </section>
    );
  }

  const limits = wipLimitsOf(pr);
  const usage = wipOf(p);
  const reason = wipReason(usage);
  const describe = (max: number | null, used: number | undefined, what: string) =>
    max === null ? "sem limite" : used === undefined ? `até ${max} ${what}` : `${used} de ${max} ${what}`;

  const save = async () => {
    if (!form) return;
    const s = parseWipField(form.sessions);
    const r = parseWipField(form.prs);
    if ("error" in s) return settings.fail(`Sessões simultâneas: ${s.error}.`);
    if ("error" in r) return settings.fail(`PRs abertos: ${r.error}.`);
    const ok = await settings.run(async () => {
      await api.hubCall("hub.projects.update", {
        owner: pr.gh_owner,
        repo: pr.gh_repo,
        wip_limits: { max_sessions: s.value, max_prs: r.value },
      });
      return s.value === null && r.value === null
        ? `${pr.gh_owner}/${pr.gh_repo} ficou sem limite de trabalho em andamento.`
        : `Limite de trabalho em andamento de ${pr.gh_owner}/${pr.gh_repo} salvo.`;
    });
    if (ok) setForm(null);
  };

  return (
    <section className="hub-card" aria-labelledby={id}>
      <div className="hub-card-head">
        <h3 id={id}>Trabalho em andamento</h3>
        {!readOnly && !form && (
          <button
            type="button"
            className="btn btn-sm"
            onClick={() => setForm({ sessions: fieldText(limits.max_sessions), prs: fieldText(limits.max_prs) })}
          >
            Alterar limite
          </button>
        )}
        {readOnly && <DesktopOnly className="btn btn-sm" labels="Alterar limite" />}
      </div>
      <p className="muted hub-card-sub">
        Conta as sessões vivas em issues e épicos de todos os ambientes e os PRs abertos do espelho. No limite, iniciar
        sessão pede confirmação, o início automático espera uma vaga e o gestor não propõe trabalho novo.
      </p>
      {form ? (
        <form
          className="env-form hub-card-form"
          aria-label={`Limite de trabalho em andamento de ${pr.name}`}
          onSubmit={(e) => {
            e.preventDefault();
            void save();
          }}
        >
          <div className="field">
            <label htmlFor={`${id}-s`}>Sessões simultâneas</label>
            <input
              id={`${id}-s`}
              inputMode="numeric"
              placeholder="sem limite"
              value={form.sessions}
              onChange={(e) => setForm({ ...form, sessions: e.target.value })}
            />
          </div>
          <div className="field">
            <label htmlFor={`${id}-p`}>PRs abertos</label>
            <input
              id={`${id}-p`}
              inputMode="numeric"
              placeholder="sem limite"
              value={form.prs}
              onChange={(e) => setForm({ ...form, prs: e.target.value })}
            />
          </div>
          <div className="form-actions">
            <button type="button" className="btn" onClick={() => setForm(null)}>
              Cancelar
            </button>
            <button type="submit" className="btn btn-primary" disabled={settings.busy}>
              {settings.busy ? "Salvando…" : "Salvar limite"}
            </button>
          </div>
        </form>
      ) : (
        <dl className="hub-kv">
          <div>
            <dt>Sessões simultâneas</dt>
            <dd>{describe(limits.max_sessions, usage?.sessions, "sessões")}</dd>
          </div>
          <div>
            <dt>PRs abertos</dt>
            <dd>{describe(limits.max_prs, usage?.prs, "PRs")}</dd>
          </div>
          {reason && (
            <div>
              <dt>Agora</dt>
              <dd>
                <span className="badge" data-tone="attention" role="status">
                  {reason}
                </span>
              </dd>
            </div>
          )}
          {!reason && wipParts(usage).length > 0 && (
            <div>
              <dt>Agora</dt>
              <dd>há vaga</dd>
            </div>
          )}
        </dl>
      )}
    </section>
  );
}
