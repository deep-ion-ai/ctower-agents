// Estado da árvore do painel lateral por projeto (épicos abertos, filtros,
// filtros visíveis) e a última árvore carregada. Só a aba ativa fica montada:
// sem isto, abrir um item noutra aba recolheria a árvore e a esvaziaria até
// o próximo carregamento.
import { useCallback, useEffect, useState } from "react";
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

/**
 * Largura de um cartão da Visão geral (#139): uma coluna do grid, duas ou a
 * linha inteira. Sem espaço para duas colunas, "larga" cai para uma (CSS).
 */
export type CardWidth = "normal" | "wide" | "full";
export const CARD_WIDTHS: readonly CardWidth[] = ["normal", "wide", "full"];
export const CARD_WIDTH_TEXT: Record<CardWidth, string> = { normal: "Normal", wide: "Larga", full: "Linha inteira" };

/** Por cartão e por máquina (localStorage), igual para todos os projetos. */
const WIDTH_KEY = "ctower.overview.cardWidths";
const widthListeners = new Set<() => void>();
/** Escolhas desta execução: valem mesmo sem armazenamento. */
const widthsNow = new Map<string, CardWidth>();

function readWidths(): Record<string, CardWidth> {
  try {
    const v: unknown = JSON.parse(localStorage.getItem(WIDTH_KEY) ?? "null");
    if (!v || typeof v !== "object") return {};
    return Object.fromEntries(
      Object.entries(v as Record<string, unknown>).filter((e): e is [string, CardWidth] =>
        CARD_WIDTHS.includes(e[1] as CardWidth),
      ),
    );
  } catch {
    return {};
  }
}

/** Largura escolhida para o cartão `card` (ou `fallback`) e como trocá-la. */
export function useCardWidth(card: string, fallback: CardWidth): [CardWidth, (w: CardWidth) => void] {
  const [, render] = useState(0);
  useEffect(() => {
    const f = () => render((n) => n + 1);
    widthListeners.add(f);
    return () => void widthListeners.delete(f);
  }, []);
  const width = widthsNow.get(card) ?? readWidths()[card] ?? fallback;
  const set = useCallback(
    (w: CardWidth) => {
      widthsNow.set(card, w);
      try {
        localStorage.setItem(WIDTH_KEY, JSON.stringify({ ...readWidths(), [card]: w }));
      } catch {
        // Sem armazenamento: a escolha não fica.
      }
      widthListeners.forEach((f) => f());
    },
    [card],
  );
  return [width, set];
}

/** Esquece tudo (entre testes). */
export function resetProjectUi(): void {
  uis.clear();
  trees.clear();
  widthsNow.clear();
}
