// Ações nos ambientes pela PWA (fase 2 do épico #205, #343): elas vão ao
// ambiente pelo canal reverso (`hub.env.call`, a `api.call` da
// `WebDaemonApi`) e só aparecem com o ambiente online (`HubEnv.online`).
import { useEffect, useState, useSyncExternalStore } from "react";
import type { DaemonApi } from "../api/daemon";
import type { Dashboard, Marked, Review, Session } from "../protocol/gen";
import { RC_CONNECT_MS } from "../terminal/remote";
import { useTower } from "../state/tower";

/** Intervalo da leitura de `session.get` enquanto espera o id do Remote Control. */
export const RC_POLL_MS = 2_000;

/** Texto das ações ocultas com o ambiente fora do canal. */
export const OFFLINE_ACTIONS =
  "Ambiente offline: iniciar, retomar, encerrar e ligar o Remote Control voltam quando ele reconectar ao hub.";

/** O ambiente tem canal com o hub agora (as ações dele funcionam). */
export function useEnvOnline(env: string | undefined): boolean {
  const { status } = useTower();
  return !!env && !!status[env]?.connected;
}

// Pedidos de Remote Control em curso (`session_id` → quando pediu), para o
// painel da sessão esperar o `rc_session_id` e abrir o link quando chegar:
// ao iniciar ou retomar com `remote_control` e ao ligar numa sessão viva.
const waits = new Map<string, number>();
const subs = new Set<() => void>();
let version = 0;

function changed() {
  version++;
  for (const cb of subs) cb();
}

/** Marca que a sessão vai conectar ao Remote Control (o painel espera o id). */
export function rcRequested(sessionId: string, at = Date.now()) {
  waits.set(sessionId, at);
  changed();
}

/** O link já foi aberto (ou o prazo venceu): não abre de novo. */
export function rcDone(sessionId: string) {
  if (waits.delete(sessionId)) changed();
}

/** Quando o Remote Control foi pedido para a sessão (`undefined` = não há pedido). */
export function useRcRequest(sessionId: string | undefined): number | undefined {
  useSyncExternalStore(
    (cb) => {
      subs.add(cb);
      return () => subs.delete(cb);
    },
    () => version,
  );
  return sessionId ? waits.get(sessionId) : undefined;
}

/** Só para os testes. */
export function resetRcRequests() {
  waits.clear();
  changed();
}

/**
 * A sessão lida no ambiente (`session.get`) enquanto ela não está na visão do
 * hub (acabou de nascer: o hub só a tem no próximo envio) ou enquanto espera
 * o `rc_session_id` (relida a cada {@link RC_POLL_MS} até {@link RC_CONNECT_MS}).
 */
export function useEnvSession(
  api: DaemonApi,
  env: string,
  sessionId: string,
  enabled: boolean,
  poll: boolean,
): Session | null {
  const [s, setS] = useState<Session | null>(null);
  useEffect(() => {
    setS(null);
  }, [env, sessionId]);
  useEffect(() => {
    if (!enabled) return;
    let alive = true;
    const read = () =>
      api.call<Session>(env, "session.get", { session_id: sessionId }).then(
        (v) => alive && setS(v),
        () => {},
      );
    void read();
    if (!poll) return () => void (alive = false);
    const started = Date.now();
    const id = setInterval(() => {
      if (Date.now() - started > RC_CONNECT_MS) clearInterval(id);
      else void read();
    }, RC_POLL_MS);
    return () => {
      alive = false;
      clearInterval(id);
    };
  }, [api, env, sessionId, enabled, poll]);
  return s;
}

const same = (a: string, b: string) => a.toLowerCase() === b.toLowerCase();

/**
 * Marca como revisadas as decisões autônomas pendentes do item em cada
 * ambiente online (as decisões ficam no ambiente onde a sessão rodou). Os
 * ids são os de cada ambiente: o work item sai do `dashboard.get` dele, e o
 * PR (aberto, ou o último) do `review.get`. Devolve quantas marcou e os
 * ambientes que tinham pendências sem PR.
 */
export async function markReviewedOnEnvs(
  api: DaemonApi,
  envs: { id: string; name: string }[],
  owner: string,
  repo: string,
  ghNumber: number,
): Promise<{ marked: number; noPr: string[] }> {
  let marked = 0;
  const noPr: string[] = [];
  for (const e of envs) {
    const d = await api.call<Dashboard>(e.id, "dashboard.get");
    const p = d.projects.find((x) => same(x.project.gh_owner, owner) && same(x.project.gh_repo, repo));
    const wi = p?.work_items.find((w) => w.gh_number === ghNumber);
    if (!wi || wi.pending_decisions === 0) continue;
    const r = await api.call<Review>(e.id, "review.get", {
      work_item_id: wi.id,
    });
    if (r.decisions.length === 0) continue;
    const pr = r.prs.find((x) => x.state === "open") ?? r.prs[r.prs.length - 1];
    if (!pr) {
      noPr.push(e.name);
      continue;
    }
    const m = await api.call<Marked>(e.id, "review.mark_reviewed", {
      work_item_id: wi.id,
      pr: pr.number,
    });
    marked += m.marked;
  }
  return { marked, noPr };
}
