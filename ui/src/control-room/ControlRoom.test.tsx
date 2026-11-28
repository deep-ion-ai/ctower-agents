import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { App } from "../App";
import { FakeDaemonApi } from "../api/fake";
import type { Dashboard } from "../protocol/gen";
import { HUB_DASHBOARD, dashboard as base, hubStatus, MIN, session, sync, workItem } from "../test/fixtures";
import { openHealth, showSessions } from "../test/control-room";
import { attentionSummary, loadCollapsed } from "./AttentionList";
import { todayText } from "./ControlRoom";
import { healthSummary } from "./Health";
import { envsText } from "./Infra";
import { PEEK_INTERVAL_MS } from "./TileGrid";
import type { AttentionItem } from "./attention";

const NOW = Date.now();
const A = session({ id: "aaaaaaaa-0000-0000-0000-000000000001", work_item_id: 31, started_at: NOW - 47 * MIN, last_event_at: NOW - 12 * MIN });
const B = session({
  id: "bbbbbbbb-0000-0000-0000-000000000002",
  work_item_id: 32,
  started_at: NOW - 84 * MIN,
  model: "opus",
  mode: "subagents",
});

// Sync recente (relógio real): não entra na lista como atrasado.
const dashboard: typeof base = (d = {}, items = []) =>
  base({ sync: [sync({ last_ok_at: NOW, last_attempt_at: NOW })], ...d }, items);

function full(): Dashboard {
  return dashboard({ live_sessions: [A, B] }, [
    workItem({ gh_number: 31, status: "aguardando_voce", title: "ui: Sala de controle" }),
    workItem({ gh_number: 32, status: "em_execucao", title: "ui: terminal embutido" }),
    workItem({ gh_number: 28, status: "em_revisao", title: "ct-server: lacunas", pending_decisions: 3 }),
    workItem({ gh_number: 19, status: "concluido", title: "migração 2", closed_at: NOW }),
  ]);
}

function setup(d: Dashboard) {
  const api = new FakeDaemonApi()
    .on("dashboard.get", d)
    .on("session.peek", (p: { session_id: string }) => ({
      data: p.session_id === A.id ? "\x1b[1mDo you want to proceed?\x1b[0m\r\n❯ 1. Yes\r\n" : "● Running vitest…\r\n",
    }));
  render(<App api={api} />);
  return api;
}

const chips = () => screen.getByRole("list", { name: "Contadores" });
const chip = (name: string | RegExp) => within(chips()).getByRole("button", { name });
const lista = () => screen.findByRole("region", { name: "Precisa de você" });
const lateral = () => screen.getByRole("complementary", { name: "Projetos e contas" });
const editor = () => screen.getByRole("tabpanel", { name: "Área de editor" });

test("tudo andando: sem lista de atenção; os chips dizem o que roda", async () => {
  const d = dashboard({ live_sessions: [B] }, [workItem({ gh_number: 32, status: "em_execucao" })]);
  setup(d);
  expect(await within(chips()).findByRole("button", { name: "1 rodando" })).toBeInTheDocument();
  expect(chip("Nada pede você")).toBeInTheDocument();
  expect(screen.queryByRole("region", { name: "Precisa de você" })).not.toBeInTheDocument();
});

