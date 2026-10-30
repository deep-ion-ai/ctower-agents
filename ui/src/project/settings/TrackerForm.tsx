import {
  DEFAULT_MAX_ISSUES,
  JIRA_BRANCH_PATTERN,
  MAX_ISSUES_CEILING,
  branchPreview,
  type JiraForm,
} from "../../hub/tracker";
import type { ProjectForm } from "./form";

interface FieldsProps {
  form: ProjectForm;
  onChange: (f: ProjectForm) => void;
  prefix: string;
}

/** Campo de texto do Jira + GitLab, guardado em `f.jira`. */
function JiraText({
  form,
  onChange,
  prefix,
  k,
  label,
  placeholder = "",
  wide = false,
  hint = "",
}: FieldsProps & { k: keyof JiraForm; label: string; placeholder?: string; wide?: boolean; hint?: string }) {
  return (
    <div className={`field${wide ? " hub-wide" : ""}`}>
      <label htmlFor={`${prefix}-${k}`}>{label}</label>
      <input
        id={`${prefix}-${k}`}
        type="text"
        placeholder={placeholder}
        value={String(form.jira[k])}
        aria-describedby={hint ? `${prefix}-${k}-hint` : undefined}
        onChange={(e) => onChange({ ...form, jira: { ...form.jira, [k]: e.target.value } })}
      />
      {hint && (
        <span id={`${prefix}-${k}-hint`} className="hint muted">
          {hint}
        </span>
      )}
    </div>
  );
}

/** Campos do GitLab (#348): a URL de clone e o comando de credencial. */
export function GitlabFields(p: FieldsProps) {
  return (
    <>
      <JiraText {...p} k="gitlab_url" label="URL do repositório no GitLab (HTTPS ou SSH)" placeholder="https://gitlab.exemplo.com/grupo/projeto.git" wide />
      <JiraText {...p} k="gitlab_cmd" label="Comando de credencial do GitLab" placeholder="pass show gitlab/token | head -1" wide />
    </>
  );
}

/**
 * Campos do Jira (#348), com o padrão de branch e a atribuição do projeto
 * Jira + GitLab. Credenciais: só o comando, que roda no ambiente; a senha
 * e o token nunca passam pelo hub.
 */
export function JiraFields(p: FieldsProps) {
  const { form: f, onChange, prefix } = p;
  const j = f.jira;
  const setJ = (patch: Partial<JiraForm>) => onChange({ ...f, jira: { ...j, ...patch } });
  const pattern = j.branch_pattern.trim() || JIRA_BRANCH_PATTERN;
  const preview = branchPreview(pattern, j.jira_key.trim().toUpperCase() || "PROJ");
  return (
    <>
      <JiraText {...p} k="jira_url" label="URL do Jira" placeholder="https://jira.exemplo.com" />
      <JiraText {...p} k="jira_key" label="Chave do projeto no Jira" placeholder="PROJ" />
      <div className="field hub-wide">
        <label className="ctl" htmlFor={`${prefix}-active_sprint_only`}>
          <input
            id={`${prefix}-active_sprint_only`}
            type="checkbox"
            checked={j.active_sprint_only}
            aria-describedby={`${prefix}-active_sprint_only-hint`}
            onChange={(e) => setJ({ active_sprint_only: e.target.checked })}
          />
          Sincronizar somente a sprint ativa
        </label>
        <span id={`${prefix}-active_sprint_only-hint`} className="hint muted">
          {j.active_sprint_only
            ? "O sync traz só as issues das sprints abertas do projeto (sprint in openSprints()), sem limite de quantidade."
            : "Desmarcado, informe o JQL do que sincronizar e o limite de issues."}
        </span>
      </div>
      {!j.active_sprint_only && (
        <>
          <JiraText
            {...p}
            k="jql"
            label="JQL do sync"
            placeholder="ex.: labels = minha-equipe AND updated >= -90d"
            wide
            hint="Obrigatório: o que é sincronizado do projeto (o primeiro sync lê issue por issue). Mudar o filtro remove do Control Tower as issues que saem dele, exceto as com work item (e os épicos delas)."
          />
          <div className="field">
            <label htmlFor={`${prefix}-max_issues`}>Limite de issues sincronizadas</label>
            <input
              id={`${prefix}-max_issues`}
              type="number"
              min={1}
              max={MAX_ISSUES_CEILING}
              step={1}
              value={j.max_issues}
              aria-describedby={`${prefix}-max_issues-hint`}
              onChange={(e) => setJ({ max_issues: e.target.value })}
            />
            <span id={`${prefix}-max_issues-hint`} className="hint muted">
              Só as {j.max_issues.trim() || DEFAULT_MAX_ISSUES} issues mais recentes do JQL (por atualização) ficam no
              Control Tower, além das que têm work item. Padrão {DEFAULT_MAX_ISSUES}, máximo {MAX_ISSUES_CEILING}.
            </span>
          </div>
        </>
      )}
      <JiraText {...p} k="jira_cmd" label="Comando de credencial do Jira" placeholder="pass show jira/token | head -1" wide />
      <div className="field">
        <label htmlFor={`${prefix}-branch_pattern`}>Padrão de branch</label>
        <input
          id={`${prefix}-branch_pattern`}
          type="text"
          placeholder={JIRA_BRANCH_PATTERN}
          value={j.branch_pattern}
          aria-describedby={`${prefix}-bp-prev`}
          onChange={(e) => setJ({ branch_pattern: e.target.value })}
        />
        <span id={`${prefix}-bp-prev`} className="hint muted">
          {preview ? (
            <>
              Prévia: <span className="mono">{preview}</span>
            </>
          ) : (
            "Padrão inválido: use {prefix}, {n}, {key} e {slug}, com {n} ou {key}."
          )}
        </span>
      </div>
      <div className="field">
        <label className="ctl" htmlFor={`${prefix}-attr`}>
          <input
            id={`${prefix}-attr`}
            type="checkbox"
            checked={j.attribution}
            onChange={(e) => setJ({ attribution: e.target.checked })}
          />
          Linhas de atribuição nos commits e MRs
        </label>
      </div>
      <JiraText {...p} k="type_map" label="Mapa de tipos do Jira (Tipo=papel)" placeholder="Epic=epic, Bug=bug" wide />
      <JiraText {...p} k="epic_link_field" label="Campo do Epic Link (opcional)" placeholder="customfield_10601" />
      <JiraText {...p} k="epic_name_field" label="Campo do Epic Name (opcional)" placeholder="customfield_10604" />
      <JiraText
        {...p}
        k="sprint_field"
        label="Campo da sprint (opcional)"
        placeholder="customfield_10600 (vazio = detectar)"
        hint="Permite filtrar a árvore e o quadro por sprint. Vazio = o ambiente procura o campo de sprint em cada issue."
      />
      <p className="muted hint hub-wide">
        O hub não alcança o Jira nem o GitLab: guarda só os comandos de credencial, que rodam no ambiente de sync.
        Ele confere tudo no primeiro ciclo de sync e relata aqui.
      </p>
    </>
  );
}
