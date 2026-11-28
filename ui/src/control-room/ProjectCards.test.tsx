// Cards por projeto na Sala de controle (#358): sem hub (só o que vem dos
// ambientes) e com hub (PRs/MRs, release e épicos), com dados fictícios.
import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { App } from "../App";
import { FakeDaemonApi } from "../api/fake";
import type { ControlRoomProjects, HubDashboard, HubSession, ProjectRelease } from "../protocol/hub-gen";
import {
  HUB_DASHBOARD,
  dashboard,
  hubProject,
  hubProjectView,
  hubStatus,
  MIN,
  project,
  session,
  sync,
  workItem,
} from "../test/fixtures";

const NOW = Date.now();

const cards = () => screen.findByRole("list", { name: "Cards dos projetos" });
const card = async (name: string) => {
  const list = await cards();
  return within(list).getByRole("listitem", { name });
};

function localApi() {
  const acme = project({ id: 1, name: "acme-web", gh_owner: "acme", gh_repo: "acme-web" });
  const other = project({ id: 2, name: "acme-api", gh_owner: "acme", gh_repo: "acme-api" });
  const d = dashboard({
    projects: [
      { project: other, work_items: [workItem({ id: 50, project_id: 2, gh_number: 50, status: "backlog" })] },
      {
        project: acme,
        work_items: [
          workItem({ id: 31, gh_number: 31, status: "aguardando_voce", title: "Exportar relatório" }),
          workItem({ id: 32, gh_number: 32, status: "em_execucao", title: "Filtro por data" }),
          workItem({ id: 33, gh_number: 33, status: "em_revisao", title: "Menu lateral", pending_decisions: 3 }),
          workItem({ id: 34, gh_number: 34, status: "concluido", closed_at: NOW - MIN }),
        ],
      },
    ],
    live_sessions: [
      session({ id: "aaaaaaaa-0000-0000-0000-000000000031", work_item_id: 31, last_event_at: NOW - 5 * MIN }),
      session({ id: "aaaaaaaa-0000-0000-0000-000000000032", work_item_id: 32, last_event_at: NOW - MIN }),
    ],
    failed_sessions: [
      session({ id: "aaaaaaaa-0000-0000-0000-000000000033", work_item_id: 33, state: "exited", exit_code: 1, ended_at: NOW - 20 * MIN }),
    ],
    sync: [
      sync({ project_id: 1, last_ok_at: NOW - MIN, last_attempt_at: NOW - MIN }),
      sync({ project_id: 2, last_ok_at: NOW - 40 * MIN, last_attempt_at: NOW - MIN }),
    ],
  });
  return new FakeDaemonApi().on("dashboard.get", d).on("session.peek", { data: "" });
}

test("sem hub: um card por projeto com sessões, fluxo, decisões e sync; sem PRs, release e épicos", async () => {
  render(<App api={localApi()} />);
  const user = userEvent.setup();
  const list = await cards();
  // O que mais pede você vem primeiro (acme-web, 2 itens), depois a ordem dos projetos.
  expect(within(list).getAllByRole("heading", { level: 3 }).map((h) => h.textContent)).toEqual(["acme-web", "acme-api"]);

  const web = await card("acme-web");
  expect(within(web).getByText("acme/acme-web")).toBeInTheDocument();
  // Aguardando você e a que saiu com erro; o sync atrasado é do outro projeto.
  expect(within(web).getByText("2 pedem você")).toBeInTheDocument();
  const agora = within(web).getByRole("list", { name: "Sessões de acme-web" });
  expect(within(web).getByText("Agora · 3 sessões")).toBeInTheDocument();
  expect(within(agora).getAllByRole("button").map((b) => b.textContent)).toEqual([
    "#31Exportar relatórioAguardando",
    "#33Menu lateralFalhou",
    "#32Filtro por dataRodando",
  ]);
  const fluxo = within(web).getByLabelText("Fluxo de acme-web");
  expect(within(fluxo).getByText("Em andamento").nextSibling).toHaveTextContent("2");
  expect(within(fluxo).getByText("Em revisão").nextSibling).toHaveTextContent("1");
  expect(within(fluxo).getByText("Feitas 7 dias").nextSibling).toHaveTextContent("1");
  expect(within(web).getByText("Decisões a revisar").nextSibling).toHaveTextContent("3 autônomas");
  expect(within(web).getByText("GitHub · sincronizado há 1 min")).toBeInTheDocument();
  // Sem hub: nada de PRs, release ou épicos (nem seção vazia).
  expect(within(list).queryByText(/PRs abertos|MRs abertos|Release|épico/i)).not.toBeInTheDocument();

  const api = await card("acme-api");
  expect(within(api).getByText("1 pede você")).toBeInTheDocument();
  expect(within(api).getByText("Nenhuma sessão rodando")).toBeInTheDocument();
  expect(within(api).getByText("GitHub · sync atrasado (último há 40 min)")).toHaveAttribute("data-warn", "true");

  // Filtros com as contagens.
  const filtros = screen.getByRole("group", { name: "Filtrar projetos" });
  expect(within(filtros).getAllByRole("button").map((b) => b.textContent)).toEqual([
    "Todos · 2",
    "Com pendência · 2",
    "Com sessão rodando · 1",
  ]);
  await user.click(within(filtros).getByRole("button", { name: "Com sessão rodando · 1" }));
  expect(within(await cards()).getAllByRole("heading", { level: 3 }).map((h) => h.textContent)).toEqual(["acme-web"]);
  await user.click(within(filtros).getByRole("button", { name: "Todos · 2" }));

  // A sessão abre o terminal; "Abrir projeto" abre a tela do projeto.
  await user.click(within(agora).getByRole("button", { name: /Filtro por data/ }));
  expect(screen.getByRole("button", { name: "Terminal", description: /sess(ão|ões)$/ })).toHaveAttribute("aria-current", "page");
  await user.click(screen.getByRole("button", { name: "Sala de controle" }));
  await user.click(within(await card("acme-web")).getByRole("button", { name: "Abrir projeto ›" }));
  expect(await screen.findByRole("tab", { name: /acme-web/, selected: true })).toBeInTheDocument();
});

