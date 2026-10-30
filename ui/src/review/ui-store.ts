// Estado da Revisão que sobrevive à troca de aba (só a aba ativa fica
// montada): o último `review.get` de cada item (contadores e PR do painel
// lateral) e, por item, o filtro e as decisões abertas.
import { useCallback, useEffect, useState } from "react";
import type { Review } from "../protocol/gen";

export type ReviewFilter = "autonomas" | "orientadas" | "revisadas";

export interface ReviewUi {
  filter: ReviewFilter;
  /** Aberta/recolhida por escolha do usuário (uid → aberta). */
  open: Record<string, boolean>;
}

const EMPTY: ReviewUi = { filter: "autonomas", open: {} };
const uis = new Map<string, ReviewUi>();
const reviews = new Map<string, Review>();
const subs = new Set<() => void>();

export const reviewKey = (env: string, workItemId: number) => `${env}:${workItemId}`;

/** Estado da tela do item `key`, lembrado entre as abas. */
export function useReviewUi(key: string): [ReviewUi, (f: (u: ReviewUi) => ReviewUi) => void] {
  const [, render] = useState(0);
  const ui = uis.get(key) ?? EMPTY;
  const update = useCallback(
    (f: (u: ReviewUi) => ReviewUi) => {
      uis.set(key, f(uis.get(key) ?? EMPTY));
      render((n) => n + 1);
    },
    [key],
  );
  return [ui, update];
}

function put(key: string, r: Review) {
  reviews.set(key, r);
  for (const s of subs) s();
}

/** Último `review.get` por item; gravar avisa todos os que leem (painel e editor). */
export function useReviews(): [ReadonlyMap<string, Review>, (key: string, r: Review) => void] {
  const [, render] = useState(0);
  useEffect(() => {
    const cb = () => render((n) => n + 1);
    subs.add(cb);
    return () => void subs.delete(cb);
  }, []);
  return [reviews, put];
}

/** Esquece tudo (entre testes). */
export function resetReviewUi(): void {
  uis.clear();
  reviews.clear();
}
