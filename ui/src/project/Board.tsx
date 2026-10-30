import type { WorkItemStatus } from "../protocol/gen";
import { changeRef, itemShort, type ProviderProject } from "../util/item";
import { ago } from "../util/time";
import { decisionsText, decisionsTitle, PriorityBadge, type RowNode } from "./TreeRow";
import { currentSprint, sprintLabel } from "./sprints";
import { STATUS } from "./status";
import { boardColumns, boardItems, byPriority, statusKey, type TreeFilters, type TreeView } from "./tree-view";

const PR_TEXT = { open: "aberto", merged: "mesclado", closed: "fechado" } as const;

/** Nó do Kanban: o resumo completo mais o branch do work item (local e hub têm o mesmo formato). */
export type BoardNode = RowNode & { work_item: { status: WorkItemStatus; branch: string | null } | null };

/**
 * Kanban SOMENTE LEITURA: uma coluna por status e um card por item, a partir da
 * mesma visão da árvore. Os cards são botões que abrem o painel de detalhe; não
 * há arrastar e soltar (o status só muda pelo menu do painel).
 */
export function Board<T extends BoardNode>({
  view,
  filters,
  now,
  focus,
  project,
  liveLabel,
  onSelect,
}: {
  view: TreeView<T>;
  filters: TreeFilters;
  now: number;
  focus: number | null;
  /** Rastreador e forja do projeto (#352); sem ele, GitHub. */
  project?: ProviderProject | null;
  /** Ambiente(s) com sessão viva do item; null = sem sessão viva. */
  liveLabel: (n: T) => string | null;
  onSelect: (ghNumber: number) => void;
}) {
  const items = boardItems(view, filters);
  return (
    <div className="board-wrap">
      <p className="board-note">
        <span className="lock">somente leitura</span>
        Os cards não se movem; para mudar o status abra o item e use o menu do painel.
      </p>
      <div className="board">
        {boardColumns(filters).map((k) => {
          // Dentro da coluna, a ordem da prioridade (spec do gestor §4.3); sem
          // prioridade, a ordem da árvore (a ordenação é estável).
          const list = items.filter((i) => statusKey(i.node) === k).sort((a, b) => byPriority(a.node, b.node));
          const st = STATUS[k];
          return (
            <section
              key={k}
              className="col"
              aria-label={`Coluna ${st.text}, ${list.length} ${list.length === 1 ? "item" : "itens"}`}
            >
              <div className="col-head">
                <span className="badge" data-tone={st.tone}>
                  {st.text}
                </span>
                <span className="count">{list.length}</span>
              </div>
              <ul className="col-body">
                {list.length === 0 && <li className="col-empty">Nenhum item.</li>}
                {list.map(({ node, epicOf }) => {
                  const n = node.issue.gh_number;
                  const isEpic = node.children.length > 0;
                  const { pr, decisions, last_activity_at } = node.summary;
                  const live = liveLabel(node);
                  const branch = node.work_item?.branch;
                  const done = node.children.filter((c) => statusKey(c) === "concluido").length;
                  const sprint = currentSprint(node.issue);
                  return (
                    <li key={n}>
                      <button
                        type="button"
                        className={`card${isEpic ? " is-epic" : ""}`}
                        draggable={false}
                        aria-current={focus === n ? "true" : undefined}
                        onClick={() => onSelect(n)}
                      >
                        <span className="card-top">
                          {live !== null && (
                            <span
                              className="live"
                              role="img"
                              aria-label={`sessão viva em ${live}`}
                              title={`sessão viva em ${live}`}
                            />
                          )}
                          <span className="mono muted">{itemShort(project, n)}</span>
                          <PriorityBadge node={node} />
                          {isEpic && <span className="epic-label">ÉPICO</span>}
                          {epicOf !== null && <span className="card-epic">épico {itemShort(project, epicOf)}</span>}
                          {sprint && (
                            <span className="pj-sprint" data-state={sprint.state} title={sprintLabel(sprint)}>
                              {sprint.name}
                            </span>
                          )}
                        </span>
                        <span className="card-title">{node.issue.title}</span>
                        {branch && <span className="mono muted branch">{branch}</span>}
                        <span className="card-meta">
                          {isEpic && (
                            <span className="muted" title="sub-issues concluídas">
                              {done}/{node.children.length}
                            </span>
                          )}
                          {pr && (
                            <span className="pr" data-state={pr.state}>
                              {changeRef(project, pr.number)} {PR_TEXT[pr.state]}
                            </span>
                          )}
                          {decisions.total > 0 && (
                            <span
                              className={decisions.pending_autonomous > 0 ? "dec pending" : "dec"}
                              title={decisionsTitle(decisions)}
                            >
                              {decisionsText(decisions)}
                            </span>
                          )}
                          <span className="ago" title="última atividade">
                            {last_activity_at !== null ? ago(last_activity_at, now) : "—"}
                          </span>
                        </span>
                      </button>
                    </li>
                  );
                })}
              </ul>
            </section>
          );
        })}
      </div>
    </div>
  );
}
