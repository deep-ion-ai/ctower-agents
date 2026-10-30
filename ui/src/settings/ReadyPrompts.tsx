// Prompts prontos do console nas Configurações (#425; spec da #87, §2.4):
// nome e texto de cada um, na ordem do menu, com "Restaurar padrão" nos
// padrões, subir/descer, remover e adicionar. As edições ficam num rascunho
// até "Salvar prompts" (como o editor de template).
import { useEffect, useRef, useState } from "react";
import type { ReadyPrompt } from "../api/types";
import { useTower } from "../state/tower";
import { DEFAULT_PROMPTS } from "./defaults";

const defaultOf = (id: string) => DEFAULT_PROMPTS.find((p) => p.id === id);

/** A lista difere da padrão (ordem, itens, nomes ou textos). */
export function promptsChanged(list: ReadyPrompt[]): boolean {
  return JSON.stringify(list) !== JSON.stringify(DEFAULT_PROMPTS);
}

/** O que impede salvar: nome ou texto vazio, ou nome repetido. */
export function promptsProblem(list: ReadyPrompt[]): string | null {
  if (list.some((p) => !p.name.trim())) return "Todo prompt precisa de um nome.";
  if (list.some((p) => !p.text.trim())) return "Todo prompt precisa de um texto.";
  const names = list.map((p) => p.name.trim());
  if (new Set(names).size !== names.length) return "Há dois prompts com o mesmo nome.";
  return null;
}

/** Id novo, que não colide com os padrões nem com os da lista. */
function newId(list: ReadyPrompt[]): string {
  for (let n = list.length + 1; ; n++) {
    const id = `meu-${n}`;
    if (!list.some((p) => p.id === id) && !defaultOf(id)) return id;
  }
}

export function ReadyPromptsControl() {
  const { config, setConfig } = useTower();
  const [draft, setDraft] = useState(config.prompts);
  const [saved, setSaved] = useState(false);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => setDraft(config.prompts), [config.prompts]);
  const dirty = JSON.stringify(draft) !== JSON.stringify(config.prompts);
  const problem = promptsProblem(draft);
  // Foco no nome do prompt recém-adicionado.
  const added = useRef<string | null>(null);

  const edit = (next: ReadyPrompt[]) => {
    setDraft(next);
    setSaved(false);
    setError(null);
  };
  const patch = (i: number, p: Partial<ReadyPrompt>) => edit(draft.map((x, j) => (j === i ? { ...x, ...p } : x)));
  const move = (i: number, d: -1 | 1) => {
    const next = [...draft];
    [next[i], next[i + d]] = [next[i + d], next[i]];
    edit(next);
  };
  const add = () => {
    const id = newId(draft);
    added.current = id;
    edit([...draft, { id, name: "", text: "" }]);
  };
  const save = async () => {
    try {
      await setConfig({ ...config, prompts: draft.map((p) => ({ ...p, name: p.name.trim() })) });
      setSaved(true);
    } catch (e) {
      setError(String(e));
    }
  };

  return (
    <div className="set-prompts">
      {draft.length === 0 && <p className="muted">Nenhum prompt: o botão do console fica desligado.</p>}
      <ol className="set-prompt-list">
        {draft.map((p, i) => {
          const def = defaultOf(p.id);
          const changed = !!def && (def.name !== p.name || def.text !== p.text);
          const label = p.name.trim() || `Prompt ${i + 1}`;
          return (
            <li key={p.id} className="set-prompt" aria-label={label}>
              <div className="field">
                <label htmlFor={`prompt-${p.id}-name`}>Nome</label>
                <input
                  id={`prompt-${p.id}-name`}
                  className="set-input"
                  value={p.name}
                  ref={(el) => {
                    if (el && added.current === p.id) {
                      added.current = null;
                      el.focus();
                    }
                  }}
                  onChange={(e) => patch(i, { name: e.target.value })}
                />
              </div>
              <div className="field">
                <label htmlFor={`prompt-${p.id}-text`}>Texto</label>
                <textarea
                  id={`prompt-${p.id}-text`}
                  className="mono"
                  rows={Math.min(8, Math.max(2, p.text.split("\n").length + 1))}
                  value={p.text}
                  onChange={(e) => patch(i, { text: e.target.value })}
                />
              </div>
              <div className="set-inline">
                <button
                  type="button"
                  className="btn btn-sm"
                  disabled={i === 0}
                  aria-label={`Subir ${label}`}
                  onClick={() => move(i, -1)}
                >
                  Subir
                </button>
                <button
                  type="button"
                  className="btn btn-sm"
                  disabled={i === draft.length - 1}
                  aria-label={`Descer ${label}`}
                  onClick={() => move(i, 1)}
                >
                  Descer
                </button>
                <button
                  type="button"
                  className="btn btn-sm"
                  aria-label={`Remover ${label}`}
                  onClick={() => edit(draft.filter((_, j) => j !== i))}
                >
                  Remover
                </button>
                {changed && (
                  <button type="button" className="link-btn" onClick={() => patch(i, { name: def.name, text: def.text })}>
                    Restaurar padrão<span className="sr-only"> de {def.name}</span>
                  </button>
                )}
              </div>
            </li>
          );
        })}
      </ol>
      <span className="muted hint">
        Variáveis dos templates: <span className="mono">{"{{kind}} {{number}} {{key}} {{title}} {{owner}} {{repo}}"}</span>,{" "}
        <span className="mono">{"{{branch}} {{base}} {{protected}}"}</span> (no gestor,{" "}
        <span className="mono">{"{{env}} {{scope}}"}</span>). O texto vai colado inteiro e depois o Enter.
      </span>
      <div className="form-actions">
        {saved && !dirty && <span className="state-on">Prompts salvos.</span>}
        {dirty && problem && <span className="state-off">{problem}</span>}
        <button type="button" className="btn" onClick={add}>
          Adicionar prompt
        </button>
        <button type="button" className="btn" disabled={!dirty} onClick={() => edit(config.prompts)}>
          Descartar
        </button>
        <button type="button" className="btn btn-primary" disabled={!dirty || !!problem} onClick={() => void save()}>
          Salvar prompts
        </button>
      </div>
      {error && (
        <p role="alert" className="start-error">
          {error}
        </p>
      )}
    </div>
  );
}