function hubApi(edit: (room: ControlRoomProjects) => void = () => {}) {
  const web = hubProject({
    id: 1,
    name: "acme-web",
    gh_owner: "acme",
    gh_repo: "acme-web",
    autonomy: { enabled: true, env_id: "env-1", branch: "", base_branch: "", strategy: "merge" },
  });
  const mobile = hubProject({
    id: 2,
    name: "acme-mobile",
    gh_owner: "grupo",
    gh_repo: "acme-mobile",
    forge: { kind: "gitlab", path: "grupo/acme-mobile", clone_url: "https://gitlab.example.com/grupo/acme-mobile.git", connection: 1 },
  });
  const live: HubSession = {
    id: "bbbbbbbb-0000-0000-0000-000000000031",
    env_id: "env-1",
    work_item_id: 101,
    tool: "claude",
    tool_session_id: null,
    cwd: "/w",
    state: "running",
    exit_code: null,
    started_at: NOW - 10 * MIN,
    last_event_at: NOW - MIN,
    ended_at: null,
    model: null,
    mode: "native",
    rc_session_id: null,
  };
  const hd: HubDashboard = {
    ...HUB_DASHBOARD,
    projects: [
      {
        project: web,
        work_items: [
          { title: "Relatório em PDF", pending_decisions: 0, id: 101, project_id: 1, gh_number: 31, kind: "issue", status: "em_execucao", branch: null, created_at: 0, closed_at: null, priority: null, rank: null, model: null, effort: null },
        ],
      },
      { project: mobile, work_items: [] },
    ],
    live_sessions: [live],
  };
  const room: ControlRoomProjects = {
    now: NOW,
    projects: [
      {
        project_id: 1,
        owner: "acme",
        repo: "acme-web",
        name: "acme-web",
        tracker: "github",
        forge: "github",
        prs: [
          {
            number: 40,
            title: "Relatório em PDF",
            issue: 31,
            issue_ref: "acme/acme-web#31",
            url: "https://github.com/acme/acme-web/pull/40",
            head: "feat/31-pdf",
            base: "develop",
            updated_at: NOW - MIN,
            checks: { state: "success", total: 3, failed: 0, pending: 0, text: "3 checks ok", failed_names: [], head_sha: "abc", synced_at: NOW - MIN },
          },
        ],
        epic_runs: [
          {
            run_id: 7,
            epic: 30,
            epic_ref: "acme/acme-web#30",
            title: "Relatórios v2",
            status: "executando",
            env_id: "env-1",
            env_online: true,
            plan_version: 1,
            done: 4,
            total: 7,
            current: [{ number: 31, item_ref: "acme/acme-web#31", title: null, state: "em_execucao", pr: 40 }],
            next: { number: 35, item_ref: "acme/acme-web#35", title: null, state: "pronta", pr: null },
            pending_decisions: 0,
            stop: null,
            updated_at: NOW,
          },
        ],
      },
      { project_id: 2, owner: "grupo", repo: "acme-mobile", name: "acme-mobile", tracker: "github", forge: "gitlab", prs: [], epic_runs: [] },
    ],
  };
  edit(room);
  let unreleased = 5;
  const releases = (): ProjectRelease[] => [
    {
      project_id: 1,
      owner: "acme",
      repo: "acme-web",
      supported: true,
      synced: true,
      release: { tag: "v1.4.2", name: "v1.4.2", published_at: NOW - 86_400_000, url: "https://github.com/acme/acme-web/releases/v1.4.2" },
      unreleased,
      deliveries: [],
      truncated: false,
      synced_at: NOW - MIN,
    },
    { project_id: 2, owner: "grupo", repo: "acme-mobile", supported: false, synced: false, release: null, unreleased: 0, deliveries: [], truncated: false, synced_at: null },
  ];
  const api = new FakeDaemonApi()
    .on("dashboard.get", dashboard({ projects: [], sync: [] }))
    .withHub()
    .onHub("hub.status", hubStatus())
    .onHub("hub.dashboard.get", hd)
    .onHub("hub.projects.list", [
      { ...hubProjectView(web), sync: sync({ last_ok_at: NOW - 2 * MIN, last_attempt_at: NOW - 2 * MIN }) },
      hubProjectView(mobile),
    ])
    .onHub("hub.control_room.projects", room)
    .onHub("hub.projects.release", () => releases());
  return { api, setUnreleased: (n: number) => (unreleased = n) };
}

