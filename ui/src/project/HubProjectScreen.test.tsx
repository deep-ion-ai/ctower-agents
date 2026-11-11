import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { App } from "../App";
import { FakeDaemonApi } from "../api/fake";
import type { EnvInfo, IssueRow } from "../protocol/gen";
import type { HubDashboard, HubInfo, HubProject, HubTree, HubTreeNode, HubWorkItem } from "../protocol/hub-gen";
import { HUB_CONFIG_UNLINKED, dashboard, healthSignal, hubAssignment, project, session, sync, workItem, RC_AVAILABLE } from "../test/fixtures";
// Cartão Propostas do gestor (#100).
import * as fx from "../test/fixtures";
import type { HubProposal, HubRanking, ProposalDecided } from "../protocol/hub-gen";
import { resetProjectUi } from "./ui-store";
import { hubItem as webItem, webApi } from "../test/web";
import { DESKTOP_ONLY_NOTE } from "../web/mode";

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
  session_templates: { issue: "", epic: "", project: "", autonomous: "" },
  autonomy: { enabled: false, env_id: null, branch: "", base_branch: "", strategy: "merge" },
  autonomy_report: null,
  wip_limits: { max_sessions: null, max_prs: null },
  approval: { label: "aprovada", required: false },
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
    model: null,
    effort: null,
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

test("limite de trabalho em andamento (#126): iniciar no limite pede confirmação", async () => {
  const limited = { ...HUB_PROJECT, wip_limits: { max_sessions: 2, max_prs: null } };
  const api = setup(true).onHub("hub.projects.list", [
    fx.hubProjectView(limited, [], { sessions: 2, prs: 1, limits: limited.wip_limits }),
  ]);
  render(<App api={api} initial={{ name: "project", owner: "deep-ion-ai", repo: "ctower-agents" }} />);
  await screen.findByText("ui: hub");
  await waitFor(() => expect(api.callsTo("hub.projects.list").length).toBeGreaterThan(0));
  const user = userEvent.setup();
  await user.click(play(62));
  const modal = screen.getByRole("dialog");
  expect(await within(modal).findByText(/limite de trabalho em andamento \(2 de 2 sessões\)/)).toBeInTheDocument();
  await user.click(startButton());
  expect(within(modal).getByRole("alert")).toHaveTextContent("Limite de trabalho em andamento atingido. Iniciar mesmo assim?");
  expect(api.callsTo("session.start")).toHaveLength(0);
  await user.click(within(modal).getByRole("button", { name: "Voltar" }));
  await user.click(startButton());
  await user.click(within(modal).getByRole("button", { name: "Iniciar mesmo assim" }));
  await waitFor(() => expect(api.callsTo("session.start")).toHaveLength(1));
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
    render(<App api={api} initial={{ name: "project", owner: "deep-ion-ai", repo: "ctower-agents", view: "items" }} />);
    await waitFor(() => expect(screen.getByRole("region", { name: /^Coluna backlog/ })).toBeInTheDocument());
    expect(screen.getByRole("button", { name: "Quadro" })).toHaveAttribute("aria-pressed", "true");
  });
});

