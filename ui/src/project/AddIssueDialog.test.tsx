// "Adicionar issue pelo ID" (#150): botão do cabeçalho do projeto Jira, o
// diálogo (formato, erro do ambiente, item já no espelho) e a paleta, na
// visão do hub e na local.
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { App } from "../App";
import { FakeDaemonApi } from "../api/fake";
import type { IssueRow, ProjectTree, TrackerIssueAdded } from "../protocol/gen";
import type { HubProject, HubTree, HubTreeNode } from "../protocol/hub-gen";
import {
  HUB_DASHBOARD,
  NO_HEALTH,
  NO_PROPOSALS,
  dashboard,
  envInfo,
  hubAssignment,
  hubEnv,
  hubInfo,
  hubProjectView,
  hubStatus,
  jiraHubProject,
  jiraProject,
  sync,
} from "../test/fixtures";
import { ADD_ISSUE_PRUNE_NOTE, checkIssueKey, resetAddIssueRequests } from "./add-issue";
import { resetProjectUi } from "./ui-store";

const NOW = Date.now();
const JIRA = jiraHubProject();
const LOCAL = jiraProject();

function issue(n: number, title: string, p: Partial<IssueRow> = {}): IssueRow {
  return {
    gh_number: n,
    title,
    state: "open",
    labels: [],
    parent_number: null,
    has_sub_issues: false,
    url: `https://jira.example.com/browse/PROJ-${n}`,
    updated_at: 0,
    ...p,
  };
}

const node = (i: IssueRow): HubTreeNode => ({
  issue: i,
  work_item: null,
  summary: { pr: null, decisions: { total: 0, pending_autonomous: 0 }, last_activity_at: null, live_envs: [] },
  children: [],
});

const ISSUE1 = node(issue(1, "Tela de login"));
const ISSUE5 = node(issue(5, "Relatório fora do filtro"));

const added = (p: Partial<TrackerIssueAdded> = {}): TrackerIssueAdded => ({
  key: "PROJ-5",
  number: 5,
  title: "Relatório fora do filtro",
  already: false,
  ancestors: [],
  work_item: false,
  warning: null,
  ...p,
});

beforeEach(() => {
  resetProjectUi();
  resetAddIssueRequests();
});

/**
 * Projeto Jira `grupo/projeto/app` com o ambiente de sync `daemon-wsl` (o
 * `wsl` da UI). `syncEnv` muda o ambiente de sync do cadastro (`null` = sem
 * ambiente); `envUp = false` deixa o `wsl` desconectado. A árvore do hub
 * ganha a PROJ-5 quando o `hub.tracker.add_issue` dá certo.
 */
function setupHub(opts: { syncEnv?: string | null; envUp?: boolean; result?: Partial<TrackerIssueAdded> } = {}) {
  const project: HubProject = { ...JIRA, sync_env_id: opts.syncEnv === undefined ? "daemon-wsl" : opts.syncEnv };
  const roots = [ISSUE1];
  const tree = (): HubTree => ({ project, sync: sync({ project_id: 7, last_ok_at: NOW }), roots: [...roots], untracked: [] });
  const api = new FakeDaemonApi([{ ...FakeDaemonApi.wslEnv(), name: "WSL" }])
    .on("dashboard.get", dashboard({ projects: [{ project: LOCAL, work_items: [] }], sync: [] }))
    .on("env.info", envInfo())
    .withHub("ws://mac-mini:7879/ws", true)
    .onHub("hub.info", hubInfo({ protocol: "1.12" }))
    .onHub("hub.status", hubStatus())
    .onHub("hub.projects.list", [
      hubProjectView(project, [hubAssignment({ project_id: 7, gh_owner: "grupo/projeto", gh_repo: "app" })]),
    ])
    .onHub("hub.envs.list", [hubEnv({ online: true })])
    .onHub("hub.dashboard.get", { ...HUB_DASHBOARD, projects: [{ project, work_items: [] }] })
    .onHub("hub.proposals.list", NO_PROPOSALS)
    .onHub("hub.health.get", NO_HEALTH)
    .onHub("hub.tree.get", tree)
    .onHub("hub.tracker.add_issue", () => {
      if (!roots.includes(ISSUE5)) roots.push(ISSUE5);
      return added(opts.result);
    });
  if (opts.envUp === false) api.setStatus("wsl", false, "conexão recusada");
  render(<App api={api} initial={{ name: "project", owner: "grupo/projeto", repo: "app", view: "items" }} />);
  return { api, user: userEvent.setup() };
}

const head = () => document.querySelector<HTMLElement>(".project-head")!;
// O botão só aparece quando o cadastro do projeto (Jira) chega, o que pode vir
// depois da árvore: espera por ele em vez de procurá-lo uma vez só.
const addButton = () =>
  waitFor(() => within(document.querySelector<HTMLElement>(".project-head")!).getByRole("button", { name: "Adicionar issue" }));
const dialog = () => screen.findByRole("dialog", { name: "Adicionar issue pelo ID" });
/** Aba do item aberta: a trilha termina na chave do Jira. */
const itemOpened = async (key: string) => {
  const trail = await screen.findByRole("list", { name: "Trilha" });
  await waitFor(() => expect(within(trail).getByText(key)).toHaveAttribute("aria-current", "page"));
};

