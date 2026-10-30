import { FakeDaemonApi } from "../api/fake";
import type { EnvInfo } from "../protocol/gen";
import { hubEnv } from "../test/fixtures";
import { maintenancePendingCount, maintenanceRows, type MaintenanceInput } from "./overviewRows";

const WSL = FakeDaemonApi.wslEnv();
const MAC = { ...FakeDaemonApi.wslEnv("mac"), name: "mac-mini" };

const info = (envId: string, update: boolean) =>
  ({ env_id: envId, update: { available: { update_available: update } } }) as unknown as EnvInfo;
const on = (env: string, connected = true) => ({ env, connected, error: null, since: 0 });
const cleanup = (items: number) => ({ checked_at: 0, items, bytes: items * 1024 });

function input(p: Partial<MaintenanceInput> = {}): MaintenanceInput {
  return {
    envs: [WSL, MAC],
    status: { wsl: on("wsl"), mac: on("mac") },
    infos: { wsl: info("d-wsl", true), mac: info("d-mac", false) },
    hubEnvs: [hubEnv({ env_id: "d-wsl", cleanup: cleanup(0) }), hubEnv({ env_id: "d-mac", cleanup: cleanup(2) })],
    envByDaemon: { "d-wsl": "wsl", "d-mac": "mac" },
    ...p,
  };
}

test("junta cada ambiente da UI ao seu registro no hub; um com atualização e outro com limpeza = 2", () => {
  const rows = maintenanceRows(input());
  expect(rows.map((r) => [r.key, r.updateAvailable, r.cleanupPending, r.hub?.env_id])).toEqual([
    ["wsl", true, false, "d-wsl"],
    ["mac", false, true, "d-mac"],
  ]);
  expect(maintenancePendingCount(rows, null)).toBe(2);
});

test("o mesmo ambiente com atualização e limpeza conta 2; o app com versão nova, mais 1", () => {
  const rows = maintenanceRows(input({ hubEnvs: [hubEnv({ env_id: "d-wsl", cleanup: cleanup(1) })] }));
  expect(maintenancePendingCount(rows, null)).toBe(2);
  const app = { current: "0.1.0", available: { version: "0.2.0", notes: null, date: null }, error: null };
  expect(maintenancePendingCount(rows, app)).toBe(3);
});

test("desconectado: a atualização não conta, a limpeza do hub sim", () => {
  const rows = maintenanceRows(
    input({ status: { wsl: on("wsl", false), mac: on("mac", false) } }),
  );
  expect(rows.map((r) => [r.connected, r.updateAvailable, r.cleanupPending])).toEqual([
    [false, false, false],
    [false, false, true],
  ]);
  expect(maintenancePendingCount(rows, null)).toBe(1);
});

test("ambientes que só o hub conhece vão no fim, sem ambiente da UI", () => {
  const rows = maintenanceRows(
    input({ hubEnvs: [hubEnv({ env_id: "d-note", name: "notebook", cleanup: cleanup(3) })] }),
  );
  expect(rows.map((r) => [r.key, r.name, r.uiEnv, r.cleanupPending])).toEqual([
    ["wsl", "ctower-agents (WSL)", "wsl", false],
    ["mac", "mac-mini", "mac", false],
    ["hub:d-note", "notebook", undefined, true],
  ]);
  expect(maintenancePendingCount(rows, null)).toBe(2);
});
