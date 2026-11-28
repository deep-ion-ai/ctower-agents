// Rodapé "Infraestrutura" da Sala de controle (#357): hub, ambientes e
// backup numa linha, com o link para a manutenção (na web, para o hub).
import { isBackupWarning, maintenanceTab } from "../maintenance/sections";
import { useNav } from "../state/nav";
import { useTower } from "../state/tower";
import { ago } from "../util/time";
import { HUB_WARNING_TITLE } from "./attention";

export interface InfraEnv {
  name: string;
  connected: boolean | null;
}

/** "notebook · servidor conectados" e, à parte, os que caíram. */
export function envsText(envs: InfraEnv[]): { up: string | null; down: string | null } {
  const up = envs.filter((e) => e.connected === true).map((e) => e.name);
  const down = envs.filter((e) => e.connected === false).map((e) => e.name);
  return {
    up: up.length ? `${up.join(" · ")} ${up.length === 1 ? "conectado" : "conectados"}` : null,
    down: down.length ? `${down.join(" · ")} ${down.length === 1 ? "desconectado" : "desconectados"}` : null,
  };
}

export function Infra({ now }: { now: number }) {
  const { hub, envs, status, web } = useTower();
  const nav = useNav();
  const { up, down } = envsText(envs.map((e) => ({ name: e.name, connected: status[e.id]?.connected ?? null })));
  const backup = hub.health?.backup;
  const hubText = !hub.conn
    ? null
    : !hub.status
      ? "conectando…"
      : !hub.connected
        ? "indisponível"
        : backup?.newest_at
          ? `snapshot ${ago(backup.newest_at, now)}`
          : "sem snapshot";
  // Só os avisos de backup e snapshots: os outros do hub ficam no botão de saúde.
  const backupWarnings = hub.connected
    ? (hub.health?.warnings ?? []).filter((w) => isBackupWarning(w.kind)).map((w) => HUB_WARNING_TITLE[w.kind] ?? w.message)
    : [];
  return (
    <footer className="cr-infra" aria-label="Infraestrutura">
      <span className="cr-infra-label">Infraestrutura</span>
      {hubText && (
        <span data-warn={hub.status && !hub.connected ? "true" : undefined}>
          {hub.info?.name ?? "Hub"} · {hubText}
        </span>
      )}
      {up && <span>{up}</span>}
      {down && <span data-warn="true">{down}</span>}
      {!up && !down && <span>{envs.length === 0 ? "nenhum ambiente" : "ambientes conectando…"}</span>}
      {backupWarnings.length > 0 && <span data-warn="true">{backupWarnings.join(" · ")}</span>}
      <span className="control-spacer" />
      {web ? (
        hub.conn && (
          <button type="button" className="cr-link" onClick={() => nav.go({ name: "hub" })}>
            Ver hub ›
          </button>
        )
      ) : (
        <button type="button" className="cr-link" onClick={() => nav.openTab(maintenanceTab("overview"))}>
          Ver manutenção ›
        </button>
      )}
    </footer>
  );
}