describe("checkIssueKey", () => {
  test.each([
    ["PROJ-5", "PROJ-5"],
    [" proj-5 ", "PROJ-5"],
    ["5", "5"],
    ["https://jira.example.com/browse/PROJ-5", "PROJ-5"],
    ["https://jira.example.com/browse/PROJ-5?focusedCommentId=1#c", "PROJ-5"],
  ])("aceita %j", (input, key) => {
    expect(checkIssueKey(input, "PROJ")).toEqual({ key });
  });

  test.each(["", "PROJ", "PROJ-", "-5", "PROJ-5a", "PROJ 5"])("recusa %j com o exemplo da chave do projeto", (input) => {
    const r = checkIssueKey(input, "PROJ");
    expect(r).toHaveProperty("error");
    expect((r as { error: string }).error).toContain("PROJ-123");
  });
});

test("hub: sucesso adiciona pelo hub.tracker.add_issue, recarrega a árvore e abre o item", async () => {
  const { api, user } = setupHub();
  await screen.findByText("visão do hub");
  const btn = await addButton();
  await waitFor(() => expect(btn).toBeEnabled());
  await user.click(btn);
  const dlg = await dialog();
  // O aviso da poda está no diálogo antes do envio.
  expect(dlg).toHaveTextContent(ADD_ISSUE_PRUNE_NOTE);
  const field = within(dlg).getByLabelText("Chave da issue no Jira");
  expect(field).toHaveFocus();
  await user.type(field, "proj-5");
  await user.click(within(dlg).getByRole("button", { name: "Adicionar" }));
  await waitFor(() => expect(api.hubCallsTo("hub.tracker.add_issue")).toHaveLength(1));
  expect(api.hubCallsTo("hub.tracker.add_issue")[0].params).toEqual({ owner: "grupo/projeto", repo: "app", key: "PROJ-5" });
  await itemOpened("PROJ-5");
  expect(screen.queryByRole("dialog", { name: "Adicionar issue pelo ID" })).toBeNull();
  // A árvore recarregou com a issue nova.
  expect(within(screen.getByRole("complementary", { name: "Árvore do projeto" })).getByText("Relatório fora do filtro")).toBeInTheDocument();
});

test("hub: formato inválido não chama o hub; o erro do ambiente fica no diálogo", async () => {
  const { api, user } = setupHub();
  api.onHub("hub.tracker.add_issue", () => {
    throw new Error("PROJ-99 não existe no Jira (ou a credencial não tem acesso a ela)");
  });
  await screen.findByText("visão do hub");
  await waitFor(async () => expect(await addButton()).toBeEnabled());
  await user.click(await addButton());
  const dlg = await dialog();
  const field = within(dlg).getByLabelText("Chave da issue no Jira");
  await user.type(field, "PROJ 99");
  await user.click(within(dlg).getByRole("button", { name: "Adicionar" }));
  expect(within(dlg).getByRole("alert")).toHaveTextContent('Chave inválida: "PROJ 99". Use PROJ-123 ou só o número.');
  expect(field).toHaveAttribute("aria-invalid", "true");
  expect(api.hubCallsTo("hub.tracker.add_issue")).toHaveLength(0);

  await user.clear(field);
  await user.type(field, "99");
  await user.click(within(dlg).getByRole("button", { name: "Adicionar" }));
  expect(await within(dlg).findByRole("alert")).toHaveTextContent(
    "PROJ-99 não existe no Jira (ou a credencial não tem acesso a ela)",
  );
  expect(api.hubCallsTo("hub.tracker.add_issue")[0].params).toMatchObject({ key: "99" });
  // O diálogo segue aberto, com o campo para corrigir; Cancelar fecha.
  expect(field).toHaveFocus();
  await user.click(within(dlg).getByRole("button", { name: "Cancelar" }));
  expect(screen.queryByRole("dialog", { name: "Adicionar issue pelo ID" })).toBeNull();
});

test("hub: issue que já estava no espelho só abre o item", async () => {
  const { api, user } = setupHub();
  api.onHub("hub.tracker.add_issue", () => added({ key: "PROJ-1", number: 1, title: "Tela de login", already: true, work_item: true }));
  await screen.findByText("visão do hub");
  await waitFor(async () => expect(await addButton()).toBeEnabled());
  await user.click(await addButton());
  const dlg = await dialog();
  await user.type(within(dlg).getByLabelText("Chave da issue no Jira"), "PROJ-1");
  await user.click(within(dlg).getByRole("button", { name: "Adicionar" }));
  await itemOpened("PROJ-1");
  expect(screen.queryByRole("dialog", { name: "Adicionar issue pelo ID" })).toBeNull();
});

