import { describe, expect, test } from "vitest";
import type { WipUsage } from "../protocol/hub-gen";
import { parseWipField, wipFull, wipLimitsOf, wipOf, wipParts, wipReason } from "./wip";

const usage = (sessions: number, prs: number, max_sessions: number | null, max_prs: number | null): WipUsage => ({
  sessions,
  prs,
  limits: { max_sessions, max_prs },
});

describe("limite de trabalho em andamento (#126)", () => {
  test("hub anterior ou sem limite: nada a mostrar", () => {
    expect(wipOf(null)).toBeNull();
    expect(wipOf({ wip: null })).toBeNull();
    expect(wipOf({} as { wip: null })).toBeNull();
    expect(wipOf({ wip: usage(3, 3, null, null) })).toBeNull();
    expect(wipLimitsOf({})).toEqual({ max_sessions: null, max_prs: null });
  });

  test("N de M de cada limite e o motivo só com os atingidos", () => {
    const u = wipOf({ wip: usage(1, 4, 2, 4) });
    expect(wipParts(u)).toEqual(["1 de 2 sessões", "4 de 4 PRs abertos"]);
    expect(wipFull(u)).toBe(true);
    expect(wipReason(u)).toBe("limite de trabalho em andamento (4 de 4 PRs abertos)");
    const livre = wipOf({ wip: usage(1, 0, 2, null) });
    expect(wipParts(livre)).toEqual(["1 de 2 sessões"]);
    expect(wipFull(livre)).toBe(false);
    expect(wipReason(livre)).toBeNull();
  });

  test("campo do formulário: vazio = sem limite; de 1 a 1000", () => {
    expect(parseWipField("  ")).toEqual({ value: null });
    expect(parseWipField("3")).toEqual({ value: 3 });
    expect(parseWipField("0")).toHaveProperty("error");
    expect(parseWipField("1001")).toHaveProperty("error");
    expect(parseWipField("2.5")).toHaveProperty("error");
  });
});
