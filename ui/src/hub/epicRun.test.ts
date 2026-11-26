import type { EpicNode, EpicPlan } from "../protocol/hub-gen";
import { areaOverlaps, closesLines, overlapPairs, overlaps, runTone, waves } from "./epicRun";

function plan(nodes: [number, string[]][], edges: [number, number][]): EpicPlan {
  return {
    run_id: 1,
    version: 1,
    state: "rascunho",
    summary: "",
    nodes: nodes.map(([number, areas], i) => ({
      number,
      position: i,
      model: "sonnet",
      effort: null,
      areas,
      reason: "",
      fingerprint: null,
    })),
    edges: edges.map(([number, blocker]) => ({ number, blocker, reason: "", source: "analysis" })),
    proposals: [],
    session_id: null,
    created_at: 0,
    approved_at: null,
    approved_by: null,
  };
}

test("ondas pelo bloqueante mais tardio; de fora do plano não conta; ciclo no fim", () => {
  const p = plan(
    [
      [11, []],
      [12, []],
      [13, []],
      [14, []],
    ],
    [
      [13, 11],
      [14, 13],
      [14, 12],
      [12, 99],
    ],
  );
  expect(waves(p)).toEqual([[11, 12], [13], [14]]);
  const c = plan(
    [
      [11, []],
      [12, []],
      [13, []],
    ],
    [
      [11, 12],
      [12, 11],
    ],
  );
  expect(waves(c)).toEqual([[13], [11, 12]]);
});

test("sobreposição de áreas: o critério do motor", () => {
  expect(areaOverlaps("crates/a/", "crates/a/src/lib.rs")).toBe(true);
  expect(areaOverlaps("crates/a", "crates/ab")).toBe(false);
  expect(areaOverlaps("ui/src/*.ts", "ui/src/main.ts")).toBe(true);
  expect(areaOverlaps("crates/a/**", "crates/b/x.rs")).toBe(false);
  const p = plan(
    [
      [11, ["web/checkout/frete.tsx", "api/shipping/**"]],
      [12, ["web/checkout/**"]],
      [13, ["docs/"]],
    ],
    [],
  );
  const o = overlaps(p);
  expect(o.get(11)?.with).toEqual([12]);
  expect([...(o.get(11)?.areas ?? [])]).toEqual(["web/checkout/frete.tsx"]);
  expect(o.get(13)).toBeUndefined();
  expect(overlapPairs(p)).toBe(1);
});

test("Closes de cada sub-issue mesclada e do épico; tons", () => {
  const node = (number: number, state: EpicNode["state"]): EpicNode => ({
    run_id: 1,
    number,
    state,
    prev_state: null,
    session_id: null,
    worktree: null,
    pr: null,
    attempts: 0,
    assumed_since: null,
    reason: null,
    rebase_head: null,
    rebase_at: null,
    updated_at: 0,
  });
  expect(closesLines({ epic: 10 }, [node(12, "mesclada"), node(11, "mesclada"), node(13, "em_pr")])).toEqual([
    "Closes #11",
    "Closes #12",
    "Closes #10",
  ]);
  expect(runTone("aguardando_aprovacao")).toBe("attention");
  expect(runTone("aguardando_merge")).toBe("attention");
  expect(runTone("executando")).toBe("running");
});
