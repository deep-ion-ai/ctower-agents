import { expect, test } from "vitest";
import { highlightLines, sanitizeHighlight } from "./highlight";

test("realça linha a linha com o texto escapado", async () => {
  const out = await highlightLines(['let x = "<script>alert(1)</script>";', "// comentário"], "rust");
  expect(out).not.toBeNull();
  const [code, comment] = out!;
  expect(code).toContain('<span class="hljs-keyword">let</span>');
  expect(code).toContain("&lt;script&gt;");
  expect(code).not.toContain("<script>");
  expect(comment).toContain("hljs-comment");
});

test("linguagem não registrada: sem realce", async () => {
  expect(await highlightLines(["x"], "brainfuck")).toBeNull();
});

test("sanitizeHighlight deixa só <span class>", () => {
  expect(sanitizeHighlight('<span class="a" onclick="x()">t</span><img src=x onerror=alert(1)><b>b</b>')).toBe(
    '<span class="a">t</span>b',
  );
});
