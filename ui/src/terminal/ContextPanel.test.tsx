import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { App } from "../App";
import { FakeDaemonApi } from "../api/fake";
import type { Decision, EnvInfo, Event, Review, Shell } from "../protocol/gen";
import type { HubHistory, HubSession } from "../protocol/hub-gen";
import { terms } from "../test/fakeXterm";
import { dashboard, HUB_CONFIG_UNLINKED, HUB_DASHBOARD, MIN, project, session, sync, workItem } from "../test/fixtures";
import { CONTEXT_RELOAD_MS, EVENTS_OLD, isUnknownMethod, pushConcerns } from "./ContextPanel";
import { resetShellStore } from "./shells";

// Painel de contexto ao lado do console (#426; spec da #87, §3). O xterm é o
// fake de ../test/fakeXterm.

const NOW = Date.now();
const WI = workItem({ gh_number: 32, title: "ui: terminal embutido", branch: "feat/32-ui-terminal-embutido" });
const S = session({ id: "bbbbbbbb-0000-0000-0000-000000000032", work_item_id: 32, started_at: NOW - 30 * MIN });
const WI2 = workItem({ gh_number: 40, title: "revisar guia", branch: "feat/40-revisar-guia" });
const S2 = session({ id: "bbbbbbbb-0000-0000-0000-000000000040", work_item_id: 40, started_at: NOW - 20 * MIN });
/** Sessão anterior do #32, interrompida: vai para a Recuperação. */
const OLD = session({
  id: "bbbbbbbb-0000-0000-0000-000000000031",
  work_item_id: 32,
  state: "interrupted",
  started_at: NOW - 300 * MIN,
  ended_at: NOW - 200 * MIN,
  exit_code: 137,
});

function decision(p: Partial<Decision>): Decision {
  return {
    id: 1,
    uid: "d1",
    work_item_id: 32,
    session_id: null,
    origin: "autonoma",
    title: "Decisão",
    context: "c",
    decision: "d",
    consequences: null,
    reviewed_at: null,
    reviewed_in_pr: null,
    created_at: NOW - 60 * MIN,
    ...p,
  };
}

function ev(id: number, kind = `evento_${id}`): Event {
  return { id, uid: `e${id}`, ts: NOW - id * MIN, project_id: 1, work_item_id: 32, session_id: null, kind, payload: { n: id } };
}

/** Eventos 12…1 (do mais novo para o mais antigo), paginados como o daemon. */
const EVENTS = Array.from({ length: 12 }, (_, i) => ev(12 - i));
function eventsList(p: { before?: number; limit?: number }) {
  return EVENTS.filter((e) => p.before === undefined || e.id < p.before).slice(0, p.limit ?? 20);
}

function review(p: Partial<Review> = {}): Review {
  return {
    work_item: WI,
    prs: [],
    decisions: [
      decision({ uid: "d-old", title: "Antiga orientada", origin: "orientada", created_at: NOW - 120 * MIN }),
      decision({ uid: "d-new", title: "Nova autônoma", created_at: NOW - 5 * MIN }),
      decision({ uid: "d-rev", title: "Revisada", created_at: NOW - 60 * MIN, reviewed_at: NOW, reviewed_in_pr: 70 }),
    ],
    ...p,
  };
}

const panel = () => screen.getByRole("complementary", { name: "Contexto da sessão" });
const queryPanel = () => screen.queryByRole("complementary", { name: "Contexto da sessão" });
const bar = (name = "Sessão") => screen.getByRole("region", { name });

function base(api = new FakeDaemonApi()) {
  return api
    .on("dashboard.get", dashboard({ live_sessions: [S, S2], interrupted_sessions: [OLD], sync: [sync({ last_ok_at: NOW })] }, [WI, WI2]))
    .on("session.peek", { data: "" })
    .on("shell.list", [])
    .on("review.get", (p: { work_item_id: number }) =>
      p.work_item_id === 40 ? review({ work_item: WI2, decisions: [] }) : review(),
    )
    .on("events.list", eventsList)
    .on("recovery.list", []);
}