test("tela inicial: topo, lista compacta e Infraestrutura; sem grade nem fim do dia (#357)", async () => {
  setup(full());
  const faixa = await lista();
  const linha = within(faixa).getByText("Aguardando você").closest("li")!;
  // Uma linha por item: tipo, projeto, item e detalhe, há quanto tempo e a ação.
  expect(within(linha).getByText("ctower-agents")).toBeInTheDocument();
  expect(within(linha).getByText("#31 ui: Sala de controle")).toBeInTheDocument();
  expect(within(linha).getByText("Permissão ou pergunta pendente no terminal")).toBeInTheDocument();
  expect(within(linha).getByText("há 12 min")).toBeInTheDocument();
  expect(within(linha).getByRole("button", { name: "Responder" })).toBeInTheDocument();
  expect(within(faixa).getByText("1 item · mais antigo há 12 min")).toBeInTheDocument();

  // Topo: título, data, projetos e os contadores (sem hub, sem épicos nem piloto).
  expect(screen.getByRole("heading", { level: 1, name: "Sala de controle" })).toBeInTheDocument();
  expect(screen.getByText(`${todayText(NOW)} · 1 projeto`)).toBeInTheDocument();
  expect(within(chips()).getAllByRole("button").map((b) => b.textContent)).toEqual([
    "1 pede você",
    "1 rodando",
    "1 concluída hoje",
    "Uso por sessão",
  ]);
  expect(screen.getByRole("button", { description: "Saúde do hub e dos ambientes" })).toHaveTextContent("1 ambiente conectado");

  // Rodapé Infraestrutura (sem hub: só os ambientes) com o link para a manutenção.
  const infra = screen.getByRole("contentinfo", { name: "Infraestrutura" });
  expect(infra).toHaveTextContent("ctower-agents (WSL) conectado");
  expect(within(infra).getByRole("button", { name: "Ver manutenção ›" })).toBeInTheDocument();

  // A grade e o fim do dia ficam na tela do projeto (#356) e nas visões.
  expect(screen.queryByText(/Sessões vivas/)).not.toBeInTheDocument();
  expect(screen.queryByText(/Em revisão ·/)).not.toBeInTheDocument();
  expect(screen.queryByText(/Do you want to proceed\?/)).not.toBeInTheDocument();
  // A lateral tem só projetos e contas: nada de visões, hub ou ambientes.
  expect(within(lateral()).queryByRole("button", { name: "Rodando" })).not.toBeInTheDocument();
  expect(within(lateral()).queryByText(/conectado ·/)).not.toBeInTheDocument();
  expect(
    within(lateral()).getByRole("button", { name: "ctower-agents", description: "1 em execução, 1 aguardando" }),
  ).toBeInTheDocument();
});

test("Rodando: tiles com estado e as últimas linhas; o tile e Responder abrem o terminal", async () => {
  setup(full());
  const user = userEvent.setup();
  await lista();
  await user.click(chip("1 rodando"));
  expect(chip("1 rodando")).toHaveAttribute("aria-pressed", "true");
  expect(screen.getByRole("heading", { level: 1, name: "Rodando" })).toBeInTheDocument();
  expect(screen.getByText("Rodando · 1")).toBeInTheDocument();
  expect(screen.getByText("rodando")).toBeInTheDocument();
  expect(await screen.findByText(/Running vitest/)).toBeInTheDocument();
  expect(screen.getByText(/claude · Opus · subagents · 1 h/)).toBeInTheDocument();
  // A que espera você não está em "Rodando", nem a lista.
  expect(screen.queryByRole("button", { name: /ui: Sala de controle/ })).not.toBeInTheDocument();
  expect(screen.queryByRole("region", { name: "Precisa de você" })).not.toBeInTheDocument();

  await user.click(screen.getByRole("button", { name: /ui: terminal embutido/ }));
  expect(screen.getByRole("button", { name: "Terminal", description: "1 sessão" })).toHaveAttribute("aria-current", "page");
  await user.click(screen.getByRole("button", { name: "Sala de controle" }));
  await user.click(await within(await lista()).findByRole("button", { name: "Responder" }));
  expect(screen.getByRole("button", { name: "Terminal", description: "2 sessões" })).toBeInTheDocument();
});

test("Precisa de você: a lista aberta e os tiles de quem espera, com o pedido no fim do terminal", async () => {
  setup(full());
  const user = userEvent.setup();
  await lista();
  await user.click(chip("1 pede você"));
  expect(screen.getByRole("heading", { level: 1, name: "Precisa de você" })).toBeInTheDocument();
  expect(await screen.findByText(/Do you want to proceed\?/)).toBeInTheDocument();
  expect(screen.getByText("aguardando você")).toBeInTheDocument();
  expect(screen.getByText(/claude · modelo padrão · native · 47 min/)).toBeInTheDocument();
  expect(screen.queryByRole("button", { name: /ui: terminal embutido/ })).not.toBeInTheDocument();
  // Na visão, a lista não recolhe.
  expect(within(await lista()).queryByRole("button", { name: "Recolher" })).not.toBeInTheDocument();
  // Clicar de novo no chip volta à tela inicial.
  await user.click(chip("1 pede você"));
  expect(screen.getByRole("heading", { level: 1, name: "Sala de controle" })).toBeInTheDocument();
  expect(screen.queryByText(/Do you want to proceed\?/)).not.toBeInTheDocument();
});

