import { expect, test } from "vitest";
import { healthSignal, project } from "../test/fixtures";
import type { Clone } from "./clones";
import { projectSignals, syncFeedback, syncPlan } from "./overview";

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
