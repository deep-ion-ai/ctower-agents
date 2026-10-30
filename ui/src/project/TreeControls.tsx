import type { Sprint, WorkItemStatus } from "../protocol/gen";
import { ACTIVE_SPRINT, ALL_SPRINTS, NO_SPRINT, sprintLabel } from "./sprints";
import { STATUS } from "./status";
import { SPRINTS_UNREAD, STATUS_ORDER, type SortKey, type TreeFilters } from "./tree-view";

export type ProjectView = "tree" | "board";

/**
 * Filtro da árvore no painel lateral: a busca sempre, e a sprint nos
 * projetos Jira; com `more`, os chips de status, a ordenação e "mostrar
 * concluídos".
 */
export function TreeControls({
  value,
  onChange,
  more,
  sprints = null,
  sprintsUnread = false,
}: {
  value: TreeFilters;
  onChange: (f: TreeFilters) => void;
  more: boolean;
  /** Sprints conhecidas do projeto Jira (da mais recente); `null` = projeto sem sprints (GitHub). */
  sprints?: readonly Sprint[] | null;
  /** Projeto Jira sem nenhuma issue com sprint no espelho: o aviso no lugar da lista vazia. */
  sprintsUnread?: boolean;
}) {
  const toggle = (k: WorkItemStatus) => {
    const chips = new Set(value.chips);
    if (!chips.delete(k)) chips.add(k);
    onChange({ ...value, chips });
  };
  return (
    <div className="tree-controls" role="group" aria-label="Filtros da árvore">
      <input
        type="search"
        className="tree-search"
        placeholder="Buscar #número ou título"
        aria-label="Buscar por número ou título"
        value={value.query}
        onChange={(e) => onChange({ ...value, query: e.target.value })}
      />
      {sprints && (
        <label className="ctl tree-sprint">
          Sprint
          <select
            aria-label="Filtrar por sprint"
            value={value.sprint ?? ALL_SPRINTS}
            onChange={(e) => onChange({ ...value, sprint: e.target.value })}
          >
            <option value={ALL_SPRINTS}>Todas</option>
            <option value={ACTIVE_SPRINT}>Sprint ativa</option>
            {sprints.map((s) => (
              <option key={s.id} value={String(s.id)}>
                {sprintLabel(s)}
              </option>
            ))}
            <option value={NO_SPRINT}>Sem sprint</option>
          </select>
        </label>
      )}
      {sprints && sprintsUnread && (
        <p className="hint muted tree-sprint-hint" role="note">
          {SPRINTS_UNREAD}
        </p>
      )}
      {more && (
        <div id="tree-more" className="tree-more">
          <div className="chips" role="group" aria-label="Filtrar por status">
            {STATUS_ORDER.map((k) => (
              <button key={k} type="button" className="chip" aria-pressed={value.chips.has(k)} onClick={() => toggle(k)}>
                {STATUS[k].text}
              </button>
            ))}
          </div>
          <label className="ctl">
            Ordenar
            <select value={value.sort} onChange={(e) => onChange({ ...value, sort: e.target.value as SortKey })}>
              <option value="activity">atividade recente</option>
              <option value="number">número</option>
              <option value="priority">prioridade</option>
            </select>
          </label>
          <label className="ctl">
            <input
              type="checkbox"
              checked={value.showDone}
              onChange={(e) => onChange({ ...value, showDone: e.target.checked })}
            />
            mostrar concluídos
          </label>
        </div>
      )}
    </div>
  );
}
