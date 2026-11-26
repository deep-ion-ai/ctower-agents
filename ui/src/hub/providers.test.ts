import { PROVIDER_INFOS } from "../protocol/gen/providers";
import {
  CRED_TEXT,
  PROVIDERS,
  acceptsCredential,
  projectRef,
  providerLabel,
  providerOf,
  refusedCredentialText,
  usesConnection,
} from "./providers";

test("o registro segue o descritor do Rust: ordem, rótulos e formas de credencial", () => {
  expect(PROVIDERS.map((p) => p.kind)).toEqual(PROVIDER_INFOS.map((i) => i.kind));
  expect(PROVIDERS.map((p) => p.label)).toEqual(["Jira", "GitLab", "GitHub"]);
  expect(providerOf("jira").credential_kinds).toEqual(["secret", "command", "file"]);
  expect(providerOf("gitlab").credential_kinds).toEqual(["secret", "command", "file"]);
  expect(acceptsCredential("github", "cli")).toBe(true);
  expect(acceptsCredential("jira", "cli")).toBe(false);
  expect(providerOf("github").default_connection).toBe("github");
  expect(providerOf("jira").default_connection).toBeNull();
  expect(providerLabel("gitlab")).toBe("GitLab");
  // Toda forma de credencial tem rótulo no seletor.
  for (const p of PROVIDERS) for (const k of p.credential_kinds) expect(CRED_TEXT[k]).toBeTruthy();
});

test("textos do formulário por tipo, sem comparar tipos nas telas", () => {
  expect(providerOf("jira").endpointLabel).toBe("URL");
  expect(providerOf("gitlab").endpointLabel).toBe("Host");
  expect(providerOf("github").endpointLabel).toBe("API");
  expect(providerOf("jira").options.map((o) => o.key)).toEqual(["config_file"]);
  expect(providerOf("github").cliTokenCmd).toBe("gh auth token");
  expect(providerOf("gitlab").cliTokenCmd).toBeUndefined();
});

test("forma recusada: as aceitas e quem tem o login da CLI", () => {
  expect(refusedCredentialText("jira")).toBe(
    "Jira: a credencial precisa ser um secret do hub, um comando ou um arquivo; o login da CLI é só do GitHub",
  );
});

test("referência e uso da conexão pelos lados do projeto, com a padrão do tipo", () => {
  const gh = { tracker: { kind: "github" as const }, forge: { kind: "github" as const, connection: 4 } };
  expect(projectRef(gh, "github")).toBe(4);
  expect(projectRef(gh, "jira")).toBeUndefined();
  expect(projectRef({ tracker: { kind: "github" }, forge: { kind: "github" } }, "github")).toBeNull();
  const jira = { tracker: { kind: "jira" as const, connection: 7 }, forge: { kind: "gitlab" as const, connection: 8 } };
  expect(usesConnection({ id: 7, name: "jira-acme", kind: "jira" }, jira)).toBe(true);
  // O id bate, mas o tipo não: a conexão é de outro lado.
  expect(usesConnection({ id: 8, name: "jira-beta", kind: "jira" }, jira)).toBe(false);
  expect(usesConnection({ id: 8, name: "gitlab-acme", kind: "gitlab" }, jira)).toBe(true);
  // Sem referência, só a padrão do tipo (a `github`).
  const semRef = { tracker: { kind: "github" as const }, forge: { kind: "github" as const } };
  expect(usesConnection({ id: 1, name: "github", kind: "github" }, semRef)).toBe(true);
  expect(usesConnection({ id: 2, name: "github-acme", kind: "github" }, semRef)).toBe(false);
  expect(usesConnection({ id: 1, name: "github", kind: "github" }, jira)).toBe(false);
});
