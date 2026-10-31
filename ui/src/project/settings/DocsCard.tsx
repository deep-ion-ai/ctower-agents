import { useState } from "react";
import { useTower } from "../../state/tower";
import type { HubProjectView } from "../../protocol/hub-gen";
import { docsDirName, docsFormOf, docsPatch, docsText } from "../../hub/docs";
import { DocsForm } from "./DocsForm";
import { projectEditForm } from "./RepositoryCard";
import type { SettingsRun } from "./run";
import { DesktopOnly } from "../../web/DesktopOnly";

/**
 * Docs do projeto (#444) como seção própria das Configurações do projeto
 * (#17): o resumo de hoje e, no desktop, a edição só dos docs, com o mesmo
 * `DocsForm` e o mesmo `docsPatch` da edição completa (`RepositoryCard`),
 * que segue editando tudo junto. Hub anterior à #440: só o aviso do form.
 */
export function DocsCard({
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
  const [form, setForm] = useState<ReturnType<typeof projectEditForm> | null>(null);

  const save = async () => {
    const patch = form && docsPatch(form.docs, pr);
    if (!patch) {
      setForm(null);
      return;
    }
    const ok = await settings.run(async () => {
      await api.hubCall("hub.projects.update", { owner: pr.gh_owner, repo: pr.gh_repo, docs: patch });
      return `Docs de ${pr.gh_owner}/${pr.gh_repo} atualizados; os ambientes recebem no próximo pull.`;
    });
    if (ok) setForm(null);
  };

  return (
    <section className="hub-card" aria-labelledby={`pc-docs-${pr.id}`}>
      <div className="hub-card-head">
        <h3 id={`pc-docs-${pr.id}`}>Docs do projeto</h3>
        {!readOnly && !form && docsFormOf(pr) && (
          <button type="button" className="btn btn-sm" onClick={() => setForm(projectEditForm(p))}>
            Editar docs
          </button>
        )}
        {/* Só leitura (web): desabilitado com o motivo (#26). */}
        {readOnly && docsFormOf(pr) && <DesktopOnly className="btn btn-sm" labels="Editar docs" />}
      </div>
      {form ? (
        <form
          className="env-form hub-card-form"
          aria-label={`Editar docs de ${pr.name}`}
          onSubmit={(e) => {
            e.preventDefault();
            void save();
          }}
        >
          <DocsForm form={form} onChange={setForm} prefix={`pd-${pr.id}`} />
          <div className="form-actions">
            <button type="button" className="btn" onClick={() => setForm(null)}>
              Cancelar
            </button>
            <button type="submit" className="btn btn-primary" disabled={settings.busy}>
              {settings.busy ? "Salvando…" : "Salvar docs"}
            </button>
          </div>
        </form>
      ) : pr.docs ? (
        <dl className="hub-kv">
          <div>
            <dt>Docs</dt>
            <dd>
              {docsText(pr.docs)}
              {pr.docs.enabled && hub.info?.docs_dir && (
                <span className="hub-card-sub mono">
                  {hub.info.docs_dir}/{docsDirName(pr.gh_owner, pr.gh_repo)}
                </span>
              )}
            </dd>
          </div>
        </dl>
      ) : (
        <p className="muted hub-card-empty">
          Este hub não tem docs dos projetos (anterior ao protocolo 1.8): atualize o hub para configurar a pasta de docs.
        </p>
      )}
    </section>
  );
}
