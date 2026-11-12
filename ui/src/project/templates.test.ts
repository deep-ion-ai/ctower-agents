import { DEFAULT_UI_CONFIG } from "../settings/defaults";
import { renderTemplate, resolveTemplate, sessionTemplateKind, templateFor, templateKeyFor } from "./templates";

const vars = {
  kind: "issue" as const,
  number: 33,
  owner: "deep-ion-ai",
  repo: "ctower-agents",
  branch: "feat/33-ui-projeto",
  protected: ["develop", "main"],
};

test("template de issue preenchido", () => {
  const p = renderTemplate(DEFAULT_UI_CONFIG.templates.issue, vars);
  expect(p).toContain("Trabalhe na issue #33 do repositório deep-ion-ai/ctower-agents (leia com `gh issue view 33`).");
  expect(p).toContain("Você pode mesclar apenas em feat/33-ui-projeto — nunca em develop, main.");
  expect(p).not.toMatch(/\{\{/);
});

test("issue e épico pedem `Closes #n` no PR (#375)", () => {
  expect(renderTemplate(DEFAULT_UI_CONFIG.templates.issue, vars)).toContain("`Closes #33`");
  const e = renderTemplate(DEFAULT_UI_CONFIG.templates.epic, { ...vars, kind: "epic", number: 27 });
  expect(e).toContain("`Closes #27` e um `Closes #<n>` por sub-issue");
});

test("épico usa o template próprio", () => {
  const t = templateFor(DEFAULT_UI_CONFIG.templates, "epic");
  const p = renderTemplate(t, { ...vars, kind: "epic", number: 27, branch: "wip/27-ui" });
  expect(p.startsWith("Trabalhe no épico #27 de deep-ion-ai/ctower-agents")).toBe(true);
  expect(renderTemplate("{{kind}}", { ...vars, kind: "epic" })).toBe("épico");
});

test("variável desconhecida fica intacta; espaços nas chaves são aceitos", () => {
  expect(renderTemplate("{{ number }} {{nada}}", vars)).toBe("33 {{nada}}");
});

// Projetos Jira + GitLab (#351, spec do Jira e GitLab §9).
const jira = {
  kind: "jira" as const,
  url: "https://jira.example.com/",
  project_key: "PROJ",
  jql: "",
  credential_cmd: "pass show jira | head -1",
  config_file: "",
  epic_link_field: "customfield_10601",
  epic_name_field: "customfield_10604",
  type_map: {},
};
const gitlab = {
  kind: "gitlab" as const,
  host: "gitlab.example.com",
  path: "grupo/projeto/app",
  clone_url: "https://gitlab.example.com/grupo/projeto/app.git",
  credential_cmd: "pass show gitlab | head -1",
};
const exemplo = {
  kind: "issue" as const,
  number: 5450,
  owner: "grupo/projeto",
  repo: "app",
  branch: "PROJ-5450",
  protected: ["develop", "main"],
  title: "Portar o plugin",
  base: "develop",
  tracker: jira,
  forge: gitlab,
};
const JIRA = 'JIRA_API_TOKEN="$(pass show jira | head -1)" jira';
const GLAB = 'GITLAB_HOST=gitlab.example.com GITLAB_TOKEN="$(pass show gitlab | head -1)" glab';

test("template escolhido pelo rastreador; GitHub com os de sempre", () => {
  const t = DEFAULT_UI_CONFIG.templates;
  expect(templateFor(t, "issue", "jira")).toBe(t.jira_issue);
  expect(templateFor(t, "epic", "jira")).toBe(t.jira_epic);
  expect(templateFor(t, "project", "jira")).toBe(t.jira_project);
  expect(templateFor(t, "issue")).toBe(t.issue);
  expect(templateFor(t, "epic", "github")).toBe(t.epic);
  expect(templateFor(t, "project", "github")).toBe(t.project);
  expect(templateKeyFor("issue", "jira")).toBe("jira_issue");
  expect(templateKeyFor("project")).toBe("project");
});

test("template de issue do Jira: chave, jira-cli e glab com o comando de credencial, MR comentado, sem transição", () => {
  const p = renderTemplate(DEFAULT_UI_CONFIG.templates.jira_issue, exemplo);
  expect(p).not.toMatch(/\{\{/);
  expect(p).toContain("Trabalhe na issue PROJ-5450 do Jira (https://jira.example.com/browse/PROJ-5450): Portar o plugin.");
  expect(p).toContain(`\`${JIRA} issue view PROJ-5450 --comments 20 --plain\``);
  expect(p).toContain(`\`${GLAB} mr create -R grupo/projeto/app -s PROJ-5450 -b develop -t '<título>' -d '<descrição>' -y\``);
  expect(p).toContain(`\`${JIRA} issue comment add PROJ-5450 'MR aberto: <url do MR>' --no-input\``);
  expect(p).toContain("nada de `jira issue move`");
  expect(p).toContain("`rules_get`");
  // Nada do GitHub, nem a convenção de um projeto só.
  expect(p).not.toMatch(/GitHub|gh issue|Closes|\bPR\b|\[PROJ/);
});

test("épico e planejamento do Jira: lista do épico e criação com Epic Link", () => {
  const e = renderTemplate(DEFAULT_UI_CONFIG.templates.jira_epic, { ...exemplo, kind: "epic", number: 5014, branch: "PROJ-5014" });
  expect(e).toContain("`" + JIRA + " epic list PROJ-5014 --plain`");
  expect(e).toContain("-P PROJ-5014 --no-input");
  expect(e).toContain("-b PROJ-5014 -t");
  const p = renderTemplate(DEFAULT_UI_CONFIG.templates.jira_project, { ...exemplo, number: 0, branch: "develop", title: "" });
  expect(p).toContain("Sessão de planejamento no clone principal de grupo/projeto/app (GitLab), com as issues no projeto PROJ do Jira");
  expect(p).toContain(`\`${JIRA} issue create -p PROJ -t <tipo>`);
  expect(p).toContain(`\`${JIRA} epic create -p PROJ`);
  for (const t of [e, p]) expect(t).not.toMatch(/\{\{|GitHub|gh issue|Closes|\bPR\b/);
});

test("variáveis novas: chave neutra e prefixos com JIRA_CONFIG_FILE e caminho com espaço", () => {
  expect(renderTemplate("{{key}} {{title}} {{base}}", { ...vars, title: "T", base: "develop" })).toBe("#33 T develop");
  expect(renderTemplate("{{forge_path}}", vars)).toBe("deep-ion-ai/ctower-agents");
  const cfg = { ...exemplo, tracker: { ...jira, config_file: "/home/u/meu jira.yml" } };
  expect(renderTemplate("{{jira}}", cfg)).toBe(`JIRA_CONFIG_FILE='/home/u/meu jira.yml' ${JIRA}`);
  expect(renderTemplate("{{jira_url}} {{jira_project}}", exemplo)).toBe("https://jira.example.com PROJ");
});

// Templates por projeto (#166): projeto > global do rastreador > padrão de fábrica.
describe("resolveTemplate", () => {
  const t = DEFAULT_UI_CONFIG.templates;
  const none = { issue: "", epic: "", project: "", autonomous: "", release: "" };

  test("sem override: idêntico ao de hoje, no GitHub e no Jira", () => {
    for (const ov of [undefined, null, none]) {
      for (const kind of ["issue", "epic", "project"] as const) {
        for (const tr of ["github", "jira"] as const) {
          const r = resolveTemplate(t, kind, tr, ov);
          expect(r).toEqual({ text: templateFor(t, kind, tr), source: "global", key: templateKeyFor(kind, tr) });
        }
      }
    }
    // O prompt renderizado não muda.
    expect(renderTemplate(resolveTemplate(t, "issue", "jira", none).text, exemplo)).toBe(
      renderTemplate(t.jira_issue, exemplo),
    );
  });

  test("override de issue: só a issue usa; épico e sessão do projeto ficam no global", () => {
    const ov = { ...none, issue: "Projeto: {{key}} {{title}}" };
    expect(resolveTemplate(t, "issue", "github", ov)).toEqual({ text: ov.issue, source: "project", key: "issue" });
    expect(resolveTemplate(t, "issue", "jira", ov)).toEqual({ text: ov.issue, source: "project", key: "jira_issue" });
    expect(resolveTemplate(t, "epic", "github", ov).source).toBe("global");
    expect(resolveTemplate(t, "project", "jira", ov)).toMatchObject({ text: t.jira_project, source: "global" });
    expect(renderTemplate(resolveTemplate(t, "issue", "jira", ov).text, exemplo)).toBe("Projeto: PROJ-5450 Portar o plugin");
  });

  test("override só com espaços herda; global vazio cai no padrão de fábrica", () => {
    expect(resolveTemplate(t, "epic", "github", { ...none, epic: "  \n" }).source).toBe("global");
    const vazio = { ...t, epic: "", jira_project: " " };
    expect(resolveTemplate(vazio, "epic")).toEqual({ text: DEFAULT_UI_CONFIG.templates.epic, source: "default", key: "epic" });
    expect(resolveTemplate(vazio, "project", "jira").source).toBe("default");
    // O override vence até o padrão de fábrica.
    expect(resolveTemplate(vazio, "epic", "github", { ...none, epic: "E" }).text).toBe("E");
  });

  test("todas as variáveis de hoje renderizam no override", () => {
    const ov = {
      ...none,
      issue:
        "{{kind}}|{{number}}|{{owner}}|{{repo}}|{{branch}}|{{protected}}|{{key}}|{{title}}|{{base}}|{{jira_url}}|{{jira_project}}|{{forge_path}}",
    };
    expect(renderTemplate(resolveTemplate(t, "issue", "jira", ov).text, exemplo)).toBe(
      "issue|5450|grupo/projeto|app|PROJ-5450|develop, main|PROJ-5450|Portar o plugin|develop|https://jira.example.com|PROJ|grupo/projeto/app",
    );
    const cli = renderTemplate(resolveTemplate(t, "issue", "jira", { ...none, issue: "{{jira}} / {{glab}}" }).text, exemplo);
    expect(cli).toBe(`${JIRA} / ${GLAB}`);
  });

  test("sub-issue e sub-tarefa usam o override de issue", () => {
    expect(sessionTemplateKind("issue")).toBe("issue");
    expect(sessionTemplateKind("epic")).toBe("epic");
    expect(sessionTemplateKind("project")).toBe("project");
  });
});
