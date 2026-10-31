import { describe, expect, test } from "vitest";
import { healthSignal, project, sync } from "../test/fixtures";
import type { Clone } from "./clones";
import {
  pickSync,
  projectSignals,
  SYNC_FAIL_ALERT,
  syncAlert,
  syncFeedback,
  syncPlan,
  syncStatusLine,
} from "./overview";

const clone = (env: string, name: string, id = 1): Clone => ({
  env,
  name,
  project: project({ id }),
  items: [],
  live: [],
  state: "applied",
});

test("projectSignals: só os do projeto (sem caixa), sem informativos, críticos primeiro", () => {
  const h = {
    generated_at: 0,
    counts: { critico: 1, aviso: 1, info: 1 },
    signals: [
      healthSignal({ severity: "aviso", number: 1 }),
      healthSignal({ severity: "critico", number: 2, owner: "Deep-Ion-AI" }),
      healthSignal({ severity: "info", number: 3 }),
      healthSignal({ repo: "outro", number: 4 }),
      healthSignal({ kind: "ambiente_mudo", owner: null, repo: null, number: null, env: "wsl" }),
    ],
  };
  expect(projectSignals(h, "deep-ion-ai", "ctower-agents").map((s) => s.number)).toEqual([2, 1]);
  expect(projectSignals(null, "o", "r")).toEqual([]);
});

test("syncPlan: GitHub em cada clone; Jira só no ambiente de sync, com o motivo quando falta", () => {
  const wsl = clone("wsl", "WSL", 1);
  const mac = clone("mac", "Mac", 4);
  expect(syncPlan([wsl, mac], null).targets).toEqual([
    { env: "wsl", name: "WSL", projectId: 1 },
    { env: "mac", name: "Mac", projectId: 4 },
  ]);
  expect(syncPlan([], null).reason).toMatch(/Nenhum ambiente conectado/);
  expect(syncPlan([wsl, mac], { env: "mac", name: "Mac" })).toEqual({
    targets: [{ env: "mac", name: "Mac", projectId: 4 }],
    reason: null,
  });
  expect(syncPlan([wsl], { env: null, name: "Mac" })).toEqual({
    targets: [],
    reason: "O ambiente de sync Mac não está conectado com este projeto.",
  });
  expect(syncPlan([wsl], { env: null, name: null }).reason).toMatch(/sem ambiente de sync/);
});

test("syncFeedback: soma os que deram certo e lista os erros (da chamada ou do rastreador)", () => {
  const ok = (issues: number, prs: number, status_changes = 0) => ({
    project_id: 1,
    ok: true,
    error: null,
    issues,
    prs,
    status_changes,
  });
  expect(syncFeedback([{ name: "WSL", result: [ok(1, 0)] }], "MR")).toEqual({
    text: "Sincronizado por WSL: 1 issue, 0 MRs.",
    error: false,
  });
  expect(
    syncFeedback(
      [
        { name: "WSL", result: [ok(2, 1, 2)] },
        { name: "Mac", error: "ambiente offline" },
        { name: "Pi", result: [{ ...ok(0, 0), ok: false, error: "jira-cli ausente" }] },
      ],
      "PR",
    ),
  ).toEqual({
    text: "Sincronizado por WSL: 2 issues, 1 PR, 2 mudanças de status. Falhou em Mac: ambiente offline · Pi: jira-cli ausente",
    error: true,
  });
});

// #90: estado do sync do projeto no cartão, no cabeçalho e no aviso.
describe("sync do projeto (#90)", () => {
  const H = 3_600_000;
  const now = new Date(2026, 9, 31, 15, 0).getTime();
  const at = (h: number, m = 0) => new Date(2026, 9, 31, h, m).getTime();

  test("syncStatusLine: sem dado, nada; em dia, 'Último sync'", () => {
    expect(syncStatusLine(null, now)).toBeNull();
    // Nunca rodou com sucesso nem falhou: nada a dizer.
    expect(syncStatusLine(sync({ last_ok_at: null }), now)).toBeNull();
    expect(syncStatusLine(sync({ last_ok_at: now - 5 * 60_000, failures: 0 }), now)).toEqual({
      text: "Último sync: há 5 min",
      failing: false,
      error: null,
    });
  });

  test("syncStatusLine: falhando desde a hora do início, com as rodadas e o erro", () => {
    const s = sync({ last_error: "HTTP 410", failures: 4, failing_since: at(14, 3), last_attempt_at: at(14, 50) });
    expect(syncStatusLine(s, now)).toEqual({ text: "Sync falhando desde 14:03 (4 rodadas)", failing: true, error: "HTTP 410" });
    // Uma falha só: sem a contagem; outro dia: com a data.
    const ontem = sync({ last_error: "rede", failures: 1, failing_since: at(14, 3) - 24 * H });
    expect(syncStatusLine(ontem, now)?.text).toBe("Sync falhando desde 30/10 14:03");
  });

  test("syncStatusLine: ambiente anterior (sem failures) não mostra falha; aviso do Jira também não", () => {
    const antigo = sync({ last_ok_at: now - 60 * 60_000, last_error: "401" });
    delete (antigo as Partial<typeof antigo>).failures;
    expect(syncStatusLine(antigo, now)).toEqual({ text: "Último sync: há 1 h", failing: false, error: null });
    expect(syncStatusLine(sync({ last_ok_at: now, last_error: "2 issues não lidas", failures: 0 }), now)?.failing).toBe(false);
  });

  test("syncAlert: só a partir de SYNC_FAIL_ALERT falhas seguidas", () => {
    expect(SYNC_FAIL_ALERT).toBe(3);
    expect(syncAlert(sync({ failures: 2 }))).toBe(false);
    expect(syncAlert(sync({ failures: 3 }))).toBe(true);
    expect(syncAlert(null)).toBe(false);
  });

  test("pickSync: o que falha há mais rodadas; sem falha, o sucesso mais recente", () => {
    const hub = sync({ last_ok_at: now - H, failures: 0 });
    const env = sync({ last_ok_at: now - 2 * H, failures: 5, last_error: "410" });
    expect(pickSync([hub, env])).toBe(env);
    const recente = sync({ last_ok_at: now, failures: 0 });
    expect(pickSync([hub, null, recente])).toBe(recente);
    expect(pickSync([null, undefined])).toBeNull();
  });
});
