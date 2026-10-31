import { useEffect, useRef, useState, type KeyboardEvent } from "react";
import type { JiraFieldInfo } from "../../protocol/hub-gen";
import {
  MAX_EXTRA_FIELDS,
  catalogField,
  extraFieldErrors,
  searchJiraFields,
  typedFieldId,
  type JiraExtraFieldForm,
} from "../../hub/tracker";
import { Icon } from "../../shell/icons";

/**
 * Combobox "Campo do Jira" (padrão ARIA 1.2: input com `role=combobox` e
 * lista `role=listbox`), com busca no catálogo de campos do jira-cli (#72)
 * por nome ou ID. Sem catálogo, ou com um `customfield_…` fora dele, o texto
 * digitado vira o ID.
 */
function FieldCombobox({
  inputId,
  n,
  value,
  catalog,
  used,
  describedBy,
  invalid,
  onPick,
  onTyped,
}: {
  inputId: string;
  /** Número da linha (1, 2...), para os rótulos acessíveis. */
  n: number;
  /** O ID gravado na linha. */
  value: string;
  catalog: readonly JiraFieldInfo[];
  /** IDs das outras linhas, em minúsculas: aparecem desabilitados. */
  used: ReadonlySet<string>;
  describedBy: string;
  invalid: boolean;
  onPick: (f: JiraFieldInfo) => void;
  onTyped: (id: string) => void;
}) {
  const [text, setText] = useState(() => catalogField(catalog, value)?.name ?? value);
  const [open, setOpen] = useState(false);
  const [active, setActive] = useState(-1);
  const listId = `${inputId}-list`;
  const hasCatalog = catalog.length > 0;
  const { options, more } = searchJiraFields(catalog, text);
  const isUsed = (f: JiraFieldInfo) => used.has(f.id.toLowerCase());
  const expanded = open && hasCatalog;
  const chosen = catalogField(catalog, value);

  useEffect(() => {
    if (active >= 0) document.getElementById(`${listId}-${active}`)?.scrollIntoView?.({ block: "nearest" });
  }, [active, listId]);

  const pick = (f: JiraFieldInfo) => {
    if (isUsed(f)) return;
    setText(f.name);
    setOpen(false);
    setActive(-1);
    onPick(f);
  };

  /** Próxima opção habilitada a partir de `from`, no sentido `step`. */
  const move = (from: number, step: 1 | -1) => {
    for (let i = from + step; i >= 0 && i < options.length; i += step) {
      if (!isUsed(options[i])) return i;
    }
    return from;
  };

  const onKeyDown = (e: KeyboardEvent<HTMLInputElement>) => {
    if (!hasCatalog) return;
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      const step = e.key === "ArrowDown" ? 1 : -1;
      if (!open) {
        setOpen(true);
        if (!e.altKey) setActive(move(step === 1 ? -1 : options.length, step));
        return;
      }
      setActive((a) => move(a < 0 && step === -1 ? options.length : a, step));
    } else if (e.key === "Enter") {
      if (expanded && active >= 0 && options[active]) {
        e.preventDefault();
        pick(options[active]);
      }
    } else if (e.key === "Escape") {
      if (expanded) {
        // Fecha só a lista, sem fechar o formulário ou o diálogo em volta.
        e.preventDefault();
        e.stopPropagation();
        setOpen(false);
        setActive(-1);
      }
    }
  };

  return (
    <div className="hub-jx-combo">
      <input
        id={inputId}
        type="text"
        role="combobox"
        autoComplete="off"
        spellCheck={false}
        aria-label={`Campo do Jira ${n}`}
        aria-autocomplete="list"
        aria-expanded={expanded}
        aria-controls={listId}
        aria-activedescendant={expanded && active >= 0 ? `${listId}-${active}` : undefined}
        aria-describedby={describedBy}
        aria-invalid={invalid || undefined}
        placeholder={hasCatalog ? "Buscar campo (nome ou customfield_…)" : "customfield_…"}
        value={text}
        onChange={(e) => {
          setText(e.target.value);
          setOpen(true);
          setActive(-1);
          onTyped(typedFieldId(catalog, e.target.value));
        }}
        onClick={() => setOpen(true)}
        onKeyDown={onKeyDown}
        onBlur={() => {
          setOpen(false);
          setActive(-1);
        }}
      />
      {value.trim() && (chosen || hasCatalog) && (
        <span className="hint muted mono hub-jx-chosen">
          {value.trim()} · {chosen ? chosen.datatype || "sem tipo" : "fora do catálogo"}
        </span>
      )}
      <div className="hub-jx-pop" hidden={!expanded}>
        <ul id={listId} role="listbox" aria-label={`Campos do Jira (linha ${n})`}>
          {options.map((f, i) => {
            const off = isUsed(f);
            return (
              <li
                key={f.id}
                id={`${listId}-${i}`}
                role="option"
                aria-selected={i === active}
                aria-disabled={off || undefined}
                className="hub-jx-opt"
                // Sem tirar o foco do input (o blur fecharia a lista antes do clique).
                onMouseDown={(e) => e.preventDefault()}
                onClick={() => pick(f)}
              >
                <span>{f.name}</span>
                <span className="mono muted hub-jx-meta">
                  {f.id}
                  {f.datatype ? ` · ${f.datatype}` : ""}
                  {off ? " · já na lista" : ""}
                </span>
              </li>
            );
          })}
        </ul>
        {options.length === 0 && (
          <p className="hint muted hub-jx-note">
            Nenhum campo com “{text.trim()}”.
            {value.trim() ? " O ID digitado será usado." : " Digite o ID (customfield_…)."}
          </p>
        )}
        {more > 0 && (
          <p className="hint muted hub-jx-note">
            Mais {more} {more === 1 ? "campo" : "campos"}: refine a busca.
          </p>
        )}
      </div>
    </div>
  );
}

