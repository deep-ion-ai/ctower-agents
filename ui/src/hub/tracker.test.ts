import type { HubTrackerStatus } from "../protocol/hub-gen";
import { jiraHubProject } from "../test/fixtures";
import {
  DEFAULT_TYPE_MAP,
  EMPTY_JIRA,
  branchPreview,
  hostsOf,
  hubSupportsJira,
  extraFieldErrors,
  extraFieldsChanged,
  extraFieldsParam,
  jiraFieldCatalog,
  jiraFormOf,
  kindOf,
  parseTypeMap,
  providerParams,
  searchJiraFields,
  typedFieldId,
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

test("catálogo de campos do Jira vem do tracker_status, ou vazio", () => {
  const field = { id: "customfield_10099", name: "Responsável pela implementação", datatype: "user" };
  const status = (custom_fields: unknown) =>
    ({
      env_id: "e",
      env_name: "e",
      received_at: 1,
      readiness: { jira: { custom_fields } },
    }) as unknown as HubTrackerStatus;
  expect(jiraFieldCatalog({ tracker_status: status([field]) })).toEqual([field]);
  expect(jiraFieldCatalog({ tracker_status: null })).toEqual([]);
  // Hub anterior: o status chega sem a chave.
  expect(jiraFieldCatalog({ tracker_status: status(undefined) })).toEqual([]);
});

const CATALOG = [
  { id: "customfield_10099", name: "Responsável pela implementação", datatype: "user" },
  { id: "customfield_10098", name: "Responsável pela revisão", datatype: "user" },
  { id: "customfield_10097", name: "Sistema", datatype: "option" },
];

test("busca no catálogo por nome ou ID, sem maiúsculas nem acentos, com teto", () => {
  const ids = (q: string) => searchJiraFields(CATALOG, q).options.map((f) => f.id);
  expect(ids("responsavel")).toEqual(["customfield_10099", "customfield_10098"]);
  expect(ids("IMPLEMENTAÇÃO")).toEqual(["customfield_10099"]);
  expect(ids("10097")).toEqual(["customfield_10097"]);
  expect(ids("")).toHaveLength(3);
  expect(searchJiraFields(CATALOG, "respons", 1)).toEqual({ options: [CATALOG[0]], more: 1 });
});

test("ID digitado na combobox sem escolher a opção", () => {
  // Sem catálogo: o texto é o ID.
  expect(typedFieldId([], " customfield_1 ")).toBe("customfield_1");
  // Com catálogo: ID exato, nome exato (sem acento), ou customfield_ fora dele.
  expect(typedFieldId(CATALOG, "CUSTOMFIELD_10099")).toBe("customfield_10099");
  expect(typedFieldId(CATALOG, "sistema")).toBe("customfield_10097");
  expect(typedFieldId(CATALOG, "customfield_999")).toBe("customfield_999");
  expect(typedFieldId(CATALOG, "Respons")).toBe("");
  expect(typedFieldId(CATALOG, "  ")).toBe("");
});

test("campos do projeto: ida e volta no formulário e validação como no hub", () => {
  // Cadastro anterior, sem a lista.
  expect(jiraFormOf(jiraHubProject()).extra_fields).toEqual([]);
  const p = jiraHubProject();
  if (p.tracker?.kind === "jira") {
    Object.assign(p.tracker, { extra_fields: [{ id: "customfield_10099", label: "Implementação" }] });
  }
  const f = { ...jiraFormOf(p), jql: "x" };
  expect(f.extra_fields).toEqual([{ id: "customfield_10099", label: "Implementação" }]);
  const extra = [
    { id: " customfield_10099 ", label: " Implementação " },
    { id: "customfield_10097", label: "Sistema" },
  ];
  expect(providerParams({ ...f, extra_fields: extra })).toMatchObject({
    tracker: {
      extra_fields: [
        { id: "customfield_10099", label: "Implementação" },
        { id: "customfield_10097", label: "Sistema" },
      ],
    },
  });
  const err = (rows: { id: string; label: string }[]) => {
    const r = extraFieldsParam(rows);
    return "error" in r ? r.error : null;
  };
  expect(err([{ id: "", label: "" }])).toMatch(/informe o campo do Jira na linha 1/);
  expect(err([{ id: "customfield_1", label: " " }])).toMatch(/informe o rótulo na tela do campo customfield_1/);
  expect(err([extra[0], { id: "CUSTOMFIELD_10099", label: "B" }])).toMatch(
    /o campo CUSTOMFIELD_10099 aparece mais de uma vez \(linha 2\)/,
  );
  const many = Array.from({ length: 21 }, (_, i) => ({ id: `customfield_${i}`, label: "X" }));
  expect(err(many)).toMatch(/no máximo 20 campos/);
  expect(err(many.slice(0, 20))).toBeNull();
  expect(providerParams({ ...f, extra_fields: many })).toHaveProperty("error");
  // Erros por linha, para mostrar junto dela.
  expect(
    extraFieldErrors([
      { id: "customfield_1", label: "A" },
      { id: "customfield_1", label: "B" },
      { id: "customfield_2", label: "" },
      { id: "", label: "D" },
      { id: "", label: "" },
    ]),
  ).toEqual([
    null,
    "Campo repetido: customfield_1 já está na linha 1.",
    "Informe o rótulo na tela.",
    "Escolha um campo da lista ou digite o ID (customfield_…).",
    null,
  ]);
  // Mudança na lista (espaços à parte) entra na edição.
  expect(extraFieldsChanged([{ id: "a ", label: "b" }], [{ id: "a", label: " b" }])).toBe(false);
  expect(extraFieldsChanged([{ id: "a", label: "b" }], [{ id: "a", label: "c" }])).toBe(true);
  expect(extraFieldsChanged([], [{ id: "a", label: "b" }])).toBe(true);
});