test("Concluídas hoje: o fim do dia leva à revisão; a lateral leva ao projeto", async () => {
  setup(full());
  const user = userEvent.setup();
  await lista();
  await user.click(chip("1 concluída hoje"));
  expect(screen.getByText("3 decisões pendentes")).toBeInTheDocument();
  expect(screen.getByText("Concluídas hoje · 1")).toBeInTheDocument();
  expect(screen.getByText("migração 2")).toBeInTheDocument();
  expect(screen.queryByRole("region", { name: "Precisa de você" })).not.toBeInTheDocument();
  // "‹ Sala de controle" volta à tela inicial.
  await user.click(screen.getByRole("button", { name: "‹ Sala de controle" }));
  expect(screen.queryByText("migração 2")).not.toBeInTheDocument();
  await user.click(chip("1 concluída hoje"));

  await user.click(screen.getByRole("button", { name: /ct-server: lacunas/ }));
  expect(screen.getByRole("button", { name: "Revisão" })).toHaveAttribute("aria-current", "page");
  await user.click(screen.getByRole("button", { name: "Sala de controle" }));
  await user.click(await within(lateral()).findByRole("button", { name: "ctower-agents" }));
  expect(within(screen.getByRole("navigation", { name: "Telas" })).getByRole("button", { name: "Projetos" })).toHaveAttribute("aria-current", "page");
});

test("ambiente desconectado: na lista, no botão de saúde e na Infraestrutura", async () => {
  const api = setup(dashboard());
  await within(screen.getByRole("contentinfo", { name: "Infraestrutura" })).findByText("ctower-agents (WSL) conectado");
  act(() => api.setStatus("wsl", false, "daemon inacessível"));
  expect(within(await lista()).getByText("ctower-agents (WSL) desconectado")).toBeInTheDocument();
  expect(within(await lista()).getByText("daemon inacessível")).toBeInTheDocument();
  const health = screen.getByRole("button", { description: "Saúde do hub e dos ambientes" });
  expect(health).toHaveTextContent("0 de 1 ambiente conectado");
  expect(health).toHaveAttribute("data-warn", "true");
  const panel = await openHealth();
  expect(within(panel).getByText(/^desconectado/)).toBeInTheDocument();
  expect(within(screen.getByRole("contentinfo", { name: "Infraestrutura" })).getByText("ctower-agents (WSL) desconectado")).toHaveAttribute(
    "data-warn",
    "true",
  );
});

test("peek só roda com a grade na tela", async () => {
  vi.useFakeTimers({ shouldAdvanceTime: true });
  try {
    const api = setup(full());
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
    await lista();
    // Na tela inicial não há grade: nada de peek.
    expect(api.callsTo("session.peek")).toHaveLength(0);
    await showSessions(user, "pede");
    await screen.findByText(/Do you want to proceed\?/);
    const antes = api.callsTo("session.peek").length;
    await act(async () => {
      vi.advanceTimersByTime(PEEK_INTERVAL_MS);
    });
    await waitFor(() => expect(api.callsTo("session.peek").length).toBeGreaterThan(antes));
    await user.click(screen.getByRole("button", { name: "Configurações" }));
    const depois = api.callsTo("session.peek").length;
    await act(async () => {
      vi.advanceTimersByTime(3 * PEEK_INTERVAL_MS);
    });
    expect(api.callsTo("session.peek").length).toBe(depois);
  } finally {
    vi.useRealTimers();
  }
});

function hubApi(warnings = [
  { kind: "backup_same_device", message: "backup_dir /x está no mesmo disco do hub.db" },
  { kind: "push_quarantine", message: "12 linha(s) de push do projeto o/r em quarentena" },
]) {
  return new FakeDaemonApi()
    .on("dashboard.get", dashboard())
    .withHub()
    .onHub("hub.dashboard.get", HUB_DASHBOARD)
    .onHub("hub.status", hubStatus({ warnings }));
}