test("hub: pai não lido deixa o aviso no diálogo, com o Abrir à mão", async () => {
  const { user } = setupHub({ result: { warning: "épico PROJ-2 não foi lido: HTTP 500" } });
  await screen.findByText("visão do hub");
  await waitFor(async () => expect(await addButton()).toBeEnabled());
  await user.click(await addButton());
  const dlg = await dialog();
  await user.type(within(dlg).getByLabelText("Chave da issue no Jira"), "PROJ-5");
  await user.click(within(dlg).getByRole("button", { name: "Adicionar" }));
  expect(await within(dlg).findByRole("alert")).toHaveTextContent("épico PROJ-2 não foi lido: HTTP 500");
  expect(within(dlg).getByRole("status")).toHaveTextContent("PROJ-5 Relatório fora do filtro foi adicionada ao espelho.");
  expect(dlg).toHaveTextContent(ADD_ISSUE_PRUNE_NOTE);
  await user.click(within(dlg).getByRole("button", { name: "Abrir PROJ-5" }));
  await itemOpened("PROJ-5");
});

test("hub: sem ambiente de sync, o botão fica desabilitado com o motivo", async () => {
  setupHub({ syncEnv: null });
  await screen.findByText("visão do hub");
  const btn = await addButton();
  await waitFor(() => expect(btn).toBeDisabled());
  expect(btn).toHaveAttribute(
    "title",
    "Projeto sem ambiente de sync: escolha um em Configurações › Repositório e rastreador.",
  );
});

test("hub: ambiente de sync desconectado desabilita o botão com o motivo", async () => {
  setupHub({ envUp: false });
  await screen.findByText("visão do hub");
  const btn = await addButton();
  await waitFor(() => expect(btn).toHaveAttribute("title", "O ambiente de sync wsl-pc não está conectado com este projeto."));
  expect(btn).toBeDisabled();
});

test("projeto GitHub: sem o botão", async () => {
  const api = new FakeDaemonApi([{ ...FakeDaemonApi.wslEnv(), name: "WSL" }])
    .on("dashboard.get", dashboard())
    .on("env.info", envInfo())
    .on("tree.get", (): ProjectTree => ({ project: dashboard().projects[0].project, sync: null, roots: [], untracked: [] }));
  render(<App api={api} initial={{ name: "project", env: "wsl", projectId: 1, view: "items" }} />);
  await screen.findByRole("heading", { name: "ctower-agents", level: 1 });
  expect(within(head()).queryByRole("button", { name: "Adicionar issue" })).toBeNull();
});

/** Visão local (hub fora): o projeto Jira no próprio ambiente, pelo `tracker.add_issue`. */
function setupLocal() {
  const roots = [ISSUE1];
  const api = new FakeDaemonApi([{ ...FakeDaemonApi.wslEnv(), name: "WSL" }])
    .on("dashboard.get", dashboard({ projects: [{ project: LOCAL, work_items: [] }], sync: [] }))
    .on("env.info", envInfo())
    .on("tree.get", (): ProjectTree => ({ project: LOCAL, sync: null, roots: [...roots] as ProjectTree["roots"], untracked: [] }))
    .on("tracker.add_issue", () => {
      if (!roots.includes(ISSUE5)) roots.push(ISSUE5);
      return added();
    });
  return { api, user: userEvent.setup() };
}

test("local: o botão chama tracker.add_issue no ambiente com o project_id e abre o item", async () => {
  const { api, user } = setupLocal();
  render(<App api={api} initial={{ name: "project", env: "wsl", projectId: 7, view: "items" }} />);
  await screen.findByRole("heading", { name: "app", level: 1 });
  await user.click(await addButton());
  const dlg = await dialog();
  await user.type(within(dlg).getByLabelText("Chave da issue no Jira"), "https://jira.example.com/browse/PROJ-5");
  await user.click(within(dlg).getByRole("button", { name: "Adicionar" }));
  await waitFor(() => expect(api.callsTo("tracker.add_issue")).toHaveLength(1));
  expect(api.callsTo("tracker.add_issue")[0]).toMatchObject({ env: "wsl", params: { project_id: 7, key: "PROJ-5" } });
  await itemOpened("PROJ-5");
});

test("paleta: Projeto: Adicionar issue pelo ID… abre o diálogo do projeto", async () => {
  const { api, user } = setupLocal();
  render(<App api={api} initial={{ name: "control" }} />);
  await user.click(await screen.findByRole("button", { name: "Abrir paleta de comandos" }));
  const busca = within(screen.getByRole("dialog", { name: "Paleta de comandos" })).getByRole("combobox");
  await user.type(busca, "Adicionar issue pelo");
  const opt = await waitFor(() => {
    const o = screen.getAllByRole("option").find((x) => x.textContent?.startsWith("Projeto: Adicionar issue pelo ID… — app"));
    expect(o).toBeDefined();
    return o!;
  });
  await user.click(opt);
  const dlg = await dialog();
  await user.type(within(dlg).getByLabelText("Chave da issue no Jira"), "5");
  await user.click(within(dlg).getByRole("button", { name: "Adicionar" }));
  await waitFor(() => expect(api.callsTo("tracker.add_issue")).toHaveLength(1));
  expect(api.callsTo("tracker.add_issue")[0].params).toMatchObject({ project_id: 7, key: "5" });
  await itemOpened("PROJ-5");
});
