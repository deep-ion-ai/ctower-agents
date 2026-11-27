// Execuções de épico (#341): a lista do hub com o título de cada épico, os
// pedidos de decisão pendentes e o progresso, para a árvore do Hub (só as
// ativas) e o índice de Execução de épicos (com o histórico). Relida no push
// `hub.epic_run.changed`; título, pedidos e nós vêm do `hub.epic_run.status`
// de cada execução.
import { useCallback, useEffect, useRef, useState } from "react";
import type { EpicRun, EpicRunView } from "../protocol/hub-gen";
import { useTower } from "../state/tower";

/** Janela do histórico do índice: as encerradas nos últimos 7 dias. */
export const EPIC_RUN_HISTORY_MS = 7 * 24 * 60 * 60_000;

export interface EpicRunItem {
  run: EpicRun;
  /** Título do épico; `null` enquanto não chegou. */
  title: string | null;
  /** Pedidos de decisão sem resposta. */
  pending: number;
  /** Sub-issues mescladas na `wip` e o total do plano. */
  merged: number;
  total: number;
}

export function useEpicRuns({ history = false }: { history?: boolean } = {}): {
  runs: EpicRunItem[] | null;
  error: string | null;
} {
  const { api, hub } = useTower();
  const [runs, setRuns] = useState<EpicRunItem[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const titles = useRef(new Map<number, string>());

  const load = useCallback(async () => {
    try {
      const list = await api.hubCall<EpicRun[]>(
        "hub.epic_run.list",
        history ? { ended_since: Date.now() - EPIC_RUN_HISTORY_MS } : {},
      );
      const views = await Promise.all(
        list.map((r) => api.hubCall<EpicRunView>("hub.epic_run.status", { run_id: r.id }).catch(() => null)),
      );
      setRuns(
        list.map((run, i) => {
          const v = views[i];
          const t = v?.titles[run.epic];
          if (t) titles.current.set(run.id, t);
          return {
            run,
            title: titles.current.get(run.id) ?? null,
            pending: v?.decisions.filter((d) => d.answer === null).length ?? 0,
            merged: v?.nodes.filter((n) => n.state === "mesclada").length ?? 0,
            total: v?.nodes.length ?? 0,
          };
        }),
      );
      setError(null);
    } catch (e) {
      setError(String(e));
    }
  }, [api, history]);

  useEffect(() => {
    if (!hub.connected) return;
    void load();
    return api.onHubPush((e) => {
      if (e.event === "hub.epic_run.changed") void load();
    });
  }, [api, load, hub.connected]);

  return { runs, error };
}