async function openTab(api = base(), name: RegExp = /#32 ui: terminal embutido rodando/) {
  terms.length = 0;
  render(<App api={api} />);
  const user = userEvent.setup();
  // Com uma interrompida o app abre a Recuperação: a sessão vem pela árvore do Terminal.
  await user.click(await screen.findByRole("button", { name: /^Terminal/ }));
  await user.click(await screen.findByRole("treeitem", { name }));
  await waitFor(() => expect(terms.length).toBeGreaterThan(0));
  return { api, user };
}

beforeEach(() => resetShellStore());

test("seções: work item, PR, decisões da mais nova para a mais antiga e eventos paginados; o estado fica guardado", async () => {
  const api = base().on(
    "review.get",
    review({
      prs: [{ number: 77, work_item_id: 32, head: WI.branch!, base: "develop", state: "open", url: "https://github.com/deep-ion-ai/ctower-agents/pull/77", updated_at: 0, head_sha: null }],
    }),
  );
  const { user } = await openTab(api);
  expect(queryPanel()).not.toBeInTheDocument();
  const btn = within(bar()).getByRole("button", { name: "Contexto" });
  expect(btn).toHaveAttribute("aria-pressed", "false");
  await user.click(btn);
  expect(btn).toHaveAttribute("aria-pressed", "true");
  expect(localStorage.getItem("ctower.terminal.context")).toBe("1");

  const wi = within(panel()).getByRole("region", { name: "Work item" });
  expect(within(wi).getByText("em execução")).toBeInTheDocument();
  expect(within(wi).getByText("feat/32-ui-terminal-embutido")).toBeInTheDocument();
  expect(within(wi).getByText(WI.worktree_path!)).toBeInTheDocument();
  // Sem o hub, o status não muda daqui.
  expect(within(wi).queryByRole("button", { name: /Mudar status/ })).not.toBeInTheDocument();
  // A interrompida anterior leva à Recuperação; a sessão do painel não se repete.
  const prev = within(wi).getAllByRole("listitem");
  expect(prev).toHaveLength(1);
  expect(prev[0]).toHaveTextContent("interrompida · saída 137");

  const pr = within(panel()).getByRole("region", { name: "PR" });
  await user.click(await within(pr).findByRole("button", { name: "Abrir PR #77 no GitHub (aberto)" }));
  expect(api.opened).toEqual(["https://github.com/deep-ion-ai/ctower-agents/pull/77"]);

  const dec = within(panel()).getByRole("region", { name: "Decisões" });
  const cards = within(dec).getAllByRole("listitem");
  expect(cards.map((c) => within(c).getByText(/Nova|Revisada|Antiga/).textContent)).toEqual([
    "Nova autônoma",
    "Revisada",
    "Antiga orientada",
  ]);
  expect(within(cards[0]).getByText("autônoma")).toBeInTheDocument();
  expect(within(cards[0]).getByText("pendente de revisão")).toBeInTheDocument();
  expect(within(cards[1]).getByText("revisada no PR #70")).toBeInTheDocument();
  expect(within(cards[2]).getByText("orientada")).toBeInTheDocument();
  expect(within(dec).getByText("1 pendente")).toBeInTheDocument();

  const evs = within(panel()).getByRole("region", { name: "Eventos" });
  await waitFor(() => expect(within(evs).getAllByRole("listitem")).toHaveLength(5));
  expect(api.callsTo("events.list")[0].params).toEqual({ work_item_id: 32, limit: 6 });
  await user.click(within(evs).getByRole("button", { name: "Mostrar mais" }));
  await waitFor(() => expect(within(evs).getAllByRole("listitem")).toHaveLength(10));
  expect(api.callsTo("events.list")[1].params).toEqual({ work_item_id: 32, before: 8, limit: 6 });
  await user.click(within(evs).getByRole("button", { name: "Mostrar mais" }));
  await waitFor(() => expect(within(evs).getAllByRole("listitem")).toHaveLength(12));
  // Página com menos que o pedido: acabou.
  expect(within(evs).queryByRole("button", { name: "Mostrar mais" })).not.toBeInTheDocument();
  expect(within(evs).getByText("evento_1")).toBeInTheDocument();

  // Revisar decisões leva à tela de revisão do item.
  await user.click(within(dec).getByRole("button", { name: "Revisar decisões" }));
  expect(await screen.findByRole("tab", { name: /Revisão/ })).toBeInTheDocument();
});

