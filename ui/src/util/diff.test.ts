import { diffStats, lineDiff } from "./diff";

test("diferença por linhas: iguais, removidas e incluídas na ordem", () => {
  const d = lineDiff("a\nb\nc\nd\n", "a\nc\nx\nd");
  expect(d).toEqual([
    { op: "same", text: "a" },
    { op: "del", text: "b" },
    { op: "same", text: "c" },
    { op: "add", text: "x" },
    { op: "same", text: "d" },
  ]);
  expect(diffStats(d)).toEqual({ added: 1, removed: 1 });
});

test("de vazio para texto, e de texto para vazio", () => {
  expect(lineDiff("", "um\ndois")).toEqual([
    { op: "add", text: "um" },
    { op: "add", text: "dois" },
  ]);
  expect(lineDiff("um", "")).toEqual([{ op: "del", text: "um" }]);
  expect(lineDiff("igual", "igual")).toEqual([{ op: "same", text: "igual" }]);
});

test("CRLF conta como LF", () => {
  expect(lineDiff("a\r\nb", "a\nb")).toEqual([
    { op: "same", text: "a" },
    { op: "same", text: "b" },
  ]);
});
