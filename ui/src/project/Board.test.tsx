import { render, screen, within } from "@testing-library/react";
import { expect, test } from "vitest";
import { Board, type BoardNode } from "./Board";
import { NO_FILTERS, buildView } from "./tree-view";

const NOW = 1_800_000_000_000;

function card(n: number, priority: "P0" | "P1" | null, rank: number | null = null): BoardNode {
  return {
    issue: { gh_number: n, title: `item ${n}`, state: "open" },
    work_item: { status: "backlog", branch: null, priority, rank },
    summary: { pr: null, decisions: { total: 0, pending_autonomous: 0 }, last_activity_at: NOW, live_envs: [] },
    children: [],
  } as BoardNode;
}

test("Kanban: selo de prioridade e a ordem da prioridade dentro da coluna", () => {
  const roots = [card(1, null), card(2, "P1", 1), card(3, "P0", 1), card(4, "P1", 2)];
  render(
    <Board
      view={buildView(roots, { ...NO_FILTERS, sort: "number" })}
      filters={NO_FILTERS}
      now={NOW}
      focus={null}
      liveLabel={() => null}
      onSelect={() => {}}
    />,
  );
  const col = screen.getByRole("region", { name: /Coluna backlog/ });
  const order = within(col)
    .getAllByRole("button")
    .map((b) => b.querySelector(".mono")?.textContent);
  expect(order).toEqual(["#3", "#2", "#4", "#1"]);
  expect(within(col).getByTitle("prioridade P0")).toHaveTextContent("P0");
  expect(within(col).getAllByTitle(/prioridade/)).toHaveLength(3);
});
