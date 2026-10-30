import { useTower } from "../../state/tower";
import type { DocsForm as DocsFormValue } from "../../hub/docs";
import type { ProjectForm } from "./form";

/**
 * Docs do projeto (#444): liga e desliga e os modelos de pastas, uma por
 * linha. Hub anterior à #440 não manda `docs`: só o aviso.
 */
export function DocsForm({
  form: f,
  onChange,
  prefix,
}: {
  form: ProjectForm;
  onChange: (f: ProjectForm) => void;
  prefix: string;
}) {
  const { hub } = useTower();
  const d = f.docs;
  if (!d) {
    return (
      <p className="muted hint hub-wide" role="note">
        Este hub não tem docs dos projetos (anterior ao protocolo 1.8): atualize o hub para configurar a pasta de docs.
      </p>
    );
  }
  const setD = (patch: Partial<DocsFormValue>) => onChange({ ...f, docs: { ...d, ...patch } });
  const area = (k: "project_dirs" | "epic_dirs", label: string, hint: string, placeholder: string) => (
    <div className="field hub-wide">
      <label htmlFor={`${prefix}-${k}`}>{label}</label>
      <textarea
        id={`${prefix}-${k}`}
        rows={3}
        placeholder={placeholder}
        value={d[k]}
        disabled={!d.enabled}
        aria-describedby={`${prefix}-${k}-hint`}
        onChange={(e) => setD({ [k]: e.target.value })}
      />
      <span id={`${prefix}-${k}-hint`} className="hint muted">
        {hint}
      </span>
    </div>
  );
  return (
    <fieldset className="hub-wide hub-docs">
      <legend>Docs do projeto (fora do repositório)</legend>
      <div className="field hub-wide">
        <label className="ctl" htmlFor={`${prefix}-docs`}>
          <input
            id={`${prefix}-docs`}
            type="checkbox"
            checked={d.enabled}
            onChange={(e) => setD({ enabled: e.target.checked })}
          />
          Pasta de docs do projeto
        </label>
        <span className="hint muted">
          {hub.info?.docs_dir
            ? <>Em <span className="mono">{hub.info.docs_dir}</span>, compartilhada com os ambientes pelo docs_root.</>
            : "O hub está sem docs_dir: as pastas só nascem nos ambientes com docs_root configurado."}{" "}
          Desligar não apaga nada.
        </span>
      </div>
      {area("project_dirs", "Pastas do projeto (em _projeto/)", "Uma por linha, caminho relativo (ex.: analises, specs/telas).", "analises\nspecs/telas")}
      {area("epic_dirs", "Pastas de cada épico (em epicos/<n>-<slug>/)", "Uma por linha; o README.md do épico é gerado à parte.", "prompts\nevidencias")}
    </fieldset>
  );
}