test("com hub: PRs com checks, release com entregas e épicos em execução; GitLab com MRs e —", async () => {
  const { api, setUnreleased } = hubApi();
  render(<App api={api} />);
  const user = userEvent.setup();
  const web = await card("acme-web");
  expect(within(web).getByText("autônomo")).toBeInTheDocument();
  expect(within(web).getByText("Tudo andando")).toBeInTheDocument();
  expect(within(web).getByText("Agora · 1 sessão")).toBeInTheDocument();
  expect(within(web).getByText("PRs abertos").nextSibling).toHaveTextContent("1 · checks ok");
  expect(await within(web).findByText("v1.4.2")).toBeInTheDocument();
  expect(within(web).getByText("5 entregas na develop sem release")).toHaveAttribute("data-warn", "true");
  expect(within(web).getByText("Épicos em execução · 1")).toBeInTheDocument();
  expect(within(web).getByRole("progressbar", { name: "Progresso de #30" })).toHaveAttribute("aria-valuetext", "4 de 7 sub-issues");
  expect(within(web).getByText("Em andamento: #31 · próxima #35")).toBeInTheDocument();
  expect(within(web).getByText("GitHub · sincronizado há 2 min")).toBeInTheDocument();

  const mobile = await card("acme-mobile");
  expect(within(mobile).getByText("Parado")).toBeInTheDocument();
  expect(within(mobile).getByText("MRs abertos").nextSibling).toHaveTextContent("Nenhum");
  expect(within(mobile).getByText("Release").nextSibling).toHaveTextContent("—");
  expect(within(mobile).getByText("Nenhum épico em execução")).toBeInTheDocument();

  // Uma leitura para a tela inteira; o push do hub relê a release.
  expect(api.hubCallsTo("hub.projects.release")).toHaveLength(1);
  setUnreleased(0);
  act(() => api.hubPush("hub.control_room.changed", { project_id: 1 }));
  expect(await within(await card("acme-web")).findByText("Nada na develop sem release")).toBeInTheDocument();

  // O épico abre a execução no hub.
  await user.click(within(await card("acme-web")).getByRole("button", { name: /Relatórios v2/ }));
  expect(await screen.findByRole("tab", { name: /Épico #30/, selected: true })).toBeInTheDocument();
});

test("com hub: check vermelho conta no badge e põe o projeto na frente", async () => {
  const { api } = hubApi((room) => {
    room.projects[1].prs = [
      {
        ...room.projects[0].prs[0],
        number: 12,
        url: "https://gitlab.example.com/grupo/acme-mobile/-/merge_requests/12",
        checks: { ...room.projects[0].prs[0].checks, state: "failure", failed_names: ["e2e"] },
      },
    ];
  });
  render(<App api={api} />);
  await waitFor(async () =>
    expect(within(await cards()).getAllByRole("heading", { level: 3 }).map((h) => h.textContent)).toEqual([
      "acme-mobile",
      "acme-web",
    ]),
  );
  const mobile = await card("acme-mobile");
  expect(within(mobile).getByText("1 pede você")).toBeInTheDocument();
  expect(within(mobile).getByText("MRs abertos").nextSibling).toHaveTextContent("1 · e2e vermelho");
});
