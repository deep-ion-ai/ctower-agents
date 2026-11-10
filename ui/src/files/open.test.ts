import { expect, test } from "vitest";
import { basename, dirname, fileTab, joinPath, linkedMd, readParams, scopeOf, sizeText } from "./open";

test("aba por arquivo: id estável, título pelo nome ou pelo título dado, escopo no endereço", () => {
  const t = fileTab("wsl", "/w/acme/docs/plano.md", { session: "s1" });
  expect(t).toEqual({
    id: "file:wsl:/w/acme/docs/plano.md",
    title: "plano.md",
    screen: { name: "file", env: "wsl", path: "/w/acme/docs/plano.md", session: "s1" },
  });
  const p = fileTab("wsl", "/d/a.md", { projectId: 3 }, "Análise");
  expect(p.title).toBe("Análise");
  expect(p.screen).toMatchObject({ projectId: 3, title: "Análise" });
  expect(scopeOf(p.screen as never)).toEqual({ projectId: 3 });
  expect(readParams({ session: "s1" }, "a.md")).toEqual({ session_id: "s1", path: "a.md" });
  expect(readParams({ projectId: 3 }, "/d/a.md")).toEqual({ project_id: 3, path: "/d/a.md" });
});

test("caminhos POSIX", () => {
  expect(basename("/w/a/b.md")).toBe("b.md");
  expect(dirname("/w/a/b.md")).toBe("/w/a");
  expect(dirname("/b.md")).toBe("/");
  expect(joinPath("/w/a", "./c/../d.md")).toBe("/w/a/d.md");
  expect(joinPath("/w/a", "/x/y.md")).toBe("/x/y.md");
});

test("links do documento: só .md locais viram arquivo; o resto fica de fora", () => {
  expect(linkedMd("./b.md", "/w/docs/a.md")).toBe("/w/docs/b.md");
  expect(linkedMd("../specs/c.md#secao", "/w/docs/a.md")).toBe("/w/specs/c.md");
  expect(linkedMd("espa%C3%A7o.md", "/w/a.md")).toBe("/w/espaço.md");
  for (const href of ["https://example.com/a.md", "mailto:x@example.com", "#topo", "//host/a.md", "img.png", "javascript:a.md"]) {
    expect(linkedMd(href, "/w/a.md")).toBeNull();
  }
});

test("tamanho legível", () => {
  expect(sizeText(812)).toBe("812 B");
  expect(sizeText(3500)).toBe("3,4 KiB");
  expect(sizeText(200 * 1024)).toBe("200 KiB");
});
