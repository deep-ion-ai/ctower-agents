import { expect, test } from "vitest";
import type { EnvInfo } from "../protocol/gen";
import { HUB_CONFIG_UNLINKED, hubAssignment } from "../test/fixtures";
import { assignmentText, configLine, repoPathText } from "./hub";

const NOW = Date.UTC(2026, 9, 18, 12, 0, 0);
const info = (c: Partial<EnvInfo["hub_config"]>) =>
  ({ hub_config: { ...HUB_CONFIG_UNLINKED, ...c } }) as EnvInfo;

test("linha da configuração do hub no ambiente", () => {
  expect(configLine(undefined, NOW)).toBeNull();
  expect(configLine(info({}), NOW)).toEqual({ text: "ambiente não ligado a um hub", warn: true });
  expect(configLine(info({ linked: true }), NOW)).toEqual({ text: "configuração do hub ainda não recebida", warn: false });
  expect(configLine(info({ linked: true, last_error: "401" }), NOW)?.text).toBe(
    "configuração do hub não recebida: 401",
  );
  expect(configLine(info({ linked: true, legacy_hub: true }), NOW)?.warn).toBe(true);
  const ok = info({ linked: true, config_rev: 42, pulled_at: NOW - 3 * 3_600_000 });
  expect(configLine(ok, NOW)).toEqual({ text: "configuração rev 42 · há 3 h", warn: false });
  ok.hub_config.last_error = "hub inacessível";
  expect(configLine(ok, NOW)).toEqual({ text: "configuração rev 42 · há 3 h · pull falhou", warn: true });
  // Limite de idade (`secrets_max_age_hours`, #166).
  const velho = info({ linked: true, config_rev: 42, pulled_at: NOW - 30 * 3_600_000, max_age_ms: 24 * 3_600_000 });
  expect(configLine(velho, NOW)).toEqual({
    text: "configuração rev 42 · há 1 d · desatualizada: sessões novas usam os secrets do cache",
    warn: true,
  });
  velho.hub_config.pulled_at = NOW - 23 * 3_600_000;
  expect(configLine(velho, NOW)).toEqual({ text: "configuração rev 42 · há 23 h", warn: false });
});

test("estado da atribuição: a mensagem só vale no estado de erro", () => {
  expect(assignmentText(hubAssignment({ state: "applied" }))).toBe("aplicada");
  expect(assignmentText(hubAssignment({ state: "pending", error: "erro de uma revisão antiga" }))).toBe("pendente");
  expect(assignmentText(hubAssignment({ state: "error", error: "não é um clone" }))).toBe("com erro: não é um clone");
  expect(assignmentText(hubAssignment({ state: "error", error: null }))).toBe("com erro");
  expect(repoPathText(hubAssignment({ repo_path: "" }))).toBe("padrão (<repos_root>/ctower-agents)");
  expect(repoPathText(hubAssignment({ repo_path: "~/src/ct" }))).toBe("~/src/ct");
});
