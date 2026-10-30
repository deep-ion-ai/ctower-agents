import { pageOf, pageSuffix, projectAt, projectsPlace, sectionOf, type ProjectAddr } from "./sections";
import { itemTab } from "./tabs";

const BASE: ProjectAddr = { name: "project", owner: "deep-ion-ai", repo: "ctower-agents" };

test("projectAt: Itens sem `view` (endereço de antes da #17); seção só nas Configurações; sem item", () => {
  expect(projectAt(BASE)).toEqual(BASE);
  expect(projectAt({ ...BASE, view: "sessions", item: { ghNumber: 3 } }, "items")).toEqual(BASE);
  expect(projectAt(BASE, "overview")).toEqual({ ...BASE, view: "overview" });
  expect(projectAt(BASE, "settings", "secrets")).toEqual({ ...BASE, view: "settings", section: "secrets" });
  expect(projectAt({ ...BASE, view: "settings", section: "docs" }, "sessions", "docs")).toEqual({ ...BASE, view: "sessions" });
  // A forma local (ambiente e id) vai junto.
  expect(projectAt({ name: "project", env: "wsl", projectId: 1 }, "settings")).toEqual({
    name: "project",
    env: "wsl",
    projectId: 1,
    view: "settings",
  });
});

test("pageOf e sectionOf: padrões, item força Itens e a web não abre seção do desktop", () => {
  expect(pageOf(BASE)).toBe("items");
  expect(pageOf({ ...BASE, view: "overview" })).toBe("overview");
  expect(pageOf({ ...BASE, view: "overview", item: { ghNumber: 1 } })).toBe("items");
  expect(sectionOf({ ...BASE, view: "settings" })).toBe("general");
  expect(sectionOf({ ...BASE, view: "settings", section: "danger" })).toBe("danger");
  expect(sectionOf({ ...BASE, view: "settings", section: "danger" }, true)).toBe("general");
  expect(sectionOf({ ...BASE, view: "settings", section: "secrets" }, true)).toBe("general");
  expect(sectionOf({ ...BASE, view: "settings", section: "rules" }, true)).toBe("rules");
});

test("pageSuffix: título da aba fora de Itens", () => {
  expect(pageSuffix(BASE)).toBe("");
  expect(pageSuffix({ ...BASE, view: "sessions" })).toBe(" · Sessões");
  expect(pageSuffix({ ...BASE, view: "settings" })).toBe(" · Configurações");
  expect(pageSuffix({ ...BASE, view: "settings", section: "envs" })).toBe(" · Configurações › Ambientes e atribuições");
});

test("itemTab: a aba do item não leva a aba nem a seção do projeto", () => {
  const t = itemTab({ ...BASE, view: "settings", section: "docs" }, 5, "x");
  expect(t.id).toBe("project:deep-ion-ai/ctower-agents#5");
  expect(t.screen).toEqual({ ...BASE, item: { ghNumber: 5 } });
});

test("projectsPlace (#18): o lugar novo do antigo Hub › Projetos; projectAt tira os pedidos", () => {
  const ref = { owner: "deep-ion-ai", repo: "ctower-agents" };
  expect(projectsPlace({})).toEqual({ name: "project" });
  expect(projectsPlace({ register: { url: "u", kind: "jira" } })).toEqual({ name: "project", register: { url: "u", kind: "jira" } });
  expect(projectsPlace({ project: ref })).toEqual({ ...BASE, view: "settings" });
  expect(projectsPlace({ project: ref, assign: true })).toEqual({ ...BASE, view: "settings", section: "envs", assign: true });
  // Com projeto, o pedido de cadastro não vale (o projeto já existe).
  expect(projectsPlace({ project: ref, register: { url: "u" } })).toEqual({ ...BASE, view: "settings" });
  // Os pedidos valem uma vez: trocar de aba ou seção os tira do endereço.
  expect(projectAt({ ...BASE, view: "settings", section: "envs", assign: true }, "settings", "envs")).toEqual({
    ...BASE,
    view: "settings",
    section: "envs",
  });
  expect(projectAt({ name: "project", register: { url: "" }, notice: "x" })).toEqual({ name: "project" });
});