test("avisos do hub e hub fora: na lista, no botão de saúde e nos detalhes; levam à tela do hub", async () => {
  const api = hubApi();
  render(<App api={api} />);
  const user = userEvent.setup();
  const faixa = await lista();
  expect(await within(faixa).findByText("2 itens · mais antigo agora")).toBeInTheDocument();
  expect(within(faixa).getByText("Backup no mesmo disco do hub")).toBeInTheDocument();
  expect(within(faixa).getByText("backup_dir /x está no mesmo disco do hub.db")).toBeInTheDocument();
  expect(within(faixa).getByText("Linhas em quarentena no hub")).toBeInTheDocument();

  const health = screen.getByRole("button", { description: "Saúde do hub e dos ambientes" });
  await waitFor(() => expect(health).toHaveTextContent("Hub e 1 ambiente conectado · 2 avisos"));
  const panel = await openHealth();
  expect(within(panel).getByRole("button", { name: "Hub: conectado" })).toBeInTheDocument();
  const avisos = within(panel).getByRole("list", { name: "Avisos da infraestrutura" });
  expect(within(avisos).getByText("Linhas em quarentena no hub")).toBeInTheDocument();
  // Backup na Infraestrutura, com o snapshot do hub.
  const infra = screen.getByRole("contentinfo", { name: "Infraestrutura" });
  expect(infra).toHaveTextContent(/Hub · snapshot há/);
  expect(within(infra).getByText("Backup no mesmo disco do hub")).toHaveAttribute("data-warn", "true");

  act(() => api.setHubStatus(false, "hub inacessível: conexão recusada"));
  expect(await within(faixa).findByText("Hub indisponível")).toBeInTheDocument();
  expect(within(faixa).getByText(/hub inacessível: conexão recusada\. As sessões seguem nos ambientes/)).toBeInTheDocument();
  // Também nos detalhes da saúde, que seguem abertos.
  expect(within(panel).getByText(/hub inacessível: conexão recusada/)).toBeInTheDocument();
  expect(health).toHaveTextContent("Hub indisponível · 1 ambiente conectado");
  // Sem o hub, os avisos dele somem (não há como saber se continuam valendo).
  expect(screen.queryByText("Backup no mesmo disco do hub")).not.toBeInTheDocument();
  expect(infra).toHaveTextContent("Hub · indisponível");
  await user.click(within(faixa).getByRole("button", { name: "Ver hub" }));
  const nav = screen.getByRole("navigation", { name: "Telas" });
  expect(within(nav).getByRole("button", { name: /^Hub/ })).toHaveAttribute("aria-current", "page");
});

test("com hub: chips de épicos em execução e do piloto abrem as seções do hub; sem os métodos, somem", async () => {
  const api = hubApi([])
    .onHub("hub.epic_run.list", [{ id: 1 }, { id: 2 }])
    .onHub("hub.autopilot.status", {
      settings: { paused: false },
      accounts: [],
      projects: [1, 2, 3].map((n) => ({ owner: "acme", repo: `app-${n}`, queue: [] })),
      sessions: [],
      parked: [],
      events: [],
    });
  render(<App api={api} />);
  const user = userEvent.setup();
  expect(await within(chips()).findByRole("button", { name: "2 épicos em execução" })).toBeInTheDocument();
  const piloto = await within(chips()).findByRole("button", { name: "Piloto ligado · 3 projetos" });
  expect(api.hubCallsTo("hub.epic_run.list")[0].params).toEqual({});
  await user.click(piloto);
  expect(await screen.findByRole("tab", { name: /Painel do piloto/, selected: true })).toBeInTheDocument();
  await user.click(screen.getByRole("button", { name: "Sala de controle" }));
  await user.click(await within(chips()).findByRole("button", { name: "2 épicos em execução" }));
  expect(await screen.findByRole("tab", { name: /Execução de épicos/, selected: true })).toBeInTheDocument();
});

test("sem hub os chips e o botão de saúde mostram só o que vem dos ambientes", async () => {
  const api = setup(full());
  await lista();
  expect(within(chips()).queryByRole("button", { name: /épicos? em execução|Piloto/ })).not.toBeInTheDocument();
  expect(screen.getByRole("button", { description: "Saúde do hub e dos ambientes" })).toHaveTextContent(/^1 ambiente conectado$/);
  expect(screen.getByRole("contentinfo", { name: "Infraestrutura" })).not.toHaveTextContent(/snapshot/);
  expect(api.hubCallsTo("hub.epic_run.list")).toHaveLength(0);
});

