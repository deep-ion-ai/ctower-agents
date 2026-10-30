// Filtro por sprint (projetos Jira): a lista de sprints do projeto, o
// rótulo, a pertença de cada issue, a árvore filtrada, o chip da sprint
// atual e a escolha guardada por projeto.
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { App } from "../App";
import { FakeDaemonApi } from "../api/fake";
import type { IssueRow, Project, ProjectTree, Sprint, TreeNode } from "../protocol/gen";
import { dashboard, jiraProject, project, sync } from "../test/fixtures";
import {
  ACTIVE_SPRINT,
  NO_SPRINT,
  currentSprint,
  effectiveSprint,
  matchesSprint,
  projectSprints,
  readSprintChoice,
  sprintLabel,
} from "./sprints";
import { NO_FILTERS, buildView } from "./tree-view";

const NOW = Date.now();
const day = (d: number, m: number) => new Date(2026, m - 1, d, 12).getTime();

const S47: Sprint = {
  id: 2254,
  name: "Sprint 47",
  state: "closed",
  board_id: 402,
  start_date: day(13, 10),
  end_date: day(24, 10),
  complete_date: day(28, 10),
};
const S48: Sprint = { id: 2273, name: "Sprint 48", state: "active", board_id: 402, start_date: day(26, 10), end_date: day(6, 11), complete_date: null };
const S49: Sprint = { id: 2290, name: "Sprint 49", state: "future", board_id: 402, start_date: null, end_date: null, complete_date: null };

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

const node = (i: IssueRow, children: TreeNode[] = []): TreeNode => ({
  issue: i,
  work_item: null,
  summary: { pr: null, decisions: { total: 0, pending_autonomous: 0 }, last_activity_at: null, live_envs: [] },
  children,
});

// Épico 5400 (sem sprint) com a 5450 (47 → 48) e a 5460 (só a 47, fechada);
// avulsas: 5500 na 49 (futura) e 5510 sem sprint.
function roots(): TreeNode[] {
  return [
    node(issue(5400, "Épico do plugin", { has_sub_issues: true }), [
      node(issue(5450, "Portar o plugin", { parent_number: 5400, sprints: [S47, S48] })),
      node(issue(5460, "Ajustar o build", { parent_number: 5400, state: "closed", sprints: [S47] })),
    ]),
    node(issue(5500, "Planejar a próxima", { sprints: [S49] })),
    node(issue(5510, "Sem sprint nenhuma")),
  ];
}

const flat = (r: TreeNode[]) => r.flatMap((x) => [x, ...x.children]).map((x) => x.issue);

describe("sprints do projeto", () => {
  test("lista sem repetição, da mais recente (futura primeiro) para a mais antiga", () => {
    expect(projectSprints(flat(roots())).map((s) => s.id)).toEqual([2290, 2273, 2254]);
    expect(projectSprints([{}, { sprints: null }])).toEqual([]);
  });

  test("rótulo com estado e datas", () => {
    expect(sprintLabel(S48)).toBe("Sprint 48 · ativa · 26/10–06/11");
    expect(sprintLabel(S47)).toBe("Sprint 47 · fechada · 13/10–24/10");
    expect(sprintLabel(S49)).toBe("Sprint 49 · futura");
  });

  test("pertença: está na lista; ativa; sem sprint; sprint atual", () => {
    const i = { sprints: [S47, S48] };
    expect(matchesSprint(i, "")).toBe(true);
    expect(matchesSprint(i, "2254")).toBe(true);
    expect(matchesSprint(i, "2290")).toBe(false);
    expect(matchesSprint(i, ACTIVE_SPRINT)).toBe(true);
    expect(matchesSprint({ sprints: [S47] }, ACTIVE_SPRINT)).toBe(false);
    expect(matchesSprint({}, NO_SPRINT)).toBe(true);
    expect(matchesSprint(i, NO_SPRINT)).toBe(false);
    expect(currentSprint(i)?.id).toBe(2273);
    expect(currentSprint({ sprints: [S47] })?.id).toBe(2254);
    expect(currentSprint({})).toBeNull();
  });

  test("filtro guardado de sprint que sumiu vale como todas", () => {
    expect(effectiveSprint("2273", [S48])).toBe("2273");
    expect(effectiveSprint("9999", [S48])).toBe("");
    expect(effectiveSprint(NO_SPRINT, [])).toBe(NO_SPRINT);
  });

  test("árvore filtrada: a sprint escolhida mostra também as concluídas dela", () => {
    const v47 = buildView(roots(), { ...NO_FILTERS, sprint: "2254" });
    expect(v47.epics.map((e) => [e.epic.issue.gh_number, e.shown.map((c) => c.issue.gh_number)])).toEqual([
      [5400, [5450, 5460]],
    ]);
    expect(v47.loose).toEqual([]);
    const ativa = buildView(roots(), { ...NO_FILTERS, sprint: ACTIVE_SPRINT });
    expect(ativa.epics[0].shown.map((c) => c.issue.gh_number)).toEqual([5450]);
    const sem = buildView(roots(), { ...NO_FILTERS, sprint: NO_SPRINT });
    // O épico (sem sprint) aparece, sem as filhas, que estão em sprints.
    expect(sem.epics.map((e) => [e.epic.issue.gh_number, e.shown.length])).toEqual([[5400, 0]]);
    expect(sem.loose.map((n) => n.issue.gh_number)).toEqual([5510]);
  });
});

