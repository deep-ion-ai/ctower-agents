import { ago, duration } from "./time";

test.each([
  [5_000, "< 1 min"],
  [12 * 60_000, "12 min"],
  [130 * 60_000, "2 h 10 min"],
  [120 * 60_000, "2 h"],
  [3 * 24 * 3_600_000, "3 d"],
  [-10, "< 1 min"],
])("duration(%i) = %s", (ms, txt) => {
  expect(duration(ms)).toBe(txt);
});

test("ago", () => {
  expect(ago(1_000, 30_000)).toBe("agora");
  expect(ago(0, 12 * 60_000)).toBe("há 12 min");
});