test("a sessão interrompida anterior vai para a Recuperação", async () => {
  const { user } = await openTab();
  await user.click(within(bar()).getByRole("button", { name: "Contexto" }));
  await user.click(await within(panel()).findByRole("button", { name: /Retomar pela Recuperação a sessão claude/ }));
  expect(await screen.findByText("Nenhuma sessão interrompida.")).toBeInTheDocument();
});

test("sem PR, Criar PR abre o compare da branch", async () => {
  const api = base().on("review.get", review({ prs: [] }));
  const { user } = await openTab(api);
  await user.click(within(bar()).getByRole("button", { name: "Contexto" }));
  await user.click(await within(panel()).findByRole("button", { name: "Criar PR ↗" }));
  expect(api.opened).toEqual([
    "https://github.com/deep-ion-ai/ctower-agents/compare/develop...feat%2F32-ui-terminal-embutido?expand=1",
  ]);
});

test("aberto ao montar quando o estado guardado diz aberto; Fechar contexto e o atalho alternam", async () => {
  localStorage.setItem("ctower.terminal.context", "1");
  await openTab();
  expect(panel()).toBeInTheDocument();
  fireEvent.click(within(panel()).getByRole("button", { name: "Fechar contexto" }));
  expect(queryPanel()).not.toBeInTheDocument();
  expect(localStorage.getItem("ctower.terminal.context")).toBe("0");
  // Ctrl+Alt+B (fora do macOS), também com o foco no console.
  fireEvent.keyDown(screen.getAllByRole("region", { name: "Terminal da sessão" })[0], {
    key: "b",
    code: "KeyB",
    ctrlKey: true,
    altKey: true,
  });
  expect(panel()).toBeInTheDocument();
  fireEvent.keyDown(window, { key: "b", code: "KeyB", ctrlKey: true, altKey: true });
  expect(queryPanel()).not.toBeInTheDocument();
});

test("paleta: Sessão: Mostrar contexto", async () => {
  const { user } = await openTab();
  await user.click(screen.getByRole("button", { name: "Abrir paleta de comandos" }));
  const paleta = screen.getByRole("dialog", { name: "Paleta de comandos" });
  await user.type(within(paleta).getByRole("combobox"), "Sessão: Mostrar contexto");
  await user.keyboard("{Enter}");
  expect(panel()).toBeInTheDocument();
});

test("recarrega por push do mesmo item, com debounce de 1 s; push de outro item não recarrega", async () => {
  const { api, user } = await openTab();
  await user.click(within(bar()).getByRole("button", { name: "Contexto" }));
  await waitFor(() => expect(api.callsTo("review.get")).toHaveLength(1));
  await waitFor(() => expect(api.callsTo("events.list")).toHaveLength(1));

  act(() => {
    api.push("wsl", "work_item_status", { work_item_id: 32, status: "em_revisao" });
    api.push("wsl", "needs_human", { work_item_id: 32, session_id: S.id, message: "?" });
    // `session_exited` só traz a sessão.
    api.push("wsl", "session_exited", { session_id: S.id, exit_code: 0 });
    api.push("wsl", "work_item_status", { work_item_id: 40, status: "em_revisao" });
  });
  // Ainda dentro do debounce: nada.
  await new Promise((r) => setTimeout(r, CONTEXT_RELOAD_MS / 2));
  expect(api.callsTo("review.get")).toHaveLength(1);
  // Um recarregamento só pelos três pushes.
  await waitFor(() => expect(api.callsTo("review.get")).toHaveLength(2), { timeout: 2 * CONTEXT_RELOAD_MS });
  expect(api.callsTo("events.list")).toHaveLength(2);
  await new Promise((r) => setTimeout(r, CONTEXT_RELOAD_MS + 200));
  expect(api.callsTo("review.get")).toHaveLength(2);
});

test("push: o mesmo item, uma sessão dele ou o sync do projeto; de outro ambiente ou de outro tipo, não", () => {
  const item = { workItemId: 32, projectId: 1, sessions: new Set(["s1"]) };
  const p = (env: string, event: string, data: object) => pushConcerns({ env, event, data }, "wsl", item);
  expect(p("wsl", "work_item_status", { work_item_id: 32 })).toBe(true);
  expect(p("wsl", "session_state", { work_item_id: 32, session_id: "x" })).toBe(true);
  expect(p("wsl", "session_exited", { session_id: "s1" })).toBe(true);
  expect(p("wsl", "needs_human", { work_item_id: 32 })).toBe(true);
  expect(p("wsl", "sync_status", { project_id: 1 })).toBe(true);
  expect(p("wsl", "sync_status", { project_id: 2 })).toBe(false);
  expect(p("wsl", "work_item_status", { work_item_id: 40 })).toBe(false);
  expect(p("mac", "work_item_status", { work_item_id: 32 })).toBe(false);
  expect(p("wsl", "session_started", { work_item_id: 32 })).toBe(false);
});

