import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { App } from "../App";
import { FakeDaemonApi } from "../api/fake";
import type { EnvInfo, IssueRow } from "../protocol/gen";
import type { HubDashboard, HubInfo, HubProject, HubTree, HubTreeNode, HubWorkItem } from "../protocol/hub-gen";
import { HUB_CONFIG_UNLINKED, dashboard, healthSignal, hubAssignment, project, session, sync, workItem, RC_AVAILABLE } from "../test/fixtures";

const NOW = Date.now();

function info(env_id: string, p: Partial<EnvInfo> = {}): EnvInfo {
  return {
    protocol: "2.0",
    version: "0.1.0",
    hostname: env_id,
    worktrees_root: `/home/${env_id}/worktrees`,
    env_id,
    env_name: env_id,
    hub_sync: null,
    hub_config: HUB_CONFIG_UNLINKED,
    commit: "",
    update: null,
    remote_control: RC_AVAILABLE,
    docs: { root: null, reachable: false },
    ...p,
  };
}

const LINKED = { ...HUB_CONFIG_UNLINKED, linked: true, config_rev: 7, pulled_at: NOW, age_ms: 0 };

const HUB_INFO: HubInfo = {
  protocol: "1.0",
  version: "0.3.0",
  commit: "",
  name: "mac-mini",
  config_rev: 7,
  docs_dir: null,
  backup: {
    dir: "/backup",
    interval_hours: 6,
    keep: 28,
    newest_at: NOW,
    last_ok_at: NOW,
    last_error: null,
    same_device: false,
    remote: {
      kind: "none",
      target: "",
      encrypt: true,
      credentials_configured: false,
      last_ok_at: null,
      last_name: null,
      last_size: null,
      last_error: null,
      failures: 0,
    },
  },
};

const SENDING = {
  pending: 3,
  oldest_pending_at: NOW,
  last_attempt_at: NOW,
  last_ok_at: null,
  last_error: "hub inacessível: conexão recusada",
  failures: 2,
  syncing: false,
  prune: { retention_days: 30, pruned_work_items: 0, last_run_at: null, last_pruned: 0, last_error: null },
};

function issue(n: number, title: string, p: Partial<IssueRow> = {}): IssueRow {
  return {
    gh_number: n,
    title,
    state: "open",
    labels: ["ui"],
    parent_number: null,
    has_sub_issues: false,
    url: `https://github.com/deep-ion-ai/ctower-agents/issues/${n}`,
    updated_at: 0,
    ...p,
  };
}

const HUB_PROJECT: HubProject = {
  id: 7,
  gh_owner: "deep-ion-ai",
  gh_repo: "ctower-agents",
  name: "ctower-agents",
  base_branch: "develop",
  protected_branches: ["develop", "main"],
  created_at: 0,
  rank: null,
  tracker: { kind: "github" },
  forge: { kind: "github" },
  branch_pattern: null,
  attribution: true,
  sync_env_id: null,
  docs: { enabled: false, project_dirs: [], epic_dirs: [] },
};

function hubItem(n: number, p: Partial<HubWorkItem> = {}): HubWorkItem {
  return {
    id: 100 + n,
    project_id: 7,
    gh_number: n,
    kind: "issue",
    status: "em_execucao",
    branch: `feat/${n}-x`,
    created_at: 0,
    closed_at: null,
    priority: null,
    rank: null,
    ...p,
  };
}

const node = (i: IssueRow, wi: HubWorkItem | null, live: string[] = [], children: HubTreeNode[] = []): HubTreeNode => ({
  issue: i,
  work_item: wi,
  summary: { pr: null, decisions: { total: 0, pending_autonomous: 0 }, last_activity_at: null, live_envs: live },
  children,
});

const TREE: HubTree = {
  project: HUB_PROJECT,
  sync: sync({ project_id: 7, last_ok_at: NOW }),
  roots: [
    node(issue(64, "Épico: ledger centralizado", { has_sub_issues: true }), hubItem(64, { kind: "epic" }), [], [
      node(issue(57, "env_id e HLC", { parent_number: 64 }), hubItem(57, { status: "concluido" })),
      node(issue(59, "sync ambiente→hub", { parent_number: 64 }), hubItem(59), ["notebook"]),
      node(issue(62, "ui: hub", { parent_number: 64 }), null),
    ]),
    // Épico aberto, mas parado há 10 dias e sem sessão viva: começa recolhido.
    {
      ...node(issue(80, "Épico: parado", { has_sub_issues: true }), hubItem(80, { kind: "epic" }), [], [
        node(issue(81, "filha parada", { parent_number: 80 }), hubItem(81, { status: "bloqueado" })),
      ]),
      summary: {
        pr: null,
        decisions: { total: 0, pending_autonomous: 0 },
        last_activity_at: NOW - 10 * 24 * 3600_000,
        live_envs: [],
      },
    },
  ],
  untracked: [],
};

const HUB_DASH: HubDashboard = {
  projects: [{ project: HUB_PROJECT, work_items: [] }],
  live_sessions: [],
  interrupted_sessions: [],
  failed_sessions: [],
  sync: [],
  envs: [],
  gestor_work_item_id: null,
};

function envs() {
  return [
    { ...FakeDaemonApi.wslEnv("wsl"), name: "WSL" },
    { ...FakeDaemonApi.wslEnv("mac"), name: "Mac mini", kind: "remote" as const, distro: null },
  ];
}

const WI62 = workItem({ gh_number: 62, status: "backlog", branch: null, worktree_path: null });
const WI59 = workItem({ gh_number: 59 });
const LIVE59 = session({ id: "59595959-0000-0000-0000-000000000059", work_item_id: 59 });

const side = () => screen.getByRole("complementary", { name: "Árvore do projeto" });
const rowBtn = (name: RegExp) => within(side()).getByRole("button", { name });
const play = (n: number) => within(side()).getByRole("button", { name: `Iniciar sessão para #${n}` });
const startButton = () => within(screen.getByRole("dialog")).getByRole("button", { name: /^Iniciar sessão/ });

