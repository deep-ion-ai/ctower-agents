// Os mesmos casos de crates/ct-core/src/branch.rs.
import { branchFor, branchFromPattern, patternOf, slugify, worktreeDirName } from "./branch";

test("slug remove acentos e pontuação", () => {
  expect(slugify("Épico: Autenticação OAuth 2.0!", 60)).toBe("epico-autenticacao-oauth-2-0");
});

test("slug lida com emoji e espaços", () => {
  expect(slugify("  🚀 Lançar   versão  ", 60)).toBe("lancar-versao");
  expect(slugify("  --  ", 60)).toBe("");
});

test("slug trunca sem hífen no fim", () => {
  expect(slugify("aaaa bbbb cccc", 6)).toBe("aaaa-b");
  expect(slugify("aaaa bbbb", 5)).toBe("aaaa");
});

test("slug só tem caracteres seguros", () => {
  const s = slugify('Ação/çõ "aspas" <tag> ~^:?*[]\\ ünïcödé ß', 80);
  expect(s).toMatch(/^[a-z0-9-]+$/);
  expect(s.startsWith("-") || s.endsWith("-") || s.includes("--")).toBe(false);
});

test("branch de épico usa wip", () => {
  expect(branchFor("epic", 12, "Autenticação", [])).toBe("wip/12-autenticacao");
});

test("branch de issue por label", () => {
  expect(branchFor("issue", 7, "Corrige login", ["Bug"])).toBe("fix/7-corrige-login");
  expect(branchFor("issue", 8, "Limpa deps", ["chore"])).toBe("chore/8-limpa-deps");
  expect(branchFor("issue", 9, "Export CSV", ["enhancement"])).toBe("feat/9-export-csv");
});

test("branch sem slug usa só o número", () => {
  expect(branchFor("issue", 5, "🚀🚀", [])).toBe("feat/5");
});

test("diretório de worktree não tem barra", () => {
  expect(worktreeDirName("wip/12-auth")).toBe("wip__12-auth");
});

test("slug ignora acentos combinantes NFD", () => {
  expect(slugify("Autenticação", 40)).toBe("autenticacao");
});

// `padrao_do_jira` e `padrao_do_github_e_branch_for_iguais` do Rust (#352).
test("padrão do Jira e do exemplo", () => {
  const b = (p: string, kind: "issue" | "epic", labels: string[] = []) =>
    branchFromPattern(p, kind, 5450, "PROJ-5450", "Portar o plugin", labels);
  expect(b("{prefix}/{key}-{slug}", "issue")).toBe("feat/PROJ-5450-portar-o-plugin");
  expect(b("{prefix}/{key}-{slug}", "epic")).toBe("wip/PROJ-5450-portar-o-plugin");
  expect(b("{key}", "issue", ["bug"])).toBe("PROJ-5450");
  expect(b("{prefix}/{n}_{slug}", "issue", ["chore"])).toBe("chore/5450_portar-o-plugin");
  expect(branchFromPattern("{prefix}/{key}-{slug}", "issue", 5, "P-5", "🚀", [])).toBe("feat/P-5");
  expect(b("{prefix}/{slug}", "issue")).toBeNull();
  expect(b("{prefixo}/{n}", "issue")).toBeNull();
  expect(b("{prefix/{n}", "issue")).toBeNull();
  expect(patternOf({ tracker: { kind: "github" } })).toBe("{prefix}/{n}-{slug}");
  expect(patternOf({ tracker: { kind: "jira" } })).toBe("{prefix}/{key}-{slug}");
  expect(patternOf({ tracker: { kind: "jira" }, branch_pattern: "{key}" })).toBe("{key}");
});

test("padrão do GitHub e branchFor dão o mesmo nome", () => {
  for (const [kind, n, title, l] of [
    ["epic", 12, "Autenticação", []],
    ["issue", 7, "Corrige login", ["Bug"]],
    ["issue", 5, "🚀🚀", []],
  ] as const) {
    expect(branchFromPattern("{prefix}/{n}-{slug}", kind, n, String(n), title, [...l])).toBe(branchFor(kind, n, title, [...l]));
  }
});
