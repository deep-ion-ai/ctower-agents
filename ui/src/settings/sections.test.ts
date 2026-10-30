import { SETTINGS_PLACES, matchesSetting, scopeOf, settingsTab, templateTab } from "./sections";

const canal = {
  category: "Atualização",
  name: "Canal",
  description: "Branch ou tag de onde o ctowerd é recompilado (padrão main).",
  keys: ["update_ref", "update_repo"],
};

test("busca por nome, descrição e chave, sem acento nem caixa; todas as palavras", () => {
  expect(matchesSetting(canal, "")).toBe(true);
  expect(matchesSetting(canal, "canal")).toBe(true);
  expect(matchesSetting(canal, "ATUALIZACAO")).toBe(true);
  expect(matchesSetting(canal, "recompilado")).toBe(true);
  expect(matchesSetting(canal, "update_ref")).toBe(true);
  expect(matchesSetting(canal, "update_ref canal")).toBe(true);
  expect(matchesSetting(canal, "update_ref fonte")).toBe(false);
  expect(matchesSetting({ ...canal, keywords: "rollback" }, "rollback")).toBe(true);
});

test("escopo de cada seção e endereço das abas", () => {
  expect(scopeOf("terminal")).toBe("app");
  expect(scopeOf("daemon-update")).toBe("env");
  expect(settingsTab({ section: "github" })).toEqual({
    id: "settings",
    screen: { name: "settings", section: "github", scope: "env" },
  });
  expect(templateTab("epic")).toEqual({
    id: "settings:template:epic",
    title: "Template · Épico",
    screen: { name: "settings", template: "epic" },
  });
  // Paleta: escopos e seções, com ids estáveis.
  expect(SETTINGS_PLACES.map((p) => p.id)).toContain("settings:daemon-update");
  // As atualizações do app saíram para a Manutenção (#336).
  expect(SETTINGS_PLACES.map((p) => p.id)).not.toContain("settings:app-updates");
  expect(new Set(SETTINGS_PLACES.map((p) => p.id)).size).toBe(SETTINGS_PLACES.length);
});
