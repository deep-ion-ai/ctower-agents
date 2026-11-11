import { expect, test } from "vitest";
import { envInfo, workItem } from "../test/fixtures";
import { diffBlocked, diffParams, diffTab, fileLabel, languageOf, parsePatch, patchKey, splitRows } from "./diff";

const PATCH = [
  "diff --git a/src/a.rs b/src/a.rs",
  "index 1111111..2222222 100644",
  "--- a/src/a.rs",
  "+++ b/src/a.rs",
  "@@ -1,4 +1,5 @@ fn main() {",
  " um",
  "-dois",
  "-três",
  "+DOIS",
  "+TRÊS",
  "+quatro",
  " cinco",
  "@@ -10,2 +11,2 @@",
  "-fim",
  "\\ No newline at end of file",
  "+fim",
  "",
].join("\n");

test("parsePatch: hunks com números das linhas e o aviso de fim de arquivo", () => {
  const p = parsePatch(PATCH);
  expect(p.notes).toEqual([]);
  expect(p.hunks.map((h) => h.header)).toEqual(["@@ -1,4 +1,5 @@ fn main() {", "@@ -10,2 +11,2 @@"]);
  const [h1, h2] = p.hunks;
  expect(h1.lines.map((l) => [l.kind, l.oldNo, l.newNo, l.text])).toEqual([
    ["ctx", 1, 1, "um"],
    ["del", 2, null, "dois"],
    ["del", 3, null, "três"],
    ["add", null, 2, "DOIS"],
    ["add", null, 3, "TRÊS"],
    ["add", null, 4, "quatro"],
    ["ctx", 4, 5, "cinco"],
  ]);
  expect(h2.lines.map((l) => l.kind)).toEqual(["del", "meta", "add"]);
  expect(h2.lines[1].text).toBe("No newline at end of file");
  // Linha de código que começa com "--" ou "++" não é cabeçalho dentro do hunk.
  const tricky = parsePatch("@@ -1 +1 @@\n--x\n++y\n");
  expect(tricky.hunks[0].lines.map((l) => [l.kind, l.text])).toEqual([
    ["del", "-x"],
    ["add", "+y"],
  ]);
});

test("parsePatch: binário, renomeação pura e novo arquivo vazio viram notas", () => {
  const bin = parsePatch("diff --git a/x.png b/x.png\nindex 1..2 100644\nBinary files a/x.png and b/x.png differ\n");
  expect(bin.hunks).toEqual([]);
  expect(bin.notes).toEqual(["Binary files a/x.png and b/x.png differ"]);
  const mv = parsePatch("diff --git a/a b/b\nsimilarity index 100%\nrename from a\nrename to b\n");
  expect(mv.notes).toEqual(["similarity index 100%", "rename from a", "rename to b"]);
});

test("splitRows: pares de removidas e adicionadas, sobra sozinha no seu lado", () => {
  const rows = splitRows(parsePatch(PATCH).hunks[0].lines);
  expect(rows.map((r) => [r.left?.text ?? null, r.right?.text ?? null])).toEqual([
    ["um", "um"],
    ["dois", "DOIS"],
    ["três", "TRÊS"],
    [null, "quatro"],
    ["cinco", "cinco"],
  ]);
});

test("languageOf: pela extensão ou pelo nome; desconhecida sem realce", () => {
  expect(languageOf("crates/x/src/lib.rs")).toBe("rust");
  expect(languageOf("ui/src/App.tsx")).toBe("typescript");
  expect(languageOf("Dockerfile")).toBe("dockerfile");
  expect(languageOf("Cargo.toml")).toBe("ini");
  expect(languageOf(".gitignore")).toBeNull();
  expect(languageOf("LICENSE")).toBeNull();
  expect(languageOf("x.qualquer")).toBeNull();
});

test("aba, parâmetros e chave do patch", () => {
  const t = diffTab("wsl", 7, "#12");
  expect(t).toEqual({ id: "diff:wsl:7", title: "Diff · #12", screen: { name: "diff", env: "wsl", workItem: 7, title: "#12" } });
  expect(diffTab("wsl", 7).title).toBe("Diff");
  expect(diffParams(7, "branch", "abc")).toEqual({ work_item_id: 7, mode: "branch" });
  expect(diffParams(7, "commit", "abc")).toEqual({ work_item_id: 7, mode: "commit", commit: "abc" });
  expect(patchKey("branch", "abc", { path: "a", old_path: null })).toBe("branch:::a");
  expect(patchKey("commit", "abc", { path: "b", old_path: "a" })).toBe("commit:abc:a:b");
  expect(fileLabel({ path: "b.txt", old_path: "a.txt" })).toBe("a.txt → b.txt");
  expect(fileLabel({ path: "b.txt", old_path: null })).toBe("b.txt");
});

test("diffBlocked: sem worktree, #0, desconectado e protocolo antigo", () => {
  const wi = workItem({ gh_number: 12 });
  expect(diffBlocked(wi, envInfo({ protocol: "2.15" }), true, "WSL")).toBeNull();
  // Sem env.info (web): libera; o ambiente responde.
  expect(diffBlocked(wi, undefined, true, "WSL")).toBeNull();
  expect(diffBlocked(wi, envInfo({ protocol: "2.14" }), true, "WSL")).toMatch(/Atualize o ambiente WSL .*2\.15/);
  expect(diffBlocked(wi, undefined, false, "WSL")).toBe("Ambiente WSL desconectado.");
  expect(diffBlocked({ ...wi, worktree_path: null }, undefined, true, "WSL")).toBe("Sem worktree no ambiente WSL.");
  expect(diffBlocked(workItem({ gh_number: 0 }), undefined, true, "WSL")).toMatch(/clone principal/);
  expect(diffBlocked(undefined, undefined, true, "WSL")).not.toBeNull();
});
