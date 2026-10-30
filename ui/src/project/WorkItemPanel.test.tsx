import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { App } from "../App";
import { FakeDaemonApi } from "../api/fake";
import type { EnvInfo, IssueRow, PrRow, ProjectTree } from "../protocol/gen";
import type { HubDashboard, HubDecision, HubEvent, HubHistory, HubProject, HubSession, HubTree, HubTreeNode, HubWorkItem } from "../protocol/hub-gen";
import { HUB_CONFIG_UNLINKED, dashboard, hubInfo, project, session, sync, workItem, RC_AVAILABLE } from "../test/fixtures";

const NOW = Date.now();
const H = 3600_000;

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

function issue(n: number, title: string, p: Partial<IssueRow> = {}): IssueRow {
  return {
    gh_number: n,
    title,
    state: "open",
    labels: [],
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

const hubItem = (n: number, p: Partial<HubWorkItem> = {}): HubWorkItem => ({
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
});

const PR50: PrRow = {
  number: 120,
  work_item_id: 150,
  head: "feat/50-painel",
  base: "wip/51-epico",
  state: "open",
  url: "https://github.com/deep-ion-ai/ctower-agents/pull/120",
  updated_at: 0,
  head_sha: null,
};

const node = (i: IssueRow, wi: HubWorkItem | null, pr: PrRow | null = null): HubTreeNode => ({
  issue: i,
  work_item: wi,
  summary: { pr, decisions: { total: 0, pending_autonomous: 0 }, last_activity_at: NOW - 2 * H, live_envs: [] },
  children: [],
});

const TREE: HubTree = {
  project: HUB_PROJECT,
  sync: sync({ project_id: 7, last_ok_at: NOW }),
  roots: [
    node(issue(50, "ui: painel de detalhe"), hubItem(50), PR50),
    node(issue(62, "ui: sem histórico"), null),
    node(issue(59, "sync ambiente→hub"), hubItem(59)),
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

const hubSession = (id: string, p: Partial<HubSession> = {}): HubSession => ({
  id,
  env_id: "daemon-wsl",
  work_item_id: 150,
  tool: "claude",
  tool_session_id: null,
  cwd: "/w",
  state: "exited",
  exit_code: 0,
  started_at: NOW - 5 * H,
  last_event_at: NOW - 3 * H,
  ended_at: NOW - 3 * H,
  model: null,
  mode: "native",
  rc_session_id: null,
  ...p,
});

const event = (n: number): HubEvent => ({
  id: n,
  uid: `ev-${n}`,
  env_id: "daemon-wsl",
  hlc: n,
  ts: NOW - n * 60_000 * 10,
  project_id: 7,
  work_item_id: 150,
  session_id: null,
  kind: `kind_${n}`,
  payload: { from: "backlog", to: "em_execucao" },
});

const decision = (n: number, p: Partial<HubDecision>): HubDecision => ({
  id: n,
  uid: `d-${n}`,
  env_id: "daemon-wsl",
  work_item_id: 150,
  session_id: null,
  origin: "orientada",
  title: `Decisão ${n}`,
  context: "",
  decision: `Escolhi ${n}`,
  consequences: null,
  reviewed_at: null,
  reviewed_in_pr: null,
  created_at: n,
  ...p,
});

const LIVE_ID = "50505050-0000-0000-0000-000000000050";

const HISTORY: HubHistory = {
  work_item: hubItem(50),
  locations: [],
  sessions: [
    hubSession(LIVE_ID, { state: "running", exit_code: null, ended_at: null }),
    hubSession("40404040-0000-0000-0000-000000000040", { exit_code: 2 }),
  ],
  events: [1, 2, 3, 4, 5, 6, 7].map(event),
  next_before_hlc: null,
  sub_work_items: [],
  decisions: [
    decision(1, { title: "Revisada antes", origin: "autonoma", reviewed_at: 5, reviewed_in_pr: 99 }),
    decision(2, { title: "Orientada recente", created_at: 50, consequences: "nenhuma" }),
    decision(3, { title: "Autônoma pendente", origin: "autonoma", created_at: 1 }),
  ],
  prs: [PR50],
};

const WI50 = workItem({ gh_number: 50, status: "em_execucao" });
const WI62 = workItem({ gh_number: 62, status: "backlog", branch: null, worktree_path: null });
const LIVE50 = session({ id: LIVE_ID, work_item_id: 50 });

function build(hubUp: boolean) {
  const api = new FakeDaemonApi([
    { ...FakeDaemonApi.wslEnv("wsl"), name: "WSL" },
    { ...FakeDaemonApi.wslEnv("mac"), name: "Mac mini", kind: "remote" as const, distro: null },
  ])
    .on("dashboard.get", (_: unknown, env: string) =>
      env === "wsl" ? dashboard({ live_sessions: [LIVE50] }, [WI50, WI62]) : dashboard({ projects: [] }),
    )
    .on("env.info", (_: unknown, env: string) =>
      env === "wsl" ? info("daemon-wsl") : info("daemon-mac"),
    )
    .withHub("ws://mac-mini:7879/ws", hubUp)
    .onHub("hub.dashboard.get", HUB_DASH)
    .onHub("hub.tree.get", TREE)
    .onHub("hub.workitem.history", HISTORY)
    .onHub("hub.workitem.set_status", hubItem(50, { status: "bloqueado" }))
    .on("tree.get", { project: project(), sync: sync(), roots: [], untracked: [] })
    .on("session.peek", { data: "" })
    .on("session.start", session({ id: "62626262-0000-0000-0000-000000000062", work_item_id: 62 }));
  return api;
}

function setup(hubUp: boolean) {
  const api = build(hubUp);
  render(<App api={api} initial={{ name: "project", owner: "deep-ion-ai", repo: "ctower-agents" }} />);
  return { api, user: userEvent.setup() };
}

/** Abre o item pela linha da árvore do painel lateral (aba própria). */
async function openPanel(user: ReturnType<typeof userEvent.setup>, title: string) {
  await screen.findByText(title);
  const side = screen.getByRole("complementary", { name: "Árvore do projeto" });
  await user.click(within(side).getByRole("button", { name: new RegExp(`^#\\d+ ${title}`) }));
  return await screen.findByRole("region", { name: "Detalhe do item" });
}

test("clicar na linha abre o painel com sessões do hub; sessão viva abre o terminal", async () => {
  const { api, user } = setup(true);
  const panel = await openPanel(user, "ui: painel de detalhe");
  expect(within(panel).getByRole("heading", { name: /#50 ui: painel de detalhe/ })).toBeInTheDocument();
  const sessoes = await within(panel).findByRole("region", { name: "Sessões" });
  expect(api.callsTo("hub.workitem.history")[0]).toMatchObject({
    env: "hub",
    params: { owner: "deep-ion-ai", repo: "ctower-agents", number: 50 },
  });
  // A encerrada mostra código de saída e duração; a viva está em andamento.
  expect(within(sessoes).getByText("saída 2")).toBeInTheDocument();
  expect(within(sessoes).getByText(/duração 2 h/)).toBeInTheDocument();
  expect(within(sessoes).getByText(/em andamento/)).toBeInTheDocument();
  await user.click(within(sessoes).getByRole("button", { name: /Abrir terminal da sessão claude/ }));
  expect(screen.queryByRole("region", { name: "Detalhe do item" })).not.toBeInTheDocument();
});

test("eventos em páginas de 5 com 'Mostrar mais'", async () => {
  const { user } = setup(true);
  const panel = await openPanel(user, "ui: painel de detalhe");
  const eventos = await within(panel).findByRole("region", { name: "Eventos" });
  expect(within(eventos).getAllByRole("listitem")).toHaveLength(5);
  expect(within(eventos).getByText("kind_1")).toBeInTheDocument();
  await user.click(within(eventos).getByRole("button", { name: "Mostrar mais (2)" }));
  expect(within(eventos).getAllByRole("listitem")).toHaveLength(7);
  expect(within(eventos).queryByRole("button", { name: /Mostrar mais/ })).not.toBeInTheDocument();
});

test("decisões: autônomas pendentes primeiro, com origem, decisão e consequências; PRs com head → base", async () => {
  const { user } = setup(true);
  const panel = await openPanel(user, "ui: painel de detalhe");
  const decs = await within(panel).findByRole("region", { name: "Decisões" });
  const titles = within(decs)
    .getAllByRole("listitem")
    .map((li) => li.querySelector(".dt")!.textContent);
  expect(titles).toEqual(["Autônoma pendente", "Orientada recente", "Revisada antes"]);
  expect(within(decs).getByText("pendente de revisão")).toBeInTheDocument();
  expect(within(decs).getByText("1 pendente")).toBeInTheDocument();
  expect(within(decs).getByText("revisada no PR #99")).toBeInTheDocument();
  expect(within(decs).getByText("nenhuma")).toBeInTheDocument();
  // Atalho para a Revisão das decisões do item.
  expect(within(decs).getByRole("button", { name: "Revisar" })).toBeInTheDocument();
  const prs = within(panel).getByRole("region", { name: "PRs" });
  expect(within(prs).getByText("feat/50-painel → wip/51-epico")).toBeInTheDocument();
});

test("abrir no GitHub: issue e PR pelo openUrl do app", async () => {
  const { api, user } = setup(true);
  const panel = await openPanel(user, "ui: painel de detalhe");
  await user.click(within(panel).getByRole("button", { name: /Abrir issue no GitHub/ }));
  await user.click(within(panel).getByRole("button", { name: /Abrir PR #120 no GitHub ↗/ }));
  const prs = await within(panel).findByRole("region", { name: "PRs" });
  await user.click(within(prs).getByRole("button", { name: /Abrir PR #120/ }));
  expect(api.opened).toEqual(["https://github.com/deep-ion-ai/ctower-agents/issues/50", PR50.url, PR50.url]);
});

test("mudar status: 7 opções, motivo opcional, chama hub.workitem.set_status e relê a árvore", async () => {
  const { api, user } = setup(true);
  const panel = await openPanel(user, "ui: painel de detalhe");
  await user.click(within(panel).getByRole("button", { name: /Mudar status/ }));
  const menu = within(panel).getByRole("group", { name: "Mudar status" });
  expect(within(menu).getAllByRole("button")).toHaveLength(7);
  expect(within(menu).getByRole("button", { name: /em execução/ })).toHaveAttribute("aria-current", "true");
  await user.type(within(menu).getByLabelText("Motivo (opcional)"), "API fora");
  const trees = api.callsTo("hub.tree.get").length;
  await user.click(within(menu).getByRole("button", { name: "bloqueado" }));
  await waitFor(() => expect(api.callsTo("hub.workitem.set_status")).toHaveLength(1));
  expect(api.callsTo("hub.workitem.set_status")[0]).toMatchObject({
    env: "hub",
    params: { owner: "deep-ion-ai", repo: "ctower-agents", number: 50, status: "bloqueado", reason: "API fora" },
  });
  await waitFor(() => expect(api.callsTo("hub.tree.get").length).toBeGreaterThan(trees));
  expect(screen.queryByRole("group", { name: "Mudar status" })).not.toBeInTheDocument();
});

test("prioridade: menu com P0–P3 e não triado, chama hub.workitem.set_priority", async () => {
  const { api, user } = setup(true);
  api.onHub("hub.workitem.set_priority", hubItem(50, { priority: "P1", rank: 1 }));
  const panel = await openPanel(user, "ui: painel de detalhe");
  await user.click(within(panel).getByRole("button", { name: "Prioridade: não triado ▾" }));
  const menu = within(panel).getByRole("group", { name: "Mudar prioridade" });
  expect(within(menu).getAllByRole("button").map((b) => b.textContent)).toEqual([
    "P0",
    "P1",
    "P2",
    "P3",
    "não triadoatual",
  ]);
  await user.click(within(menu).getByRole("button", { name: "P1" }));
  await waitFor(() => expect(api.hubCallsTo("hub.workitem.set_priority")).toHaveLength(1));
  expect(api.hubCallsTo("hub.workitem.set_priority")[0].params).toEqual({
    owner: "deep-ion-ai",
    repo: "ctower-agents",
    number: 50,
    priority: "P1",
  });
});

test("prioridade: subir na ordem troca com o vizinho da classe pelo ranking", async () => {
  const api = build(true);
  const tree = structuredClone(TREE);
  for (const r of tree.roots) {
    for (const x of [r, ...r.children]) {
      if (x.issue.gh_number === 50 && x.work_item) x.work_item = { ...x.work_item, priority: "P1", rank: 2 };
    }
  }
  api.onHub("hub.tree.get", tree);
  api.onHub("hub.ranking.get", {
    items: [
      { position: 1, owner: "deep-ion-ai", repo: "ctower-agents", number: 44, title: "x", kind: "issue", status: "backlog", priority: "P1", rank: 1 },
      { position: 2, owner: "deep-ion-ai", repo: "ctower-agents", number: 50, title: "y", kind: "issue", status: "em_execucao", priority: "P1", rank: 2 },
    ],
    truncated: false,
  });
  api.onHub("hub.workitem.move", hubItem(50, { priority: "P1", rank: 1 }));
  render(<App api={api} initial={{ name: "project", owner: "deep-ion-ai", repo: "ctower-agents" }} />);
  const user = userEvent.setup();
  const panel = await openPanel(user, "ui: painel de detalhe");
  expect(within(panel).getByTitle("prioridade P1")).toBeInTheDocument();
  await user.click(within(panel).getByRole("button", { name: "Prioridade: P1 ▾" }));
  await user.click(within(panel).getByRole("button", { name: "Subir na ordem de P1" }));
  await waitFor(() => expect(api.hubCallsTo("hub.workitem.move")).toHaveLength(1));
  expect(api.hubCallsTo("hub.ranking.get")[0].params).toMatchObject({ priority: "P1", owner: "deep-ion-ai" });
  expect(api.hubCallsTo("hub.workitem.move")[0].params).toEqual({
    owner: "deep-ion-ai",
    repo: "ctower-agents",
    number: 50,
    before: 44,
  });
});

test("mudar status sem motivo não manda reason; erro do hub aparece no painel", async () => {
  const { api, user } = setup(true);
  api.onHub("hub.workitem.set_status", () => {
    throw new Error("projeto fora do hub");
  });
  const panel = await openPanel(user, "ui: painel de detalhe");
  await user.click(within(panel).getByRole("button", { name: /Mudar status/ }));
  await user.click(within(panel).getByRole("button", { name: "pausado" }));
  expect(await within(panel).findByRole("alert")).toHaveTextContent("projeto fora do hub");
  expect(api.callsTo("hub.workitem.set_status")[0].params.reason).toBeUndefined();
});

test("item sem work item: só as ações e o aviso 'sem histórico'", async () => {
  const { api, user } = setup(true);
  const panel = await openPanel(user, "ui: sem histórico");
  expect(within(panel).getByText(/Sem histórico/)).toBeInTheDocument();
  expect(within(panel).getByRole("button", { name: /Abrir issue no GitHub/ })).toBeInTheDocument();
  expect(within(panel).getByRole("button", { name: /Mudar status/ })).toBeEnabled();
  expect(api.callsTo("hub.workitem.history")).toHaveLength(0);
});

test("Iniciar sessão no cabeçalho do item abre a modal de início, que manda owner/repo", async () => {
  const { api, user } = setup(true);
  const panel = await openPanel(user, "ui: sem histórico");
  const origem = within(panel).getByRole("button", { name: "Iniciar sessão" });
  await user.click(origem);
  const modal = screen.getByRole("dialog");
  expect(modal).toHaveAccessibleName(/^#62 ui: sem histórico/);
  await user.click(within(modal).getByRole("button", { name: "Cancelar" }));
  expect(origem).toHaveFocus();
  await user.click(origem);
  await user.click(within(screen.getByRole("dialog")).getByRole("button", { name: /^Iniciar sessão/ }));
  await waitFor(() => expect(api.callsTo("session.start")).toHaveLength(1));
  expect(api.callsTo("session.start")[0].params).toMatchObject({ gh_number: 62, owner: "deep-ion-ai" });
});

test("fechar a aba do item", async () => {
  const { user } = setup(true);
  await openPanel(user, "ui: painel de detalhe");
  await user.click(screen.getByRole("button", { name: "Fechar aba #50 ui: painel de detalhe" }));
  expect(screen.queryByRole("region", { name: "Detalhe do item" })).not.toBeInTheDocument();
});

const itemTabs = () => screen.getAllByRole("tab").map((t) => t.textContent).filter((t) => t?.startsWith("#"));

test("abas provisórias: o próximo item substitui a aba; fixada, ela fica", async () => {
  const { user } = setup(true);
  await openPanel(user, "ui: painel de detalhe");
  await openPanel(user, "ui: sem histórico");
  // Uma aba só, provisória (no lugar da anterior).
  expect(itemTabs()).toEqual(["#62 ui: sem histórico"]);
  const pin = screen.getByRole("button", { name: "Fixar aba" });
  expect(pin).toHaveAttribute("aria-pressed", "false");
  await user.click(pin);
  const unpin = screen.getByRole("button", { name: "Desafixar aba" });
  expect(unpin).toHaveAttribute("aria-pressed", "true");
  // Fixada, o próximo item abre outra aba ao lado.
  await openPanel(user, "ui: painel de detalhe");
  expect(itemTabs()).toEqual(["#62 ui: sem histórico", "#50 ui: painel de detalhe"]);
  // Reabrir um item já aberto só ativa a aba dele.
  await openPanel(user, "ui: sem histórico");
  expect(itemTabs()).toHaveLength(2);
  expect(screen.getByRole("tab", { name: "#62 ui: sem histórico" })).toHaveAttribute("aria-selected", "true");
  // Duplo clique no título fixa a provisória.
  await user.dblClick(screen.getByRole("tab", { name: "#50 ui: painel de detalhe" }));
  expect(screen.getAllByRole("button", { name: "Desafixar aba" })).toHaveLength(2);
  expect(screen.queryByRole("button", { name: "Fixar aba" })).not.toBeInTheDocument();
});

test("erro ao carregar o histórico: mensagem e 'Tentar de novo'", async () => {
  const { api, user } = setup(true);
  let falha = true;
  api.onHub("hub.workitem.history", () => {
    if (falha) throw new Error("hub caiu");
    return HISTORY;
  });
  const panel = await openPanel(user, "ui: painel de detalhe");
  expect(await within(panel).findByRole("alert")).toHaveTextContent("hub caiu");
  falha = false;
  await user.click(within(panel).getByRole("button", { name: "Tentar de novo" }));
  expect(await within(panel).findByRole("region", { name: "Sessões" })).toBeInTheDocument();
});

test("push que envolve o item recarrega o histórico; o de outro item não", async () => {
  const { api, user } = setup(true);
  const panel = await openPanel(user, "ui: painel de detalhe");
  await within(panel).findByRole("region", { name: "Sessões" });
  const before = api.callsTo("hub.workitem.history").length;
  act(() => api.push("wsl", "session_started", { work_item_id: 62 }));
  act(() => api.push("wsl", "session_exited", { session_id: "ffffffff-0000-0000-0000-000000000000" }));
  await new Promise((r) => setTimeout(r, 50));
  expect(api.callsTo("hub.workitem.history")).toHaveLength(before);
  act(() => api.push("wsl", "session_started", { work_item_id: 50 }));
  await waitFor(() => expect(api.callsTo("hub.workitem.history")).toHaveLength(before + 1));
  act(() => api.push("wsl", "session_exited", { session_id: LIVE_ID }));
  await waitFor(() => expect(api.callsTo("hub.workitem.history")).toHaveLength(before + 2));
});

async function localSetup(hubSync: boolean) {
  const api = build(false);
  const local: ProjectTree = {
    project: project(),
    sync: sync(),
    roots: [
      {
        issue: issue(50, "ui: painel de detalhe"),
        work_item: { ...WI50, worktree_path: null } as never,
        summary: { pr: PR50, decisions: { total: 0, pending_autonomous: 0 }, last_activity_at: NOW, live_envs: [] },
        children: [],
      },
    ],
    untracked: [],
  };
  api.on("tree.get", local);
  if (hubSync) {
    const sending = { pending: 1, oldest_pending_at: NOW, last_attempt_at: NOW, last_ok_at: null, last_error: "x", failures: 1, syncing: false, prune: { retention_days: 30, pruned_work_items: 0, last_run_at: null, last_pruned: 0, last_error: null } };
    api.on("env.info", info("daemon-wsl", { hub_sync: sending }));
  } else {
    // Sem conexão com o hub configurada na UI.
    api.hub = null;
    api.hubState = null;
  }
  render(<App api={api} initial={{ name: "project", env: "wsl", projectId: 1 }} />);
  const user = userEvent.setup();
  const panel = await openPanel(user, "ui: painel de detalhe");
  return { api, panel };
}

test("tela local com o hub fora: aviso de visão parcial, status desabilitado e sem histórico", async () => {
  const { api, panel } = await localSetup(true);
  const status = within(panel).getByRole("button", { name: /Mudar status/ });
  expect(status).toBeDisabled();
  await waitFor(() => expect(status).toHaveAccessibleDescription(/Hub indisponível: visão parcial/));
  expect(within(panel).getByRole("button", { name: /Abrir PR #120 no GitHub/ })).toBeInTheDocument();
  expect(api.callsTo("hub.workitem.history")).toHaveLength(0);
});

test("tela local sem hub configurado: explica que histórico e status dependem do hub", async () => {
  const { panel } = await localSetup(false);
  expect(within(panel).getByRole("button", { name: /Mudar status/ })).toBeDisabled();
  expect(within(panel).getByText(/Sem conexão com o hub/)).toBeInTheDocument();
});

test("menu de status: o foco vai ao status atual e o Escape devolve o foco ao botão", async () => {
  const { user } = { user: userEvent.setup() };
  const api = build(true);
  render(<App api={api} initial={{ name: "project", owner: "deep-ion-ai", repo: "ctower-agents" }} />);
  const panel = await openPanel(user, "ui: painel de detalhe");
  const botao = within(panel).getByRole("button", { name: /Mudar status/ });
  await user.click(botao);
  const menu = within(panel).getByRole("group", { name: "Mudar status" });
  expect(botao).toHaveAttribute("aria-expanded", "true");
  expect(within(menu).getByRole("button", { name: /em execução/ })).toHaveFocus();
  // O campo de texto não fica dentro de um role=menu.
  expect(within(panel).queryByRole("menu")).not.toBeInTheDocument();
  expect(within(panel).queryByRole("menuitem")).not.toBeInTheDocument();
  await user.keyboard("{Escape}");
  expect(within(panel).queryByRole("group", { name: "Mudar status" })).not.toBeInTheDocument();
  expect(botao).toHaveFocus();
  expect(botao).toHaveAttribute("aria-expanded", "false");
});

test("o status mostrado e o atual vêm do hub quando o ledger local está defasado", async () => {
  const api = build(true);
  // O hub já tem o item bloqueado; o work item local e a árvore dizem em execução.
  api.onHub("hub.dashboard.get", {
    ...HUB_DASH,
    projects: [{ project: HUB_PROJECT, work_items: [hubItem(50, { status: "bloqueado" })] }],
  });
  const user = userEvent.setup();
  render(<App api={api} initial={{ name: "project", owner: "deep-ion-ai", repo: "ctower-agents" }} />);
  const panel = await openPanel(user, "ui: painel de detalhe");
  await waitFor(() => expect(within(panel).getAllByText("bloqueado").length).toBeGreaterThan(0));
  await user.click(within(panel).getByRole("button", { name: /Mudar status/ }));
  const menu = within(panel).getByRole("group", { name: "Mudar status" });
  expect(within(menu).getByRole("button", { name: /bloqueado/ })).toHaveAttribute("aria-current", "true");
  // Voltar ao status local defasado (em execução) não cai no atalho "já é o atual".
  await user.click(within(menu).getByRole("button", { name: /em execução/ }));
  await waitFor(() => expect(api.callsTo("hub.workitem.set_status")).toHaveLength(1));
  expect(api.callsTo("hub.workitem.set_status")[0].params).toMatchObject({ status: "em_execucao" });
});

test("épico: regras do agente do épico no painel, pelo hub; issue comum não tem", async () => {
  const api = build(true);
  const epic = node(issue(51, "épico das regras", { has_sub_issues: true }), hubItem(51, { kind: "epic" }));
  api
    .onHub("hub.tree.get", { ...TREE, roots: [epic, ...TREE.roots] })
    .onHub("hub.rules.get", (p: { owner: string; repo: string; epic_number: number }) => ({
      ...p,
      label: "deep-ion-ai/ctower-agents#51",
      current: null,
      pending: 0,
      max_bytes: 16384,
      warn_bytes: 12288,
    }))
    .onHub("hub.rules.proposals.list", { proposals: [], truncated: false });
  render(<App api={api} initial={{ name: "project", owner: "deep-ion-ai", repo: "ctower-agents" }} />);
  const user = userEvent.setup();
  const panel = await openPanel(user, "épico das regras");
  const card = await within(panel).findByRole("region", {
    name: "Regras do agente do épico deep-ion-ai/ctower-agents#51",
  });
  expect(await within(card).findByText(/Sem regras do épico/)).toBeInTheDocument();
  expect(api.hubCallsTo("hub.rules.get")[0].params).toEqual({
    owner: "deep-ion-ai",
    repo: "ctower-agents",
    epic_number: 51,
  });
  const other = await openPanel(user, "ui: painel de detalhe");
  expect(within(other).queryByRole("region", { name: /Regras do agente/ })).not.toBeInTheDocument();
});

test("sub-issue aguardando o épico: rótulo, opção no seletor e contagem separada no épico", async () => {
  const api = build(true);
  const sub = (n: number, status: HubWorkItem["status"]) =>
    node(issue(n, `sub ${n}`, { parent_number: 51 }), hubItem(n, { status }));
  const epic: HubTreeNode = {
    ...node(issue(51, "épico das sub-issues", { has_sub_issues: true }), hubItem(51, { kind: "epic" })),
    children: [sub(52, "concluido"), sub(53, "aguardando_epico"), sub(54, "aguardando_epico"), sub(55, "em_execucao")],
  };
  api
    .onHub("hub.tree.get", { ...TREE, roots: [epic, ...TREE.roots] })
    .onHub("hub.rules.get", (p: { owner: string; repo: string; epic_number: number }) => ({
      ...p,
      label: "deep-ion-ai/ctower-agents#51",
      current: null,
      pending: 0,
      max_bytes: 16384,
      warn_bytes: 12288,
    }))
    .onHub("hub.rules.proposals.list", { proposals: [], truncated: false });
  render(<App api={api} initial={{ name: "project", owner: "deep-ion-ai", repo: "ctower-agents" }} />);
  const user = userEvent.setup();
  await screen.findByText("épico das sub-issues");
  const side = screen.getByRole("complementary", { name: "Árvore do projeto" });
  // A linha do épico resume as filhas: só as concluídas contam no "x de y".
  const epicRow = within(side).getByRole("button", { name: /^Épico #51 épico das sub-issues/ });
  expect(epicRow).toHaveAccessibleName(/1 de 4 concluídas/);
  await user.click(epicRow);
  const panel = await screen.findByRole("region", { name: "Detalhe do item" });
  expect(within(panel).getByText("1 concluída, 2 aguardando o épico de 4")).toBeInTheDocument();
  expect(within(panel).getByRole("progressbar", { name: "Progresso do épico" })).toHaveAttribute("aria-valuenow", "1");
  // O épico (sem pai) não oferece "aguardando o épico".
  await user.click(within(panel).getByRole("button", { name: /Mudar status/ }));
  expect(within(within(panel).getByRole("group", { name: "Mudar status" })).getAllByRole("button")).toHaveLength(7);

  // Aberto o item, a árvore lateral é outra: busca de novo.
  const tree = screen.getByRole("complementary", { name: "Árvore do projeto" });
  await user.click(within(tree).getByRole("button", { name: /^#53 sub 53/ }));
  await screen.findByRole("heading", { level: 1, name: /#53 sub 53/ });
  const item = screen.getByRole("region", { name: "Detalhe do item" });
  expect(within(item).getAllByText("aguardando o épico").length).toBeGreaterThan(0);
  await user.click(within(item).getByRole("button", { name: /Mudar status/ }));
  const menu = within(item).getByRole("group", { name: "Mudar status" });
  expect(within(menu).getAllByRole("button")).toHaveLength(8);
  expect(within(menu).getByRole("button", { name: /aguardando o épico/ })).toHaveAttribute("aria-current", "true");
});

// ---- "Ver issue" (#434) ----

const BODY = [
  "## Contexto",
  "",
  "Depende de #59; ver [a spec](https://docs.example/spec).",
  "",
  "- [ ] critério aberto",
  "- [x] critério feito",
  "",
  "<img src=x onerror=\"alert(1)\"><script>alert(2)</script>",
].join("\n");

function setupBody(body: (p: unknown) => unknown, protocol = "1.10") {
  const api = build(true).onHub("hub.info", hubInfo({ protocol })).onHub("hub.workitem.body", body);
  render(<App api={api} initial={{ name: "project", owner: "deep-ion-ai", repo: "ctower-agents" }} />);
  return { api, user: userEvent.setup() };
}

const bodyOf = (body: string) => ({
  title: "ui: painel de detalhe",
  body,
  format: "markdown",
  url: "https://github.com/deep-ion-ai/ctower-agents/issues/50",
});

test("Ver issue: busca pelo hub ao abrir, renderiza sanitizado e descarta ao fechar", async () => {
  const { api, user } = setupBody(() => bodyOf(BODY));
  const panel = await openPanel(user, "ui: painel de detalhe");
  await user.click(within(panel).getByRole("button", { name: "Ver issue" }));
  const dialog = await screen.findByRole("dialog", { name: /#50 ui: painel de detalhe/ });
  expect(await within(dialog).findByRole("heading", { name: "Contexto" })).toBeInTheDocument();
  expect(api.hubCallsTo("hub.workitem.body")[0].params).toEqual({
    owner: "deep-ion-ai",
    repo: "ctower-agents",
    number: 50,
  });
  // Checklist só de leitura.
  const boxes = within(dialog).getAllByRole("checkbox");
  expect(boxes.map((b) => (b as HTMLInputElement).checked)).toEqual([false, true]);
  expect(boxes.every((b) => (b as HTMLInputElement).disabled)).toBe(true);
  // Nada executável do corpo chega ao DOM.
  expect(dialog.querySelector("script")).toBeNull();
  expect(dialog.querySelector("[onerror]")).toBeNull();
  // Link externo vai para o navegador.
  await user.click(within(dialog).getByRole("link", { name: "a spec" }));
  expect(api.opened).toEqual(["https://docs.example/spec"]);
  // Esc fecha; abrir de novo busca de novo.
  await user.keyboard("{Escape}");
  expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  await user.click(within(panel).getByRole("button", { name: "Ver issue" }));
  await screen.findByRole("dialog");
  await waitFor(() => expect(api.hubCallsTo("hub.workitem.body")).toHaveLength(2));
  await user.click(screen.getByRole("button", { name: "Fechar" }));
  expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
});

test("Ver issue: #n do mesmo repositório abre a aba do item no app", async () => {
  const { api, user } = setupBody(() => bodyOf(BODY));
  const panel = await openPanel(user, "ui: painel de detalhe");
  await user.click(within(panel).getByRole("button", { name: "Ver issue" }));
  const dialog = await screen.findByRole("dialog");
  await user.click(await within(dialog).findByRole("link", { name: "#59" }));
  expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  expect(await screen.findByRole("heading", { name: /#59 sync ambiente→hub/ })).toBeInTheDocument();
  expect(api.opened).toEqual([]);
});

test("Ver issue: sem descrição e erro do rastreador com a saída para o GitHub", async () => {
  let fail = false;
  const { api, user } = setupBody(() => {
    if (fail) throw new Error("GitHub: 502 Bad Gateway");
    return bodyOf("  ");
  });
  const panel = await openPanel(user, "ui: painel de detalhe");
  await user.click(within(panel).getByRole("button", { name: "Ver issue" }));
  expect(await screen.findByText("Sem descrição.")).toBeInTheDocument();
  await user.click(screen.getByRole("button", { name: "Fechar" }));

  fail = true;
  await user.click(within(panel).getByRole("button", { name: "Ver issue" }));
  const alert = await screen.findByRole("alert");
  expect(alert).toHaveTextContent("GitHub: 502 Bad Gateway");
  await user.click(within(alert).getByRole("button", { name: "Abrir no GitHub ↗" }));
  expect(api.opened).toEqual(["https://github.com/deep-ion-ai/ctower-agents/issues/50"]);
});

test("Ver issue: desabilitado com hub anterior ao método ou fora do ar", async () => {
  const { api, user } = setupBody(() => bodyOf("x"), "1.9");
  const panel = await openPanel(user, "ui: painel de detalhe");
  await waitFor(() => expect(within(panel).getByRole("button", { name: "Ver issue" })).toBeDisabled());
  expect(within(panel).getByRole("button", { name: "Ver issue" })).toHaveAttribute(
    "title",
    expect.stringContaining("Atualize o hub"),
  );
  await user.click(within(panel).getByRole("button", { name: "Ver issue" }));
  expect(api.hubCallsTo("hub.workitem.body")).toHaveLength(0);
});

test("Ver issue: tela local com o hub fora fica desabilitado", async () => {
  const { panel } = await localSetup(true);
  const ver = within(panel).getByRole("button", { name: "Ver issue" });
  expect(ver).toBeDisabled();
  expect(ver).toHaveAttribute("title", expect.stringContaining("fora do ar"));
});