function setup(hubUp: boolean) {
  const api = new FakeDaemonApi(envs())
    .on("dashboard.get", (_: unknown, env: string) =>
      env === "wsl"
        ? dashboard({ live_sessions: [LIVE59] }, [WI59, WI62])
        : dashboard({ projects: [] }),
    )
    .on("env.info", (_: unknown, env: string) =>
      env === "wsl"
        ? info("daemon-wsl", { hub_sync: SENDING, hub_config: LINKED })
        : info("daemon-mac", { hub_config: LINKED }),
    )
    .withHub("ws://mac-mini:7879/ws", hubUp)
    .onHub("hub.info", HUB_INFO)
    .onHub("hub.dashboard.get", HUB_DASH)
    .onHub("hub.tree.get", TREE)
    .on("tree.get", { project: project(), sync: sync(), roots: [], untracked: [] })
    .on("session.peek", { data: "" })
    .on("session.start", session({ id: "62626262-0000-0000-0000-000000000062", work_item_id: 62 }));
  return api;
}

test("hub fora: aviso de visão parcial e pendências do envio", async () => {
  const api = setup(false);
  render(<App api={api} />);
  expect(await screen.findByText(/hub indisponível: visão parcial/)).toBeInTheDocument();
  const side = screen.getByRole("complementary", { name: "Ambientes e projetos" });
  expect(await within(side).findByText(/hub: 3 pendentes · falhou/)).toBeInTheDocument();
  expect(api.callsTo("hub.dashboard.get")).toHaveLength(0);
});

test("hub no ar: sem aviso, conexão própria e projetos da visão consolidada", async () => {
  const api = setup(true);
  render(<App api={api} />);
  const side = screen.getByRole("complementary", { name: "Ambientes e projetos" });
  expect(await within(side).findByText("conectado · configuração rev 7")).toBeInTheDocument();
  expect(within(side).getByText("mac-mini")).toBeInTheDocument();
  await waitFor(() => expect(api.callsTo("hub.dashboard.get").length).toBeGreaterThan(0));
  expect(screen.queryByText(/hub indisponível/)).not.toBeInTheDocument();
  // O hub tem conexão própria: nenhum ambiente recebe chamadas hub.*.
  expect(api.calls.filter((c) => c.method.startsWith("hub.") && c.env !== "hub")).toHaveLength(0);
  // Tile da sessão viva diz em que ambiente roda.
  expect(await screen.findByText(/WSL · claude ·/)).toBeInTheDocument();
});

