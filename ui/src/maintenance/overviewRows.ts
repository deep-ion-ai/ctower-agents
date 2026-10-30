// Visão geral da Manutenção (#339): uma linha por ambiente, juntando o que a
// UI sabe de cada um (conexão, `env.info.update`) com o que o hub sabe
// (`hub.envs.list`: limpeza e último envio, também de ambientes que esta UI
// não conecta). Funções puras, testadas em overviewRows.test.ts; a tela e o selo
// da barra de atividades usam as mesmas linhas e a mesma contagem.
import { useMemo } from "react";
import type { EnvStatus, EnvView, UiUpdate } from "../api/types";
import type { EnvInfo } from "../protocol/gen";
import type { CleanupSummary, HubEnv, HubUpdateStatus } from "../protocol/hub-gen";
import { useTower } from "../state/tower";

export interface MaintenanceEnvRow {
  /** Chave estável: o id do ambiente na UI ou `hub:<env_id>`. */
  key: string;
  name: string;
  /** Id do ambiente nesta UI; `undefined` = só o hub o conhece. */
  uiEnv?: string;
  connected: boolean;
  /** `env.info` (o último recebido, mesmo desconectado). */
  info?: EnvInfo;
  /** Ambiente no hub; `undefined` = o hub não o conhece (ou ainda não respondeu). */
  hub?: HubEnv;
  /** Atualização disponível no último `env.info`, só com o ambiente conectado. */
  updateAvailable: boolean;
  /** Limpezas pendentes informadas ao hub (`cleanup.items > 0`). */
  cleanupPending: boolean;
}

export interface MaintenanceInput {
  envs: EnvView[];
  status: Record<string, EnvStatus>;
  infos: Record<string, EnvInfo>;
  hubEnvs: HubEnv[];
  /** `env_id` do daemon → id do ambiente na UI (`hub.envByDaemon`). */
  envByDaemon: Record<string, string>;
}

const hasCleanup = (c: CleanupSummary | null | undefined): boolean => !!c && c.items > 0;

/**
 * Linhas da Visão geral: primeiro os ambientes desta UI, na ordem das
 * Configurações, cada um com o seu registro no hub (pelo `env_id` do último
 * `env.info`); depois os que só o hub conhece. A atualização só conta com o
 * ambiente conectado: desconectado, o `env.info` guardado pode estar velho e
 * não dá para atualizar. A limpeza vem do hub e vale mesmo desconectado.
 */
export function maintenanceRows(i: MaintenanceInput): MaintenanceEnvRow[] {
  const hubByEnv = new Map<string, HubEnv>();
  for (const h of i.hubEnvs) {
    const ui = i.envByDaemon[h.env_id];
    if (ui !== undefined) hubByEnv.set(ui, h);
  }
  const rows: MaintenanceEnvRow[] = i.envs.map((e) => {
    const connected = !!i.status[e.id]?.connected;
    const info = i.infos[e.id];
    const hub = hubByEnv.get(e.id);
    return {
      key: e.id,
      name: e.name,
      uiEnv: e.id,
      connected,
      info,
      hub,
      updateAvailable: connected && !!info?.update?.available?.update_available,
      cleanupPending: hasCleanup(hub?.cleanup),
    };
  });
  const known = new Set(i.envs.map((e) => e.id));
  for (const h of i.hubEnvs) {
    const ui = i.envByDaemon[h.env_id];
    if (ui !== undefined && known.has(ui)) continue;
    rows.push({
      key: `hub:${h.env_id}`,
      name: h.name,
      connected: false,
      hub: h,
      updateAvailable: false,
      cleanupPending: hasCleanup(h.cleanup),
    });
  }
  return rows;
}

/**
 * Pendências de manutenção (o selo da barra de atividades): atualizações
 * disponíveis (ambientes, este app e o hub, #421) mais ambientes com limpeza
 * pendente. O mesmo ambiente com atualização e limpeza conta 2: são duas
 * ações, em seções diferentes.
 */
export function maintenancePendingCount(
  rows: MaintenanceEnvRow[],
  uiUpdate: UiUpdate | null,
  hubUpdate: HubUpdateStatus | null = null,
): number {
  let n = (uiUpdate?.available ? 1 : 0) + (hubUpdate?.available?.update_available ? 1 : 0);
  for (const r of rows) n += (r.updateAvailable ? 1 : 0) + (r.cleanupPending ? 1 : 0);
  return n;
}

/** Linhas e contagem a partir do estado global (`useTower`): nenhuma chamada a mais. */
export function useMaintenanceOverview(): { rows: MaintenanceEnvRow[]; pending: number } {
  const { envs, status, infos, hub, uiUpdate } = useTower();
  const { envs: hubEnvs, envByDaemon, update: hubUpdate } = hub;
  return useMemo(() => {
    const rows = maintenanceRows({ envs, status, infos, hubEnvs, envByDaemon });
    return { rows, pending: maintenancePendingCount(rows, uiUpdate, hubUpdate) };
  }, [envs, status, infos, hubEnvs, envByDaemon, uiUpdate, hubUpdate]);
}