/**
 * Campos do projeto na tela da issue (#68): pares "Campo do Jira" (combobox
 * do catálogo) e "Rótulo na tela", com adicionar e remover, até
 * `MAX_EXTRA_FIELDS`.
 */
export function JiraExtraFields({
  prefix,
  rows,
  catalog,
  onChange,
}: {
  prefix: string;
  rows: JiraExtraFieldForm[];
  catalog: readonly JiraFieldInfo[];
  onChange: (rows: JiraExtraFieldForm[]) => void;
}) {
  // Chave estável de cada linha (o texto da combobox vive nela).
  const seq = useRef(0);
  const keys = useRef<number[]>([]);
  if (keys.current.length !== rows.length) keys.current = rows.map(() => seq.current++);
  // Depois de incluir ou remover, o foco vai para a linha certa.
  const [focus, setFocus] = useState<{ id: string } | null>(null);
  useEffect(() => {
    if (focus) document.getElementById(focus.id)?.focus();
  }, [focus]);

  const base = `${prefix}-jx`;
  const hintId = `${base}-hint`;
  const errors = extraFieldErrors(rows);
  const lower = rows.map((r) => r.id.trim().toLowerCase());
  const full = rows.length >= MAX_EXTRA_FIELDS;
  const inputId = (i: number) => `${base}-${keys.current[i]}-field`;

  const set = (i: number, patch: Partial<JiraExtraFieldForm>) =>
    onChange(rows.map((r, j) => (j === i ? { ...r, ...patch } : r)));
  const add = () => {
    if (full) return;
    keys.current = [...keys.current, seq.current++];
    setFocus({ id: `${base}-${keys.current[rows.length]}-field` });
    onChange([...rows, { id: "", label: "" }]);
  };
  const remove = (i: number) => {
    keys.current = keys.current.filter((_, j) => j !== i);
    const next = rows.filter((_, j) => j !== i);
    setFocus({
      id: next.length === 0 ? `${base}-add` : `${base}-${keys.current[Math.min(i, next.length - 1)]}-field`,
    });
    onChange(next);
  };

  return (
    <fieldset className="hub-wide hub-docs hub-jx">
      <legend>Campos do projeto na tela da issue</legend>
      <p id={hintId} className="hint muted hub-wide">
        Situação, Tipo, Prioridade, Responsável e Criador já aparecem sempre. Busque pelo nome ou pelo ID. O rótulo
        começa com o nome do campo no Jira e pode ser trocado. Mudar a lista faz o ambiente reler todas as issues.
        {catalog.length === 0 && (
          <>
            {" "}
            A lista de campos vem do <span className="mono">.config.yml</span> do jira-cli no ambiente de sync, e ainda
            não chegou: rodar <span className="mono">jira init</span> no ambiente atualiza. Enquanto isso, digite o ID (
            <span className="mono">customfield_…</span>).
          </>
        )}
      </p>
      {rows.length > 0 && (
        <div className="hub-jx-rows hub-wide">
          <div className="hub-jx-row hub-jx-head" aria-hidden="true">
            <span>Campo do Jira</span>
            <span>Rótulo na tela</span>
          </div>
          {rows.map((r, i) => {
            const n = i + 1;
            const key = keys.current[i];
            const err = errors[i];
            const errId = `${base}-${key}-err`;
            const others = new Set(lower.filter((id, j) => j !== i && id));
            const labelInvalid = !!err && !!r.id.trim() && !r.label.trim();
            const fieldInvalid = !!err && !labelInvalid;
            return (
              <div key={key} className="hub-jx-row" role="group" aria-label={`Campo ${n}`}>
                <FieldCombobox
                  inputId={inputId(i)}
                  n={n}
                  value={r.id}
                  catalog={catalog}
                  used={others}
                  describedBy={err ? `${errId} ${hintId}` : hintId}
                  invalid={fieldInvalid}
                  onPick={(f) =>
                    set(i, {
                      id: f.id,
                      label: r.label.trim() ? r.label : f.name,
                    })
                  }
                  onTyped={(id) => set(i, { id })}
                />
                <input
                  type="text"
                  aria-label={`Rótulo na tela ${n}`}
                  placeholder="Rótulo"
                  value={r.label}
                  aria-invalid={labelInvalid || undefined}
                  aria-describedby={labelInvalid ? errId : undefined}
                  onChange={(e) => set(i, { label: e.target.value })}
                />
                <button
                  type="button"
                  className="btn hub-jx-remove"
                  aria-label={`Remover campo ${n}`}
                  onClick={() => remove(i)}
                >
                  <Icon name="close" />
                </button>
                {err && (
                  <span id={errId} className="hint start-error hub-jx-err">
                    {err}
                  </span>
                )}
              </div>
            );
          })}
        </div>
      )}
      <div className="hub-jx-actions hub-wide">
        <button type="button" id={`${base}-add`} className="btn" disabled={full} onClick={add}>
          + Adicionar campo
        </button>
        <span className="hint muted" aria-live="polite">
          {rows.length} de {MAX_EXTRA_FIELDS}
          {full ? " (limite)" : ""}
        </span>
      </div>
    </fieldset>
  );
}
