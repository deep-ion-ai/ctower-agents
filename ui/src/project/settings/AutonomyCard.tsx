import { useState } from "react";
import { useTower } from "../../state/tower";
import type { AlignStrategy, HubProjectView } from "../../protocol/hub-gen";
import {
  AUTONOMY_PROTOCOL,
  DEFAULT_AUTONOMOUS_TEMPLATE,
  DEFAULT_AUTONOMY_BRANCH,
  STRATEGY_LABEL,
  autonomyBase,
  autonomyBranch,
  autonomyEnvChoices,
  autonomyForm,
  autonomyFormError,
  autonomyOf,
  autonomyPatch,
  autonomyTemplatesPatch,
  autonomousTemplateOf,
  autonomyWarning,
  type AutonomyForm,
} from "../../hub/autonomy";
import type { SettingsRun } from "./run";
import { DesktopOnly } from "../../web/DesktopOnly";

/**
 * Modo autônomo do projeto (#181): ligado, as sessões autônomas rodam num
 * ambiente marcado como autônomo e atribuído ao projeto, saem da branch de
 * integração (padrão `wip/autonoma`) e abrem PR para ela; o ambiente mantém
 * o PR rascunho `branch → base` e alinha a branch com a base antes de cada
 * issue. O relato do ambiente aparece aqui (e o conflito, também na Visão
 * geral). Hub anterior à 1.14: só o aviso. O template do prompt da sessão
 * autônoma (#191, `session_templates.autonomous`) também se edita aqui;
 * vazio = o padrão, que o ambiente renderiza.
 */
