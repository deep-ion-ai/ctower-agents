import { expect, test } from "vitest";
import { hubProject } from "../test/fixtures";
import { dirLines, docsDirName, docsFormOf, docsPatch, docsText } from "./docs";

const ON = { enabled: true, project_dirs: ["analises", "specs/telas"], epic_dirs: ["prompts"] };

test("formulário do projeto: pastas uma por linha; hub sem docs = null", () => {
  expect(docsFormOf(hubProject({ docs: ON }))).toEqual({ enabled: true, project_dirs: "analises\nspecs/telas", epic_dirs: "prompts" });
  expect(docsFormOf({ ...hubProject(), docs: undefined as never })).toBeNull();
});

test("patch: só quando mudou, com o objeto inteiro; hub sem docs nunca recebe", () => {
  const p = hubProject({ docs: ON });
  expect(docsPatch(docsFormOf(p), p)).toBeNull();
  expect(docsPatch({ enabled: true, project_dirs: " analises/ \n\nspecs/telas", epic_dirs: "prompts\n" }, p)).toBeNull();
  expect(docsPatch({ enabled: false, project_dirs: "analises\nspecs/telas", epic_dirs: "prompts" }, p)).toEqual({ ...ON, enabled: false });
  expect(docsPatch({ enabled: true, project_dirs: "analises", epic_dirs: "" }, p)).toEqual({
    enabled: true,
    project_dirs: ["analises"],
    epic_dirs: [],
  });
  expect(docsPatch(null, p)).toBeNull();
  expect(docsPatch({ enabled: true, project_dirs: "", epic_dirs: "" }, { ...p, docs: undefined as never })).toBeNull();
});

test("linhas, resumo e pasta do projeto", () => {
  expect(dirLines(" a \n\n/b/c/\n")).toEqual(["a", "b/c"]);
  expect(docsText({ enabled: false, project_dirs: ["a"], epic_dirs: [] })).toBe("desligados");
  expect(docsText({ enabled: true, project_dirs: [], epic_dirs: ["p"] })).toBe("ligados · _projeto: nenhuma pasta · épico: p");
  expect(docsDirName("grupo/projeto", "api")).toBe("grupo__projeto__api");
});
