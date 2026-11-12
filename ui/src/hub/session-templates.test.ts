import type { HubProject } from "../protocol/hub-gen";
import { DEFAULT_UI_CONFIG } from "../settings/defaults";
import { customizeStart, hasOverride, sessionTemplatesOf, templatesPatch } from "./session-templates";

const none = { issue: "", epic: "", project: "", autonomous: "", release: "" };
const proj = (extra: Partial<HubProject> = {}) => ({ tracker: { kind: "github" }, ...extra }) as unknown as HubProject;

test("hub anterior à #166 não traz session_templates: não edita", () => {
  expect(sessionTemplatesOf(proj())).toBeNull();
  expect(sessionTemplatesOf(proj({ session_templates: none }))).toEqual(none);
});

test("override só com chave não vazia", () => {
  const t = { ...none, epic: "Épico {{key}}" };
  expect(hasOverride(t, "epic")).toBe(true);
  expect(hasOverride(t, "issue")).toBe(false);
  expect(hasOverride({ ...none, issue: "  \n" }, "issue")).toBe(false);
});

test("Personalizar parte do global do rastreador do projeto", () => {
  const t = { ...DEFAULT_UI_CONFIG.templates, issue: "Global GitHub", jira_issue: "Global Jira" };
  expect(customizeStart(t, proj(), "issue")).toBe("Global GitHub");
  expect(customizeStart(t, proj({ tracker: { kind: "jira" } as HubProject["tracker"] }), "issue")).toBe("Global Jira");
  // Global vazio: o padrão de fábrica.
  expect(customizeStart({ ...t, epic: "" }, proj(), "epic")).toBe(DEFAULT_UI_CONFIG.templates.epic);
});

test("patch troca só a chave, manda o objeto inteiro e volta ao global com vazio", () => {
  const cur = { ...none, project: "Planeje" };
  expect(templatesPatch(cur, "issue", "Issue {{key}}")).toEqual({ issue: "Issue {{key}}", epic: "", project: "Planeje", autonomous: "", release: "" });
  expect(templatesPatch(cur, "project", "")).toEqual(none);
  expect(templatesPatch(cur, "project", "   ")).toEqual(none);
  expect(templatesPatch(cur, "project", "Planeje")).toBeNull();
  expect(templatesPatch(none, "epic", " ")).toBeNull();
});
