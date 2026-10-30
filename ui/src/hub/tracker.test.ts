import { jiraHubProject } from "../test/fixtures";
import {
  DEFAULT_TYPE_MAP,
  EMPTY_JIRA,
  branchPreview,
  hostsOf,
  hubSupportsJira,
  jiraFormOf,
  kindOf,
  parseTypeMap,
  providerParams,
} from "./tracker";

test("prévia do padrão de branch como o daemon gera", () => {
  expect(branchPreview("{prefix}/{key}-{slug}", "PROJ")).toBe("feat/PROJ-5450-portar-o-plugin");
  expect(branchPreview("{key}", "PROJ")).toBe("PROJ-5450");
  expect(branchPreview("{prefix}/{n}-{slug}", null)).toBe("feat/5450-portar-o-plugin");
  expect(branchPreview("{prefix}/{slug}", "X")).toBeNull();
  expect(branchPreview("{x}-{n}", "X")).toBeNull();
});

test("mapa de tipos: Tipo=papel, com os papéis conhecidos", () => {
  expect(parseTypeMap(DEFAULT_TYPE_MAP)).toEqual({ map: { Epic: "epic", Épico: "epic", Bug: "bug", Erro: "bug" } });
  expect(parseTypeMap("Sub-tarefa=ignore, ")).toEqual({ map: { "Sub-tarefa": "ignore" } });
  expect(parseTypeMap("Tarefa")).toHaveProperty("error");
  expect(parseTypeMap("=epic")).toHaveProperty("error");
});

test("formulário do cadastro ida e volta", () => {
  // Cadastro anterior ao modo sprint ativa e sem JQL: o formulário pede o JQL.
  const legado = jiraFormOf(jiraHubProject());
  expect(legado).toMatchObject({ active_sprint_only: false, jql: "", max_issues: "100" });
  expect(providerParams(legado)).toHaveProperty("error");
  const f = { ...legado, jql: "labels = web" };
  expect(f).toMatchObject({ jira_key: "PROJ", branch_pattern: "{key}", attribution: false, type_map: "Epic=epic, Bug=bug" });
  const pv = providerParams(f);
  expect(pv).toMatchObject({
    tracker: { kind: "jira", project_key: "PROJ", epic_link_field: "customfield_10601", jql: "labels = web", max_issues: 100 },
    forge: { kind: "gitlab", clone_url: "https://gitlab.example.com/grupo/projeto/app.git" },
  });
  expect(providerParams({ ...EMPTY_JIRA, type_map: "x" })).toHaveProperty("error");
  // Projeto novo: sprint ativa, sem JQL.
  expect(providerParams(EMPTY_JIRA)).toMatchObject({ tracker: { active_sprint_only: true, jql: "", max_issues: 100 } });
  expect(providerParams({ ...EMPTY_JIRA, active_sprint_only: false, jql: "x", max_issues: "5001" })).toHaveProperty("error");
});

test("kindOf: só GitHub + GitHub e GitLab + Jira são tipos de projeto", () => {
  expect(kindOf("github", "github")).toBe("github");
  expect(kindOf("gitlab", "jira")).toBe("jira");
  expect(kindOf("github", "jira")).toBeNull();
  expect(kindOf("gitlab", "github")).toBeNull();
  expect(hostsOf("jira")).toEqual({ code: "gitlab", issues: "jira" });
  expect(hostsOf("github")).toEqual({ code: "github", issues: "github" });
});

test("hubSupportsJira: protocolo 1.5 ou maior; sem hub.info não bloqueia", () => {
  expect(hubSupportsJira("1.5")).toBe(true);
  expect(hubSupportsJira("1.4")).toBe(false);
  expect(hubSupportsJira("1.3")).toBe(false);
  expect(hubSupportsJira("1.10")).toBe(true);
  expect(hubSupportsJira("2.0")).toBe(true);
  expect(hubSupportsJira("1.2")).toBe(false);
  expect(hubSupportsJira("1.0")).toBe(false);
  expect(hubSupportsJira("0.9")).toBe(false);
  expect(hubSupportsJira(null)).toBe(true);
  expect(hubSupportsJira(undefined)).toBe(true);
});