function envApi(p: Project, t: TreeNode[]) {
  const tree: ProjectTree = { project: p, sync: sync({ project_id: p.id, last_ok_at: NOW }), roots: t, untracked: [] };
  return new FakeDaemonApi()
    .on("dashboard.get", dashboard({ projects: [{ project: p, work_items: [] }], sync: [sync({ project_id: p.id })] }))
    .on("session.peek", { data: "" })
    .on("tree.get", tree)
    .on("env.info", { protocol: "2.3", version: "0.1.0", hostname: "ct", worktrees_root: "/home/user/worktrees" });
}

const side = () => screen.getByRole("complementary", { name: "Árvore do projeto" });

describe("tela Projeto", () => {
  test("projeto Jira: filtro por sprint, chip da sprint atual e escolha guardada", async () => {
    const user = userEvent.setup();
    const { unmount } = render(<App api={envApi(jiraProject(), roots())} initial={{ name: "project", env: "wsl", projectId: jiraProject().id }} />);
    await screen.findByText("Épico do plugin");
    await user.click(screen.getByRole("button", { name: "Expandir épico PROJ-5400" }));
    const select = screen.getByRole("combobox", { name: "Filtrar por sprint" });
    expect([...(select as HTMLSelectElement).options].map((o) => o.text)).toEqual([
      "Todas",
      "Sprint ativa",
      "Sprint 49 · futura",
      "Sprint 48 · ativa · 26/10–06/11",
      "Sprint 47 · fechada · 13/10–24/10",
      "Sem sprint",
    ]);
    // Chip da sprint atual na linha (e no nome acessível).
    const row = within(side()).getByRole("button", { name: /^PROJ-5450 Portar o plugin, backlog, sprint Sprint 48/ });
    expect(within(row).getByText("Sprint 48")).toHaveAttribute("title", "Sprint 48 · ativa · 26/10–06/11");
    expect(within(side()).getByText("Sem sprint nenhuma")).toBeInTheDocument();

    await user.selectOptions(select, "Sprint 47 · fechada · 13/10–24/10");
    expect(within(side()).getByText("Ajustar o build")).toBeInTheDocument();
    expect(within(side()).getByText("Portar o plugin")).toBeInTheDocument();
    expect(within(side()).queryByText("Sem sprint nenhuma")).not.toBeInTheDocument();
    expect(within(side()).queryByText("Planejar a próxima")).not.toBeInTheDocument();

    await user.selectOptions(select, "Sem sprint");
    expect(within(side()).getByText("Sem sprint nenhuma")).toBeInTheDocument();
    expect(within(side()).queryByText("Portar o plugin")).not.toBeInTheDocument();
    expect(readSprintChoice("grupo/projeto/app")).toBe(NO_SPRINT);
    unmount();
  });

  test("a escolha volta ao reabrir o app", async () => {
    localStorage.setItem("ct.project.sprint", JSON.stringify({ "grupo/projeto/app": "2273" }));
    render(<App api={envApi(jiraProject(), roots())} initial={{ name: "project", env: "wsl", projectId: jiraProject().id }} />);
    await screen.findByText("Portar o plugin");
    const select = screen.getByRole("combobox", { name: "Filtrar por sprint" });
    expect(select).toHaveValue("2273");
    expect(within(side()).getByText("Portar o plugin")).toBeInTheDocument();
    expect(within(side()).queryByText("Sem sprint nenhuma")).not.toBeInTheDocument();
  });

  test("projeto Jira sem nenhuma sprint no espelho: aviso no filtro e na sprint ativa vazia", async () => {
    const user = userEvent.setup();
    const semSprint = [node(issue(5510, "Sem sprint nenhuma")), node(issue(5511, "Outra sem sprint"))];
    render(<App api={envApi(jiraProject(), semSprint)} initial={{ name: "project", env: "wsl", projectId: jiraProject().id }} />);
    await screen.findByText("Sem sprint nenhuma");
    expect(screen.getByRole("note")).toHaveTextContent("sprints ainda não lidas (aguarde o próximo sync)");
    await user.selectOptions(screen.getByRole("combobox", { name: "Filtrar por sprint" }), "Sprint ativa");
    expect(within(side()).getByText(/^Nenhum item na sprint ativa\..*\(sprints ainda não lidas \(aguarde o próximo sync\)\)$/)).toBeInTheDocument();
    expect(within(side()).queryByText("Sem sprint nenhuma")).not.toBeInTheDocument();
  });

  test("sprint ativa sem itens, com sprints lidas: a mensagem diz isso, sem o aviso", async () => {
    const user = userEvent.setup();
    const fechada = [node(issue(5460, "Ajustar o build", { sprints: [S47] }))];
    render(<App api={envApi(jiraProject(), fechada)} initial={{ name: "project", env: "wsl", projectId: jiraProject().id }} />);
    await screen.findByText("Ajustar o build");
    expect(screen.queryByText(/sprints ainda não lidas/)).not.toBeInTheDocument();
    await user.selectOptions(screen.getByRole("combobox", { name: "Filtrar por sprint" }), "Sprint ativa");
    expect(within(side()).getByText("Nenhum item na sprint ativa. Escolha outra sprint ou “Todas”.")).toBeInTheDocument();
  });

  test("projeto GitHub: sem filtro nem chip de sprint", async () => {
    render(<App api={envApi(project(), [node({ ...issue(7, "Issue do GitHub"), url: "https://github.com/o/r/issues/7" })])} initial={{ name: "project", env: "wsl", projectId: project().id }} />);
    expect(await screen.findByText("Issue do GitHub")).toBeInTheDocument();
    expect(screen.queryByRole("combobox", { name: "Filtrar por sprint" })).not.toBeInTheDocument();
  });
});
