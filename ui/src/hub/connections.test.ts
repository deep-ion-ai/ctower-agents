import type { Connection } from "../protocol/gen";
import {
  CRED_KINDS,
  EMPTY_CONNECTION_FORM,
  connectionFormOf,
  connectionLabel,
  connectionParams,
  connectionUsers,
  connectionsOfKind,
  credentialText,
  readinessText,
  renameLockedText,
} from "./connections";
import { hubProject, hubProjectView, jiraHubProject } from "../test/fixtures";

const JIRA: Connection = {
  id: 7,
  name: "jira-acme",
  kind: "jira",
  endpoint: "https://jira.example.com",
  credential: { kind: "command", cmd: "pass show jira | head -1" },
  options: { config_file: "~/.config/.jira/acme.yml" },
  created_at: 1,
  updated_at: 1,
};

test("cadastro novo: tipo, endereço e credencial; o arquivo do jira-cli só no Jira", () => {
  const f = {
    ...EMPTY_CONNECTION_FORM,
    name: " jira-acme ",
    endpoint: "https://jira.example.com",
    cred_value: "pass show jira | head -1",
    config_file: " ~/.config/.jira/acme.yml ",
  };
  expect(connectionParams(f)).toEqual({
    params: {
      name: "jira-acme",
      endpoint: "https://jira.example.com",
      credential: { kind: "command", cmd: "pass show jira | head -1" },
      options: { config_file: "~/.config/.jira/acme.yml" },
      kind: "jira",
    },
  });
  const gh = connectionParams({ ...EMPTY_CONNECTION_FORM, kind: "github", name: "github-acme", cred: "cli", config_file: "x" });
  expect(gh).toEqual({
    params: { name: "github-acme", endpoint: "", credential: { kind: "cli" }, options: {}, kind: "github" },
  });
});

test("edição: com id e sem o tipo, que não muda", () => {
  const p = connectionParams(connectionFormOf(JIRA), 7);
  expect(p).toEqual({
    params: {
      id: 7,
      name: "jira-acme",
      endpoint: "https://jira.example.com",
      credential: { kind: "command", cmd: "pass show jira | head -1" },
      options: { config_file: "~/.config/.jira/acme.yml" },
    },
  });
});

test("ida e volta do formulário com a credencial de cada forma", () => {
  expect(connectionFormOf(JIRA)).toMatchObject({
    cred: "command",
    cred_value: "pass show jira | head -1",
    config_file: "~/.config/.jira/acme.yml",
  });
  // Secret do Jira (cadastro antigo): o formulário mostra a forma, e a
  // gravação diz o que falta, sem mandar ao hub.
  const secret = { ...JIRA, credential: { kind: "secret" as const, name: "JIRA_TOKEN" } };
  expect(connectionFormOf(secret)).toMatchObject({ cred: "secret", cred_value: "JIRA_TOKEN" });
  expect(connectionParams(connectionFormOf(secret), 7)).toEqual({
    error:
      "Jira: a credencial precisa ser um comando que imprime o token; secret, arquivo e CLI ainda não chegam ao ambiente",
  });
});

test("Jira e GitLab só aceitam comando de credencial; GitHub aceita as formas", () => {
  expect(CRED_KINDS.jira).toEqual(["command"]);
  expect(CRED_KINDS.gitlab).toEqual(["command"]);
  expect(CRED_KINDS.github).toContain("cli");
  expect(
    connectionParams({ ...EMPTY_CONNECTION_FORM, name: "gitlab-acme", kind: "gitlab", cred: "file", cred_value: "/t" }),
  ).toMatchObject({ error: expect.stringContaining("GitLab: a credencial precisa ser um comando que imprime o token") });
});

test("credencial que falta: o erro diz o que informar", () => {
  expect(connectionParams({ ...EMPTY_CONNECTION_FORM, name: "jira-acme" })).toEqual({
    error: "informe o comando que imprime o token",
  });
  expect(
    connectionParams({ ...EMPTY_CONNECTION_FORM, cred: "secret", cred_value: "JIRA_TOKEN", name: "jira-acme" }),
  ).toMatchObject({ error: expect.stringContaining("Jira:") });
  expect(
    connectionParams({ ...EMPTY_CONNECTION_FORM, kind: "github", cred: "secret", cred_value: " ", name: "github-acme" }),
  ).toEqual({ error: "informe o nome do secret do hub" });
  expect(connectionParams({ ...EMPTY_CONNECTION_FORM, cred_value: "x" })).toEqual({
    error: "informe o nome da conexão (ex.: jira-empresa)",
  });
});

test("textos: a credencial sem valor de secret, a prontidão e o rótulo", () => {
  expect(credentialText(JIRA.credential)).toBe("comando: pass show jira | head -1");
  expect(credentialText({ kind: "none" })).toBe("sem autenticação");
  expect(readinessText({ state: "ready" })).toEqual({ ok: true, text: "pronta" });
  expect(readinessText({ state: "needs_setup", reason: "informe o comando de credencial" })).toEqual({
    ok: false,
    text: "informe o comando de credencial",
  });
  expect(connectionLabel(JIRA)).toBe("jira-acme (https://jira.example.com)");
  expect(connectionLabel({ ...JIRA, endpoint: "" })).toBe("jira-acme");
});

test("conexões de um tipo, por nome", () => {
  const gl = { ...JIRA, id: 8, name: "gitlab-acme", kind: "gitlab" as const };
  const jiraB = { ...JIRA, id: 9, name: "jira-beta" };
  expect(connectionsOfKind([JIRA, gl, jiraB], "jira").map((c) => c.name)).toEqual(["jira-acme", "jira-beta"]);
  expect(connectionsOfKind([JIRA, gl], "github")).toEqual([]);
});

test("connectionUsers lista os projetos que usam a conexão no rastreador ou na forja", () => {
  const projects = [
    hubProjectView(jiraHubProject()),
    hubProjectView(jiraHubProject({ id: 8, gh_owner: "acme", gh_repo: "acme-web" })),
    hubProjectView(hubProject()),
  ];
  expect(connectionUsers(7, projects)).toEqual(["acme/acme-web", "grupo/projeto/app"]);
  expect(connectionUsers(8, projects)).toEqual(["acme/acme-web", "grupo/projeto/app"]);
  expect(connectionUsers(9, projects)).toEqual([]);
  expect(renameLockedText("jira-acme", ["acme/acme-web"])).toContain('[connections."jira-acme"]');
});
