import { useState } from "react";
import { useTower } from "../../state/tower";
import type { HubProjectView } from "../../protocol/hub-gen";
import {
  GITHUB_BRANCH_PATTERN,
  JIRA_BRANCH_PATTERN,
  branchPreview,
  gitlabOf,
  hostsOf,
  isJira,
  jiraFormOf,
  jiraOf,
  providerParams,
  DEFAULT_MAX_ISSUES,
  type JiraForm,
} from "../../hub/tracker";
import { docsDirName, docsFormOf, docsPatch, docsText } from "../../hub/docs";
import { DocsForm } from "./DocsForm";
import { GeneralForm } from "./GeneralForm";
import { branches, type ProjectForm } from "./form";
import type { SettingsRun } from "./run";
import { syncText } from "./sync";

/** Formulário de edição do projeto, preenchido com o que o hub tem hoje. */
export function projectEditForm(p: HubProjectView): ProjectForm {
  const pr = p.project;
  return {
    ...hostsOf(isJira(pr) ? "jira" : "github"),
    url: "",
    name: pr.name,
    base_branch: pr.base_branch,
    protected: pr.protected_branches.join(", "),
    jira: jiraFormOf(pr),
    docs: docsFormOf(pr),
  };
}

/** Posição do projeto na ordem entre projetos e como mudá-la. */
export interface ProjectOrder {
  pos: number;
  total: number;
  canUp: boolean;
  canDown: boolean;
  onStep: (up: boolean) => void;
}

/**
 * Repositório do projeto: os dados de hoje e, com `editing`, o formulário de
 * edição (aberto pelo "Editar" de quem hospeda a seção).
 */
export function RepositoryCard({
  project: p,
  editing,
  onEditingChange,
  settings,
  order,
  now,
  readOnly = false,
}: {
  project: HubProjectView;
  editing: boolean;
  onEditingChange: (editing: boolean) => void;
  settings: SettingsRun;
  order: ProjectOrder;
  now: number;
  /** Web (épico #205): sem reordenar. */
  readOnly?: boolean;
}) {
  const { hub } = useTower();
  const pr = p.project;
  const jira = jiraOf(pr);
  const gitlab = gitlabOf(pr);
  const sync = syncText(p.sync, now);
  const syncName = pr.sync_env_id ? (p.assignments.find((a) => a.env_id === pr.sync_env_id)?.env_name ?? pr.sync_env_id) : null;
  return (
    <section className="hub-card" aria-labelledby={`pc-repo-${pr.id}`}>
      <div className="hub-card-head">
        <h3 id={`pc-repo-${pr.id}`}>Repositório</h3>
      </div>
      {editing ? (
        <RepositoryEdit project={p} settings={settings} onDone={() => onEditingChange(false)} />
      ) : (
        <dl className="hub-kv">
          {jira && (
            <>
              <div>
                <dt>Jira</dt>
                <dd>
                  <span className="mono">{jira.url}</span> · projeto <span className="mono">{jira.project_key}</span>
                  {jira.active_sprint_only ? (
                    <span className="hub-card-sub">Sync: somente a sprint ativa</span>
                  ) : (
                    <span className="hub-card-sub">
                      {jira.jql ? <span className="mono">JQL: {jira.jql}</span> : "Sem JQL (cadastro anterior)"}
                      {` · até ${jira.max_issues ?? DEFAULT_MAX_ISSUES} issues`}
                    </span>
                  )}
                </dd>
              </div>
              <div>
                <dt>GitLab</dt>
                <dd className="mono">{gitlab ? gitlab.clone_url || `${gitlab.host}/${gitlab.path}` : "—"}</dd>
              </div>
              <div>
                <dt>Credenciais (no ambiente)</dt>
                <dd>
                  <span className="mono">{jira.credential_cmd}</span>
                  {gitlab && <span className="hub-card-sub mono">{gitlab.credential_cmd}</span>}
                </dd>
              </div>
              <div>
                <dt>Padrão de branch</dt>
                <dd>
                  <span className="mono">{pr.branch_pattern ?? JIRA_BRANCH_PATTERN}</span>
                  <span className="hub-card-sub">
                    ex.: <span className="mono">{branchPreview(pr.branch_pattern ?? JIRA_BRANCH_PATTERN, jira.project_key)}</span>
                    {pr.attribution ? "" : " · sem linhas de atribuição"}
                  </span>
                </dd>
              </div>
            </>
          )}
          {!jira && pr.branch_pattern && pr.branch_pattern !== GITHUB_BRANCH_PATTERN && (
            <div>
              <dt>Padrão de branch</dt>
              <dd className="mono">{pr.branch_pattern}</dd>
            </div>
          )}
          <div>
            <dt>Branch base</dt>
            <dd className="mono">{pr.base_branch}</dd>
          </div>
          <div>
            <dt>Protegidas</dt>
            <dd className="mono">{pr.protected_branches.join(", ")}</dd>
          </div>
          {pr.docs && (
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
          )}
          <div>
            <dt>Sincronização</dt>
            <dd>
              <span className="hub-dot" data-tone={sync.tone} aria-hidden="true" /> {sync.text}
              {jira && <span className="muted">{syncName ? ` · pelo ambiente de sync ${syncName}` : " · sem ambiente de sync"}</span>}
              {p.sync?.last_error && <span className="hub-card-sub">{p.sync.last_error}</span>}
            </dd>
          </div>
          <div>
            <dt>Ordem entre projetos</dt>
            <dd className="hub-order">
              <span className="mono">
                #{order.pos} de {order.total}
              </span>
              {!readOnly && (
                <>
                  <button
                    type="button"
                    className="btn btn-sm"
                    disabled={settings.busy || !order.canUp}
                    aria-label={`Subir ${pr.name} na ordem dos projetos`}
                    onClick={() => order.onStep(true)}
                  >
                    ↑
                  </button>
                  <button
                    type="button"
                    className="btn btn-sm"
                    disabled={settings.busy || !order.canDown}
                    aria-label={`Descer ${pr.name} na ordem dos projetos`}
                    onClick={() => order.onStep(false)}
                  >
                    ↓
                  </button>
                </>
              )}
            </dd>
          </div>
        </dl>
      )}
    </section>
  );
}

