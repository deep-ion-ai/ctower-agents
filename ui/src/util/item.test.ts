import { itemLabel } from "../control-room/attention";
import { jiraProject, project, workItem } from "../test/fixtures";
import {
  changeLabel,
  changeNewUrl,
  changeRef,
  changeShort,
  changeUrl,
  forgeLabel,
  itemFullRef,
  itemKey,
  itemRef,
  itemShort,
  itemTitle,
  itemUrl,
  numberQuery,
  repoUrl,
  trackerLabel,
} from "./item";

test("work item #0 é a sessão do projeto", () => {
  expect(itemRef(0)).toBe("sessão do projeto");
  expect(itemRef(31)).toBe("#31");
  expect(itemRef(0, jiraProject())).toBe("sessão do projeto");
  expect(itemTitle({ gh_number: 0, title: null })).toBe("Sessão do projeto");
  expect(itemTitle({ gh_number: 31, title: "ui" })).toBe("#31 ui");
  expect(itemLabel(project(), workItem({ gh_number: 0, title: null }))).toBe("ctower-agents · sessão do projeto");
});

// Mesmos casos dos testes de `ct_core::tracker` (item_* e change_*).
test("GitHub: #n, PR e links do github.com, como antes da #352", () => {
  const p = project();
  expect(itemFullRef(p, 12)).toBe("deep-ion-ai/ctower-agents#12");
  expect(itemShort(p, 12)).toBe("#12");
  expect(itemKey(p, 12)).toBe("12");
  expect(itemUrl(p, 12)).toBe("https://github.com/deep-ion-ai/ctower-agents/issues/12");
  expect(itemRef(12, p)).toBe("#12");
  expect(itemTitle({ gh_number: 12, title: "ui" }, p)).toBe("#12 ui");
  expect(changeLabel(p)).toBe("PR");
  expect(changeShort(p, 30)).toBe("#30");
  expect(changeRef(p, 30)).toBe("PR #30");
  expect(changeUrl(p, 30)).toBe("https://github.com/deep-ion-ai/ctower-agents/pull/30");
  expect(changeNewUrl(p, "feat/12-x", "develop")).toBe(
    "https://github.com/deep-ion-ai/ctower-agents/compare/develop...feat%2F12-x?expand=1",
  );
  expect(repoUrl(p)).toBe("https://github.com/deep-ion-ai/ctower-agents");
  expect(trackerLabel(p)).toBe("GitHub");
  expect(forgeLabel(p)).toBe("GitHub");
});

test("sem projeto (ou sem tracker/forge de hub antigo) vale o GitHub", () => {
  expect(itemShort(null, 3)).toBe("#3");
  expect(itemFullRef(undefined, 3)).toBe("#3");
  expect(changeRef(undefined, 3)).toBe("PR #3");
  expect(itemFullRef({ gh_owner: "o", gh_repo: "r" }, 3)).toBe("o/r#3");
  expect(trackerLabel(null)).toBe("GitHub");
});

test("Jira + GitLab: chave, /browse/, MR e !iid", () => {
  const p = jiraProject();
  expect(itemFullRef(p, 5450)).toBe("PROJ-5450");
  expect(itemShort(p, 5450)).toBe("PROJ-5450");
  expect(itemKey(p, 5450)).toBe("PROJ-5450");
  // A barra final da URL do cadastro some, como no `item_url`.
  expect(itemUrl(p, 5450)).toBe("https://jira.example.com/browse/PROJ-5450");
  expect(itemRef(5450, p)).toBe("PROJ-5450");
  expect(itemTitle({ gh_number: 5450, title: "portar" }, p)).toBe("PROJ-5450 portar");
  expect(changeLabel(p)).toBe("MR");
  expect(changeShort(p, 7)).toBe("!7");
  expect(changeRef(p, 7)).toBe("MR !7");
  expect(changeUrl(p, 7)).toBe("https://gitlab.example.com/grupo/projeto/app/-/merge_requests/7");
  expect(changeNewUrl(p, "feat/PROJ-5450-x", "develop")).toBe(
    "https://gitlab.example.com/grupo/projeto/app/-/merge_requests/new?merge_request[source_branch]=feat%2FPROJ-5450-x&merge_request[target_branch]=develop",
  );
  expect(repoUrl(p)).toBe("https://gitlab.example.com/grupo/projeto/app");
  expect(trackerLabel(p)).toBe("Jira");
  expect(forgeLabel(p)).toBe("GitLab");
});

test("GitLab sem caminho no cadastro usa owner/repo", () => {
  const p = jiraProject({ forge: { kind: "gitlab", host: "gl.x", path: "/", clone_url: "", credential_cmd: "" } });
  expect(changeUrl(p, 2)).toBe("https://gl.x/grupo/projeto/app/-/merge_requests/2");
});

test("busca por número: #43, 43 e a chave do Jira", () => {
  expect(numberQuery("#43")).toBe("43");
  expect(numberQuery("43")).toBe("43");
  expect(numberQuery("ui")).toBeNull();
  expect(numberQuery("proj-43", jiraProject())).toBe("43");
  expect(numberQuery("PROJ-43", jiraProject())).toBe("43");
  expect(numberQuery("OUTRO-43", jiraProject())).toBeNull();
  expect(numberQuery("PROJ-43")).toBeNull();
});
