// Partes visuais do "Sincronizar este projeto" (#90): a linha do estado do
// sync (cartão "Buscar issues" e cabeçalho do projeto) e o botão do
// cabeçalho. A regra fica em overview.ts e o andamento em sync-now.ts.
import { useNow } from "../App";
import { useTower } from "../state/tower";
import { DesktopOnly } from "../web/DesktopOnly";
import { syncStatusLine } from "./overview";
import { runProjectSync, useSyncRun, type ProjectSyncInfo } from "./sync-now";

/** Nota do botão desabilitado na web: o canal reverso recusa `sync.now`. */
export const SYNC_DESKTOP_NOTE = "Buscar issues fica no app desktop";

/**
 * "Último sync: há 5 min" ou "Sync falhando desde 14:03 (3 rodadas)" com o
 * erro abaixo. Sem dado (ambiente anterior ou projeto que nunca rodou),
 * nada.
 */
export function SyncStatus({ info, className = "" }: { info: ProjectSyncInfo; className?: string }) {
  const now = useNow();
  const line = syncStatusLine(info.state, now);
  if (!line) return null;
  return (
    <span className={`pj-sync-status ${className}`} data-failing={line.failing || undefined}>
      <span className={line.failing ? "start-error" : "muted"}>{line.text}</span>
      {line.error && <span className="pj-sync-error">{line.error}</span>}
    </span>
  );
}

/**
 * Botão "Sincronizar este projeto" do cabeçalho, com o estado do sync ao
 * lado e o resultado (`syncFeedback`) logo abaixo. `feedback` = false na
 * Visão geral, onde o cartão "Buscar issues" já mostra o mesmo resultado.
 * Na web, desabilitado com a nota do app desktop (#26).
 */
export function SyncHead({ info, feedback = true }: { info: ProjectSyncInfo; feedback?: boolean }) {
  const { api, web } = useTower();
  const run = useSyncRun(info.key);
  const failing = (info.state?.failures ?? 0) > 0;
  return (
    <span className="pj-sync-head">
      <SyncStatus info={info} />
      {web ? (
        <DesktopOnly labels="Sincronizar este projeto" note={SYNC_DESKTOP_NOTE} />
      ) : (
        <button
          type="button"
          className="btn"
          disabled={run.busy || info.plan.targets.length === 0}
          title={info.plan.reason ?? "Buscar agora no rastreador as issues só deste projeto"}
          onClick={() => void runProjectSync(api, info)}
        >
          {run.busy ? "Buscando issues…" : failing ? "Tentar sincronizar de novo" : "Sincronizar este projeto"}
        </button>
      )}
      {feedback && run.msg && (
        <span role={run.msg.error ? "alert" : "status"} className={`pj-sync-msg ${run.msg.error ? "start-error" : "state-on"}`}>
          {run.msg.text}
        </span>
      )}
    </span>
  );
}
