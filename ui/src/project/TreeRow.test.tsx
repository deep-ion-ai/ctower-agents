import { describe, expect, it } from "vitest";
import type { IssueRow, WorkItemSummary } from "../protocol/gen";
import { decisionsText, decisionsTitle, mergedOpen, rowLabel, type RowNode } from "./TreeRow";

const NOW = 1_000_000_000_000;

const issue: IssueRow = {
  gh_number: 49,
  title: "Linha enriquecida",
  state: "open",
  labels: [],
  parent_number: null,
  has_sub_issues: false,
  url: "https://github.com/o/r/issues/49",
  updated_at: 0,
};

const base: WorkItemSummary = {
  pr: null,
  decisions: { total: 0, pending_autonomous: 0 },
  last_activity_at: null,
  live_envs: [],
};

const node = (s: Partial<WorkItemSummary> = {}): RowNode => ({
  issue,
  work_item: { status: "em_execucao" },
  summary: { ...base, ...s },
  children: [],
});

describe("texto do contador de decisões", () => {
  it("marca ! só com autônomas pendentes", () => {
    expect(decisionsText({ total: 3, pending_autonomous: 0 })).toBe("◆3");
    expect(decisionsText({ total: 3, pending_autonomous: 2 })).toBe("◆3!");
    expect(decisionsTitle({ total: 1, pending_autonomous: 0 })).toBe("1 decisão");
    expect(decisionsTitle({ total: 3, pending_autonomous: 2 })).toBe("3 decisões, 2 autônomas pendentes de revisão");
  });
});

describe("nome acessível da linha", () => {
  it("traz status, PR, decisões e atividade", () => {
    const name = rowLabel(
      node({
        pr: { number: 7, work_item_id: 1, head: "a", base: "b", state: "open", url: "#pr", updated_at: 0, head_sha: null },
        decisions: { total: 2, pending_autonomous: 1 },
        last_activity_at: NOW - 5 * 60_000,
      }),
      NOW,
    );
    expect(name).toBe("#49 Linha enriquecida, em execução, PR #7 aberto, 2 decisões, 1 autônoma pendente de revisão, última atividade há 5 min");
  });

  it("sem dados: só status e 'sem atividade'", () => {
    expect(rowLabel(node(), NOW)).toBe("#49 Linha enriquecida, em execução, sem atividade");
  });
});

describe("PR mesclado com a issue aberta (#375)", () => {
  const pr = (state: "open" | "merged" | "closed", prBase = "develop") => ({
    number: 280,
    work_item_id: 1,
    head: "feat/49-x",
    base: prBase,
    state,
    url: "#pr",
    updated_at: 0,
    head_sha: null,
  });

  it("só sinaliza PR mesclado na base do projeto com a issue aberta", () => {
    expect(mergedOpen(node({ pr: pr("merged") }), "develop")).toBe(true);
    expect(mergedOpen(node({ pr: pr("open") }), "develop")).toBe(false);
    expect(mergedOpen(node({ pr: pr("closed") }), "develop")).toBe(false);
    // PR para a branch do épico: a sub-issue só fecha com o PR do épico.
    expect(mergedOpen(node({ pr: pr("merged", "wip/27-ui") }), "develop")).toBe(false);
    expect(mergedOpen({ ...node({ pr: pr("merged") }), issue: { ...issue, state: "closed" } }, "develop")).toBe(false);
    expect(mergedOpen(node(), "develop")).toBe(false);
    expect(mergedOpen(node({ pr: pr("merged") }), undefined)).toBe(false);
  });

  it("entra no nome acessível da linha", () => {
    expect(rowLabel(node({ pr: pr("merged") }), NOW, "develop")).toBe(
      "#49 Linha enriquecida, em execução, PR #280 mesclado, PR mesclado, issue aberta, sem atividade",
    );
  });
});
