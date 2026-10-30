import { PLACES } from "../shell/places";
import { search } from "../shell/palette/fuzzy";
import { paletteItems } from "../shell/palette/items";
import {
  MAINTENANCE_GROUPS,
  MAINTENANCE_PLACES,
  MAINTENANCE_SECTIONS,
  isBackupWarning,
  isEnvSection,
  maintenancePath,
  maintenanceTab,
  maintenanceTitle,
} from "./sections";

test("grupos e seções: todas aparecem uma vez, na ordem do painel", () => {
  expect(MAINTENANCE_GROUPS.map((g) => g.label)).toEqual([undefined, "Atualizações", undefined]);
  expect(MAINTENANCE_SECTIONS).toEqual(["overview", "app-updates", "env-updates", "hub-updates", "cleanup", "backup"]);
  expect(maintenancePath("env-updates")).toEqual(["Atualizações", "Ambientes"]);
  expect(maintenancePath("hub-updates")).toEqual(["Atualizações", "Hub"]);
  expect(maintenancePath("cleanup")).toEqual(["Limpeza"]);
  expect(MAINTENANCE_SECTIONS.filter(isEnvSection)).toEqual(["cleanup"]);
});

test("aba por seção: a Visão geral é a aba da tela; o ambiente só nas seções de ambiente", () => {
  expect(maintenanceTab("overview")).toEqual({ id: "maintenance", screen: { name: "maintenance" } });
  expect(maintenanceTab("overview", "wsl")).toEqual({ id: "maintenance", screen: { name: "maintenance" } });
  expect(maintenanceTab("cleanup", "wsl")).toEqual({
    id: "maintenance:cleanup",
    screen: { name: "maintenance", section: "cleanup", env: "wsl" },
  });
  expect(maintenanceTab("backup", "wsl").screen).toEqual({ name: "maintenance", section: "backup" });
  // Atualizações › Ambientes mostra todos: sem ambiente no endereço.
  expect(maintenanceTab("env-updates", "wsl").screen).toEqual({ name: "maintenance", section: "env-updates" });
  expect(maintenanceTitle(undefined)).toBe("Manutenção · Visão geral");
  expect(maintenanceTitle("app-updates")).toBe("Manutenção · Atualizações · Este app");
});

test("paleta: seções com ids estáveis, dentro de PLACES", () => {
  expect(MAINTENANCE_PLACES.map((p) => p.label)).toEqual([
    "Manutenção › Visão geral",
    "Manutenção › Atualizações › Este app",
    "Manutenção › Atualizações › Ambientes",
    "Manutenção › Atualizações › Hub",
    "Manutenção › Limpeza",
    "Manutenção › Backup e snapshots",
  ]);
  expect(MAINTENANCE_PLACES.find((p) => p.id === "maintenance:cleanup")?.open).toEqual({ tab: maintenanceTab("cleanup") });
  const ids = PLACES.map((p) => p.id);
  expect(ids).toContain("screen:maintenance");
  expect(ids).toContain("maintenance:cleanup");
  expect(new Set(ids).size).toBe(ids.length);
});

test("backup e snapshots do hub (#338): a paleta acha em Manutenção, não no Hub", () => {
  const items = paletteItems({
    envs: [],
    status: {},
    dashboards: {},
    hub: { up: false, dashboard: null, connected: false },
    uiUpdate: null,
    liveGestor: [],
  });
  expect(PLACES.map((p) => p.id)).not.toContain("hub:backup");
  for (const q of ["backup", "snapshots", "restaurar"]) {
    const labels = search(items, q).map((r) => r.item.label);
    expect(labels[0]).toBe("Manutenção › Backup e snapshots");
    expect(labels.filter((l) => l.startsWith("Hub ›"))).toEqual([]);
  }
});

test("avisos de hub.status que se resolvem em Backup e snapshots", () => {
  expect(isBackupWarning("backup_stale")).toBe(true);
  expect(isBackupWarning("secrets_snapshot_same_device")).toBe(true);
  expect(isBackupWarning("docs_backup_too_large")).toBe(true);
  expect(isBackupWarning("secrets_rotate_pending")).toBe(false);
  expect(isBackupWarning("push_quarantine")).toBe(false);
});
