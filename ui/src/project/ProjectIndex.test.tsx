// Índice da atividade Projetos (#16).
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { App } from "../App";
import { FakeDaemonApi } from "../api/fake";
import type { HubDashboard, HubTree } from "../protocol/hub-gen";
import type { Screen } from "../state/nav";
import {
  HUB_DASHBOARD,
  NO_PROPOSALS,
  dashboard,
  healthSignal,
  hubAssignment,
  hubEnv,
  hubInfo,
  hubProject,
  hubProjectView,
  hubStatus,
  jiraProject,
  project,
  sync,
} from "../test/fixtures";

const CT = hubProject();
const DEMO = hubProject({ id: 2, gh_repo: "demo", name: "demo" });

const HUB_DASH: HubDashboard = {
  ...HUB_DASHBOARD,
  projects: [
    { project: CT, work_items: [{ id: 50, status: "em_andamento" } as never, { id: 51, status: "aguardando_voce" } as never] },
    { project: DEMO, work_items: [] },
  ],
  live_sessions: [
    { id: "s1", env_id: "daemon-wsl", work_item_id: 50 },
    { id: "s2", env_id: "daemon-wsl", work_item_id: 51 },
  ] as never,
};

const TREE = (p = CT): HubTree => ({ project: p, sync: null, roots: [], untracked: [] });

function hubSetup(initial: Screen = { name: "project" }) {
  const api = new FakeDaemonApi([{ ...FakeDaemonApi.wslEnv(), name: "WSL" }])
    .on("dashboard.get", dashboard({ projects: [] }))
    .withHub("ws://mac-mini:7879/ws", true)
    .onHub("hub.info", hubInfo())
    .onHub("hub.status", hubStatus())
    .onHub("hub.projects.list", [
      hubProjectView(CT, [hubAssignment(), hubAssignment({ env_id: "daemon-mac", env_name: "mac-mini", state: "pending" })]),
      hubProjectView(DEMO),
    ])
    .onHub("hub.envs.list", [hubEnv()])
    .onHub("hub.dashboard.get", HUB_DASH)
    .onHub("hub.proposals.list", NO_PROPOSALS)
    .onHub("hub.health.get", {
      generated_at: 0,
      signals: [healthSignal({ owner: "deep-ion-ai", repo: "ctower-agents", severity: "critico" })],
      counts: { critico: 1, aviso: 0, info: 0 },
    })
    .onHub("hub.tree.get", (p: { repo: string }) => TREE(p.repo === "demo" ? DEMO : CT));
  render(<App api={api} initial={initial} />);
  return { api, user: userEvent.setup() };
}

const rows = () => within(screen.getByRole("main")).getAllByRole("listitem");
const telas = () => screen.getByRole("navigation", { name: "Telas" });

test("com o hub: linhas na ordem de prioridade com estado, sessões, atribuições e saúde", async () => {
  hubSetup();
  expect(screen.getByRole("heading", { name: "Projetos" })).toBeInTheDocument();
  await waitFor(() => expect(rows().map((li) => li.querySelector(".pidx-main")!.getAttribute("aria-label"))).toEqual(["ctower-agents", "demo"]));
  const ct = screen.getByRole("button", { name: "ctower-agents" });
  // Atribuição pendente pede atenção antes da sincronização (`projectState`).
  await waitFor(() => expect(ct).toHaveAccessibleDescription(/1 atribuição pendente/));
  expect(ct).toHaveAccessibleDescription(/1 em execução, 1 aguardando/);
  expect(ct).toHaveAccessibleDescription(/atribuído a: wsl-pc, mac-mini/);
  await waitFor(() => expect(ct).toHaveAccessibleDescription(/1 crítico/));
  const demo = screen.getByRole("button", { name: "demo" });
  expect(demo).toHaveAccessibleDescription(/sem ambiente/);
  expect(demo).toHaveAccessibleDescription(/sem sessões/);
  expect(within(rows()[1]).getByText("#2")).toBeInTheDocument();
});

test("abrir um projeto abre a aba dele; 'Projetos' (trilha ou atividade) volta ao índice sem fechar o projeto", async () => {
  const { user } = hubSetup();
  await user.click(await screen.findByRole("button", { name: "demo" }));
  // Já na visão do hub (a trilha não é remontada no meio do clique).
  await screen.findByText("visão do hub");
  expect(screen.getByRole("tab", { name: /demo/, selected: true })).toBeInTheDocument();
  const trilha = screen.getByRole("navigation", { name: "Trilha do projeto" });
  expect(within(trilha).getByText("demo")).toHaveAttribute("aria-current", "page");
  await user.click(within(trilha).getByRole("button", { name: "Projetos" }));
  expect(await screen.findByRole("heading", { name: "Projetos" })).toBeInTheDocument();
  expect(screen.getByRole("tab", { name: /Projetos/, selected: true })).toBeInTheDocument();
  // O último projeto aberto segue na aba dele.
  await user.click(screen.getByRole("tab", { name: /demo/ }));
  expect(await screen.findByText("visão do hub")).toBeInTheDocument();
  // A atividade Projetos é sempre o índice, mesmo com a aba do projeto ativa.
  await user.click(within(telas()).getByRole("button", { name: "Projetos" }));
  expect(await screen.findByRole("heading", { name: "Projetos" })).toBeInTheDocument();
  expect(screen.getAllByRole("tab").map((t) => t.textContent)).toEqual(["Projetos", "demo"]);
});

