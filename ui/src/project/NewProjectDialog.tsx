import { useState } from "react";
import { useTower } from "../state/tower";
import type { ProjectAdded } from "../protocol/hub-gen";
import { hostsOf, hubSupportsJira, kindOf, providerParams } from "../hub/tracker";
import type { ProjectRef } from "../hub/sections";
import { EMPTY_PROJECT_FORM, GeneralForm, branches, type ProjectForm, type SettingsRun } from "./settings";

/** Cadastrar projeto com a URL preenchida (vinda da quarentena). Cada pedido (objeto novo) abre o formulário. */
export interface RegisterIntent {
  url: string;
  /** Tipo do projeto no formulário; ausente = GitHub. Jira + GitLab vem sem URL (#352). */
  kind?: "github" | "jira";
}

/**
 * Formulário de cadastro de projeto no hub (spec §3.1), preenchido pelo
 * pedido (`intent`) e usado por quem o abre: o índice de Projetos e a
 * quarentena. `onDone` roda ao cadastrar (com o projeto novo) e ao cancelar.
 * O erro e a mensagem de sucesso ficam com quem hospeda (`settings`).
 */
export function NewProjectDialog({
  intent,
  settings,
  onDone,
}: {
  intent?: RegisterIntent | null;
  settings: SettingsRun;
  onDone: (added?: ProjectRef) => void;
}) {
  const { hub, api } = useTower();
  const jiraOk = hubSupportsJira(hub.info?.protocol ?? null);
  const [form, setForm] = useState<ProjectForm>({
    ...EMPTY_PROJECT_FORM,
    ...hostsOf(intent?.kind ?? "github"),
    url: intent?.url ?? "",
  });

  const save = async () => {
    let added: ProjectRef | undefined;
    const ok = await settings.run(async () => {
      // Jira + GitLab: sem URL do GitHub; o repositório vem da URL do GitLab.
      let source: Record<string, unknown> = { url: form.url.trim() };
      if (kindOf(form.code, form.issues) === "jira") {
        const pv = providerParams(form.jira);
        if ("error" in pv) throw new Error(pv.error);
        source = {
          ...pv,
          ...(form.jira.branch_pattern.trim() ? { branch_pattern: form.jira.branch_pattern.trim() } : {}),
          attribution: form.jira.attribution,
        };
      }
      const r = await api.hubCall<ProjectAdded>("hub.projects.add", {
        ...source,
        ...(form.name.trim() ? { name: form.name.trim() } : {}),
        ...(form.base_branch.trim() ? { base_branch: form.base_branch.trim() } : {}),
        ...(form.protected.trim() ? { protected_branches: branches(form.protected) } : {}),
      });
      added = { owner: r.project.gh_owner, repo: r.project.gh_repo };
      const extra = r.reapplied > 0 ? ` ${r.reapplied} linha(s) da quarentena reaplicada(s).` : "";
      return `${r.project.gh_owner}/${r.project.gh_repo} cadastrado no hub.${extra}`;
    });
    if (ok) onDone(added);
  };

  const kind = kindOf(form.code, form.issues);
  const incomplete =
    kind === "github"
      ? !form.url.trim()
      : kind === "jira"
        ? !jiraOk || !form.jira.jira_url.trim() || !form.jira.jira_key.trim() || !form.jira.gitlab_url.trim()
        : true;

  return (
    <form
      className="env-form"
      aria-label="Novo projeto"
      onSubmit={(e) => {
        e.preventDefault();
        void save();
      }}
    >
      <h2 className="hub-ptitle hub-wide">Cadastrar projeto</h2>
      <GeneralForm form={form} onChange={setForm} prefix="np" withUrl />
      <div className="form-actions">
        <button type="button" className="btn" onClick={() => onDone()}>
          Cancelar
        </button>
        <button type="submit" className="btn btn-primary" disabled={settings.busy || incomplete}>
          {settings.busy ? "Validando…" : "Cadastrar"}
        </button>
      </div>
    </form>
  );
}