function RepositoryEdit({
  project: p,
  settings,
  onDone,
}: {
  project: HubProjectView;
  settings: SettingsRun;
  onDone: () => void;
}) {
  const pr = p.project;
  const [form, setForm] = useState(() => projectEditForm(p));
  const { api } = useTower();

  const update = async () => {
    const cur = p.project;
    const f = form;
    const patch: Record<string, unknown> = {
      ...(f.name.trim() && f.name.trim() !== cur.name ? { name: f.name.trim() } : {}),
      ...(f.base_branch.trim() && f.base_branch.trim() !== cur.base_branch ? { base_branch: f.base_branch.trim() } : {}),
      ...(branches(f.protected).join(",") !== cur.protected_branches.join(",")
        ? { protected_branches: branches(f.protected) }
        : {}),
    };
    if (isJira(cur)) {
      // Rastreador e forja vão inteiros quando algo deles mudou; o tipo e o
      // repositório não mudam (o hub recusa).
      const was = jiraFormOf(cur);
      const j = f.jira;
      const same = (keys: (keyof JiraForm)[]) => keys.every((k) => String(j[k]).trim() === String(was[k]).trim());
      if (!same(["jira_url", "jira_key", "active_sprint_only", "jql", "max_issues", "jira_cmd", "type_map", "epic_link_field", "epic_name_field", "sprint_field", "gitlab_url", "gitlab_cmd"])) {
        const pv = providerParams(j);
        if ("error" in pv) {
          settings.fail(pv.error);
          return;
        }
        Object.assign(patch, pv);
      }
      if (j.branch_pattern.trim() !== was.branch_pattern.trim()) patch.branch_pattern = j.branch_pattern.trim();
      if (j.attribution !== was.attribution) patch.attribution = j.attribution;
    }
    // Docs (#444): o objeto inteiro, se mudou; hub sem `docs` não recebe.
    const docs = docsPatch(f.docs, cur);
    if (docs) patch.docs = docs;
    if (Object.keys(patch).length === 0) {
      onDone();
      return;
    }
    const ok = await settings.run(async () => {
      await api.hubCall("hub.projects.update", { owner: cur.gh_owner, repo: cur.gh_repo, ...patch });
      return `${cur.gh_owner}/${cur.gh_repo} atualizado; os ambientes recebem no próximo pull.`;
    });
    if (ok) onDone();
  };

  return (
    <form
      className="env-form hub-card-form"
      aria-label={`Editar ${pr.name}`}
      onSubmit={(e) => {
        e.preventDefault();
        void update();
      }}
    >
      <GeneralForm form={form} onChange={setForm} prefix={`ep-${pr.id}`} withUrl={false} />
      <DocsForm form={form} onChange={setForm} prefix={`ep-${pr.id}`} />
      <div className="form-actions">
        <button type="button" className="btn" onClick={onDone}>
          Cancelar
        </button>
        <button type="submit" className="btn btn-primary" disabled={settings.busy}>
          {settings.busy ? "Salvando…" : "Salvar projeto"}
        </button>
      </div>
    </form>
  );
}