test("reordenar: arrastar e Alt+↑/↓ mudam a prioridade no índice", async () => {
  const { api, user } = hubSetup();
  api.onHub("hub.projects.move", { ...CT, rank: 2 });
  await waitFor(() => expect(rows()).toHaveLength(2));
  expect(screen.getByText(/A ordem da lista é a prioridade entre projetos/)).toBeInTheDocument();
  const [ct, demo] = rows();
  fireEvent.dragStart(ct);
  fireEvent.dragOver(demo, { clientY: 10 });
  expect(demo).toHaveAttribute("data-drop", "after");
  fireEvent.drop(demo, { clientY: 10 });
  await waitFor(() => expect(api.hubCallsTo("hub.projects.move")).toHaveLength(1));
  expect(api.hubCallsTo("hub.projects.move")[0].params).toEqual({
    owner: "deep-ion-ai",
    repo: "ctower-agents",
    after: { owner: "deep-ion-ai", repo: "demo" },
  });
  expect(await screen.findByText("ctower-agents desceu na ordem dos projetos.")).toBeInTheDocument();
  screen.getByRole("button", { name: "demo" }).focus();
  await user.keyboard("{Alt>}{ArrowUp}{/Alt}");
  await waitFor(() => expect(api.hubCallsTo("hub.projects.move")).toHaveLength(2));
  expect(api.hubCallsTo("hub.projects.move")[1].params).toEqual({
    owner: "deep-ion-ai",
    repo: "demo",
    before: { owner: "deep-ion-ai", repo: "ctower-agents" },
  });
});

test("filtro, ⚙ (configurações no Hub), ⋯ (menu) e + Novo projeto", async () => {
  const { user } = hubSetup();
  await waitFor(() => expect(rows()).toHaveLength(2));
  await user.type(screen.getByLabelText("Filtrar projetos"), "dem");
  expect(rows()).toHaveLength(1);
  await user.clear(screen.getByLabelText("Filtrar projetos"));
  await user.type(screen.getByLabelText("Filtrar projetos"), "nada");
  expect(screen.getByText(/Nenhum projeto com/)).toBeInTheDocument();
  await user.clear(screen.getByLabelText("Filtrar projetos"));

  await user.click(screen.getByRole("button", { name: "Mais ações de demo" }));
  const menu = screen.getByRole("menu", { name: "Ações do projeto demo" });
  expect(within(menu).getAllByRole("menuitem").map((b) => b.textContent)).toEqual(
    expect.arrayContaining(["Abrir", "Configurações", "Atribuir a ambiente", "Abrir no GitHub"]),
  );
  await user.keyboard("{Escape}");
  expect(screen.getByRole("button", { name: "Mais ações de demo" })).toHaveFocus();

  await user.click(screen.getByRole("button", { name: "+ Novo projeto" }));
  expect(await screen.findByRole("form", { name: "Novo projeto" })).toBeInTheDocument();
  await user.click(screen.getByRole("button", { name: "Cancelar" }));
  expect(screen.queryByRole("form", { name: "Novo projeto" })).not.toBeInTheDocument();

  await user.click(screen.getByRole("button", { name: "Configurações de demo" }));
  expect(await screen.findByRole("tab", { name: /Hub › Projetos|Projetos e atribuições/, selected: true })).toBeInTheDocument();
});

test("sem hub: lista local por ambiente, que abre o projeto local", async () => {
  const api = new FakeDaemonApi([
    { ...FakeDaemonApi.wslEnv(), name: "WSL" },
    { ...FakeDaemonApi.wslEnv("mac"), name: "Mac" },
  ])
    .on("dashboard.get", (_: unknown, env: string) =>
      env === "wsl"
        ? dashboard({ projects: [{ project: project(), work_items: [] }], sync: [sync({ last_ok_at: Date.now() })] })
        : dashboard({ projects: [{ project: jiraProject(), work_items: [] }], sync: [] }),
    )
    .on("env.info", { protocol: "2.0", version: "0.1.0", hostname: "ct", worktrees_root: "/w" })
    .on("tree.get", { project: project(), sync: null, roots: [], untracked: [] });
  render(<App api={api} initial={{ name: "project" }} />);
  const user = userEvent.setup();
  const wsl = await screen.findByRole("region", { name: "Projetos de WSL" });
  const mac = screen.getByRole("region", { name: "Projetos de Mac" });
  expect(within(mac).getByRole("button", { name: "app" })).toHaveAccessibleDescription(/ainda não sincronizado/);
  expect(screen.queryByText(/A ordem da lista é a prioridade/)).not.toBeInTheDocument();
  // Sem hub conectado: sem configurações, sem cadastro.
  expect(screen.queryByRole("button", { name: /Configurações de/ })).not.toBeInTheDocument();
  expect(screen.queryByRole("button", { name: "+ Novo projeto" })).not.toBeInTheDocument();
  await user.click(within(wsl).getByRole("button", { name: "ctower-agents" }));
  expect(await screen.findByRole("navigation", { name: "Trilha do projeto" })).toBeInTheDocument();
  expect(screen.getByRole("tab", { name: /ctower-agents/, selected: true })).toBeInTheDocument();
  await waitFor(() => expect(api.callsTo("tree.get").map((c) => c.env)).toContain("wsl"));
});
