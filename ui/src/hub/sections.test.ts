import { NO_HEALTH, healthSignal } from "../test/fixtures";
import { maintenanceTab } from "../maintenance/sections";
import { HUB_GROUPS, HUB_PLACES, HUB_SECTIONS, hubCounts, hubTab, hubTitle, warningAction } from "./sections";

const q = (project_key: string, env_id = "e1") => ({
  project_key,
  env_id,
  env_name: env_id,
  rows: 1,
  first_received_at: 0,
  last_received_at: 0,
});

test("grupos e seções: todas aparecem uma vez, na ordem do painel", () => {
  // Backup e snapshots ficam em Manutenção (#338): o Hub não tem mais o grupo.
  expect(HUB_GROUPS.map((g) => g.label)).toEqual(["Operação", "Cadastro", "Segurança", "Acesso"]);
  expect(new Set(HUB_SECTIONS).size).toBe(HUB_SECTIONS.length);
  expect(HUB_SECTIONS).toHaveLength(10);
  expect(HUB_SECTIONS).not.toContain("backup");
  // Projetos e atribuições saiu do Hub (#18): o Cadastro fica só com Ambientes.
  expect(HUB_GROUPS.find((g) => g.label === "Cadastro")?.sections).toEqual(["envs"]);
  expect(HUB_SECTIONS).not.toContain("projects");
});

test("aba por seção: a Visão geral é a aba da tela", () => {
  expect(hubTab("overview")).toEqual({ id: "hub", screen: { name: "hub" } });
  expect(hubTab("secrets")).toEqual({ id: "hub:secrets", screen: { name: "hub", section: "secrets" } });
  expect(hubTitle(undefined)).toBe("Hub · Visão geral");
  expect(hubTitle("github")).toBe("Hub · Token do GitHub");
});

test("hubTab(\"projects\") (#18): o projeto nas Configurações ou o índice de Projetos, sem id próprio", () => {
  const ref = { owner: "deep-ion-ai", repo: "ctower-agents" };
  expect(hubTab("projects")).toEqual({ screen: { name: "project" } });
  expect(hubTab("projects", { register: { url: "u" } })).toEqual({ screen: { name: "project", register: { url: "u" } } });
  expect(hubTab("projects", { project: ref })).toEqual({ screen: { name: "project", ...ref, view: "settings" } });
  expect(hubTab("projects", { project: ref, assign: true })).toEqual({
    screen: { name: "project", ...ref, view: "settings", section: "envs", assign: true },
  });
  // A paleta mantém o lugar antigo como sinônimo, levando ao índice.
  const place = HUB_PLACES.find((p) => p.id === "hub:projects")!;
  expect(place.label).toBe("Hub › Projetos e atribuições");
  expect(place.keywords).toMatch(/atribuições/);
  expect(place.open.tab).toEqual({ screen: { name: "project" } });
});

test("aviso → onde se resolve: seção do Hub ou, backup, Manutenção", () => {
  const backup = { section: null, tab: maintenanceTab("backup"), label: "Configurar backup" };
  expect(warningAction("backup_same_device")).toEqual(backup);
  expect(warningAction("secrets_snapshot_stale")).toEqual(backup);
  expect(warningAction("recovery_reverify_due")).toEqual({ section: "recovery", tab: hubTab("recovery"), label: "Verificar chave" });
  expect(warningAction("push_quarantine")).toEqual({ section: "quarantine", tab: hubTab("quarantine"), label: "Revisar quarentena" });
  expect(warningAction("secrets_rotate_pending")?.section).toBe("secrets");
  expect(warningAction("algo_novo")).toBeNull();
});

test("contadores: avisos, pendentes, sinais de aviso/críticos e projetos em quarentena", () => {
  const c = hubCounts({
    warnings: [
      { kind: "push_quarantine", message: "" },
      { kind: "backup_stale", message: "" },
    ],
    quarantine: [q("a/b"), q("a/b", "e2"), q("a/c")],
    pending: 1,
    health: {
      ...NO_HEALTH,
      signals: [healthSignal({ severity: "aviso" }), healthSignal({ severity: "info" })],
    },
    envs: 2,
  });
  expect(c.overview).toEqual({ n: 2, text: "2 avisos", tone: "attention" });
  expect(c.proposals).toEqual({ n: 1, text: "1 pendente", tone: "attention" });
  expect(c.health).toEqual({ n: 1, text: "1 sinal", tone: undefined });
  expect(c.quarantine).toEqual({ n: 2, text: "2 projetos", tone: "attention" });
  // O aviso de backup conta na Visão geral; a seção dele fica em Manutenção.
  expect(Object.keys(c)).not.toContain("backup");
  expect(c.envs?.text).toBe("2 ambientes");
  expect(c.recovery).toBeUndefined();
  // Crítico deixa o contador da saúde em tom de erro.
  expect(
    hubCounts({ warnings: [], quarantine: [], pending: 0, health: { ...NO_HEALTH, signals: [healthSignal()] }, envs: 0 })
      .health?.tone,
  ).toBe("error");
  expect(hubCounts({ warnings: [], quarantine: [], pending: 0, health: null, envs: 0 })).toEqual({});
});
