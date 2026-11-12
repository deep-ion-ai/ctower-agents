import type { AutonomyConfig, HubAssignment, HubEnv, HubProject } from "../protocol/hub-gen";
import {
  DEFAULT_AUTONOMOUS_TEMPLATE,
  MAX_AUTONOMOUS_TEMPLATE_BYTES,
  autonomousTemplateOf,
  autonomyEnvChoices,
  autonomyForm,
  autonomyFormError,
  autonomyOf,
  autonomyPatch,
  autonomyTemplatesPatch,
  autonomyError,
  autonomyWarning,
} from "./autonomy";

const OFF: AutonomyConfig = { enabled: false, env_id: null, branch: "", base_branch: "", strategy: "merge" };

const project = (autonomy?: Partial<AutonomyConfig>, report?: HubProject["autonomy_report"]): HubProject =>
  ({
    id: 1,
    gh_owner: "acme",
    gh_repo: "acme-web",
    name: "acme-web",
    base_branch: "develop",
    protected_branches: ["develop", "main"],
    created_at: 0,
    autonomy: autonomy ? { ...OFF, ...autonomy } : undefined,
    autonomy_report: report ?? null,
  }) as unknown as HubProject;

test("hub anterior à #181: sem modo", () => {
  expect(autonomyOf(project())).toBeNull();
  expect(autonomyWarning(project())).toBeNull();
});

test("aviso só com o modo ligado e a branch em conflito", () => {
  const conflito = {
    state: "conflict" as const,
    branch: "wip/autonoma",
    base: "develop",
    message: null,
    conflicts: ["app.txt"],
    pr_url: null,
    at: 1,
  };
  expect(autonomyWarning(project({ enabled: true, env_id: "env-auto" }, conflito))).toBe("wip/autonoma em conflito com a base");
  expect(autonomyWarning(project({ enabled: false }, conflito))).toBeNull();
  expect(autonomyWarning(project({ enabled: true, env_id: "e" }, { ...conflito, state: "ok" }))).toBeNull();
});

test("erro da branch: só com o modo ligado e o relato error", () => {
  const erro = {
    state: "error" as const,
    branch: "wip/autonoma",
    base: "develop",
    message: "push recusado (403)",
    conflicts: [],
    pr_url: null,
    at: 1,
  };
  expect(autonomyError(project({ enabled: true, env_id: "e" }, erro))).toBe("push recusado (403)");
  expect(autonomyWarning(project({ enabled: true, env_id: "e" }, erro))).toBeNull();
  expect(autonomyError(project({ enabled: false }, erro))).toBeNull();
  expect(autonomyError(project({ enabled: true, env_id: "e" }, { ...erro, state: "ok" }))).toBeNull();
});

test("formulário: padrões vazios no patch e erros", () => {
  const p = project({});
  const f = autonomyForm(autonomyOf(p)!, p);
  expect(f).toEqual({ enabled: false, envId: "", branch: "wip/autonoma", base: "develop", strategy: "merge", template: null });
  expect(autonomyPatch(f, p)).toEqual(OFF);
  expect(autonomyPatch({ ...f, enabled: true, envId: "env-auto", branch: "wip/acme", base: "main" }, p)).toEqual({
    enabled: true,
    env_id: "env-auto",
    branch: "wip/acme",
    base_branch: "main",
    strategy: "merge",
  });
  expect(autonomyFormError({ ...f, enabled: true }, p)).toMatch(/ambiente/);
  expect(autonomyFormError({ ...f, branch: "develop" }, p)).toMatch(/própria base/);
  expect(autonomyFormError({ ...f, branch: "main" }, p)).toMatch(/protegida/);
  expect(autonomyFormError(f, p)).toBeNull();
});

test("ambientes: atribuídos e marcados como autônomos", () => {
  const a = (env_id: string) => ({ env_id, env_name: env_id }) as HubAssignment;
  const e = (env_id: string, autonomous: boolean) => ({ env_id, autonomous }) as HubEnv;
  expect(autonomyEnvChoices([a("env-auto"), a("env-mac")], [e("env-auto", true), e("env-mac", false)]).map((x) => x.env_id)).toEqual([
    "env-auto",
  ]);
});

test("template autônomo (#191): campo do hub, patch do objeto inteiro e limite", () => {
  const templates = { issue: "Issue {{key}}", epic: "", project: "", autonomous: "" };
  const p = { ...project({}), session_templates: templates } as HubProject;
  // Hub sem o campo: não edita.
  expect(autonomousTemplateOf(project({}))).toBeNull();
  expect(autonomousTemplateOf({ ...p, session_templates: { issue: "", epic: "", project: "" } } as unknown as HubProject)).toBeNull();
  const f = autonomyForm(autonomyOf(p)!, p);
  expect(f.template).toBe("");
  expect(autonomyTemplatesPatch(f, p)).toBeNull();
  expect(autonomyTemplatesPatch({ ...f, template: "  " }, p)).toBeNull();
  expect(autonomyTemplatesPatch({ ...f, template: "Faça {{ref}}" }, p)).toEqual({ ...templates, autonomous: "Faça {{ref}}" });
  const own = { ...p, session_templates: { ...templates, autonomous: "Faça {{ref}}" } };
  expect(autonomyTemplatesPatch({ ...f, template: " \n" }, own)).toEqual(templates);
  expect(autonomyFormError({ ...f, template: "x".repeat(MAX_AUTONOMOUS_TEMPLATE_BYTES + 1) }, p)).toMatch(/64 KiB/);
  expect(DEFAULT_AUTONOMOUS_TEMPLATE).toContain("{{integration_branch}}");
});
