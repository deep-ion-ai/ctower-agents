import { jiraProject, project, session, workItem } from "../test/fixtures";
import { actionUrl, restartTarget, sessionActions, type SessionAction } from "./actions";
import type { TabInfo } from "./SessionTree";

const P = project();
const ISSUE: TabInfo = { s: session({ tool_session_id: "tool-1" }), wi: workItem({ gh_number: 32 }), project: P };
const EPIC: TabInfo = { ...ISSUE, wi: workItem({ gh_number: 50, kind: "epic" }) };
const ZERO: TabInfo = { ...ISSUE, wi: workItem({ gh_number: 0, branch: null, worktree_path: null }) };
const GESTOR: TabInfo = { s: session({ tool_session_id: "tool-g" }), gestor: true, scope: { kind: "health" } };

/** "Retomar (bar)", "Abrir PR (menu, off)"… para comparar de uma vez. */
const view = (list: SessionAction[]) =>
  list.map((a) => `${a.label}${a.place === "bar" ? " (bar)" : ""}${a.disabled ? " (off)" : ""}`);

test("issue e épico: Abrir issue, PR, item no projeto, VS Code e Reiniciar; Retomar só encerrada", () => {
  const menu = ["Abrir issue no GitHub", "Abrir PR", "Abrir item no projeto", "Abrir no VS Code", "Reiniciar"];
  expect(view(sessionActions({ info: ISSUE, ended: false, vscode: true }))).toEqual(menu);
  expect(view(sessionActions({ info: EPIC, ended: false, vscode: true }))).toEqual(menu);
  expect(view(sessionActions({ info: ISSUE, ended: true, vscode: true }))).toEqual(["Retomar (bar)", ...menu]);
  expect(view(sessionActions({ info: EPIC, ended: true, vscode: true }))).toEqual(["Retomar (bar)", ...menu]);
  // Fora do WSL, o VS Code fica no menu, desabilitado com o motivo.
  const mac = sessionActions({ info: ISSUE, ended: false, vscode: false }).find((a) => a.id === "vscode");
  expect(mac).toMatchObject({ disabled: true, title: "Só nos ambientes WSL" });
});

test("Retomar precisa do id da ferramenta e não vale para a arquivada", () => {
  const ids = (info: TabInfo) => sessionActions({ info, ended: true, vscode: true }).map((a) => a.id);
  expect(ids({ ...ISSUE, s: session({ tool_session_id: null }) })).not.toContain("resume");
  expect(ids({ ...ISSUE, s: session({ tool_session_id: "t", state: "archived" }) })).not.toContain("resume");
  expect(ids({ ...ISSUE, s: session({ tool_session_id: "t", state: "interrupted" }) })).toContain("resume");
});

test("sessão do projeto (#0): repositório no lugar da issue, sem PR nem item; VS Code no clone principal", () => {
  expect(view(sessionActions({ info: ZERO, ended: false, vscode: true }))).toEqual([
    "Abrir repositório no GitHub",
    "Abrir no VS Code",
    "Reiniciar",
  ]);
  expect(view(sessionActions({ info: ZERO, ended: true, vscode: true }))[0]).toBe("Retomar (bar)");
});

test("gestor: sem Abrir issue nem VS Code; Revisar decisões e Reiniciar", () => {
  expect(view(sessionActions({ info: GESTOR, ended: false, vscode: true }))).toEqual([
    "Revisar decisões do gestor",
    "Reiniciar",
  ]);
  expect(view(sessionActions({ info: GESTOR, ended: true, vscode: true }))).toEqual([
    "Retomar (bar)",
    "Revisar decisões do gestor",
    "Reiniciar",
  ]);
});

test("sem o contexto da sessão ainda, nenhuma ação", () => {
  expect(sessionActions({ info: {}, ended: false, vscode: true })).toEqual([]);
});

test("URLs da issue e do repositório", () => {
  expect(actionUrl("open-issue", ISSUE)).toBe("https://github.com/deep-ion-ai/ctower-agents/issues/32");
  expect(actionUrl("open-issue", EPIC)).toBe("https://github.com/deep-ion-ai/ctower-agents/issues/50");
  expect(actionUrl("open-repo", ZERO)).toBe("https://github.com/deep-ion-ai/ctower-agents");
  // Jira e GitLab (#352): a issue no Jira, o repositório no GitLab.
  const J = jiraProject();
  const jira = { ...ISSUE, project: J };
  expect(sessionActions({ info: jira, ended: false, vscode: true })[0].label).toBe("Abrir issue no Jira");
  expect(actionUrl("open-issue", { ...jira, wi: workItem({ gh_number: 5450 }) })).toMatch(/\/browse\/PROJ-5450$/);
  expect(sessionActions({ info: { ...ZERO, project: J }, ended: false, vscode: true })[0].label).toBe(
    "Abrir repositório no GitLab",
  );
});

test("Reiniciar: o mesmo item, modelo e modo; #0 sem número; gestor com o escopo", () => {
  const tab = { env: "wsl", session: "s" };
  const s = session({ model: "opus", mode: "subagents" });
  expect(restartTarget(tab, { ...ISSUE, s }, true)).toEqual({
    env: "wsl",
    replace: tab,
    kill: true,
    model: "opus",
    mode: "subagents",
    projectId: 1,
    ghNumber: 32,
  });
  expect(restartTarget(tab, { ...ZERO, s }, false)).toEqual({
    env: "wsl",
    replace: tab,
    kill: false,
    model: "opus",
    mode: "subagents",
    projectId: 1,
  });
  expect(restartTarget(tab, GESTOR, false)).toMatchObject({ gestor: { kind: "health" }, replace: tab });
  expect(restartTarget(tab, { ...GESTOR, scope: undefined }, false)).toMatchObject({ gestor: { kind: "portfolio" } });
});
