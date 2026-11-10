import { useState } from "react";
import { useTower } from "../../state/tower";
import type { HubProjectView } from "../../protocol/hub-gen";
import {
  SESSION_TEMPLATES_PROTOCOL,
  SESSION_TEMPLATE_LABEL,
  customizeStart,
  hasOverride,
  sessionTemplatesOf,
  templatesPatch,
} from "../../hub/session-templates";
import { SESSION_TEMPLATE_KINDS, type SessionTemplateKind } from "../templates";
import type { SettingsRun } from "./run";
import { DesktopOnly } from "../../web/DesktopOnly";

/** As variáveis de `renderTemplate`, para a dica do editor. */
const VARS =
  "{{key}} {{title}} {{number}} {{kind}} {{owner}} {{repo}} {{branch}} {{base}} {{protected}} {{jira}} {{jira_url}} {{jira_project}} {{glab}} {{forge_path}}";

/**
 * Templates do prompt inicial do projeto (#166): uma linha por tipo de
 * sessão, herdando o template global do rastreador (o `ui.json` deste app)
 * ou com override gravado no hub, que vale em todos os ambientes do projeto.
 * "Personalizar" parte do global; "Voltar ao global" apaga o override. Hub
 * anterior à 1.13: só o aviso.
 */
export function SessionTemplatesCard({
  project: p,
  settings,
  readOnly = false,
}: {
  project: HubProjectView;
  settings: SettingsRun;
  /** Web (épico #205): só o resumo. */
  readOnly?: boolean;
}) {
  const { api, config } = useTower();
  const pr = p.project;
  const cur = sessionTemplatesOf(pr);
  const [editing, setEditing] = useState<{ kind: SessionTemplateKind; text: string } | null>(null);
  const jira = pr.tracker?.kind === "jira";

  if (!cur) {
    return (
      <section className="hub-card" aria-labelledby={`pc-tpl-${pr.id}`}>
        <div className="hub-card-head">
          <h3 id={`pc-tpl-${pr.id}`}>Templates de sessão</h3>
        </div>
        <p className="muted hub-card-empty" role="note">
          Este hub não tem templates de sessão por projeto (anterior ao protocolo {SESSION_TEMPLATES_PROTOCOL}): atualize o
          hub para personalizar o prompt inicial deste projeto. As sessões usam os templates globais.
        </p>
      </section>
    );
  }

  const save = async (kind: SessionTemplateKind, text: string) => {
    const patch = templatesPatch(cur, kind, text);
    if (!patch) {
      setEditing(null);
      return;
    }
    const ok = await settings.run(async () => {
      await api.hubCall("hub.projects.update", { owner: pr.gh_owner, repo: pr.gh_repo, session_templates: patch });
      return patch[kind]
        ? `Template de ${SESSION_TEMPLATE_LABEL[kind].toLowerCase()} de ${pr.gh_owner}/${pr.gh_repo} salvo; os ambientes recebem no próximo pull.`
        : `${SESSION_TEMPLATE_LABEL[kind]} de ${pr.gh_owner}/${pr.gh_repo} voltou ao template global.`;
    });
    if (ok) setEditing(null);
  };

  return (
    <section className="hub-card" aria-labelledby={`pc-tpl-${pr.id}`}>
      <div className="hub-card-head">
        <h3 id={`pc-tpl-${pr.id}`}>Templates de sessão</h3>
      </div>
      <p className="muted hub-card-sub">
        Sem override, o prompt inicial usa o template global{jira ? " do Jira" : ""} das Configurações deste app. O
        override fica no hub e vale em todos os ambientes do projeto.
      </p>
      <dl className="hub-tpl-list">
        {SESSION_TEMPLATE_KINDS.map((k) => {
          const own = hasOverride(cur, k);
          const open = editing?.kind === k;
          const label = SESSION_TEMPLATE_LABEL[k];
          return (
            <div key={k}>
              <dt>{label}</dt>
              <dd>
                <span className="hub-tpl-state">
                  <span className="hub-dot" data-tone={own ? "ok" : undefined} aria-hidden="true" />{" "}
                  {own ? "template do projeto" : "herdando o template global"}
                </span>
                {!readOnly && !open && (
                  <span className="hub-card-actions">
                    <button
                      type="button"
                      className="btn btn-sm"
                      aria-label={`${own ? "Editar" : "Personalizar"} ${label}`}
                      onClick={() => setEditing({ kind: k, text: own ? cur[k] : customizeStart(config.templates, pr, k) })}
                    >
                      {own ? "Editar" : "Personalizar"}
                    </button>
                    {own && (
                      <button
                        type="button"
                        className="btn btn-sm"
                        aria-label={`Voltar ao global ${label}`}
                        disabled={settings.busy}
                        onClick={() => void save(k, "")}
                      >
                        Voltar ao global
                      </button>
                    )}
                  </span>
                )}
                {readOnly && <DesktopOnly className="btn btn-sm" labels={own ? "Editar" : "Personalizar"} />}
                {open && editing && (
                  <form
                    className="env-form hub-card-form"
                    aria-label={`Template de ${label} de ${pr.name}`}
                    onSubmit={(e) => {
                      e.preventDefault();
                      void save(k, editing.text);
                    }}
                  >
                    <div className="field hub-wide">
                      <label htmlFor={`pt-${pr.id}-${k}`}>Template</label>
                      <textarea
                        id={`pt-${pr.id}-${k}`}
                        className="mono"
                        rows={14}
                        value={editing.text}
                        aria-describedby={`pt-${pr.id}-${k}-hint`}
                        onChange={(e) => setEditing({ kind: k, text: e.target.value })}
                      />
                      <span id={`pt-${pr.id}-${k}-hint`} className="hint muted">
                        Variáveis: <span className="mono">{VARS}</span>. Vazio volta ao template global.
                      </span>
                    </div>
                    <div className="form-actions">
                      <button type="button" className="btn" onClick={() => setEditing(null)}>
                        Cancelar
                      </button>
                      <button type="submit" className="btn btn-primary" disabled={settings.busy}>
                        {settings.busy ? "Salvando…" : "Salvar template"}
                      </button>
                    </div>
                  </form>
                )}
              </dd>
            </div>
          );
        })}
      </dl>
    </section>
  );
}