export function AutonomyCard({
  project: p,
  settings,
  readOnly = false,
}: {
  project: HubProjectView;
  settings: SettingsRun;
  /** Web (épico #205): só o resumo. */
  readOnly?: boolean;
}) {
  const { api, hub } = useTower();
  const pr = p.project;
  const cur = autonomyOf(pr);
  const [form, setForm] = useState<AutonomyForm | null>(null);

  if (!cur) {
    return (
      <section className="hub-card" aria-labelledby={`pc-auto-${pr.id}`}>
        <div className="hub-card-head">
          <h3 id={`pc-auto-${pr.id}`}>Modo autônomo</h3>
        </div>
        <p className="muted hub-card-empty" role="note">
          Este hub não tem modo autônomo (anterior ao protocolo {AUTONOMY_PROTOCOL}): atualize o hub para ligar o modo neste
          projeto.
        </p>
      </section>
    );
  }

  const choices = autonomyEnvChoices(p.assignments, hub.envs);
  const envName = (id: string | null) => (id ? (p.assignments.find((a) => a.env_id === id)?.env_name ?? id) : null);
  const report = pr.autonomy_report ?? null;
  const warning = autonomyWarning(pr);
  const branch = autonomyBranch(cur);
  const base = autonomyBase(cur, pr);
  const formError = form ? autonomyFormError(form, pr) : null;
  const template = autonomousTemplateOf(pr);

  const save = async (f: AutonomyForm) => {
    const autonomy = autonomyPatch(f, pr);
    const templates = autonomyTemplatesPatch(f, pr);
    const ok = await settings.run(async () => {
      await api.hubCall("hub.projects.update", {
        owner: pr.gh_owner,
        repo: pr.gh_repo,
        autonomy,
        ...(templates ? { session_templates: templates } : {}),
      });
      return autonomy.enabled
        ? `Modo autônomo de ${pr.name} ligado em ${envName(autonomy.env_id)}; o ambiente cria a branch e o PR rascunho no próximo pull.`
        : `Modo autônomo de ${pr.name} desligado.`;
    });
    if (ok) setForm(null);
  };

  return (
    <section className="hub-card" aria-labelledby={`pc-auto-${pr.id}`}>
      <div className="hub-card-head">
        <h3 id={`pc-auto-${pr.id}`}>Modo autônomo</h3>
        {!readOnly && form === null && (
          <button type="button" className="btn btn-sm" onClick={() => setForm(autonomyForm(cur, pr))}>
            Editar <span className="sr-only">modo autônomo</span>
          </button>
        )}
        {readOnly && <DesktopOnly className="btn btn-sm" labels="Editar" />}
      </div>
      {warning && (
        <p role="alert" className="start-error">
          {warning}. Nenhuma issue autônoma começa até alguém resolver: inicie uma sessão do projeto em{" "}
          {envName(cur.env_id) ?? "o ambiente do modo"} para trazer {base} para {branch}.
          {report && report.conflicts.length > 0 && (
            <span className="hub-card-sub">
              Em conflito: <span className="mono">{report.conflicts.join(", ")}</span>
            </span>
          )}
        </p>
      )}
      {form === null ? (
        <dl className="hub-kv">
          <div>
            <dt>Modo</dt>
            <dd>
              <span className="hub-dot" data-tone={cur.enabled ? "ok" : undefined} aria-hidden="true" />{" "}
              {cur.enabled ? `ligado em ${envName(cur.env_id)}` : "desligado"}
            </dd>
          </div>
          <div>
            <dt>Branch de integração</dt>
            <dd>
              <span className="mono">{branch}</span> → <span className="mono">{base}</span> · {STRATEGY_LABEL[cur.strategy].toLowerCase()}
            </dd>
          </div>
          {template !== null && (
            <div>
              <dt>Prompt da sessão</dt>
              <dd>{template.trim() ? "template do projeto" : "template padrão"}</dd>
            </div>
          )}
          {cur.enabled && (
            <div>
              <dt>Situação</dt>
              <dd>
                {!report ? (
                  <span className="muted">o ambiente ainda não relatou</span>
                ) : (
                  <>
                    <span
                      className="hub-dot"
                      data-tone={report.state === "ok" ? "ok" : "error"}
                      aria-hidden="true"
                    />{" "}
                    {report.state === "ok" ? "alinhada com a base" : report.state === "conflict" ? "em conflito com a base" : "erro"}
                    {report.pr_url && (
                      <>
                        {" · "}
                        <a href={report.pr_url} target="_blank" rel="noreferrer">
                          PR rascunho
                        </a>
                      </>
                    )}
                    {report.message && report.state !== "conflict" && <span className="hub-card-sub">{report.message}</span>}
                  </>
                )}
              </dd>
            </div>
          )}
        </dl>
      ) : (
        <form
          className="env-form hub-card-form"
          aria-label={`Modo autônomo de ${pr.name}`}
          onSubmit={(e) => {
            e.preventDefault();
            if (formError) settings.fail(formError);
            else void save(form);
          }}
        >
          <div className="field hub-wide">
            <label>
              <input type="checkbox" checked={form.enabled} onChange={(e) => setForm({ ...form, enabled: e.target.checked })} /> Ligar o
              modo autônomo
            </label>
          </div>
          <div className="field">
            <label htmlFor={`au-env-${pr.id}`}>Ambiente (atribuído e marcado como autônomo)</label>
            <select id={`au-env-${pr.id}`} value={form.envId} onChange={(e) => setForm({ ...form, envId: e.target.value })}>
              <option value="">—</option>
              {choices.map((a) => (
                <option key={a.env_id} value={a.env_id}>
                  {a.env_name}
                </option>
              ))}
            </select>
            {choices.length === 0 && (
              <span className="hint muted">
                Nenhum ambiente atribuído está marcado como autônomo: marque um em Hub › Ambientes e atribua o projeto a ele.
              </span>
            )}
          </div>
          <div className="field">
            <label htmlFor={`au-br-${pr.id}`}>Branch de integração</label>
            <input
              id={`au-br-${pr.id}`}
              className="mono"
              value={form.branch}
              placeholder={DEFAULT_AUTONOMY_BRANCH}
              onChange={(e) => setForm({ ...form, branch: e.target.value })}
            />
          </div>
          <div className="field">
            <label htmlFor={`au-base-${pr.id}`}>Branch base</label>
            <input
              id={`au-base-${pr.id}`}
              className="mono"
              value={form.base}
              placeholder={pr.base_branch}
              onChange={(e) => setForm({ ...form, base: e.target.value })}
            />
          </div>
          <div className="field">
            <label htmlFor={`au-st-${pr.id}`}>Alinhamento antes de cada issue</label>
            <select
              id={`au-st-${pr.id}`}
              value={form.strategy}
              onChange={(e) => setForm({ ...form, strategy: e.target.value as AlignStrategy })}
            >
              {(Object.keys(STRATEGY_LABEL) as AlignStrategy[]).map((s) => (
                <option key={s} value={s}>
                  {STRATEGY_LABEL[s]}
                </option>
              ))}
            </select>
          </div>
          {form.template !== null && (
            <div className="field hub-wide">
              <label htmlFor={`au-tpl-${pr.id}`}>Template do prompt da sessão autônoma</label>
              <textarea
                id={`au-tpl-${pr.id}`}
                className="mono"
                rows={8}
                value={form.template}
                placeholder={DEFAULT_AUTONOMOUS_TEMPLATE}
                onChange={(e) => setForm({ ...form, template: e.target.value })}
              />
              <span className="hint muted">
                Vazio = o padrão acima. O ambiente monta o prompt quando o hub inicia a sessão sem a UI. Variáveis:{" "}
                <span className="mono">
                  {"{{kind}} {{number}} {{ref}} {{owner}} {{repo}} {{title}} {{branch}} {{integration_branch}} {{protected}} {{change}}"}
                </span>{" "}
                (<span className="mono">{"{{change}}"}</span> = PR ou MR).
              </span>
            </div>
          )}
          {formError && (
            <p className="hint muted" role="note">
              {formError}
            </p>
          )}
          <div className="form-actions">
            <button type="button" className="btn" onClick={() => setForm(null)}>
              Cancelar
            </button>
            <button type="submit" className="btn btn-primary" disabled={settings.busy || formError !== null}>
              {settings.busy ? "Salvando…" : "Salvar modo autônomo"}
            </button>
          </div>
        </form>
      )}
    </section>
  );
}