test("Recolher fica lembrado", async () => {
  setup(full());
  const user = userEvent.setup();
  const faixa = await lista();
  await user.click(within(faixa).getByRole("button", { name: "Recolher" }));
  expect(within(faixa).getByRole("button", { name: "Mostrar" })).toHaveAttribute("aria-expanded", "false");
  expect(within(faixa).queryByRole("button", { name: "Responder" })).not.toBeInTheDocument();
  // O resumo continua à vista.
  expect(within(faixa).getByText("1 item · mais antigo há 12 min")).toBeInTheDocument();
  expect(loadCollapsed()).toBe(true);
  // Volta recolhida ao reabrir a tela.
  await user.click(screen.getByRole("button", { name: "Configurações" }));
  await user.click(screen.getByRole("button", { name: "Sala de controle" }));
  expect(within(await lista()).getByRole("button", { name: "Mostrar" })).toBeInTheDocument();
  await user.click(within(await lista()).getByRole("button", { name: "Mostrar" }));
  expect(loadCollapsed()).toBe(false);
  expect(within(await lista()).getByRole("button", { name: "Responder" })).toBeInTheDocument();
});

test("sessão do gestor: rótulo próprio no tile e no terminal, que leva à revisão das decisões dele", async () => {
  const G = session({ id: "99999999-0000-0000-0000-000000000099", work_item_id: 900, cwd: "/u/.local/share/ctower/gestor" });
  const api = setup(dashboard({ live_sessions: [G], gestor_work_item_id: 900 }));
  api.on("review.get", { work_item: workItem({ id: 900, gh_number: 0 }), prs: [], decisions: [] });
  const user = userEvent.setup();
  await showSessions(user);
  const tile = await screen.findByRole("button", { name: /Gestor de projetos/ });
  expect(tile).toHaveTextContent("hub inteiro");
  await user.click(tile);
  const bar = await screen.findByRole("region", { name: "Sessão" });
  expect(bar).toHaveTextContent("Gestor de projetos");
  expect(screen.getByRole("contentinfo", { name: "Barra de status" })).toHaveTextContent("/u/.local/share/ctower/gestor");
  await user.click(within(bar).getByRole("button", { name: "Mais ações" }));
  expect(screen.queryByRole("menuitem", { name: "Abrir PR" })).toBeNull();
  await user.click(screen.getByRole("menuitem", { name: "Revisar decisões do gestor" }));
  expect(await screen.findByRole("heading", { level: 1, name: "Gestor de projetos" })).toBeInTheDocument();
  await waitFor(() => expect(api.callsTo("review.get")[0].params).toEqual({ work_item_id: 900 }));
});

test("os chips ficam no topo da área de editor; a lateral do shell não duplica (#316)", async () => {
  setup(full());
  const user = userEvent.setup();
  await lista();
  expect(within(screen.getByRole("region", { name: "Painel lateral" })).getByRole("complementary", { name: "Projetos e contas" })).toBeInTheDocument();
  expect(within(editor()).queryByRole("complementary")).not.toBeInTheDocument();
  expect(within(editor()).getByRole("list", { name: "Contadores" })).toBeInTheDocument();

  await user.click(chip("Uso por sessão"));
  expect(chip("Uso por sessão")).toHaveAttribute("aria-pressed", "true");
  expect(chip("1 rodando")).toHaveAttribute("aria-pressed", "false");
  expect(screen.getByRole("heading", { level: 1, name: "Uso por sessão" })).toBeInTheDocument();
  await user.click(chip("Uso por sessão"));
  expect(screen.getByRole("heading", { level: 1, name: "Sala de controle" })).toBeInTheDocument();
});

test("agrupar por projeto e densidade da grade", async () => {
  const DOCS = { ...base().projects[0].project, id: 2, name: "docs-site", gh_repo: "docs-site" };
  const C = session({ id: "cccccccc-0000-0000-0000-000000000003", work_item_id: 50 });
  const d = full();
  d.live_sessions.push(C);
  d.projects.push({ project: DOCS, work_items: [workItem({ id: 50, gh_number: 50, project_id: 2, title: "guia" })] });
  setup(d);
  const user = userEvent.setup();
  await lista();
  // Os controles da grade só aparecem nas visões com grade.
  expect(screen.queryByRole("button", { name: "Agrupar por projeto" })).not.toBeInTheDocument();
  await showSessions(user);
  await screen.findByText("Rodando · 2");
  const agrupar = screen.getByRole("button", { name: "Agrupar por projeto" });
  expect(agrupar).toHaveAttribute("aria-pressed", "false");
  await user.click(agrupar);
  expect(agrupar).toHaveAttribute("aria-pressed", "true");
  const ct = screen.getByRole("region", { name: "Projeto ctower-agents" });
  expect(within(ct).getAllByRole("button")).toHaveLength(1);
  expect(within(screen.getByRole("region", { name: "Projeto docs-site" })).getByRole("button", { name: /guia/ })).toBeInTheDocument();

  const densidade = screen.getByRole("group", { name: "Densidade da grade" });
  await user.click(within(densidade).getByRole("button", { name: "3 colunas" }));
  expect(within(densidade).getByRole("button", { name: "3 colunas" })).toHaveAttribute("aria-pressed", "true");
  expect(within(ct).getByRole("list")).toHaveAttribute("data-density", "3");
});

