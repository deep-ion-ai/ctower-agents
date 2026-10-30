// Estado da árvore do painel lateral por projeto (épicos abertos, filtros,
// filtros visíveis) e a última árvore carregada. Só a aba ativa fica montada:
// sem isto, abrir um item noutra aba recolheria a árvore e a esvaziaria até
// o próximo carregamento.
import { useCallback, useState } from "react";
import { NO_FILTERS, type TreeFilters } from "./tree-view";

export interface ProjectUi {
  open: Record<number, boolean>;
  filters: TreeFilters;
  showFilters: boolean;
}

const EMPTY: ProjectUi = { open: {}, filters: NO_FILTERS, showFilters: false };
const uis = new Map<string, ProjectUi>();
const trees = new Map<string, unknown>();

/** Estado da árvore do projeto `key`, lembrado entre as abas. */
export function useProjectUi(key: string): [ProjectUi, (f: (u: ProjectUi) => ProjectUi) => void] {
  const [, render] = useState(0);
  const ui = uis.get(key) ?? EMPTY;
  const update = useCallback(
    (f: (u: ProjectUi) => ProjectUi) => {
      uis.set(key, f(uis.get(key) ?? EMPTY));
      render((n) => n + 1);
    },
    [key],
  );
  return [ui, update];
}

/** Última árvore carregada do projeto (para mostrar algo enquanto relê). */
export const cachedTree = <T,>(key: string): T | null => (trees.get(key) as T | undefined) ?? null;
export const cacheTree = (key: string, t: unknown) => void trees.set(key, t);

/** Esquece tudo (entre testes). */
export function resetProjectUi(): void {
  uis.clear();
  trees.clear();
}
