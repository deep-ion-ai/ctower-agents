// Os mesmos casos de crates/ct-core/src/tracker.rs.
import { isAssignee, sameIdentity } from "./assignee";

const ids = ["fulano", "JIRAUSER10001", "Fulano@Example.com"];

test("compara sem caixa e com qualquer identificador", () => {
  expect(isAssignee(ids, "FULANO")).toBe(true);
  expect(isAssignee(ids, " fulano@example.com ")).toBe(true);
  expect(isAssignee(ids, "jirauser10001")).toBe(true);
  expect(isAssignee(ids, "beltrano")).toBe(false);
});

test("vazio, sem eu e sem responsáveis não batem", () => {
  expect(isAssignee(ids, "")).toBe(false);
  expect(isAssignee([""], "")).toBe(false);
  expect(isAssignee([], "fulano")).toBe(false);
  expect(isAssignee(undefined, "fulano")).toBe(false);
  expect(isAssignee(ids, undefined)).toBe(false);
  expect(isAssignee(ids, null)).toBe(false);
  expect(sameIdentity("  ", "  ")).toBe(false);
});