test("linha de quem espera você mostra o pedido do agente e Responder abre o terminal com a faixa", async () => {
  const api = setup(full());
  const user = userEvent.setup();
  const faixa = await lista();
  expect(within(faixa).getByText("Permissão ou pergunta pendente no terminal")).toBeInTheDocument();
  act(() => api.push("wsl", "needs_human", { session_id: A.id, work_item_id: 31, message: "Claude needs your permission to use Bash" }));
  expect(await within(faixa).findByText("Claude needs your permission to use Bash")).toBeInTheDocument();
  await user.click(within(faixa).getByRole("button", { name: "Responder" }));
  const pedido = await screen.findByRole("group", { name: "Pedido pendente" });
  expect(within(pedido).getByText("Claude needs your permission to use Bash")).toBeInTheDocument();
});

test("eventos ficam no painel inferior do shell", async () => {
  const api = setup(full());
  const user = userEvent.setup();
  await lista();
  act(() => api.push("wsl", "needs_human", { session_id: A.id, gh_number: 31 }));
  await user.keyboard("{Control>}j{/Control}");
  const painel = await screen.findByRole("region", { name: "Painel inferior" });
  expect(within(painel).getByText("sessão aguardando você #31")).toBeInTheDocument();
});

describe("funções do topo", () => {
  const item = (p: Partial<AttentionItem>): AttentionItem => ({
    key: "k",
    kind: "hub",
    env: "hub",
    label: "Hub",
    title: "t",
    detail: "d",
    since: NOW,
    actionLabel: "Ver hub",
    action: { to: "hub" },
    ...p,
  });

  test("saúde: hub e ambientes, avisos de infraestrutura; sem hub, só os ambientes", () => {
    const up = { connected: true };
    const down = { connected: false };
    expect(healthSummary({ hub: { connected: true, known: true }, envs: [up, up, up], items: [] })).toEqual({
      text: "Hub e 3 ambientes conectados",
      warnings: 0,
      warn: false,
    });
    expect(
      healthSummary({
        hub: { connected: true, known: true },
        envs: [up, down],
        items: [item({ key: "hub:x:0" }), item({ kind: "docs", key: "docs:wsl" }), item({ kind: "waiting", key: "w" })],
      }),
    ).toEqual({ text: "Hub conectado · 1 de 2 ambientes conectados", warnings: 2, warn: true });
    expect(healthSummary({ hub: { connected: false, known: true }, envs: [up], items: [item({ key: "hub:down" })] })).toEqual({
      text: "Hub indisponível · 1 ambiente conectado",
      warnings: 0,
      warn: true,
    });
    expect(healthSummary({ hub: { connected: false, known: false }, envs: [up], items: [] }).text).toBe(
      "Conectando ao hub · 1 ambiente conectado",
    );
    expect(healthSummary({ hub: null, envs: [up, up], items: [] }).text).toBe("2 ambientes conectados");
    expect(healthSummary({ hub: null, envs: [], items: [] }).text).toBe("Nenhum ambiente");
  });

  test("Infraestrutura: conectados e caídos à parte", () => {
    expect(envsText([{ name: "a", connected: true }, { name: "b", connected: true }, { name: "c", connected: false }])).toEqual({
      up: "a · b conectados",
      down: "c desconectado",
    });
    expect(envsText([{ name: "a", connected: null }])).toEqual({ up: null, down: null });
  });

  test("resumo da lista e data do topo", () => {
    expect(attentionSummary([item({ since: NOW - 60 * MIN }), item({ since: NOW - 5 * MIN })], NOW)).toBe(
      "2 itens · mais antigo há 1 h",
    );
    expect(todayText(new Date(2026, 10, 27, 10).getTime())).toBe("Sexta, 27 de novembro");
    expect(todayText(new Date(2026, 10, 29, 10).getTime())).toBe("Domingo, 29 de novembro");
  });
});