describe("Só minhas (#151)", () => {
  afterEach(() => window.sessionStorage.clear());
  /** Árvore do hub com o "eu" do sync e a filha #59 atribuída a ele. */
  const meuTree = (): HubTree => ({
    ...TREE,
    sync: sync({ project_id: 7, last_ok_at: NOW, me: "fulano" }),
    roots: TREE.roots.map((r) =>
      r.issue.gh_number !== 64
        ? r
        : {
            ...r,
            children: r.children.map((c) =>
              c.issue.gh_number === 59 ? { ...c, issue: { ...c.issue, assignees: ["fulano"] } } : c,
            ),
          },
    ),
  });

  test("na Árvore e no Quadro do hub, só as minhas", async () => {
    const api = setup(true).onHub("hub.tree.get", meuTree());
    render(<App api={api} initial={{ name: "project", owner: "deep-ion-ai", repo: "ctower-agents" }} />);
    await screen.findByText("sync ambiente→hub");
    const user = userEvent.setup();
    await user.click(screen.getByRole("checkbox", { name: "Só minhas" }));
    expect(within(side()).getByText("sync ambiente→hub")).toBeInTheDocument();
    expect(within(side()).queryByText("env_id e HLC")).not.toBeInTheDocument();
    expect(within(side()).queryByText("Épico: parado")).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Quadro" }));
    const cards = screen.getAllByRole("button", { name: /#\d+/ }).filter((b) => b.classList.contains("card"));
    expect(cards).toHaveLength(1);
    expect(cards[0]).toHaveTextContent("sync ambiente→hub");
  });

  test("sem o 'eu' no hub, o toggle fica desabilitado", async () => {
    render(<App api={setup(true)} initial={{ name: "project", owner: "deep-ion-ai", repo: "ctower-agents" }} />);
    await screen.findByText("sync ambiente→hub");
    expect(screen.getByRole("checkbox", { name: "Só minhas" })).toBeDisabled();
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

test("abas do projeto (#17): sem `view`, Visão geral (#99); cada aba troca o endereço na mesma aba de editor", async () => {
  const api = setupTabs();
  render(<App api={api} initial={{ name: "project", owner: "deep-ion-ai", repo: "ctower-agents" }} />);
  const user = userEvent.setup();
  await screen.findByText("visão do hub");
  // Sem `view` no endereço: a Visão geral (#99).
  expect(pageBtn("Visão geral")).toHaveAttribute("aria-current", "page");
  expect(screen.getByRole("tab", { name: "ctower-agents · Visão geral" })).toHaveAttribute("aria-selected", "true");

  // Itens: a árvore e o resumo de sempre, na mesma aba de editor.
  await user.click(pageBtn("Itens"));
  expect(pageBtn("Itens")).toHaveAttribute("aria-current", "page");
  expect(screen.getByRole("tab", { name: "ctower-agents" })).toHaveAttribute("aria-selected", "true");
  expect(screen.getAllByRole("tab").filter((t) => /ctower-agents/.test(t.textContent ?? ""))).toHaveLength(1);

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
  // Descartar ao lado (#138): a confirmação, e só então o `session.archive`.
  api.on("session.archive", {});
  await user.click(within(ints).getByRole("button", { name: "Descartar: #62 Item 62" }));
  const dlg = await screen.findByRole("alertdialog", { name: "Descartar a sessão?" });
  expect(dlg).toHaveTextContent("#62 Item 62");
  expect(api.callsTo("session.archive")).toHaveLength(0);
  await user.click(within(dlg).getByRole("button", { name: "Descartar" }));
  await waitFor(() => expect(api.callsTo("session.archive")[0].params).toEqual({ session_id: INT62.id }));
  expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument();
  await user.click(within(ints).getByRole("button", { name: "Abrir na Recuperação: #62 Item 62" }));
  expect(await screen.findByRole("tab", { name: /Recuperação/, selected: true })).toBeInTheDocument();

  // De volta ao projeto, que lembra a aba; Árvore volta a Itens.
  await user.click(screen.getByRole("tab", { name: "ctower-agents · Sessões" }));
  await user.click(within(side()).getByRole("button", { name: "Árvore" }));
  expect(pageBtn("Itens")).toHaveAttribute("aria-current", "page");
  expect(screen.getByRole("tab", { name: "ctower-agents" })).toBeInTheDocument();
});

test("Visão geral como aba padrão (#99): Árvore e Quadro levam a Itens; abrir o item não muda a aba dele", async () => {
  const api = setupTabs();
  render(<App api={api} initial={{ name: "project", owner: "deep-ion-ai", repo: "ctower-agents" }} />);
  const user = userEvent.setup();
  await screen.findByText("visão do hub");
  expect(pageBtn("Visão geral")).toHaveAttribute("aria-current", "page");
  // Quadro, da Visão geral: a aba Itens, com o Kanban.
  await user.click(within(side()).getByRole("button", { name: "Quadro" }));
  expect(pageBtn("Itens")).toHaveAttribute("aria-current", "page");
  expect(await screen.findByRole("region", { name: /^Coluna backlog/ })).toBeInTheDocument();
  // De uma aba de item, Quadro volta à aba do projeto em Itens (não à Visão geral).
  await user.click(within(side()).getByRole("button", { name: "Árvore" }));
  await user.click(rowBtn(/^#59 /));
  expect(await screen.findByRole("region", { name: "Detalhe do item" })).toBeInTheDocument();
  await user.click(within(side()).getByRole("button", { name: "Quadro" }));
  expect(pageBtn("Itens")).toHaveAttribute("aria-current", "page");
  expect(screen.getByRole("region", { name: /^Coluna backlog/ })).toBeInTheDocument();
  window.sessionStorage.clear();
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
    "Modo autônomo",
    "Regras do agente",
    "Templates de sessão",
    "Aprovação",
    "Secrets e variáveis",
    "Docs",
    "Limpeza",
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
  await user.click(within(geral).getByRole("button", { name: "Cancelar" }));

  // Limpeza (#22): um clique leva a Manutenção › Limpeza filtrada pelo projeto.
  await user.click(within(secoes).getByRole("button", { name: "Limpeza" }));
  const limpeza = await screen.findByRole("group", { name: "Limpeza" });
  await user.click(within(limpeza).getByRole("button", { name: "Abrir a limpeza do projeto ›" }));
  expect(screen.getByRole("tab", { name: "Manutenção · Limpeza" })).toHaveAttribute("aria-selected", "true");
  expect(screen.getByLabelText("Projeto da limpeza")).toHaveValue("deep-ion-ai/ctower-agents");
  expect(screen.getByLabelText("Ambiente da manutenção")).toHaveValue("wsl");
});

test("Trabalho em andamento (#126): N de M no Geral e o limite salvo inteiro, vazio = sem limite", async () => {
  const limited = { ...HUB_PROJECT, wip_limits: { max_sessions: 3, max_prs: null } };
  const api = setupTabs()
    .onHub("hub.projects.update", null)
    .onHub("hub.projects.list", [fx.hubProjectView(limited, [], { sessions: 1, prs: 2, limits: limited.wip_limits })]);
  render(
    <App api={api} initial={{ name: "project", owner: "deep-ion-ai", repo: "ctower-agents", view: "settings", section: "general" }} />,
  );
  const user = userEvent.setup();
  const geral = await screen.findByRole("group", { name: "Geral" });
  const card = await within(geral).findByRole("region", { name: "Trabalho em andamento" });
  expect(card).toHaveTextContent(/Sessões simultâneas\s*1 de 3 sessões/);
  expect(card).toHaveTextContent(/PRs abertos\s*sem limite/);
  await user.click(within(card).getByRole("button", { name: "Alterar limite" }));
  await user.clear(within(card).getByLabelText("Sessões simultâneas"));
  await user.type(within(card).getByLabelText("PRs abertos"), "4");
  await user.click(within(card).getByRole("button", { name: "Salvar limite" }));
  await waitFor(() => expect(api.callsTo("hub.projects.update")).toHaveLength(1));
  expect(api.callsTo("hub.projects.update")[0].params).toEqual({
    owner: "deep-ion-ai",
    repo: "ctower-agents",
    wip_limits: { max_sessions: null, max_prs: 4 },
  });
});

test("Aprovação (#124): edita o rótulo e a exigência e manda o objeto inteiro", async () => {
  const api = setupTabs()
    .onHub("hub.projects.update", null)
    .onHub("hub.projects.list", [
      {
        project: HUB_PROJECT,
        assignments: [hubAssignment({ project_id: 7, env_id: "daemon-wsl", env_name: "wsl" })],
        locations: [],
        sync: sync({ project_id: 7, last_ok_at: NOW }),
        tracker_status: null,
      },
    ]);
  render(
    <App
      api={api}
      initial={{ name: "project", owner: "deep-ion-ai", repo: "ctower-agents", view: "settings", section: "approval" }}
    />,
  );
  const user = userEvent.setup();
  const card = await screen.findByRole("group", { name: "Aprovação" });
  expect(card).toHaveTextContent(/sem exigir para iniciar sessão/);
  await user.click(within(card).getByRole("button", { name: "Editar aprovação" }));
  const form = within(card).getByRole("form", { name: "Editar rótulo de aprovação de ctower-agents" });
  const input = within(form).getByLabelText("Rótulo de aprovação");
  expect(input).toHaveValue("aprovada");
  await user.clear(input);
  await user.type(input, " pronta ");
  await user.click(within(form).getByLabelText("Exigir para iniciar sessão"));
  await user.click(within(form).getByRole("button", { name: "Salvar aprovação" }));
  await waitFor(() => expect(api.hubCallsTo("hub.projects.update")).toHaveLength(1));
  expect(api.hubCallsTo("hub.projects.update")[0].params).toEqual({
    owner: "deep-ion-ai",
    repo: "ctower-agents",
    approval: { label: "pronta", required: true },
  });
});

test("Templates de sessão (#166): Personalizar parte do global, salva o objeto inteiro e Voltar ao global remove", async () => {
  const api = setupTabs()
    .onHub("hub.projects.update", null)
    .onHub("hub.projects.list", [
      {
        project: { ...HUB_PROJECT, session_templates: { issue: "", epic: "Épico do projeto {{key}}", project: "", autonomous: "" } },
        assignments: [hubAssignment({ project_id: 7, env_id: "daemon-wsl", env_name: "wsl" })],
        locations: [],
        sync: sync({ project_id: 7, last_ok_at: NOW }),
        tracker_status: null,
      },
    ]);
  render(
    <App
      api={api}
      initial={{ name: "project", owner: "deep-ion-ai", repo: "ctower-agents", view: "settings", section: "templates" }}
    />,
  );
  const user = userEvent.setup();
  const card = await screen.findByRole("group", { name: "Templates de sessão" });
  expect(card).toHaveTextContent(/Issue e sub-issue\s*herdando o template global/);
  expect(card).toHaveTextContent(/Épico\s*template do projeto/);
  expect(within(card).queryByRole("button", { name: "Voltar ao global Issue e sub-issue" })).toBeNull();

  // Personalizar: o editor abre com o template global de issue.
  await user.click(within(card).getByRole("button", { name: "Personalizar Issue e sub-issue" }));
  const area = within(card).getByLabelText("Template");
  expect((area as HTMLTextAreaElement).value).toContain("Trabalhe na {{kind}} #{{number}}");
  await user.clear(area);
  fireEvent.change(area, { target: { value: "Só a issue {{key}}" } });
  await user.click(within(card).getByRole("button", { name: "Salvar template" }));
  await waitFor(() => expect(api.hubCallsTo("hub.projects.update")).toHaveLength(1));
  expect(api.hubCallsTo("hub.projects.update")[0].params).toEqual({
    owner: "deep-ion-ai",
    repo: "ctower-agents",
    session_templates: { issue: "Só a issue {{key}}", epic: "Épico do projeto {{key}}", project: "", autonomous: "" },
  });

  // Voltar ao global: a chave vazia, o resto igual.
  await user.click(within(card).getByRole("button", { name: "Voltar ao global Épico" }));
  await waitFor(() => expect(api.hubCallsTo("hub.projects.update")).toHaveLength(2));
  expect(api.hubCallsTo("hub.projects.update")[1].params).toMatchObject({
    session_templates: { issue: "", epic: "", project: "", autonomous: "" },
  });
});

test("Templates de sessão (#166): hub anterior à 1.13 mostra o aviso, sem as ações", async () => {
  const { session_templates: _omit, ...antigo } = HUB_PROJECT;
  const api = setupTabs().onHub("hub.projects.list", [
    {
      project: antigo as HubProject,
      assignments: [],
      locations: [],
      sync: null,
      tracker_status: null,
    },
  ]);
  render(
    <App
      api={api}
      initial={{ name: "project", owner: "deep-ion-ai", repo: "ctower-agents", view: "settings", section: "templates" }}
    />,
  );
  const card = await screen.findByRole("group", { name: "Templates de sessão" });
  expect(card).toHaveTextContent(/anterior ao protocolo 1\.13/);
  expect(within(card).queryByRole("button")).toBeNull();
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

// #90: "Sincronizar este projeto" de vários lugares e a falha de sync por projeto.

/** O sync do ambiente falhando há `failures` rodadas (dashboard do wsl). */
function setupFailing(failures: number, hubUp = true) {
  const failing = sync({
    project_id: 1,
    last_ok_at: NOW - 3 * 3_600_000,
    last_attempt_at: NOW - 60_000,
    last_error: "GitHub: HTTP 410",
    failures,
    failing_since: NOW - 2 * 3_600_000,
  });
  return setupOverview(hubUp).on("dashboard.get", (_: unknown, env: string) =>
    env === "wsl"
      ? dashboard({ live_sessions: [LIVE59], sync: [failing] }, [WI59])
      : dashboard({ projects: [] }),
  );
}

test("cabeçalho (#90): Sincronizar este projeto roda sync.now só deste projeto e mostra o resultado", async () => {
  const api = setupOverview();
  render(<App api={api} initial={{ name: "project", owner: "deep-ion-ai", repo: "ctower-agents", view: "items" }} />);
  const user = userEvent.setup();
  await screen.findByText("visão do hub");
  const head = document.querySelector<HTMLElement>(".project-head")!;
  // O hub conta o último sucesso.
  expect(head).toHaveTextContent("Último sync: agora");
  await user.click(within(head).getByRole("button", { name: "Sincronizar este projeto" }));
  await waitFor(() => expect(api.callsTo("sync.now")).toHaveLength(1));
  expect(api.callsTo("sync.now")[0]).toMatchObject({ env: "wsl", params: { project_id: 1 } });
  // Na aba Itens, o resultado fica no próprio cabeçalho (o mesmo texto do cartão).
  expect(await within(head).findByRole("status")).toHaveTextContent("Sincronizado por WSL: 12 issues, 3 PRs, 1 mudança de status.");
});

test("falha repetida (#90): cartão e cabeçalho mostram desde quando e o erro, com o tentar de novo", async () => {
  const api = setupFailing(4);
  render(<App api={api} initial={overviewAt} />);
  const user = userEvent.setup();
  await screen.findByRole("list", { name: "Sinais de saúde do projeto" });
  const sync = await card("Buscar issues");
  // O pior estado entre hub (em dia) e ambiente (falhando).
  await waitFor(() => expect(sync).toHaveTextContent(/Sync falhando desde .* \(4 rodadas\)/));
  expect(sync).toHaveTextContent("GitHub: HTTP 410");
  const head = document.querySelector<HTMLElement>(".project-head")!;
  expect(head).toHaveTextContent(/Sync falhando desde/);
  expect(head).toHaveTextContent("GitHub: HTTP 410");
  await user.click(within(sync).getByRole("button", { name: "Tentar de novo este projeto" }));
  await waitFor(() => expect(api.callsTo("sync.now")).toHaveLength(1));
  expect(api.callsTo("sync.now")[0]).toMatchObject({ env: "wsl", params: { project_id: 1 } });
  // Na Visão geral o resultado aparece só no cartão (sem repetir no cabeçalho).
  expect(await within(sync).findByRole("status")).toHaveTextContent("Sincronizado por WSL");
  expect(within(head).queryByRole("status")).toBeNull();
});

test("compatibilidade (#90): ambiente sem o campo de falhas não mostra falha nem aviso", async () => {
  const antigo = sync({ project_id: 1, last_ok_at: NOW - 60_000, last_error: "GitHub: HTTP 410" });
  delete (antigo as Partial<typeof antigo>).failures;
  const api = setupOverview(false).on("dashboard.get", (_: unknown, env: string) =>
    env === "wsl" ? dashboard({ sync: [antigo] }, [WI59]) : dashboard({ projects: [] }),
  );
  render(<App api={api} initial={{ name: "project", env: "wsl", projectId: 1, view: "overview" }} />);
  const sync1 = await card("Buscar issues");
  await waitFor(() => expect(sync1).toHaveTextContent("Último sync: há 1 min"));
  expect(screen.queryByText(/Sync falhando/)).toBeNull();
});

test("lista de Projetos (#90): menu com Sincronizar este projeto, progresso e resultado na linha; selo de sync falhando", async () => {
  const api = setupFailing(3, false);
  render(<App api={api} initial={{ name: "project" }} />);
  const user = userEvent.setup();
  const linha = (await screen.findByRole("button", { name: "ctower-agents" })).closest("li")!;
  expect(within(linha).getByText("sync falhando")).toHaveAttribute("title", expect.stringContaining("GitHub: HTTP 410"));
  await user.click(within(linha).getByRole("button", { name: "Mais ações de ctower-agents" }));
  await user.click(screen.getByRole("menuitem", { name: "Sincronizar este projeto" }));
  await waitFor(() => expect(api.callsTo("sync.now")).toHaveLength(1));
  expect(api.callsTo("sync.now")[0]).toMatchObject({ env: "wsl", params: { project_id: 1 } });
  expect(await within(linha).findByRole("status")).toHaveTextContent("Sincronizado por WSL: 12 issues, 3 PRs, 1 mudança de status.");
});

test("lista de Projetos (#90): abaixo de 3 falhas seguidas, sem selo", async () => {
  const api = setupFailing(2, false);
  render(<App api={api} initial={{ name: "project" }} />);
  const linha = (await screen.findByRole("button", { name: "ctower-agents" })).closest("li")!;
  expect(within(linha).queryByText("sync falhando")).toBeNull();
});

test("faixa Precisa de você (#90): sync falhando há 3 ciclos, com o link para o projeto; some no sucesso", async () => {
  const api = setupFailing(3, false);
  render(<App api={api} initial={{ name: "control" }} />);
  const faixa = await screen.findByRole("region", { name: /Precisa de você/ });
  expect(faixa).toHaveTextContent("ctower-agents: sync falhando há 3 ciclos");
  expect(faixa).toHaveTextContent("GitHub: HTTP 410");
  // A rodada seguinte dá certo: o push do ambiente traz o dashboard novo e o aviso some.
  api.on("dashboard.get", (_: unknown, env: string) =>
    env === "wsl"
      ? dashboard({ sync: [sync({ project_id: 1, last_ok_at: NOW, failures: 0 })] }, [WI59])
      : dashboard({ projects: [] }),
  );
  act(() => api.push("wsl", "sync_status", { project_id: 1, ok: true, last_ok_at: NOW, error: null }));
  await waitFor(() => expect(screen.queryByText(/sync falhando há/)).toBeNull());
});

test("paleta (#90): Sincronizar este projeto na aba do projeto e Sincronizar projeto… escolhendo o projeto", async () => {
  const api = setupOverview(false);
  render(<App api={api} initial={{ name: "project", env: "wsl", projectId: 1, view: "items" }} />);
  const user = userEvent.setup();
  await screen.findByRole("button", { name: "Sincronizar este projeto" });
  const paleta = async () => {
    await user.click(screen.getByRole("button", { name: "Abrir paleta de comandos" }));
    return within(screen.getByRole("dialog", { name: "Paleta de comandos" })).getByRole("combobox");
  };
  const busca = await paleta();
  await user.type(busca, "Sincronizar este");
  await user.click(await screen.findByRole("option", { name: /Sincronizar este projeto/ }));
  await waitFor(() => expect(api.callsTo("sync.now")).toHaveLength(1));
  expect(api.callsTo("sync.now")[0]).toMatchObject({ env: "wsl", params: { project_id: 1 } });
  // O resultado aparece no cabeçalho do projeto (aba Itens).
  const head = document.querySelector<HTMLElement>(".project-head")!;
  expect(await within(head).findByRole("status")).toHaveTextContent("Sincronizado por WSL");

  // "Sincronizar projeto…" troca a busca e lista os projetos; o escolhido abre a Visão geral.
  const busca2 = await paleta();
  await user.type(busca2, "Sincronizar projeto");
  // Pelo texto: o nome acessível separa os trechos destacados da busca.
  const opt = (text: string) => screen.getAllByRole("option").find((o) => o.textContent?.startsWith(text))!;
  await user.click(opt("Sincronizar projeto…"));
  expect(busca2).toHaveValue("Sincronizar projeto: ");
  await waitFor(() => expect(opt("Sincronizar projeto: ctower-agents")).toBeDefined());
  await user.click(opt("Sincronizar projeto: ctower-agents"));
  await waitFor(() => expect(api.callsTo("sync.now")).toHaveLength(2));
  expect(await card("Buscar issues")).toBeInTheDocument();
});

// Cartão Propostas do gestor na Visão geral do projeto (#100): só as
// pendentes do projeto aberto, na ordem do ranking, com aprovar e rejeitar.
describe("Visão geral (#100): Propostas do gestor", () => {
  const ACME = fx.hubProject({ id: 30, gh_owner: "acme", gh_repo: "acme-web", name: "acme-web" });
  const acmeAt = { name: "project" as const, owner: "acme", repo: "acme-web", view: "overview" as const };
  const LONG =
    "O #15 trava o cadastro de clientes novos desde a última versão; sem ele, o time de vendas não fecha contrato " +
    "e o suporte recebe chamados repetidos. Detalhe: só acontece com o SSO ligado.";
  const acmeProposal = (p: Partial<HubProposal>) =>
    fx.hubProposal({ owner: "acme", repo: "acme-web", ref: `acme/acme-web#${p.number}`, ...p });
  // #15 é a mais velha, mas vai para P1; #12 vai para P0 (antes do #15) e vem
  // primeiro na ordem do ranking. A de status vem depois das de prioridade.
  const P15 = acmeProposal({
    id: 1,
    number: 15,
    title: "Cadastro trava",
    created_at: NOW - 3000,
    current: { priority: null },
    proposed: { priority: "P1" },
    justification: LONG,
  });
  const P12 = acmeProposal({
    id: 2,
    number: 12,
    title: "Login no Safari",
    created_at: NOW - 2000,
    current: { priority: "P2" },
    proposed: { priority: "P0", before: 15 },
    justification: "quebra o login de Fulano",
  });
  const S12 = acmeProposal({
    id: 3,
    kind: "status",
    number: 12,
    title: "Login no Safari",
    created_at: NOW - 1000,
    current: { status: "backlog" },
    proposed: { status: "pausado" },
    justification: "aguarda o fornecedor",
  });
  // De outro projeto: o hub filtra por owner/repo e ela não aparece.
  const OTHER = fx.hubProposal({ id: 9, owner: "acme", repo: "acme-api", number: 7, ref: "acme/acme-api#7", title: "de outro projeto" });
  const RANK: HubRanking = {
    truncated: false,
    items: [12, 15].map((number, i) => ({
      position: i + 1,
      owner: "acme",
      repo: "acme-web",
      number,
      ref: `acme/acme-web#${number}`,
      title: `Issue ${number}`,
      kind: "issue",
      status: "backlog",
      priority: number === 12 ? "P2" : null,
      rank: number === 12 ? 1 : null,
    })),
  };

  function withProposals(api: FakeDaemonApi, all: () => HubProposal[]): FakeDaemonApi {
    return api
      .onHub("hub.tree.get", { project: ACME, sync: sync({ project_id: 30, last_ok_at: NOW }), roots: [], untracked: [] })
      .onHub("hub.dashboard.get", { ...HUB_DASH, projects: [{ project: ACME, work_items: [] }] })
      .onHub("hub.projects.list", [fx.hubProjectView(ACME)])
      .onHub("hub.health.get", fx.NO_HEALTH)
      .onHub("hub.ranking.get", RANK)
      .onHub("hub.proposals.list", (p: { owner?: string; repo?: string }) => ({
        proposals: all().filter((x) => x.state === "pendente" && (!p.owner || (x.owner === p.owner && x.repo === p.repo))),
        truncated: false,
      }));
  }

  /** Desktop com o hub; `pending` é a lista viva do hub fictício (decidir tira dela). */
  function setupProposals(initial: HubProposal[]) {
    let pending = [...initial];
    const api = withProposals(setup(true), () => pending).onHub(
      "hub.proposals.decide",
      (p: { id: number; approve: boolean }) => {
        const x = pending.find((y) => y.id === p.id)!;
        pending = pending.filter((y) => y.id !== p.id);
        return { ...x, state: p.approve ? "aprovada" : "rejeitada" };
      },
    );
    return { api, drop: (id: number) => (pending = pending.filter((y) => y.id !== id)) };
  }

  const rows = (c: HTMLElement) => within(c).queryAllByRole("listitem").map((li) => li.getAttribute("aria-label"));

  it("lista só as pendentes do projeto, na ordem do ranking, com o atual → proposto e a justificativa resumida", async () => {
    const { api } = setupProposals([P15, P12, S12, OTHER]);
    render(<App api={api} initial={acmeAt} />);
    const c = await card("Propostas do gestor · 3");
    expect(rows(c)).toEqual(["Proposta #2", "Proposta #1", "Proposta #3"]);
    expect(c).not.toHaveTextContent("de outro projeto");
    expect(api.hubCallsTo("hub.proposals.list")[0].params).toMatchObject({ state: "pendente", owner: "acme", repo: "acme-web" });
    // Só as pendentes: o cartão não lê o histórico.
    expect(api.hubCallsTo("hub.proposals.list").every((x) => (x.params as { state: string }).state === "pendente")).toBe(true);
    const p12 = within(c).getByRole("listitem", { name: "Proposta #2" });
    expect(p12).toHaveTextContent("#12 Login no Safari P2 → P0 · antes de #15");
    expect(p12).toHaveTextContent("quebra o login de Fulano");
    expect(within(c).getByRole("listitem", { name: "Proposta #3" })).toHaveTextContent("backlog → pausado");
    // Justificativa longa: resumida, com o resto sob demanda.
    const p15 = within(c).getByRole("listitem", { name: "Proposta #1" });
    expect(p15).toHaveTextContent("#15 Cadastro trava não triado → P1");
    expect(p15.querySelector("summary")!.textContent!.endsWith("…")).toBe(true);
    expect(p15.querySelector("details")).toHaveTextContent("só acontece com o SSO ligado");
  });

  it("Aprovar e Rejeitar chamam hub.proposals.decide e a proposta sai do cartão, sem trocar de tela", async () => {
    const { api } = setupProposals([P15, P12]);
    render(<App api={api} initial={acmeAt} />);
    const user = userEvent.setup();
    const c = await card("Propostas do gestor · 2");
    await user.click(within(c).getByRole("button", { name: "Aprovar a proposta #2" }));
    await waitFor(() => expect(api.hubCallsTo("hub.proposals.decide")).toHaveLength(1));
    expect(api.hubCallsTo("hub.proposals.decide")[0].params).toEqual({ id: 2, approve: true });
    expect(await within(c).findByRole("status")).toHaveTextContent("Proposta #2 aprovada.");
    await waitFor(() => expect(rows(c)).toEqual(["Proposta #1"]));

    await user.click(within(c).getByRole("button", { name: "Rejeitar a proposta #1" }));
    await waitFor(() => expect(api.hubCallsTo("hub.proposals.decide")).toHaveLength(2));
    expect(api.hubCallsTo("hub.proposals.decide")[1].params).toEqual({ id: 1, approve: false });
    expect(await within(c).findByText("Nenhuma proposta pendente neste projeto.")).toBeInTheDocument();
    // Continua na Visão geral do projeto.
    expect(screen.getByRole("tab", { name: "acme-web · Visão geral" })).toHaveAttribute("aria-selected", "true");
  });

  it("proposta decidida em outro lugar some do cartão pelo push hub.proposals.changed", async () => {
    const { api, drop } = setupProposals([P15, P12]);
    render(<App api={api} initial={acmeAt} />);
    const c = await card("Propostas do gestor · 2");
    drop(2);
    act(() => api.hubPush("hub.proposals.changed"));
    await waitFor(() => expect(rows(c)).toEqual(["Proposta #1"]));
  });

  it("vazio e erros: sem pendentes, o aviso; falha na leitura ou na decisão, o erro no cartão", async () => {
    render(<App api={setupProposals([]).api} initial={acmeAt} />);
    const c = await card("Propostas do gestor · 0");
    expect(await within(c).findByText("Nenhuma proposta pendente neste projeto.")).toBeInTheDocument();
    cleanup();

    const failing = setupProposals([P12]).api.onHub("hub.proposals.list", () =>
      Promise.reject(new Error("hub sem o ledger de propostas")),
    );
    render(<App api={failing} initial={acmeAt} />);
    const f = await card("Propostas do gestor · 0");
    expect(await within(f).findByRole("alert")).toHaveTextContent("hub sem o ledger de propostas");
    expect(within(f).queryByText("Nenhuma proposta pendente neste projeto.")).not.toBeInTheDocument();
    cleanup();

    const refused = setupProposals([P12]).api.onHub("hub.proposals.decide", () =>
      Promise.reject(new Error("proposta já decidida")),
    );
    render(<App api={refused} initial={acmeAt} />);
    const user = userEvent.setup();
    const r = await card("Propostas do gestor · 1");
    await user.click(within(r).getByRole("button", { name: "Aprovar a proposta #2" }));
    expect(await within(r).findByRole("alert")).toHaveTextContent("proposta já decidida");
    expect(rows(r)).toEqual(["Proposta #2"]);
  });

  it("na web (somente leitura), Aprovar e Rejeitar ficam desabilitados com a explicação padrão", async () => {
    const api = withProposals(webApi(), () => [P12]);
    render(<App api={api} initial={acmeAt} />);
    const c = await card("Propostas do gestor · 1");
    for (const name of ["Aprovar a proposta #2", "Rejeitar a proposta #2"]) {
      const b = within(c).getByRole("button", { name });
      expect(b).toBeDisabled();
      expect(b).toHaveAccessibleDescription(DESKTOP_ONLY_NOTE);
    }
    expect(api.hubCallsTo("hub.proposals.decide")).toHaveLength(0);
  });
});

// Propostas do gestor na Visão geral (#139): decisão em lote e largura ajustável.
describe("Visão geral (#139): propostas em lote e largura do cartão", () => {
  const ACME = fx.hubProject({ id: 30, gh_owner: "acme", gh_repo: "acme-web", name: "acme-web" });
  const acmeAt = { name: "project" as const, owner: "acme", repo: "acme-web", view: "overview" as const };
  const prop = (id: number, number: number, title: string) =>
    fx.hubProposal({
      id,
      owner: "acme",
      repo: "acme-web",
      number,
      ref: `acme/acme-web#${number}`,
      title,
      created_at: NOW - 1000 * id,
      current: { priority: null },
      proposed: { priority: "P2" },
      justification: `justificativa de ${title}`,
    });
  const A = prop(1, 12, "Login no Safari");
  const B = prop(2, 15, "Cadastro trava");
  const C = prop(3, 20, "Relatório lento");

  /**
   * Hub fictício: `pending` é a lista viva; `obsolete` são os ids que, na
   * decisão, viram obsoletos (o valor atual mudou) em vez de aplicados.
   */
  function setupBatch(initial: HubProposal[], { obsolete = [] as number[], web = false } = {}) {
    let pending = [...initial];
    const decideOne = (id: number, approve: boolean): ProposalDecided => {
      const x = pending.find((y) => y.id === id);
      if (!x) return { id, proposal: null, error: `proposta #${id} não existe` };
      pending = pending.filter((y) => y.id !== id);
      return obsolete.includes(id)
        ? { id, proposal: { ...x, state: "obsoleta", decision_note: "valor atual mudou: prioridade P1" }, error: null }
        : { id, proposal: { ...x, state: approve ? "aprovada" : "rejeitada" }, error: null };
    };
    const api = (web ? webApi() : setup(true))
      .onHub("hub.tree.get", { project: ACME, sync: sync({ project_id: 30, last_ok_at: NOW }), roots: [], untracked: [] })
      .onHub("hub.dashboard.get", { ...HUB_DASH, projects: [{ project: ACME, work_items: [] }] })
      .onHub("hub.projects.list", [fx.hubProjectView(ACME)])
      .onHub("hub.health.get", fx.NO_HEALTH)
      .onHub("hub.ranking.get", { truncated: false, items: [] })
      .onHub("hub.proposals.list", () => ({ proposals: pending, truncated: false }))
      .onHub("hub.proposals.decide", (p: { id?: number; ids?: number[]; approve: boolean }) =>
        p.ids ? p.ids.map((id) => decideOne(id, p.approve)) : decideOne(p.id!, p.approve).proposal,
      );
    return { api, drop: (id: number) => (pending = pending.filter((y) => y.id !== id)) };
  }

  it("Marcar todas e aprovar, com confirmação: uma chamada com todos os ids e o resultado no cartão", async () => {
    const { api } = setupBatch([A, B, C]);
    render(<App api={api} initial={acmeAt} />);
    const user = userEvent.setup();
    const c = await card("Propostas do gestor · 3");
    for (const id of [1, 2, 3]) expect(within(c).getByRole("checkbox", { name: `Marcar a proposta #${id}` })).not.toBeChecked();
    expect(within(c).getByRole("button", { name: "Aprovar marcadas (0)" })).toBeDisabled();

    await user.click(within(c).getByRole("button", { name: "Marcar todas" }));
    expect(within(c).getByRole("button", { name: "Desmarcar todas" })).toBeInTheDocument();
    await user.type(within(c).getByLabelText(/Nota/), "trimestre que vem");
    await user.click(within(c).getByRole("button", { name: "Aprovar marcadas (3)" }));
    // Mais de uma: pede confirmação antes de chamar o hub.
    const ask = within(c).getByRole("alertdialog", { name: "Aprovar as marcadas" });
    expect(ask).toHaveTextContent("Aprovar 3 propostas com a nota “trimestre que vem”?");
    expect(api.hubCallsTo("hub.proposals.decide")).toHaveLength(0);
    await user.click(within(ask).getByRole("button", { name: "Aprovar 3" }));

    await waitFor(() => expect(api.hubCallsTo("hub.proposals.decide")).toHaveLength(1));
    const call = api.hubCallsTo("hub.proposals.decide")[0].params as { ids: number[]; approve: boolean; note: string };
    expect(call).toEqual({ ids: [1, 2, 3], approve: true, note: "trimestre que vem" });
    const ok = await within(c).findByRole("status");
    expect(ok).toHaveTextContent("3 proposta(s) aprovada(s).");
    expect(ok).toHaveTextContent("Aplicadas: #1, #2, #3");
    expect(await within(c).findByText("Nenhuma proposta pendente neste projeto.")).toBeInTheDocument();
  });

  it("Cancelar a confirmação não decide nada; uma só marcada vai direto, sem confirmação", async () => {
    const { api } = setupBatch([A, B]);
    render(<App api={api} initial={acmeAt} />);
    const user = userEvent.setup();
    const c = await card("Propostas do gestor · 2");
    await user.click(within(c).getByRole("button", { name: "Marcar todas" }));
    await user.click(within(c).getByRole("button", { name: "Rejeitar marcadas (2)" }));
    const ask = within(c).getByRole("alertdialog", { name: "Rejeitar as marcadas" });
    await user.click(within(ask).getByRole("button", { name: "Cancelar" }));
    expect(within(c).queryByRole("alertdialog")).not.toBeInTheDocument();
    expect(api.hubCallsTo("hub.proposals.decide")).toHaveLength(0);

    await user.click(within(c).getByRole("checkbox", { name: "Marcar a proposta #1" }));
    await user.click(within(c).getByRole("button", { name: "Rejeitar marcadas (1)" }));
    expect(within(c).queryByRole("alertdialog")).not.toBeInTheDocument();
    await waitFor(() => expect(api.hubCallsTo("hub.proposals.decide")).toHaveLength(1));
    expect(api.hubCallsTo("hub.proposals.decide")[0].params).toEqual({ id: 2, approve: false });
  });

  it("obsoleta entre a leitura e a decisão: aparece como recusada com o motivo, e as outras são aplicadas", async () => {
    const { api } = setupBatch([A, B, C], { obsolete: [2] });
    render(<App api={api} initial={acmeAt} />);
    const user = userEvent.setup();
    const c = await card("Propostas do gestor · 3");
    await user.click(within(c).getByRole("button", { name: "Marcar todas" }));
    await user.click(within(c).getByRole("button", { name: "Aprovar marcadas (3)" }));
    await user.click(within(c).getByRole("button", { name: "Aprovar 3" }));
    const alert = await within(c).findByRole("alert");
    expect(alert).toHaveTextContent("2 proposta(s) aprovada(s); 1 não (obsoleta ou já decidida).");
    expect(alert).toHaveTextContent("Aplicadas: #1, #3");
    const refused = within(alert).getByRole("list", { name: "Propostas recusadas" });
    expect(refused).toHaveTextContent("Proposta #2 (acme/acme-web#15): obsoleta: valor atual mudou: prioridade P1");
  });

  it("push hub.proposals.changed: a que saiu da lista deixa de estar marcada e a confirmação aberta fecha", async () => {
    const { api, drop } = setupBatch([A, B, C]);
    render(<App api={api} initial={acmeAt} />);
    const user = userEvent.setup();
    const c = await card("Propostas do gestor · 3");
    await user.click(within(c).getByRole("checkbox", { name: "Marcar a proposta #1" }));
    await user.click(within(c).getByRole("checkbox", { name: "Marcar a proposta #2" }));
    await user.click(within(c).getByRole("button", { name: "Aprovar marcadas (2)" }));
    expect(within(c).getByRole("alertdialog")).toBeInTheDocument();

    drop(2);
    act(() => api.hubPush("hub.proposals.changed"));
    await waitFor(() => expect(within(c).queryByRole("listitem", { name: "Proposta #2" })).not.toBeInTheDocument());
    expect(within(c).queryByRole("alertdialog")).not.toBeInTheDocument();
    expect(within(c).getByRole("button", { name: "Aprovar marcadas (1)" })).toBeInTheDocument();
    await user.click(within(c).getByRole("button", { name: "Aprovar marcadas (1)" }));
    await waitFor(() => expect(api.hubCallsTo("hub.proposals.decide")).toHaveLength(1));
    expect(api.hubCallsTo("hub.proposals.decide")[0].params).toEqual({ id: 1, approve: true });
  });

  it("largura: larga por padrão, com rótulo; a escolha fica por cartão nesta máquina, depois de reabrir o app", async () => {
    const { api } = setupBatch([A]);
    const { unmount } = render(<App api={api} initial={acmeAt} />);
    const user = userEvent.setup();
    const c = await card("Propostas do gestor · 1");
    const group = within(c).getByRole("group", { name: "Largura do cartão Propostas do gestor" });
    const opt = (g: HTMLElement, name: string) => within(g).getByRole("button", { name });
    expect(within(group).getAllByRole("button").map((b) => b.getAttribute("aria-label"))).toEqual([
      "Normal",
      "Larga",
      "Linha inteira",
    ]);
    expect(opt(group, "Larga")).toHaveAttribute("aria-pressed", "true");
    expect(opt(group, "Normal")).toHaveAttribute("aria-pressed", "false");
    expect(c).toHaveAttribute("data-width", "wide");
    // O de Revisões tem o próprio controle, normal por padrão.
    const rv = await card("Revisões · 0");
    const rvGroup = within(rv).getByRole("group", { name: "Largura do cartão Revisões" });
    expect(opt(rvGroup, "Normal")).toHaveAttribute("aria-pressed", "true");

    await user.click(opt(group, "Linha inteira"));
    expect(c).toHaveAttribute("data-width", "full");
    expect(opt(group, "Linha inteira")).toHaveAttribute("aria-pressed", "true");
    expect(rv).toHaveAttribute("data-width", "normal");

    // Reabrir o app: só o que ficou no armazenamento da máquina.
    unmount();
    resetProjectUi();
    render(<App api={setupBatch([A]).api} initial={acmeAt} />);
    const again = await card("Propostas do gestor · 1");
    expect(again).toHaveAttribute("data-width", "full");
    const g2 = within(again).getByRole("group", { name: "Largura do cartão Propostas do gestor" });
    expect(opt(g2, "Linha inteira")).toHaveAttribute("aria-pressed", "true");
  });

  it("na web, as caixas e os botões do lote ficam desabilitados com a explicação padrão", async () => {
    const { api } = setupBatch([A, B], { web: true });
    render(<App api={api} initial={acmeAt} />);
    const c = await card("Propostas do gestor · 2");
    for (const el of [
      within(c).getByRole("checkbox", { name: "Marcar a proposta #1" }),
      within(c).getByRole("button", { name: "Marcar todas" }),
      within(c).getByRole("button", { name: "Aprovar marcadas (0)" }),
      within(c).getByRole("button", { name: "Rejeitar marcadas (0)" }),
    ]) {
      expect(el).toBeDisabled();
      expect(el).toHaveAccessibleDescription(DESKTOP_ONLY_NOTE);
    }
    expect(within(c).queryByLabelText(/Nota/)).not.toBeInTheDocument();
    expect(api.hubCallsTo("hub.proposals.decide")).toHaveLength(0);
  });
});

// Cartão Revisões na Visão geral do projeto (#101): os itens deste projeto
// em revisão ou com decisões pendentes, com "Marcar como revisada".
describe("Visão geral (#101): Revisões", () => {
  const ACME = fx.hubProject({ id: 30, gh_owner: "acme", gh_repo: "acme-web", name: "acme-web" });
  const ACME_LOCAL = project({ id: 3, gh_owner: "acme", gh_repo: "acme-web", name: "acme-web" });
  const OUTRO = project({ id: 4, gh_owner: "acme", gh_repo: "acme-api", name: "acme-api" });
  const acmeAt = { name: "project" as const, owner: "acme", repo: "acme-web", view: "overview" as const };
  // #12 em revisão com 2 pendentes; #15 em execução com 1; #20 sem nada (fica de fora).
  const W12 = workItem({ id: 312, project_id: 3, gh_number: 12, status: "em_revisao", title: "Login no Safari" });
  const W15 = workItem({ id: 315, project_id: 3, gh_number: 15, title: "Cadastro trava" });
  const W20 = workItem({ id: 320, project_id: 3, gh_number: 20, title: "Sem decisões" });
  // De outro projeto: não aparece.
  const W7 = workItem({ id: 407, project_id: 4, gh_number: 7, status: "em_revisao", title: "de outro projeto", pending_decisions: 3 });

  const dec = (id: number, wi: number) => ({
    id,
    uid: `acme-${id}`,
    work_item_id: wi,
    session_id: null,
    origin: "autonoma" as const,
    title: `Decisão ${id} de Fulano`,
    context: "c",
    decision: "d",
    consequences: null,
    reviewed_at: null,
    reviewed_in_pr: null,
    created_at: NOW,
  });
  const pr = (number: number, wi: number) => ({
    number,
    work_item_id: wi,
    head: `feat/${wi}`,
    base: "develop",
    state: "open" as const,
    url: `https://github.com/acme/acme-web/pull/${number}`,
    updated_at: 0,
    head_sha: null,
  });

  function hubSide(api: FakeDaemonApi) {
    return api
      .onHub("hub.tree.get", { project: ACME, sync: sync({ project_id: 30, last_ok_at: NOW }), roots: [], untracked: [] })
      .onHub("hub.dashboard.get", { ...HUB_DASH, projects: [{ project: ACME, work_items: [] }] })
      .onHub("hub.projects.list", [fx.hubProjectView(ACME)])
      .onHub("hub.health.get", fx.NO_HEALTH)
      .onHub("hub.proposals.list", fx.NO_PROPOSALS);
  }

  /** Ambiente com o acme-web: `pending` são as decisões pendentes por work item (marcar esvazia). */
  function envSide(api: FakeDaemonApi, items = [W12, W15, W20]) {
    const pending: Record<number, ReturnType<typeof dec>[]> = { 312: [dec(1, 312), dec(2, 312)], 315: [dec(3, 315)] };
    const prs: Record<number, ReturnType<typeof pr>[]> = { 312: [pr(40, 312)] };
    const byId = (id: number) => [...items, W7].find((w) => w.id === id)!;
    return api
      .on("dashboard.get", (_: unknown, env: string) =>
        env === "mac"
          ? dashboard({ projects: [] })
          : dashboard({
              projects: [
                { project: ACME_LOCAL, work_items: items.map((w) => ({ ...w, pending_decisions: pending[w.id]?.length ?? 0 })) },
                { project: OUTRO, work_items: [W7] },
              ],
            }),
      )
      .on("review.get", (p: { work_item_id: number }) => ({
        work_item: byId(p.work_item_id),
        prs: prs[p.work_item_id] ?? [],
        decisions: pending[p.work_item_id] ?? [],
      }))
      .on("review.mark_reviewed", (p: { work_item_id: number }) => {
        const n = pending[p.work_item_id]?.length ?? 0;
        pending[p.work_item_id] = [];
        return { marked: n };
      });
  }

  const rows = (c: HTMLElement) => within(c).queryAllByRole("listitem").map((li) => li.getAttribute("aria-label"));

  it("lista só os itens em revisão deste projeto, com as pendentes e o link para a tela de Revisão", async () => {
    const api = envSide(hubSide(setup(true)));
    render(<App api={api} initial={acmeAt} />);
    const c = await card("Revisões · 2");
    expect(rows(c)).toEqual(["Revisão de #12 Login no Safari", "Revisão de #15 Cadastro trava"]);
    expect(c).not.toHaveTextContent("de outro projeto");
    expect(c).not.toHaveTextContent("Sem decisões");
    const r12 = within(c).getByRole("listitem", { name: "Revisão de #12 Login no Safari" });
    await waitFor(() => expect(r12).toHaveTextContent("2 decisões pendentes · PR #40 aberto"));
    await waitFor(() =>
      expect(within(c).getByRole("listitem", { name: "Revisão de #15 Cadastro trava" })).toHaveTextContent(
        "1 decisão pendente · sem PR aberto",
      ),
    );
    // O título abre a tela completa de Revisão do item.
    const user = userEvent.setup();
    await user.click(within(r12).getByRole("button", { name: "#12 Login no Safari" }));
    expect(await screen.findByRole("button", { name: "Marcar as 2 como revisadas" })).toBeInTheDocument();
  });

  it("Marcar como revisada grava as pendentes no PR e o item sai do cartão, sem trocar de tela", async () => {
    const api = envSide(hubSide(setup(true)));
    render(<App api={api} initial={acmeAt} />);
    const user = userEvent.setup();
    const c = await card("Revisões · 2");
    await user.click(await within(c).findByRole("button", { name: "Marcar #12 Login no Safari como revisada" }));
    await waitFor(() => expect(api.callsTo("review.mark_reviewed")).toHaveLength(1));
    expect(api.callsTo("review.mark_reviewed")[0]).toMatchObject({ env: "wsl", params: { work_item_id: 312, pr: 40 } });
    expect(await within(c).findByRole("status")).toHaveTextContent(
      "#12 Login no Safari revisado: 2 decisões marcadas como revisadas.",
    );
    // Segue em_revisao no dashboard, mas já foi revisado: sai do cartão.
    await waitFor(() => expect(rows(c)).toEqual(["Revisão de #15 Cadastro trava"]));
    expect(screen.getByRole("heading", { name: "Revisões · 1" })).toBeInTheDocument();
    expect(screen.getByRole("tab", { name: "acme-web · Visão geral" })).toHaveAttribute("aria-selected", "true");
  });

  it("vazio e erros: sem itens, o aviso; sem PR ou falha na leitura, o erro no cartão", async () => {
    render(<App api={envSide(hubSide(setup(true)), [W20])} initial={acmeAt} />);
    const c = await card("Revisões · 0");
    expect(await within(c).findByText("Nada em revisão neste projeto.")).toBeInTheDocument();
    cleanup();

    // #15 sem PR no espelho: o erro, e o item fica.
    const noPr = envSide(hubSide(setup(true)));
    render(<App api={noPr} initial={acmeAt} />);
    const user = userEvent.setup();
    const n = await card("Revisões · 2");
    await user.click(await within(n).findByRole("button", { name: "Marcar #15 Cadastro trava como revisada" }));
    expect(await within(n).findByRole("alert")).toHaveTextContent("#15 Cadastro trava: sem PR no espelho do GitHub");
    expect(noPr.callsTo("review.mark_reviewed")).toHaveLength(0);
    expect(rows(n)).toContain("Revisão de #15 Cadastro trava");
    cleanup();

    const failing = envSide(hubSide(setup(true))).on("review.get", () => Promise.reject(new Error("ledger indisponível")));
    render(<App api={failing} initial={acmeAt} />);
    const f = await card("Revisões · 2");
    expect(await within(f).findByRole("alert")).toHaveTextContent("ledger indisponível");
  });

  it("na web (somente leitura), Marcar como revisada fica desabilitado com a explicação padrão", async () => {
    // Na web os itens vêm do dashboard do hub (sem `review.get` no ambiente).
    const items = [
      webItem({ id: 12, project_id: 30, gh_number: 12, status: "em_revisao", title: "Login no Safari", pending_decisions: 2 }),
      webItem({ id: 15, project_id: 30, gh_number: 15, title: "Cadastro trava", pending_decisions: 1 }),
      webItem({ id: 20, project_id: 30, gh_number: 20, title: "Sem decisões" }),
    ];
    const api = webApi({ projects: [{ project: ACME, work_items: items }] })
      .onHub("hub.projects.list", [fx.hubProjectView(ACME, [fx.hubAssignment({ project_id: 30, gh_owner: "acme", gh_repo: "acme-web" })])])
      .onHub("hub.tree.get", { project: ACME, sync: null, roots: [], untracked: [] });
    render(<App api={api} initial={acmeAt} />);
    const c = await card("Revisões · 2");
    for (const name of ["Marcar #12 Login no Safari como revisada", "Marcar #15 Cadastro trava como revisada"]) {
      const b = within(c).getByRole("button", { name });
      expect(b).toBeDisabled();
      expect(b).toHaveAccessibleDescription(DESKTOP_ONLY_NOTE);
    }
    expect(api.callsTo("review.mark_reviewed")).toHaveLength(0);
    expect(api.callsTo("review.get")).toHaveLength(0);
  });
});
