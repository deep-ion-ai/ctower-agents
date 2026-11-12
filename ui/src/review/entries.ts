// O que a tela de Revisão e o cartão Revisões da Visão geral do projeto
// (#101) têm em comum: os itens a revisar (do dashboard de cada ambiente),
// a leitura do `review.get` de cada um e o `review.mark_reviewed`.
import { useEffect, useState } from "react";
import { GESTOR_LABEL } from "../control-room/attention";
import type { DashboardWorkItem, Marked, Project, Review } from "../protocol/gen";
import type { DaemonApi } from "../api/daemon";
import { itemRef, RELEASE_ITEM, noIssue } from "../util/item";
import { noteReviewed, reviewKey, useReviews } from "./ui-store";

/** Item com decisões a revisar (ou em revisão), num ambiente. */
export interface ReviewEntry {
  env: string;
  workItemId: number;
  /** Ausente no gestor (o sentinela fica fora de `projects`). */
  project?: Project;
  ghNumber?: number;
  title: string;
  gestor: boolean;
  /** Pendentes: do `review.get` quando já lido; senão, do dashboard. */
  count: number;
  review?: Review;
}

/** O item entra na Revisão: em revisão ou com decisões autônomas pendentes. */
export const inReview = (w: DashboardWorkItem) => w.status === "em_revisao" || w.pending_decisions > 0;

/** Os itens do projeto `project` do ambiente `env` que entram na Revisão. */
export function projectEntries(env: string, project: Project, items: readonly DashboardWorkItem[]): ReviewEntry[] {
  return items.filter(inReview).map((w) => ({
    env,
    workItemId: w.id,
    project,
    ghNumber: w.gh_number,
    title: w.title ?? "",
    gestor: false,
    count: w.pending_decisions,
  }));
}

export const openPr = (r?: Review) => r?.prs.find((p) => p.state === "open");

/** PR da marcação: o aberto; sem ele, o último (já mesclado ou fechado). */
export const markPr = (r?: Review) => openPr(r) ?? r?.prs[r.prs.length - 1];

/** Nome do item: "#28 título", "Planejamento · projeto" ou o gestor. */
export function entryName(e: Pick<ReviewEntry, "gestor" | "ghNumber" | "title" | "project">): string {
  if (e.gestor) return GESTOR_LABEL;
  if (e.ghNumber === RELEASE_ITEM) return `Release · ${e.project?.name ?? ""}`.trim();
  if (noIssue(e.ghNumber)) return `Planejamento · ${e.project?.name ?? ""}`.trim();
  return `${e.ghNumber !== undefined ? itemRef(e.ghNumber, e.project) : ""} ${e.title}`.trim();
}

/**
 * Lê o `review.get` de cada candidato (de novo quando a lista ou os
 * contadores do dashboard mudam) e devolve os candidatos com o último lido
 * e a contagem dele. O lido fica no `useReviews`, comum à tela de Revisão e
 * ao cartão. `error`: a última falha de leitura (some no sucesso seguinte).
 * `read: false` (web): só o dashboard, sem `review.get` (lá os ids dos
 * work items são os do hub, não os do ambiente).
 */
export function useReviewEntries(
  api: DaemonApi,
  candidates: readonly ReviewEntry[],
  { read = true }: { read?: boolean } = {},
): { entries: ReviewEntry[]; error: string | null } {
  const [reviews, putReview] = useReviews();
  const [error, setError] = useState<string | null>(null);
  const sig = read ? candidates.map((c) => `${c.env}:${c.workItemId}:${c.count}`).join(",") : "";

  useEffect(() => {
    let live = true;
    for (const c of sig.split(",").filter(Boolean)) {
      const [e, id] = c.split(":");
      api.call<Review>(e, "review.get", { work_item_id: Number(id) }).then(
        (r) => {
          putReview(reviewKey(e, Number(id)), r);
          if (live) setError(null);
        },
        (err) => {
          if (live) setError(String(err));
        },
      );
    }
    return () => {
      live = false;
    };
  }, [api, sig, putReview]);

  if (!read) return { entries: [...candidates], error: null };
  const entries = candidates.map((c) => {
    const review = reviews.get(reviewKey(c.env, c.workItemId));
    return { ...c, review, count: review?.decisions.length ?? c.count };
  });
  return { entries, error };
}

/**
 * `review.mark_reviewed` do item: `pr` é o PR em que a revisão fica
 * registrada (o gestor não abre PR e vai sem); `ids`, só essas decisões
 * (sem, todas as pendentes). Anota o item como revisado nesta execução.
 */
export async function markReviewed(
  api: DaemonApi,
  env: string,
  workItemId: number,
  opts: { pr?: number; ids?: number[] } = {},
): Promise<Marked> {
  const m = await api.call<Marked>(env, "review.mark_reviewed", {
    work_item_id: workItemId,
    ...(opts.pr !== undefined ? { pr: opts.pr } : {}),
    ...(opts.ids ? { decision_ids: opts.ids } : {}),
  });
  noteReviewed(reviewKey(env, workItemId));
  return m;
}
