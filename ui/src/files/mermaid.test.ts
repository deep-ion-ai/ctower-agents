import { beforeEach, expect, test, vi } from "vitest";

// O Mermaid de verdade não desenha no jsdom: o módulo é trocado por um fake
// que devolve um SVG com conteúdo perigoso, para conferir a sanitização.
const initialize = vi.fn();
const parse = vi.fn(async (code: string) => {
  if (code.includes("erro")) throw new Error("Parse error on line 1");
  return true;
});
const render = vi.fn(async (id: string) => ({
  svg: `<svg id="${id}" onload="alert(1)"><style>#${id}{fill:red}</style><script>alert(2)</script><foreignObject><div>x</div></foreignObject><a href="javascript:alert(3)"><text>link</text></a><g><text>A</text></g></svg>`,
}));
vi.mock("mermaid", () => ({ default: { initialize, parse, render } }));

const { mermaidConfig, renderDiagram, sanitizeSvg } = await import("./mermaid");

beforeEach(() => {
  initialize.mockClear();
  render.mockClear();
});

test("configuração segura: strict, sem HTML nos rótulos, sem diretiva mudando segurança ou tema", () => {
  const c = mermaidConfig("dark");
  expect(c.securityLevel).toBe("strict");
  expect(c.htmlLabels).toBe(false);
  expect(c.flowchart.htmlLabels).toBe(false);
  expect(c.startOnLoad).toBe(false);
  expect(c.secure).toEqual(expect.arrayContaining(["securityLevel", "htmlLabels", "theme", "themeCSS"]));
  expect(c.theme).toBe("dark");
  expect(mermaidConfig("light").theme).toBe("default");
});

test("SVG sanitizado: sem script, evento, foreignObject nem link", async () => {
  const r = await renderDiagram("graph TD; A-->B", "light");
  expect(initialize).toHaveBeenCalledWith(expect.objectContaining({ securityLevel: "strict", theme: "default" }));
  if (!("svg" in r)) throw new Error("esperava svg");
  expect(r.svg).toContain("<text>A</text>");
  expect(r.svg).toContain("<style>");
  expect(r.svg).not.toMatch(/onload|<script|foreignObject|javascript:|<a /i);
  expect(sanitizeSvg('<svg><image href="x" onerror="alert(1)"/></svg>')).not.toContain("onerror");
});

test("erro de sintaxe vira { error } e não desenha", async () => {
  const r = await renderDiagram("graph erro", "dark");
  expect(r).toEqual({ error: "Parse error on line 1" });
  expect(render).not.toHaveBeenCalled();
});
