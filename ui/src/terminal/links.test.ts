import { isOpenClick, linkActivator, mdLinkTarget, mdPathsIn, safeUrl } from "./links";

const ctrl = { ctrlKey: true, metaKey: false, button: 0 };
const plain = { ctrlKey: false, metaKey: false, button: 0 };

test("safeUrl aceita só http e https", () => {
  expect(safeUrl("https://github.com/o/r/pull/1")).toBe("https://github.com/o/r/pull/1");
  expect(safeUrl("http://localhost:5173")).toBe("http://localhost:5173/");
  expect(safeUrl("HTTPS://Exemplo.com/a b")).toBe("https://exemplo.com/a%20b");
  for (const uri of ["file:///etc/passwd", "javascript:alert(1)", "vscode://file/x", "ftp://h/f", "data:text/html,x", "não é url", ""]) {
    expect(safeUrl(uri)).toBeNull();
  }
});

test("Ctrl+clique abre; no macOS, Cmd+clique", () => {
  expect(isOpenClick(ctrl, false)).toBe(true);
  expect(isOpenClick(plain, false)).toBe(false);
  expect(isOpenClick({ ...ctrl, button: 2 }, false)).toBe(false);
  expect(isOpenClick(ctrl, true)).toBe(false);
  expect(isOpenClick({ ctrlKey: false, metaKey: true, button: 0 }, true)).toBe(true);
});

test("o tratador abre pelo openUrl, sem confirmação, e ignora o resto", async () => {
  const opened: string[] = [];
  const confirm = vi.spyOn(window, "confirm");
  const activate = linkActivator(async (u) => {
    opened.push(u);
  });
  activate(ctrl, "https://github.com");
  activate(plain, "https://ignorado.com");
  activate(ctrl, "javascript:alert(1)");
  expect(opened).toEqual(["https://github.com/"]);
  expect(confirm).not.toHaveBeenCalled();
});

test("falha ao abrir vai para onError", async () => {
  const errors: unknown[] = [];
  linkActivator(
    () => Promise.reject("bloqueado"),
    (e) => errors.push(e),
  )(ctrl, "https://x.com");
  await Promise.resolve();
  await Promise.resolve();
  expect(errors).toEqual(["bloqueado"]);
});

describe("caminhos .md no console (#167)", () => {
  test("acha absolutos, ./ e relativos; nunca dentro de URL", () => {
    const line = "Gravei /w/acme/docs/plano.md e ./notas.md, veja specs/a.MD; https://example.com/x/README.md";
    expect(mdPathsIn(line).map((m) => m.path)).toEqual(["/w/acme/docs/plano.md", "./notas.md", "specs/a.MD"]);
    const m = mdPathsIn("ok: docs/b.md")[0];
    expect("ok: docs/b.md".slice(m.start, m.end)).toBe("docs/b.md");
    expect(mdPathsIn("nada.mdx arquivo.md5 a.md.bak")).toEqual([]);
  });

  test("só dentro das pastas da sessão viram link; relativo é da pasta da sessão", () => {
    const roots = ["/w/acme/feat__1", "/srv/docs/"];
    expect(mdLinkTarget("/w/acme/feat__1/a.md", roots)).toBe("/w/acme/feat__1/a.md");
    expect(mdLinkTarget("/srv/docs/acme__web/_projeto/x.md", roots)).toBe("/srv/docs/acme__web/_projeto/x.md");
    expect(mdLinkTarget("./b.md", roots)).toBe("/w/acme/feat__1/b.md");
    expect(mdLinkTarget("specs/c.md", roots)).toBe("/w/acme/feat__1/specs/c.md");
    expect(mdLinkTarget("/w/acme/feat__10/a.md", roots)).toBeNull();
    expect(mdLinkTarget("/etc/a.md", roots)).toBeNull();
    expect(mdLinkTarget("../a.md", roots)).toBeNull();
    expect(mdLinkTarget("a.md", [])).toBeNull();
  });
});
