import { useState } from "react";
import { useTower } from "../../state/tower";
import type { HubProjectView, ReleaseConfig } from "../../protocol/hub-gen";
import { RELEASE_PROTOCOL, releaseFormError, releaseOf, releasePatch, releasePrHead } from "../../hub/release";
import type { SettingsRun } from "./run";
import { DesktopOnly } from "../../web/DesktopOnly";

/**
 * Release do projeto (#204), só GitHub + GitHub: liga o botão "Criar
 * release" e a sessão de release, com as branches do fluxo do
 * release-please, o back-merge, a aprovação dos runs pendentes, o nome do
 * secret com o token exclusivo da release e os prazos de cada espera. Mostra
 * se o secret existe no projeto, nunca o valor. Hub anterior à 1.21: só o
 * aviso.
 */
export function ReleaseCard({
  project: p,
  settings,
  secretPresent,
  readOnly = false,
}: {
  project: HubProjectView;
  settings: SettingsRun;
  /** O secret do token existe no projeto; `null` = não se sabe (web). */
  secretPresent: boolean | null;
  /** Web (épico #205): só o resumo. */
  readOnly?: boolean;
}) {
  const { api } = useTower();
  const pr = p.project;
  const cur = releaseOf(pr);
  const [form, setForm] = useState<ReleaseConfig | null>(null);
  const id = (k: string) => `rl-${pr.id}-${k}`;

  const head = (
    <div className="hub-card-head">
      <h3 id={id("head")}>Release</h3>
      {cur && !readOnly && !form && (
        <button type="button" className="btn btn-sm" onClick={() => setForm(cur)}>
          Editar release
        </button>
      )}
      {cur && readOnly && <DesktopOnly className="btn btn-sm" labels="Editar release" />}
    </div>
  );

  if (!cur) {
    return (
      <section className="hub-card" aria-labelledby={id("head")}>
        {head}
        <p className="muted hub-card-empty" role="note">
          Este hub não tem release por projeto (anterior ao protocolo {RELEASE_PROTOCOL}): atualize o hub para ligar a sessão
          de release.
        </p>
      </section>
    );
  }

  const err = form ? releaseFormError(form) : null;
  const save = async () => {
    if (!form || err) return;
    const release = releasePatch(cur, form);
    if (!release) {
      setForm(null);
      return;
    }
    const ok = await settings.run(async () => {
      await api.hubCall("hub.projects.update", { owner: pr.gh_owner, repo: pr.gh_repo, release });
      return `Release de ${pr.gh_owner}/${pr.gh_repo} salva; os ambientes recebem no próximo pull.`;
    });
    if (ok) setForm(null);
  };

  const text = (k: "source" | "target" | "release_pr_branch" | "token_secret", label: string, hint?: string) =>
    form && (
      <div className="field">
        <label htmlFor={id(k)}>{label}</label>
        <input
          id={id(k)}
          className="mono"
          value={form[k]}
          aria-describedby={hint ? id(`${k}-hint`) : undefined}
          onChange={(e) => setForm({ ...form, [k]: e.target.value })}
        />
        {hint && (
          <span id={id(`${k}-hint`)} className="hint muted">
            {hint}
          </span>
        )}
      </div>
    );
  const minutes = (k: keyof ReleaseConfig["timeouts"], label: string) =>
    form && (
      <div className="field">
        <label htmlFor={id(`t-${k}`)}>{label}</label>
        <input
          id={id(`t-${k}`)}
          type="number"
          min={1}
          max={1440}
          value={form.timeouts[k]}
          onChange={(e) => setForm({ ...form, timeouts: { ...form.timeouts, [k]: Number(e.target.value) } })}
        />
      </div>
    );

  const secretText =
    secretPresent === null ? "veja em Secrets e variáveis (app desktop)" : secretPresent ? "cadastrado" : "ausente";

  return (
    <section className="hub-card" aria-labelledby={id("head")}>
      {head}
      {form ? (
        <form
          className="env-form hub-card-form"
          aria-label={`Editar release de ${pr.name}`}
          onSubmit={(e) => {
            e.preventDefault();
            void save();
          }}
        >
          <label className="ctl">
            <input type="checkbox" checked={form.enabled} onChange={(e) => setForm({ ...form, enabled: e.target.checked })} />
            Ligar a sessão de release (botão "Criar release")
          </label>
          {text("source", "Branch de origem", "Promovida para o destino pelo PR de promoção.")}
          {text("target", "Branch de destino", "O push nela dispara o workflow do release-please.")}
          {text("release_pr_branch", "Head do PR de release", "{target} vira a branch de destino.")}
          {text(
            "token_secret",
            "Secret do token exclusivo",
            "Secret do projeto com o token do GitHub só da release (Contents, Pull requests e Actions de escrita; Metadata de leitura). Só a sessão de release o recebe.",
          )}
          <label className="ctl">
            <input
              type="checkbox"
              checked={form.back_merge}
              onChange={(e) => setForm({ ...form, back_merge: e.target.checked })}
            />
            Esperar e mesclar o back-merge (destino → origem)
          </label>
          <label className="ctl">
            <input
              type="checkbox"
              checked={form.approve_pending_runs}
              onChange={(e) => setForm({ ...form, approve_pending_runs: e.target.checked })}
            />
            Aprovar os runs em action_required dos PRs do fluxo
          </label>
          {minutes("release_pr", "Prazo do PR de release (min)")}
          {minutes("runs", "Prazo dos runs (min)")}
          {minutes("publish", "Prazo da publicação (min)")}
          {minutes("back_merge", "Prazo do back-merge (min)")}
          {err && (
            <p className="form-error" role="alert">
              {err}
            </p>
          )}
          <div className="form-actions">
            <button type="button" className="btn" onClick={() => setForm(null)}>
              Cancelar
            </button>
            <button type="submit" className="btn btn-primary" disabled={settings.busy || !!err}>
              {settings.busy ? "Salvando…" : "Salvar release"}
            </button>
          </div>
        </form>
      ) : (
        <dl className="hub-kv">
          <div>
            <dt>Sessão de release</dt>
            <dd>{cur.enabled ? "ligada" : "desligada"}</dd>
          </div>
          <div>
            <dt>Fluxo</dt>
            <dd>
              release-please: <span className="mono">{cur.source}</span> → <span className="mono">{cur.target}</span>, PR de
              release <span className="mono">{releasePrHead(cur)}</span>
              {cur.back_merge ? ", com back-merge" : ", sem back-merge"}
              {cur.approve_pending_runs ? ", aprovando os runs pendentes" : ""}
            </dd>
          </div>
          <div>
            <dt>Token</dt>
            <dd>
              secret <span className="mono">{cur.token_secret}</span>: {secretText}
            </dd>
          </div>
          <div>
            <dt>Prazos</dt>
            <dd>
              PR de release {cur.timeouts.release_pr} min, runs {cur.timeouts.runs} min, publicação {cur.timeouts.publish}{" "}
              min, back-merge {cur.timeouts.back_merge} min
            </dd>
          </div>
        </dl>
      )}
      <p className="muted hub-card-sub">
        A sessão de release roda no clone principal só para leitura. Mesclar e aprovar só pelo{" "}
        <span className="mono">ctower release</span>, que confere as pré-condições e usa sempre merge commit preso ao SHA,
        nunca squash, rebase nem push direto.
      </p>
    </section>
  );
}
