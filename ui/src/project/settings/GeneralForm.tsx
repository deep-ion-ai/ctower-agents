import { useTower } from "../../state/tower";
import { JIRA_PROTOCOL, hubSupportsJira, kindOf, type CodeHostForm, type IssueHostForm } from "../../hub/tracker";
import type { ProjectForm } from "./form";
import { GitlabFields, JiraFields } from "./TrackerForm";

/**
 * Os dois repositórios do projeto, cada um com os campos dele. Na edição o
 * tipo não muda (o hub recusa): os seletores só mostram o atual.
 */
export function GeneralForm({
  form: f,
  onChange,
  prefix,
  withUrl,
}: {
  form: ProjectForm;
  onChange: (f: ProjectForm) => void;
  prefix: string;
  /** Cadastro: pede a URL e escolhe o tipo; na edição, só mostra. */
  withUrl: boolean;
}) {
  const { hub } = useTower();
  // Hub anterior ao protocolo 1.5 (épico #353) não cadastra Jira + GitLab (ignora `tracker` e `forge`).
  const protocol = hub.info?.protocol ?? null;
  const jiraOk = hubSupportsJira(protocol);
  const kind = kindOf(f.code, f.issues);
  return (
    <>
      <div className="field">
        <label htmlFor={`${prefix}-code`}>Repositório do código-fonte</label>
        <select
          id={`${prefix}-code`}
          value={f.code}
          disabled={!withUrl}
          onChange={(e) => onChange({ ...f, code: e.target.value as CodeHostForm })}
        >
          <option value="github">GitHub</option>
          <option value="gitlab" disabled={withUrl && !jiraOk}>
            GitLab
          </option>
        </select>
      </div>
      <div className="field">
        <label htmlFor={`${prefix}-issues`}>Repositório de demandas (issues)</label>
        <select
          id={`${prefix}-issues`}
          value={f.issues}
          disabled={!withUrl}
          onChange={(e) => onChange({ ...f, issues: e.target.value as IssueHostForm })}
        >
          <option value="github">GitHub</option>
          <option value="jira" disabled={withUrl && !jiraOk}>
            Jira
          </option>
        </select>
      </div>
      {withUrl && !jiraOk && (
        <p className="start-error hub-wide" role="note">
          O hub (protocolo {protocol}) não suporta Jira/GitLab; atualize o hub para {JIRA_PROTOCOL}+.
        </p>
      )}
      {withUrl && !kind && (
        <p className="start-error hub-wide" role="note">
          Combinação ainda não suportada: use GitHub + GitHub ou GitLab + Jira.
        </p>
      )}
      {withUrl && f.code === "github" && (
        <div className="field hub-wide">
          <label htmlFor={`${prefix}-url`}>URL do GitHub</label>
          <input
            id={`${prefix}-url`}
            type="text"
            placeholder="https://github.com/owner/repo"
            value={f.url}
            onChange={(e) => onChange({ ...f, url: e.target.value })}
          />
        </div>
      )}
      {f.code === "gitlab" && <GitlabFields form={f} onChange={onChange} prefix={prefix} />}
      {f.issues === "jira" && <JiraFields form={f} onChange={onChange} prefix={prefix} />}
      <div className="field">
        <label htmlFor={`${prefix}-name`}>Nome{withUrl ? " (opcional)" : ""}</label>
        <input id={`${prefix}-name`} type="text" value={f.name} onChange={(e) => onChange({ ...f, name: e.target.value })} />
      </div>
      <div className="field">
        <label htmlFor={`${prefix}-base`}>Branch base{withUrl ? " (padrão develop)" : ""}</label>
        <input
          id={`${prefix}-base`}
          type="text"
          value={f.base_branch}
          onChange={(e) => onChange({ ...f, base_branch: e.target.value })}
        />
      </div>
      <div className="field">
        <label htmlFor={`${prefix}-prot`}>Protegidas{withUrl ? " (padrão develop, main, master)" : ""}</label>
        <input
          id={`${prefix}-prot`}
          type="text"
          placeholder="develop, main"
          value={f.protected}
          onChange={(e) => onChange({ ...f, protected: e.target.value })}
        />
      </div>
    </>
  );
}