test("no split, segue a coluna com o foco", async () => {
  const { user } = await openTab();
  await user.click(screen.getByRole("treeitem", { name: /#40 revisar guia rodando/ }));
  await user.click(within(bar()).getByRole("button", { name: "Dividir à direita" }));
  // Coluna 1 = #40, coluna 2 = #32.
  await user.click(within(bar()).getByRole("button", { name: "Contexto" }));
  expect(within(panel()).getByText("#40 revisar guia")).toBeInTheDocument();
  expect(within(bar("Sessão à direita")).getByRole("button", { name: "Contexto" })).toHaveAttribute("aria-pressed", "false");

  // Clicar no console da direita leva o painel para a #32.
  fireEvent.pointerDown(document.querySelector('.term-pane[data-col="2"]')!);
  expect(within(panel()).getByText("#32 ui: terminal embutido")).toBeInTheDocument();
  expect(await within(panel()).findByText("Nova autônoma")).toBeInTheDocument();
  expect(within(bar("Sessão à direita")).getByRole("button", { name: "Contexto" })).toHaveAttribute("aria-pressed", "true");

  // "Contexto" da coluna 1 com o painel na 2: passa para a 1 (não fecha).
  await user.click(within(bar()).getByRole("button", { name: "Contexto" }));
  expect(within(panel()).getByText("#40 revisar guia")).toBeInTheDocument();
  // De novo na mesma coluna: fecha.
  await user.click(within(bar()).getByRole("button", { name: "Contexto" }));
  expect(queryPanel()).not.toBeInTheDocument();
});

test("sessão do projeto (#0): sem a seção PR; a pasta é o clone principal", async () => {
  const W0 = workItem({ gh_number: 0, id: 70, title: null, branch: null, worktree_path: null });
  const S0 = session({ id: "eeeeeeee-0000-0000-0000-000000000000", work_item_id: 70 });
  const api = base()
    .on("dashboard.get", dashboard({ live_sessions: [S0], sync: [sync({ last_ok_at: NOW })] }, [W0]))
    .on("review.get", review({ work_item: W0, decisions: [decision({ uid: "z", title: "Do planejamento", work_item_id: 70 })] }));
  localStorage.setItem("ctower.terminal.context", "1");
  await openTab(api, /Sessão do projeto/);
  expect(await within(panel()).findByText("Do planejamento")).toBeInTheDocument();
  expect(within(panel()).queryByRole("region", { name: "PR" })).not.toBeInTheDocument();
  const wi = within(panel()).getByRole("region", { name: "Work item" });
  expect(within(wi).getByText(project().repo_path)).toBeInTheDocument();
  expect(within(wi).getByText(/clone principal/)).toBeInTheDocument();
  expect(within(wi).queryByText("Branch")).not.toBeInTheDocument();
  expect(api.callsTo("review.get")[0].params).toEqual({ work_item_id: 70 });
  expect(api.callsTo("events.list")[0].params).toMatchObject({ work_item_id: 70 });
});

test("não aparece em aba de shell", async () => {
  const SH: Shell = {
    id: "cccccccc-0000-0000-0000-000000000001",
    work_item_id: 32,
    cwd: WI.worktree_path!,
    program: "/bin/zsh",
    started_at: NOW - 3 * MIN,
    busy: false,
  };
  const api = base().on("shell.open", SH);
  localStorage.setItem("ctower.terminal.context", "1");
  const { user } = await openTab(api);
  expect(panel()).toBeInTheDocument();
  await user.click(within(bar()).getByRole("button", { name: "Abrir shell no worktree" }));
  expect(await screen.findByRole("region", { name: "Shell" })).toBeInTheDocument();
  expect(queryPanel()).not.toBeInTheDocument();
  // De volta à sessão, o painel volta (o estado segue aberto).
  await user.click(screen.getByRole("tab", { name: /#32 ui: terminal embutido/ }));
  expect(panel()).toBeInTheDocument();
});

test("ambiente sem events.list (-32601): aviso para atualizar, e o resto do painel funciona", async () => {
  const api = base().on("events.list", () => {
    throw new Error("método desconhecido: events.list");
  });
  const { user } = await openTab(api);
  await user.click(within(bar()).getByRole("button", { name: "Contexto" }));
  const evs = within(panel()).getByRole("region", { name: "Eventos" });
  expect(await within(evs).findByText(EVENTS_OLD)).toBeInTheDocument();
  expect(within(evs).queryByRole("alert")).not.toBeInTheDocument();
  expect(await within(panel()).findByText("Nova autônoma")).toBeInTheDocument();
  expect(isUnknownMethod("método indisponível pelo canal: events.list")).toBe(true);
  expect(isUnknownMethod({ code: -32601 } as unknown as string)).toBe(false);
  expect(isUnknownMethod("work_item não encontrado: 32")).toBe(false);
});

test("com o ambiente falando com o hub: o seletor de status da tela Projeto e as sessões do hub", async () => {
  const info: EnvInfo = {
    protocol: "2.9",
    version: "0.1.0",
    commit: "",
    hostname: "ct",
    worktrees_root: "/w",
    env_id: "daemon-wsl",
    env_name: "wsl",
    hub_sync: {
      pending: 0,
      oldest_pending_at: null,
      last_attempt_at: NOW,
      last_ok_at: NOW,
      last_error: null,
      failures: 0,
      syncing: false,
      prune: { retention_days: 30, pruned_work_items: 0, last_run_at: null, last_pruned: 0, last_error: null },
    },
    hub_config: { ...HUB_CONFIG_UNLINKED, linked: true },
    update: null,
    remote_control: { available: false, reason: null },
    docs: { root: null, reachable: false },
  };
  const other: HubSession = {
    id: "dddddddd-0000-0000-0000-000000000001",
    env_id: "daemon-mac",
    work_item_id: 132,
    tool: "claude",
    tool_session_id: null,
    cwd: "/x",
    state: "exited",
    exit_code: 0,
    started_at: NOW - 900 * MIN,
    last_event_at: NOW - 800 * MIN,
    ended_at: NOW - 800 * MIN,
  } as HubSession;
  const history = { sessions: [other], events: [], decisions: [], prs: [], locations: [], sub_work_items: [] } as unknown as HubHistory;
  const api = base()
    .on("env.info", info)
    .withHub()
    .onHub("hub.info", { protocol: "1.0", version: "0.3.0", commit: "", name: "mac-mini", config_rev: 7, backup: null })
    .onHub("hub.dashboard.get", HUB_DASHBOARD)
    .onHub("hub.status", { warnings: [] })
    .onHub("hub.projects.list", [])
    .onHub("hub.envs.list", [])
    .onHub("hub.workitem.history", history)
    .onHub("hub.workitem.set_status", {});
  const { user } = await openTab(api);
  await user.click(within(bar()).getByRole("button", { name: "Contexto" }));
  const wi = within(panel()).getByRole("region", { name: "Work item" });
  await user.click(await within(wi).findByRole("button", { name: /Mudar status/ }));
  const menu = within(wi).getByRole("group", { name: "Mudar status" });
  await user.type(within(menu).getByLabelText("Motivo (opcional)"), "esperando revisão");
  await user.click(within(menu).getByRole("button", { name: "em revisão" }));
  await waitFor(() =>
    expect(api.hubCallsTo("hub.workitem.set_status")[0].params).toEqual({
      owner: "deep-ion-ai",
      repo: "ctower-agents",
      number: 32,
      status: "em_revisao",
      reason: "esperando revisão",
    }),
  );
  expect(api.hubCallsTo("hub.workitem.history")[0].params).toEqual({
    owner: "deep-ion-ai",
    repo: "ctower-agents",
    number: 32,
    limit: 1,
  });
  // A sessão do outro ambiente vem do hub; a interrompida, do dashboard.
  await waitFor(() => expect(within(wi).getAllByRole("listitem")).toHaveLength(2));
  expect(within(wi).getByText(/encerrada/)).toBeInTheDocument();
});
