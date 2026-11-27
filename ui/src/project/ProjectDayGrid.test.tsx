import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { App } from "../App";
import { FakeDaemonApi } from "../api/fake";
import { dashboard, project, session, sync, workItem } from "../test/fixtures";

const NOW = Date.now();
const LOCAL = { name: "project", env: "wsl", projectId: 1 } as const;

const OTHER = project({ id: 2, name: "acme-web", gh_owner: "acme", gh_repo: "acme-web" });
const MINE_LIVE = session({ id: "cccccccc-0000-0000-0000-000000000030", work_item_id: 30 });
const OTHER_LIVE = session({ id: "dddddddd-0000-0000-0000-000000000041", work_item_id: 41 });

function setup(view: "sessions" | undefined) {
  const d = dashboard({ live_sessions: [MINE_LIVE, OTHER_LIVE], sync: [sync({ last_ok_at: NOW })] }, [
    workItem({ id: 30, gh_number: 30, title: "Item do projeto", status: "em_execucao" }),
    workItem({ id: 31, gh_number: 31, title: "Revisão do projeto", status: "em_revisao", pending_decisions: 2 }),
    workItem({ id: 32, gh_number: 32, title: "Feita hoje aqui", status: "concluido", closed_at: NOW }),
  ]);
  d.projects.push({
    project: OTHER,
    work_items: [
      workItem({ id: 41, project_id: 2, gh_number: 41, title: "Item de outro", status: "em_execucao" }),
      workItem({ id: 42, project_id: 2, gh_number: 42, title: "Revisão de outro", status: "em_revisao" }),
      workItem({ id: 43, project_id: 2, gh_number: 43, title: "Feita de outro", status: "concluido", closed_at: NOW }),
    ],
  });
  const api = new FakeDaemonApi()
    .on("dashboard.get", d)
    .on("session.peek", { data: "" })
    .on("tree.get", { project: project(), sync: sync({ last_ok_at: NOW }), roots: [], untracked: [] });
  render(<App api={api} initial={view ? { ...LOCAL, view } : LOCAL} />);
  return userEvent.setup();
}

test("aba Sessões: a grade traz só as sessões do projeto e abre o terminal", async () => {
  setup("sessions");
  const grade = (await screen.findByText("Sessões vivas · 1")).closest("section")!;
  expect(within(grade).getByText("Item do projeto")).toBeInTheDocument();
  expect(screen.queryByText("Item de outro")).not.toBeInTheDocument();
  // Densidade e agrupamento, como na Sala de controle.
  expect(screen.getByRole("group", { name: "Densidade da grade" })).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Agrupar por ambiente" })).toBeInTheDocument();
  // A lista por ambiente (recuperar e descartar) segue logo abaixo.
  expect(screen.getByRole("list", { name: /Vivas em/ })).toBeInTheDocument();
});

test("Visão geral: o fim do dia é só do projeto", async () => {
  setup(undefined);
  const fim = await screen.findByRole("region", { name: "Fim do dia do projeto" });
  expect(within(fim).getByText("Em revisão · 1")).toBeInTheDocument();
  expect(within(fim).getByText("Revisão do projeto")).toBeInTheDocument();
  expect(within(fim).getByText("2 decisões pendentes")).toBeInTheDocument();
  expect(within(fim).getByText("Concluídas hoje · 1")).toBeInTheDocument();
  expect(within(fim).getByText(/Feita hoje aqui/)).toBeInTheDocument();
  expect(within(fim).queryByText(/de outro/)).not.toBeInTheDocument();
});