test("tela Projeto pelo hub: ambiente da sessão viva e início por owner/repo", async () => {
  const api = setup(true);
  render(<App api={api} initial={{ name: "project", owner: "deep-ion-ai", repo: "ctower-agents" }} />);
  expect(await screen.findByText("visão do hub")).toBeInTheDocument();
  // "visão do hub" aparece antes da primeira chamada ser registrada: espera por ela.
  await waitFor(() => expect(api.callsTo("hub.tree.get").length).toBeGreaterThan(0));
  expect(api.callsTo("hub.tree.get")[0]).toMatchObject({
    env: "hub",
    params: { owner: "deep-ion-ai", repo: "ctower-agents" },
  });
  // Sessão viva em outro ambiente (que esta UI não conhece): o hub diz onde.
  await screen.findByText("sync ambiente→hub");
  expect(rowBtn(/^#59 sync ambiente→hub, em execução.*sessão viva em notebook$/)).toBeInTheDocument();
  expect(rowBtn(/^#57 env_id e HLC, concluído/)).toBeInTheDocument();

  // No item, a sessão viva do WSL abre o terminal e o hub diz onde mais roda.
  const user = userEvent.setup();
  await user.click(rowBtn(/^#59 /));
  const item = await screen.findByRole("region", { name: "Detalhe do item" });
  expect(within(item).getByRole("button", { name: "Abrir terminal · WSL" })).toBeInTheDocument();
  expect(within(item).getByText("rodando em notebook")).toBeInTheDocument();

  await user.click(play(62));
  const env = within(screen.getByRole("dialog")).getByLabelText("Ambiente") as HTMLSelectElement;
  expect(env.value).toBe("wsl");
  await user.click(startButton());
  await waitFor(() => expect(api.callsTo("session.start")).toHaveLength(1));
  const call = api.callsTo("session.start")[0];
  expect(call.env).toBe("wsl");
  expect(call.params).toMatchObject({
    owner: "deep-ion-ai",
    repo: "ctower-agents",
    gh_number: 62,
    title: "ui: hub",
    labels: ["ui"],
  });
  expect(call.params.project_id).toBeUndefined();
});

test("endereço do hub com o hub fora cai no projeto local", async () => {
  const api = setup(false);
  render(<App api={api} initial={{ name: "project", owner: "Deep-Ion-AI", repo: "ctower-agents" }} />);
  await waitFor(() => expect(api.callsTo("tree.get")).toHaveLength(1));
  expect(api.callsTo("tree.get")[0]).toMatchObject({ env: "wsl", params: { project_id: 1 } });
  expect(screen.queryByText("visão do hub")).not.toBeInTheDocument();
});

test("hub: só o épico com sessão viva ou atividade recente começa expandido, com motivo no recolhido", async () => {
  const api = setup(true);
  render(<App api={api} initial={{ name: "project", owner: "deep-ion-ai", repo: "ctower-agents" }} />);
  await screen.findByText("sync ambiente→hub");
  expect(screen.queryByText("filha parada")).not.toBeInTheDocument();
  const user = userEvent.setup();
  await user.click(screen.getByRole("button", { name: "Expandir épico #80" }));
  expect(screen.getByText("filha parada")).toBeInTheDocument();
  // Chip ativo expande tudo e filtra pelas filhas do status.
  await user.click(screen.getByRole("button", { name: "Recolher épico #80" }));
  await user.click(screen.getByRole("button", { name: "Filtros" }));
  await user.click(screen.getByRole("button", { name: "bloqueado" }));
  expect(screen.getByText("filha parada")).toBeInTheDocument();
  expect(screen.queryByText("sync ambiente→hub")).not.toBeInTheDocument();
  expect(screen.queryByText("env_id e HLC")).not.toBeInTheDocument();
});

describe("Quadro (Kanban) somente leitura", () => {
  const open = async (api = setup(true)) => {
    render(<App api={api} initial={{ name: "project", owner: "deep-ion-ai", repo: "ctower-agents" }} />);
    await screen.findByText("sync ambiente→hub");
    const user = userEvent.setup();
    await user.click(screen.getByRole("button", { name: "Quadro" }));
    return user;
  };
  const col = (nome: RegExp) => screen.getByRole("region", { name: nome });
  afterEach(() => window.sessionStorage.clear());

  test("uma coluna por status, com épico (ÉPICO e progresso) e sub-issue com 'épico #N'", async () => {
    await open();
    for (const t of ["backlog", "em execução", "aguardando você", "em revisão", "bloqueado", "pausado", "concluído"]) {
      expect(screen.getByRole("region", { name: new RegExp(`^Coluna ${t}, `) })).toBeInTheDocument();
    }
    const exec = col(/^Coluna em execução/);
    const epico = within(exec).getByRole("button", { name: /^#64 ÉPICO/ });
    expect(within(epico).getByText("ÉPICO")).toBeInTheDocument();
    expect(within(epico).getByText("1/3")).toBeInTheDocument();
    const filha = within(exec).getByRole("button", { name: /sync ambiente→hub/ });
    expect(within(filha).getByText("épico #64")).toBeInTheDocument();
    expect(within(filha).getByText("feat/59-x")).toBeInTheDocument();
    expect(within(filha).getByRole("img", { name: /sessão viva em notebook/ })).toBeInTheDocument();
    expect(within(col(/^Coluna concluído/)).getByRole("button", { name: /env_id e HLC/ })).toBeInTheDocument();
    // Item sem work item cai em backlog; a filha do épico parado aparece no seu status.
    expect(within(col(/^Coluna backlog/)).getByRole("button", { name: /ui: hub/ })).toBeInTheDocument();
    expect(within(col(/^Coluna bloqueado/)).getByRole("button", { name: /filha parada/ })).toBeInTheDocument();
  });

  test("busca e chips filtram as colunas", async () => {
    const user = await open();
    await user.type(screen.getByRole("searchbox"), "hub");
    expect(screen.getAllByRole("button", { name: /ui: hub/ }).length).toBeGreaterThan(0);
    expect(screen.queryByRole("button", { name: /env_id e HLC/ })).not.toBeInTheDocument();
    await user.clear(screen.getByRole("searchbox"));
    await user.click(screen.getByRole("button", { name: "Filtros" }));
    await user.click(screen.getByRole("button", { name: "bloqueado" }));
    expect(screen.getAllByRole("region", { name: /^Coluna / })).toHaveLength(1);
    expect(screen.getAllByRole("button", { name: /filha parada/ }).length).toBeGreaterThan(0);
    expect(screen.queryByRole("button", { name: /sync ambiente→hub/ })).not.toBeInTheDocument();
  });

  test("cards não são arrastáveis nem aceitam drop", async () => {
    await open();
    const cards = screen.getAllByRole("button", { name: /#\d+/ }).filter((b) => b.classList.contains("card"));
    expect(cards.length).toBeGreaterThan(3);
    for (const c of cards) expect(c.getAttribute("draggable")).not.toBe("true");
    const alvo = col(/^Coluna concluído/);
    const card = within(col(/^Coluna em execução/)).getByRole("button", { name: /sync ambiente→hub/ });
    const dt = { setData: vi.fn(), getData: () => "", effectAllowed: "", dropEffect: "" };
    fireEvent.dragStart(card, { dataTransfer: dt });
    fireEvent.dragOver(alvo, { dataTransfer: dt });
    fireEvent.drop(alvo, { dataTransfer: dt });
    expect(dt.setData).not.toHaveBeenCalled();
    expect(within(alvo).queryByRole("button", { name: /sync ambiente→hub/ })).not.toBeInTheDocument();
    expect(within(col(/^Coluna em execução/)).getByRole("button", { name: /sync ambiente→hub/ })).toBeInTheDocument();
  });

  test("clicar no card abre o item em aba própria; a escolha da visão persiste", async () => {
    const api = setup(true);
    const user = await open(api);
    await user.click(within(col(/^Coluna em execução/)).getByRole("button", { name: /sync ambiente→hub/ }));
    const panel = await screen.findByRole("region", { name: "Detalhe do item" });
    expect(within(panel).getByText(/sync ambiente→hub/)).toBeInTheDocument();
    expect(rowBtn(/^#59 sync ambiente→hub/)).toHaveAttribute("aria-current", "true");
    cleanup();
    render(<App api={api} initial={{ name: "project", owner: "deep-ion-ai", repo: "ctower-agents" }} />);
    await waitFor(() => expect(screen.getByRole("region", { name: /^Coluna backlog/ })).toBeInTheDocument());
    expect(screen.getByRole("button", { name: "Quadro" })).toHaveAttribute("aria-pressed", "true");
  });
});

describe("cabeçalho do épico", () => {
  const epicTree = (): HubTree => ({
    ...TREE,
    roots: [
      {
        ...TREE.roots[0],
        summary: {
          pr: { number: 88, work_item_id: 164, head: "wip/64", base: "develop", state: "open", url: "#pr88", updated_at: 0, head_sha: null },
          decisions: { total: 3, pending_autonomous: 1 },
          last_activity_at: NOW - 5 * 60_000,
          live_envs: [],
        },
      },
      TREE.roots[1],
    ],
  });

  test("linha do épico: progresso, decisões pendentes e sessão viva da família no nome", async () => {
    const api = setup(true).onHub("hub.tree.get", epicTree());
    render(<App api={api} initial={{ name: "project", owner: "deep-ion-ai", repo: "ctower-agents" }} />);
    await screen.findByText("sync ambiente→hub");
    // A sessão viva é da filha #59 (notebook): o épico sinaliza a família.
    expect(
      rowBtn(/^Épico #64 Épico: ledger centralizado, em execução, 1 de 3 concluídas, 3 decisões, 1 autônoma pendente de revisão, sessão viva em notebook$/),
    ).toBeInTheDocument();
  });

  test("clicar no épico abre a aba dele, com o PR; o botão de expandir não abre", async () => {
    const api = setup(true).onHub("hub.tree.get", epicTree());
    render(<App api={api} initial={{ name: "project", owner: "deep-ion-ai", repo: "ctower-agents" }} />);
    await screen.findByText("sync ambiente→hub");
    const user = userEvent.setup();
    await user.click(screen.getByRole("button", { name: "Recolher épico #64" }));
    expect(screen.queryByRole("region", { name: "Detalhe do item" })).not.toBeInTheDocument();
    await user.click(rowBtn(/^Épico #64/));
    const panel = await screen.findByRole("region", { name: "Detalhe do item" });
    expect(within(panel).getByRole("heading", { level: 1, name: /#64 Épico: ledger centralizado/ })).toBeInTheDocument();
    expect(within(panel).getByRole("button", { name: "Abrir PR #88 no GitHub ↗" })).toBeInTheDocument();
    expect(rowBtn(/^Épico #64/)).toHaveAttribute("aria-current", "true");
  });
});

test("resposta antiga de hub.tree.get de outro projeto não sobrescreve a árvore do projeto atual", async () => {
  const OUTRO: HubProject = { ...HUB_PROJECT, id: 8, gh_repo: "outro", name: "outro" };
  const treeOutro: HubTree = {
    project: OUTRO,
    sync: sync({ project_id: 8, last_ok_at: NOW }),
    roots: [node(issue(1, "issue do outro"), hubItem(1))],
    untracked: [],
  };
  let soltar: (t: HubTree) => void = () => {};
  const api = setup(true)
    .onHub("hub.dashboard.get", {
      ...HUB_DASH,
      projects: [
        { project: HUB_PROJECT, work_items: [] },
        { project: OUTRO, work_items: [] },
      ],
    })
    .onHub("hub.tree.get", (p: { repo: string }) =>
      p.repo === "outro" ? treeOutro : new Promise<HubTree>((r) => (soltar = r)),
    );
  render(<App api={api} initial={{ name: "project", owner: "deep-ion-ai", repo: "ctower-agents" }} />);
  const user = userEvent.setup();
  // Troca pela trilha "Projetos › X" (#16): o índice e, nele, o outro projeto.
  // Já na visão do hub (antes, a local): a trilha não é remontada no meio do clique.
  await screen.findByText("visão do hub");
  const trilha = screen.getByRole("navigation", { name: "Trilha do projeto" });
  await user.click(within(trilha).getByRole("button", { name: "Projetos" }));
  await user.click(await screen.findByRole("button", { name: "outro" }));
  expect(await screen.findByText("issue do outro")).toBeInTheDocument();
  // A resposta do projeto anterior chega depois e deve ser descartada.
  await act(async () => soltar(TREE));
  expect(screen.getByText("issue do outro")).toBeInTheDocument();
  expect(screen.queryByText("sync ambiente→hub")).not.toBeInTheDocument();
  expect(screen.getByRole("heading", { name: "outro" })).toBeInTheDocument();
});

test("chip Concluído sozinho mostra os encerrados no Kanban e na árvore", async () => {
  const api = setup(true);
  render(<App api={api} initial={{ name: "project", owner: "deep-ion-ai", repo: "ctower-agents" }} />);
  await screen.findByText("sync ambiente→hub");
  const user = userEvent.setup();
  await user.click(screen.getByRole("button", { name: "Filtros" }));
  await user.click(screen.getByRole("button", { name: "concluído" }));
  expect(screen.getByText("env_id e HLC")).toBeInTheDocument();
  expect(screen.queryByText(/Nada casa com a busca/)).not.toBeInTheDocument();
});

test("iniciar escolhe um ambiente onde o projeto está atribuído, aplicadas primeiro", async () => {
  const api = setup(true)
    .on("dashboard.get", dashboard({ projects: [{ project: project(), work_items: [WI62] }] }))
    .onHub("hub.projects.list", [
      {
        project: HUB_PROJECT,
        assignments: [
          hubAssignment({ env_id: "daemon-wsl", env_name: "wsl", state: "pending" }),
          hubAssignment({ env_id: "daemon-mac", env_name: "mac", state: "applied" }),
        ],
        locations: [],
        sync: null,
      },
    ]);
  render(<App api={api} initial={{ name: "project", owner: "deep-ion-ai", repo: "ctower-agents" }} />);
  await screen.findByText("ui: hub");
  await waitFor(() => expect(api.hubCallsTo("hub.projects.list").length).toBeGreaterThan(0));
  const user = userEvent.setup();
  await user.click(play(62));
  const pick = within(screen.getByRole("dialog")).getByRole("combobox", { name: "Ambiente" });
  await waitFor(() =>
    expect(within(pick).getAllByRole("option").map((o) => o.textContent)).toEqual(["Mac mini", "WSL · pendente"]),
  );
  expect(pick).toHaveValue("mac");
  await user.click(startButton());
  await waitFor(() => expect(api.callsTo("session.start")).toHaveLength(1));
  expect(api.callsTo("session.start")[0].env).toBe("mac");
});

test("sem ambiente conectado com o projeto atribuído: acompanha, mas não inicia", async () => {
  const api = setup(true).on("dashboard.get", dashboard({ projects: [] }));
  render(<App api={api} initial={{ name: "project", owner: "deep-ion-ai", repo: "ctower-agents" }} />);
  expect(await screen.findByText(/Nenhum ambiente conectado tem este projeto atribuído/)).toBeInTheDocument();
  expect(screen.queryByRole("button", { name: /^Iniciar sessão/ })).not.toBeInTheDocument();
});

test("aviso sem ambiente: Atribuir a um ambiente abre o formulário nas Configurações › Ambientes do projeto, a cada pedido (#7, #18)", async () => {
  const api = setup(true)
    .on("dashboard.get", dashboard({ projects: [] }))
    .onHub("hub.envs.list", [
      { env_id: "daemon-wsl", name: "wsl", created_at: 0, last_seen_at: NOW, last_seq: 1, pending: 0, cleanup: null },
    ])
    .onHub("hub.projects.list", [
      { project: { ...HUB_PROJECT, id: 1, gh_repo: "outro", name: "outro" }, assignments: [], locations: [], sync: null },
      { project: HUB_PROJECT, assignments: [], locations: [], sync: null },
    ]);
  render(<App api={api} initial={{ name: "project", owner: "deep-ion-ai", repo: "ctower-agents" }} />);
  const user = userEvent.setup();
  await user.click(await screen.findByRole("button", { name: "Atribuir a um ambiente" }));
  // Na aba do próprio projeto (não do primeiro da lista), Configurações › Ambientes, com o formulário
  // aberto e o foco nele.
  const form = await screen.findByRole("form", { name: "Atribuir ctower-agents" });
  const env = within(form).getByRole("combobox", { name: "Ambiente" });
  expect(env).toHaveValue("daemon-wsl");
  expect(env).toHaveFocus();
  expect(screen.getByRole("tab", { name: "ctower-agents · Configurações › Ambientes e atribuições" })).toHaveAttribute(
    "aria-selected",
    "true",
  );
  expect(screen.queryByRole("form", { name: "Atribuir outro" })).not.toBeInTheDocument();

  // O pedido sai do endereço da aba: voltar à aba não reabre o formulário, e um novo clique abre.
  await user.click(within(form).getByRole("button", { name: "Cancelar" }));
  await user.click(within(screen.getByRole("navigation", { name: "Telas" })).getByRole("button", { name: "Hub" }));
  await user.click(screen.getByRole("tab", { name: /ctower-agents/ }));
  expect(await screen.findByRole("group", { name: "Ambientes e atribuições" })).toBeInTheDocument();
  expect(screen.queryByRole("form", { name: "Atribuir ctower-agents" })).not.toBeInTheDocument();
  await user.click(within(screen.getByRole("navigation", { name: "Abas do projeto" })).getByRole("button", { name: "Itens" }));
  await user.click(await screen.findByRole("button", { name: "Atribuir a um ambiente" }));
  expect(await screen.findByRole("form", { name: "Atribuir ctower-agents" })).toBeInTheDocument();
});

test("Configurações do projeto: um clique do cabeçalho à aba Configurações do próprio projeto, sem o toggle de secrets (#8, #17)", async () => {
  const api = setup(true)
    .on("dashboard.get", dashboard({ projects: [] }))
    .onHub("hub.projects.list", [
      { project: { ...HUB_PROJECT, id: 1, gh_repo: "outro", name: "outro" }, assignments: [], locations: [], sync: null },
      { project: HUB_PROJECT, assignments: [], locations: [], sync: null },
    ]);
  render(<App api={api} initial={{ name: "project", owner: "deep-ion-ai", repo: "ctower-agents" }} />);
  const user = userEvent.setup();
  await screen.findByText("visão do hub");
  expect(screen.queryByRole("button", { name: "Secrets e variáveis" })).not.toBeInTheDocument();
  await user.click(screen.getByRole("button", { name: "Configurações do projeto" }));
  // A aba Configurações do projeto do cabeçalho (não o primeiro da lista), na mesma aba de editor.
  const abas = screen.getByRole("navigation", { name: "Abas do projeto" });
  expect(within(abas).getByRole("button", { name: "Configurações" })).toHaveAttribute("aria-current", "page");
  expect(screen.getByRole("tab", { name: /ctower-agents · Configurações/ })).toHaveAttribute("aria-selected", "true");
  expect(screen.queryByRole("tab", { name: /Projetos e atribuições/ })).not.toBeInTheDocument();
  expect(await screen.findByRole("group", { name: "Geral" })).toHaveTextContent("Branch base");
  // Os secrets dele ao alcance, na seção própria.
  const secoes = screen.getByRole("navigation", { name: "Seções das configurações do projeto" });
  await user.click(within(secoes).getByRole("button", { name: "Secrets e variáveis" }));
  const sec = await screen.findByRole("group", { name: "Secrets e variáveis" });
  expect(within(sec).getByRole("heading", { name: /Secrets e variáveis do projeto/ })).toBeInTheDocument();
  expect(within(sec).getByRole("button", { name: "Gerenciar secrets do projeto" })).toBeInTheDocument();
});

test("com o hub no ar, ambiente conhecido do hub e sem atribuição do projeto não entra no início", async () => {
  const api = setup(true)
    .on("dashboard.get", dashboard({ projects: [{ project: project(), work_items: [WI62] }] }))
    .onHub("hub.envs.list", [
      { env_id: "daemon-wsl", name: "wsl", created_at: 0, last_seen_at: NOW, last_seq: 1, pending: 0, cleanup: null },
      { env_id: "daemon-mac", name: "mac", created_at: 0, last_seen_at: NOW, last_seq: 1, pending: 0, cleanup: null },
    ])
    .onHub("hub.projects.list", [
      {
        project: HUB_PROJECT,
        assignments: [hubAssignment({ env_id: "daemon-wsl", env_name: "wsl", state: "pending" })],
        locations: [],
        sync: null,
      },
    ]);
  render(<App api={api} initial={{ name: "project", owner: "deep-ion-ai", repo: "ctower-agents" }} />);
  await waitFor(() => expect(api.hubCallsTo("hub.envs.list").length).toBeGreaterThan(0));
  await screen.findByText("ui: hub");
  await waitFor(() => expect(api.hubCallsTo("hub.projects.list").length).toBeGreaterThan(0));
  // Só o WSL (atribuído) sobra no seletor, e a sessão sobe nele.
  const user = userEvent.setup();
  await user.click(play(62));
  const pick = within(screen.getByRole("dialog")).getByRole("combobox", { name: "Ambiente" });
  await waitFor(() =>
    expect(within(pick).getAllByRole("option").map((o) => o.textContent)).toEqual(["WSL · pendente"]),
  );
  await user.click(startButton());
  await waitFor(() => expect(api.callsTo("session.start")).toHaveLength(1));
  expect(api.callsTo("session.start")[0].env).toBe("wsl");
});

test("Abrir pasta de docs: no projeto e no épico pelo ambiente WSL; inacessível, desabilitado com a dica (#444)", async () => {
  const ON = { enabled: true, project_dirs: ["analises"], epic_dirs: ["prompts"] };
  const ROOT = "/mnt/ct-docs";
  const mount = (reachable: boolean) =>
    setup(true)
      .on("dashboard.get", (_: unknown, env: string) =>
        env === "wsl"
          ? dashboard({ projects: [{ project: project({ docs: ON }), work_items: [WI62] }] })
          : dashboard({ projects: [] }),
      )
      .on("env.info", (_: unknown, env: string) =>
        info(env === "wsl" ? "daemon-wsl" : "daemon-mac", {
          protocol: "2.8",
          docs: reachable ? { root: ROOT, reachable } : { root: ROOT, reachable, warning: "docs_unreachable" },
        }),
      )
      .on("docs.dir", (p: { epic?: number }) => ({
        path: p.epic ? `${ROOT}/deep-ion-ai__ctower-agents/epicos/64-x` : `${ROOT}/deep-ion-ai__ctower-agents`,
      }));

  const down = mount(false);
  render(<App api={down} initial={{ name: "project", owner: "deep-ion-ai", repo: "ctower-agents" }} />);
  const off = () => screen.getByRole("button", { name: "Abrir pasta de docs" });
  await waitFor(() => expect(off()).toHaveAttribute("title", expect.stringContaining(`docs_root ${ROOT} inacessível no ambiente WSL`)));
  expect(off()).toBeDisabled();
  cleanup();

  const api = mount(true);
  render(<App api={api} initial={{ name: "project", owner: "deep-ion-ai", repo: "ctower-agents" }} />);
  const user = userEvent.setup();
  const btn = () => screen.getByRole("button", { name: "Abrir pasta de docs" });
  await waitFor(() => expect(btn()).toBeEnabled());
  await user.click(btn());
  await waitFor(() => expect(api.folders).toHaveLength(1));
  expect(api.callsTo("docs.dir")[0]).toMatchObject({ env: "wsl", params: { project_id: 1 } });
  expect(api.folders[0]).toEqual({ env: "wsl", path: `${ROOT}/deep-ion-ai__ctower-agents` });

  // Épico: pelo menu de contexto da linha, com o número do épico.
  const linha = await screen.findByText("Épico: ledger centralizado");
  fireEvent.contextMenu(linha.closest("li")!, { clientX: 10, clientY: 10 });
  await user.click(within(screen.getByRole("menu")).getByRole("menuitem", { name: "Abrir pasta de docs do épico" }));
  await waitFor(() => expect(api.folders).toHaveLength(2));
  expect(api.callsTo("docs.dir")[1].params).toEqual({ project_id: 1, epic: 64 });
  expect(api.folders[1].path).toBe(`${ROOT}/deep-ion-ai__ctower-agents/epicos/64-x`);
  // Sub-issue não tem a ação.
  fireEvent.contextMenu((await screen.findByText("ui: hub")).closest("li")!, { clientX: 10, clientY: 10 });
  expect(within(screen.getByRole("menu")).queryByRole("menuitem", { name: /pasta de docs/ })).not.toBeInTheDocument();
});

test("modal de início com o hub: atalhos para as regras do projeto e do épico pai (#11)", async () => {
  const api = setup(true);
  render(<App api={api} initial={{ name: "project", owner: "deep-ion-ai", repo: "ctower-agents" }} />);
  await screen.findByText("ui: hub");
  const user = userEvent.setup();

  // Sub-issue: as regras do épico são as do pai (#64), no detalhe dele.
  await user.click(play(62));
  await user.click(within(screen.getByRole("dialog")).getByRole("button", { name: "Regras do épico #64 ›" }));
  expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  expect(screen.getByRole("tab", { name: /^#64 Épico: ledger centralizado/ })).toHaveAttribute("aria-selected", "true");

  await user.click(play(62));
  await user.click(within(screen.getByRole("dialog")).getByRole("button", { name: "Regras do projeto ›" }));
  expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  // Configurações › Regras do agente do próprio projeto (#18), na aba dele.
  expect(screen.getByRole("tab", { name: "ctower-agents · Configurações › Regras do agente" })).toHaveAttribute(
    "aria-selected",
    "true",
  );
});

// Abas do projeto (#17): Visão geral, Itens, Sessões e Configurações.

const INT62 = session({ id: "62626262-0000-0000-0000-00000000aaaa", work_item_id: 62, state: "interrupted", exit_code: null });
const FAIL59 = session({ id: "59595959-0000-0000-0000-00000000ffff", work_item_id: 59, state: "exited", exit_code: 2, ended_at: NOW });

function setupTabs(hubUp = true) {
  return setup(hubUp)
    .on("dashboard.get", (_: unknown, env: string) =>
      env === "wsl"
        ? dashboard({ live_sessions: [LIVE59], interrupted_sessions: [INT62], failed_sessions: [FAIL59] }, [WI59, WI62])
        : dashboard({ projects: [] }),
    )
    .onHub("hub.projects.list", [
      {
        project: HUB_PROJECT,
        assignments: [hubAssignment({ project_id: 7, env_id: "daemon-wsl", env_name: "wsl" })],
        locations: [],
        sync: sync({ project_id: 7, last_ok_at: NOW }),
        tracker_status: null,
      },
    ]);
}

const tabsNav = () => screen.getByRole("navigation", { name: "Abas do projeto" });
const pageBtn = (name: string) => within(tabsNav()).getByRole("button", { name });

test("abas do projeto (#17): sem `view`, Itens; cada aba troca o endereço na mesma aba de editor", async () => {
  const api = setupTabs();
  render(<App api={api} initial={{ name: "project", owner: "deep-ion-ai", repo: "ctower-agents" }} />);
  const user = userEvent.setup();
  await screen.findByText("visão do hub");
  // Endereço antigo (sem `view`): Itens, com a árvore e o resumo de sempre.
  expect(pageBtn("Itens")).toHaveAttribute("aria-current", "page");
  expect(screen.getByRole("tab", { name: "ctower-agents" })).toHaveAttribute("aria-selected", "true");

  // Visão geral: estado, atribuições e a sessão viva, que abre o terminal.
  await user.click(pageBtn("Visão geral"));
  expect(screen.getByRole("tab", { name: "ctower-agents · Visão geral" })).toHaveAttribute("aria-selected", "true");
  expect(screen.getAllByRole("tab").filter((t) => /ctower-agents/.test(t.textContent ?? ""))).toHaveLength(1);
  const atribs = await screen.findByRole("list", { name: "Atribuições do projeto" });
  expect(atribs).toHaveTextContent("wsl");
  expect(atribs).toHaveTextContent("aplicada");
  expect(await screen.findByRole("heading", { name: "Sessões vivas · 1" })).toBeInTheDocument();
  expect(screen.getByText(/1 viva · 1 interrompida · 1 com erro/)).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Abrir terminal: #59 Item 59" })).toBeInTheDocument();

  // Sessões: por ambiente, vivas, interrompidas (Recuperação) e encerradas com erro.
  await user.click(screen.getByRole("button", { name: "Ver sessões" }));
  expect(pageBtn("Sessões")).toHaveAttribute("aria-current", "page");
  expect(screen.getByRole("list", { name: "Vivas em WSL" })).toHaveTextContent("#59 Item 59");
  expect(screen.getByRole("list", { name: "Encerradas com erro (últimas 24 h) em WSL" })).toHaveTextContent(
    "saiu com código 2",
  );
  const ints = screen.getByRole("list", { name: "Interrompidas em WSL" });
  expect(ints).toHaveTextContent("processo perdido");
  await user.click(within(ints).getByRole("button", { name: "Abrir na Recuperação: #62 Item 62" }));
  expect(await screen.findByRole("tab", { name: /Recuperação/, selected: true })).toBeInTheDocument();

  // De volta ao projeto, que lembra a aba; Árvore volta a Itens.
  await user.click(screen.getByRole("tab", { name: "ctower-agents · Sessões" }));
  await user.click(within(side()).getByRole("button", { name: "Árvore" }));
  expect(pageBtn("Itens")).toHaveAttribute("aria-current", "page");
  expect(screen.getByRole("tab", { name: "ctower-agents" })).toBeInTheDocument();
});

test("Configurações do projeto (#17): a seção pelo endereço e cada seção com o cartão dela", async () => {
  const api = setupTabs()
    .onHub("hub.projects.update", null)
    .onHub("hub.secrets.list", { items: [], delivery: [] });
  render(
    <App
      api={api}
      initial={{ name: "project", owner: "deep-ion-ai", repo: "ctower-agents", view: "settings", section: "docs" }}
    />,
  );
  const user = userEvent.setup();
  const docs = await screen.findByRole("group", { name: "Docs" });
  expect(screen.getByRole("tab", { name: "ctower-agents · Configurações › Docs" })).toHaveAttribute("aria-selected", "true");
  expect(docs).toHaveTextContent("desligados");
  // Editar só os docs: o mesmo `docs` inteiro da edição completa.
  await user.click(within(docs).getByRole("button", { name: "Editar docs" }));
  await user.click(within(docs).getByLabelText("Pasta de docs do projeto"));
  await user.type(within(docs).getByLabelText("Pastas do projeto (em _projeto/)"), "analises");
  await user.click(within(docs).getByRole("button", { name: "Salvar docs" }));
  await waitFor(() => expect(api.hubCallsTo("hub.projects.update")).toHaveLength(1));
  expect(api.hubCallsTo("hub.projects.update")[0].params).toMatchObject({
    owner: "deep-ion-ai",
    repo: "ctower-agents",
    docs: { enabled: true, project_dirs: ["analises"], epic_dirs: [] },
  });

  const secoes = screen.getByRole("navigation", { name: "Seções das configurações do projeto" });
  expect(within(secoes).getAllByRole("button").map((b) => b.textContent)).toEqual([
    "Geral",
    "Repositório e rastreador",
    "Ambientes e atribuições",
    "Regras do agente",
    "Secrets e variáveis",
    "Docs",
    "Zona de perigo",
  ]);
  await user.click(within(secoes).getByRole("button", { name: "Repositório e rastreador" }));
  expect(await screen.findByRole("group", { name: "Repositório e rastreador" })).toHaveTextContent(/sincronizado/);
  await user.click(within(secoes).getByRole("button", { name: "Ambientes e atribuições" }));
  expect(await screen.findByRole("group", { name: "Ambientes e atribuições" })).toHaveTextContent("wsl");
  await user.click(within(secoes).getByRole("button", { name: "Zona de perigo" }));
  expect(await screen.findByRole("group", { name: "Zona de perigo" })).toBeInTheDocument();
  expect(screen.getByRole("tab", { name: "ctower-agents · Configurações › Zona de perigo" })).toBeInTheDocument();
  // Geral: o cartão do repositório, com Editar.
  await user.click(within(secoes).getByRole("button", { name: "Geral" }));
  const geral = await screen.findByRole("group", { name: "Geral" });
  await user.click(within(geral).getByRole("button", { name: /^Editar/ }));
  expect(within(geral).getByRole("form", { name: "Editar ctower-agents" })).toBeInTheDocument();
});

test("projeto local (hub fora, #17): Sessões pelo ambiente e Configurações apontam para o hub", async () => {
  const api = setupTabs(false);
  render(<App api={api} initial={{ name: "project", env: "wsl", projectId: 1, view: "sessions" }} />);
  const user = userEvent.setup();
  expect(await screen.findByRole("list", { name: "Vivas em WSL" })).toHaveTextContent("#59 Item 59");
  await user.click(pageBtn("Configurações"));
  expect(await screen.findByText(/As configurações do projeto ficam no hub/)).toBeInTheDocument();
  await user.click(pageBtn("Visão geral"));
  expect(await screen.findByText(/Dados locais de WSL/)).toBeInTheDocument();
});

// Visão geral do projeto (#19): saúde, "Buscar issues agora" e gestor do projeto.

const BLOQUEADO = healthSignal({
  kind: "bloqueado",
  severity: "aviso",
  number: 81,
  ref: "deep-ion-ai/ctower-agents#81",
  title: "filha parada",
  value: "3 dias",
  threshold: "2 dias",
  detail: "bloqueado: aguardando o cliente",
});

function setupOverview(hubUp = true) {
  return setupTabs(hubUp)
    .on("env.info", (_: unknown, env: string) =>
      env === "wsl"
        ? info("daemon-wsl", { hub_sync: SENDING, hub_config: { ...LINKED, agent_token: true } })
        : info("daemon-mac", { hub_config: LINKED }),
    )
    .on("sync.now", [{ project_id: 1, ok: true, error: null, issues: 12, prs: 3, status_changes: 1 }])
    .on("session.start_gestor", session({ id: "9e570000-0000-0000-0000-000000000001", work_item_id: 900 }))
    .onHub("hub.health.get", {
      generated_at: NOW,
      // De outro projeto e informativo: ficam fora da Visão geral deste.
      signals: [
        BLOQUEADO,
        healthSignal({ repo: "outro", number: 5, title: "de outro projeto" }),
        healthSignal({ kind: "pausado_longo", severity: "info", number: 82, title: "pausado" }),
      ],
      counts: { critico: 1, aviso: 1, info: 1 },
    });
}

const overviewAt = { name: "project" as const, owner: "deep-ion-ai", repo: "ctower-agents", view: "overview" as const };
const card = async (heading: string) => (await screen.findByRole("heading", { name: heading })).closest("section")!;

test("Visão geral (#19): item bloqueado há mais de 2 dias aparece na saúde do projeto, só os sinais dele", async () => {
  const api = setupOverview();
  render(<App api={api} initial={overviewAt} />);
  const user = userEvent.setup();
  const sinais = await screen.findByRole("list", { name: "Sinais de saúde do projeto" });
  expect(screen.getByRole("heading", { name: "Saúde · 1 sinal" })).toBeInTheDocument();
  expect(sinais).toHaveTextContent("bloqueado");
  expect(sinais).toHaveTextContent("filha parada");
  expect(sinais).toHaveTextContent("3 dias · limiar 2 dias");
  expect(sinais).not.toHaveTextContent("de outro projeto");
  expect(sinais).not.toHaveTextContent("pausado");
  // O número abre o item do mesmo projeto.
  await user.click(within(sinais).getByRole("button", { name: "#81" }));
  expect(await screen.findByRole("button", { name: "Ver issue" })).toBeInTheDocument();
  expect(screen.queryByRole("list", { name: "Sinais de saúde do projeto" })).not.toBeInTheDocument();
});

test("Visão geral (#19): Buscar issues agora roda sync.now só deste projeto no ambiente e mostra o resultado", async () => {
  const api = setupOverview();
  render(<App api={api} initial={overviewAt} />);
  const user = userEvent.setup();
  // Espera a visão do hub (antes dele conectar, a tela é a local).
  await screen.findByRole("list", { name: "Sinais de saúde do projeto" });
  const sync = await card("Buscar issues");
  expect(sync).toHaveTextContent("Só este projeto, pelo ambiente WSL.");
  await user.click(within(sync).getByRole("button", { name: "Buscar issues agora este projeto" }));
  await waitFor(() => expect(api.callsTo("sync.now")).toHaveLength(1));
  expect(api.callsTo("sync.now")[0]).toMatchObject({ env: "wsl", params: { project_id: 1 } });
  expect(await within(sync).findByRole("status")).toHaveTextContent(
    "Sincronizado por WSL: 12 issues, 3 PRs, 1 mudança de status.",
  );

  // Falha do ambiente: o erro, como alerta.
  api.on("sync.now", () => Promise.reject(new Error("sincronização com o GitHub desativada")));
  await user.click(within(sync).getByRole("button", { name: "Buscar issues agora este projeto" }));
  expect(await within(sync).findByRole("alert")).toHaveTextContent("Falhou em WSL: sincronização com o GitHub desativada");
});

test("Visão geral (#19): Gestor deste projeto abre já com o escopo Um projeto neste projeto", async () => {
  const api = setupOverview();
  render(<App api={api} initial={overviewAt} />);
  const user = userEvent.setup();
  await screen.findByRole("list", { name: "Sinais de saúde do projeto" });
  const gestor = await card("Gestor deste projeto");
  await user.click(await within(gestor).findByRole("button", { name: "Gestor deste projeto" }));
  const dialog = within(gestor).getByRole("region", { name: "Iniciar o gestor de projetos" });
  expect(within(dialog).getByRole("radio", { name: "Um projeto" })).toBeChecked();
  expect(within(dialog).getByLabelText("Projeto do escopo")).toHaveValue("deep-ion-ai/ctower-agents");
  expect((within(dialog).getByLabelText("Prompt inicial (template do gestor)") as HTMLTextAreaElement).value).toContain(
    "Foco: deep-ion-ai/ctower-agents.",
  );
  await user.click(within(dialog).getByRole("button", { name: "▶ Iniciar gestor" }));
  await waitFor(() => expect(api.callsTo("session.start_gestor")).toHaveLength(1));
  expect(api.callsTo("session.start_gestor")[0].params).toMatchObject({
    scope: { kind: "project", owner: "deep-ion-ai", repo: "ctower-agents" },
  });
});

test("Visão geral local (hub fora, #19): só o Buscar issues agora, no ambiente do projeto", async () => {
  const api = setupOverview(false);
  render(<App api={api} initial={{ name: "project", env: "wsl", projectId: 1, view: "overview" }} />);
  const user = userEvent.setup();
  await user.click(await screen.findByRole("button", { name: "Buscar issues agora este projeto" }));
  await waitFor(() => expect(api.callsTo("sync.now")).toHaveLength(1));
  expect(api.callsTo("sync.now")[0]).toMatchObject({ env: "wsl", params: { project_id: 1 } });
  expect(screen.queryByRole("heading", { name: /^Saúde/ })).not.toBeInTheDocument();
  expect(screen.queryByRole("heading", { name: "Gestor deste projeto" })).not.toBeInTheDocument();
});
