import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { App } from "../App";
import { FakeDaemonApi } from "../api/fake";
import type { Dashboard } from "../protocol/gen";
import { HUB_DASHBOARD, dashboard as base, hubStatus, MIN, session, sync, workItem } from "../test/fixtures";
import { PEEK_INTERVAL_MS } from "./TileGrid";

const NOW = Date.now();
const A = session({ id: "aaaaaaaa-0000-0000-0000-000000000001", work_item_id: 31, started_at: NOW - 47 * MIN, last_event_at: NOW - 12 * MIN });
const B = session({
  id: "bbbbbbbb-0000-0000-0000-000000000002",
  work_item_id: 32,
  started_at: NOW - 84 * MIN,
  model: "opus",
  mode: "subagents",
});

// Sync recente (relógio real): não entra na faixa como atrasado.
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

test("tudo andando: sem faixa de atenção", async () => {
  const d = dashboard({ live_sessions: [B] }, [workItem({ gh_number: 32, status: "em_execucao" })]);
  setup(d);
  expect(await screen.findByText("Sessões vivas · 1")).toBeInTheDocument();
  expect(screen.queryByText(/Precisa de você ·/)).not.toBeInTheDocument();
});

test("faixa, tiles, lateral e rodapé", async () => {
  setup(full());
  const faixa = (await screen.findByText("Precisa de você · 1")).closest("section")!;
  expect(within(faixa).getByText("Aguardando você")).toBeInTheDocument();
  expect(within(faixa).getByText("ctower-agents #31 ui: Sala de controle")).toBeInTheDocument();
  expect(within(faixa).getByText("há 12 min")).toBeInTheDocument();

  // Estado escrito em cada tile, e as últimas linhas do terminal sem ANSI.
  expect(screen.getByText("aguardando você")).toBeInTheDocument();
  expect(screen.getByText("rodando")).toBeInTheDocument();
  expect(await screen.findByText(/Do you want to proceed\?/)).toBeInTheDocument();
  expect(screen.getByText(/claude · modelo padrão · native · 47 min/)).toBeInTheDocument();
  expect(screen.getByText(/claude · Opus · subagents · 1 h/)).toBeInTheDocument();

  const lateral = screen.getByRole("complementary", { name: "Ambientes e projetos" });
  expect(within(lateral).getByText("conectado · 2 sessões")).toBeInTheDocument();
  expect(
    within(lateral).getByRole("button", { name: "ctower-agents", description: "1 em execução, 1 aguardando" }),
  ).toBeInTheDocument();

  expect(screen.getByText("3 decisões pendentes")).toBeInTheDocument();
  expect(screen.getByText("Concluídas hoje · 1")).toBeInTheDocument();
  expect(screen.getByText("migração 2")).toBeInTheDocument();
});

test("tile e cartão abrem o terminal da sessão", async () => {
  setup(full());
  const user = userEvent.setup();
  await user.click(await screen.findByRole("button", { name: /ui: terminal embutido/ }));
  expect(screen.getByRole("button", { name: "Terminal", description: "1 sessão" })).toHaveAttribute("aria-current", "page");
  await user.click(screen.getByRole("button", { name: "Sala de controle" }));
  await user.click(await screen.findByRole("button", { name: "Responder" }));
  expect(screen.getByRole("button", { name: "Terminal", description: "2 sessões" })).toBeInTheDocument();
});

test("rodapé leva à revisão e a lateral ao projeto", async () => {
  setup(full());
  const user = userEvent.setup();
  await user.click(await screen.findByRole("button", { name: /ct-server: lacunas/ }));
  expect(screen.getByRole("button", { name: "Revisão" })).toHaveAttribute("aria-current", "page");
  await user.click(screen.getByRole("button", { name: "Sala de controle" }));
  await user.click(await screen.findByRole("button", { name: "ctower-agents" }));
  expect(within(screen.getByRole("navigation", { name: "Telas" })).getByRole("button", { name: "Projetos" })).toHaveAttribute("aria-current", "page");
});

test("ambiente desconectado aparece na faixa e na lateral", async () => {
  const api = setup(dashboard());
  await screen.findByText("Sessões vivas · 0");
  act(() => api.setStatus("wsl", false, "daemon inacessível"));
  expect(await screen.findByText("ctower-agents (WSL) desconectado")).toBeInTheDocument();
  expect(screen.getByText("daemon inacessível")).toBeInTheDocument();
  expect(screen.getByText(/^desconectado/)).toBeInTheDocument();
});

test("peek só roda com a Sala de controle na tela", async () => {
  vi.useFakeTimers({ shouldAdvanceTime: true });
  try {
    const api = setup(full());
    await screen.findByText(/Do you want to proceed\?/);
    const antes = api.callsTo("session.peek").length;
    await act(async () => {
      vi.advanceTimersByTime(PEEK_INTERVAL_MS);
    });
    await waitFor(() => expect(api.callsTo("session.peek").length).toBeGreaterThan(antes));
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
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

test("avisos do hub e hub fora entram na faixa e levam à tela do hub", async () => {
  const api = new FakeDaemonApi()
    .on("dashboard.get", dashboard())
    .withHub()
    .onHub("hub.dashboard.get", HUB_DASHBOARD)
    .onHub(
      "hub.status",
      hubStatus({
        warnings: [
          { kind: "backup_same_device", message: "backup_dir /x está no mesmo disco do hub.db" },
          { kind: "push_quarantine", message: "12 linha(s) de push do projeto o/r em quarentena" },
        ],
      }),
    );
  render(<App api={api} />);
  const user = userEvent.setup();
  const faixa = (await screen.findByText("Precisa de você · 2")).closest("section")!;
  expect(within(faixa).getByText("Backup no mesmo disco do hub")).toBeInTheDocument();
  expect(within(faixa).getByText("backup_dir /x está no mesmo disco do hub.db")).toBeInTheDocument();
  expect(within(faixa).getByText("Linhas em quarentena no hub")).toBeInTheDocument();

  act(() => api.setHubStatus(false, "hub inacessível: conexão recusada"));
  expect(await within(document.body).findByText("Hub indisponível")).toBeInTheDocument();
  expect(screen.getByText(/hub inacessível: conexão recusada\. As sessões seguem nos ambientes/)).toBeInTheDocument();
  // Sem o hub, os avisos dele somem (não há como saber se continuam valendo).
  expect(screen.queryByText("Backup no mesmo disco do hub")).not.toBeInTheDocument();
  // As sessões do ambiente seguem: o tile e a lateral continuam.
  expect(screen.getByText("Sessões vivas · 0")).toBeInTheDocument();
  await user.click(screen.getByRole("button", { name: "Ver hub" }));
  const nav = screen.getByRole("navigation", { name: "Telas" });
  expect(within(nav).getByRole("button", { name: /^Hub/ })).toHaveAttribute("aria-current", "page");
});

test("sessão do gestor: rótulo próprio no tile e no terminal, que leva à revisão das decisões dele", async () => {
  const G = session({ id: "99999999-0000-0000-0000-000000000099", work_item_id: 900, cwd: "/u/.local/share/ctower/gestor" });
  const api = setup(dashboard({ live_sessions: [G], gestor_work_item_id: 900 }));
  api.on("review.get", { work_item: workItem({ id: 900, gh_number: 0 }), prs: [], decisions: [] });
  const user = userEvent.setup();
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

const lateral = () => screen.getByRole("complementary", { name: "Ambientes e projetos" });
const editor = () => screen.getByRole("tabpanel", { name: "Área de editor" });

test("painel lateral do shell: visões com contadores filtram a área de editor; sem barra lateral duplicada (#316)", async () => {
  setup(full());
  const user = userEvent.setup();
  await screen.findByText("Precisa de você · 1");
  expect(within(screen.getByRole("region", { name: "Painel lateral" })).getByRole("complementary", { name: "Ambientes e projetos" })).toBeInTheDocument();
  expect(within(editor()).queryByRole("complementary")).not.toBeInTheDocument();
  const rodando = within(lateral()).getByRole("button", { name: "Rodando", description: "1 sessão" });
  expect(within(lateral()).getByRole("button", { name: "Precisa de você", description: "1 item" })).toBeInTheDocument();
  expect(within(lateral()).getByRole("button", { name: "Concluídas hoje", description: "1 concluída" })).toBeInTheDocument();

  await user.click(rodando);
  expect(rodando).toHaveAttribute("aria-current", "true");
  expect(screen.getByRole("heading", { level: 1, name: "Rodando" })).toBeInTheDocument();
  expect(screen.getByText("Rodando · 1")).toBeInTheDocument();
  expect(screen.queryByText("Precisa de você · 1")).not.toBeInTheDocument();
  expect(screen.queryByRole("button", { name: /ui: Sala de controle/ })).not.toBeInTheDocument();
  expect(screen.getByRole("button", { name: /ui: terminal embutido/ })).toBeInTheDocument();

  await user.click(within(lateral()).getByRole("button", { name: "Precisa de você" }));
  expect(screen.getByText("Precisa de você · 1")).toBeInTheDocument();
  expect(screen.getByText("Aguardando você · 1")).toBeInTheDocument();
  expect(screen.queryByRole("button", { name: /ui: terminal embutido/ })).not.toBeInTheDocument();

  await user.click(within(lateral()).getByRole("button", { name: "Concluídas hoje" }));
  expect(screen.getByText("Concluídas hoje · 1")).toBeInTheDocument();
  expect(screen.queryByText(/Sessões vivas/)).not.toBeInTheDocument();
  // Clicar de novo na visão volta a tudo.
  await user.click(within(lateral()).getByRole("button", { name: "Concluídas hoje" }));
  expect(screen.getByRole("heading", { level: 1, name: "Sala de controle" })).toBeInTheDocument();
  expect(screen.getByText("Sessões vivas · 2")).toBeInTheDocument();
});

test("agrupar por projeto e densidade da grade", async () => {
  const DOCS = { ...base().projects[0].project, id: 2, name: "docs-site", gh_repo: "docs-site" };
  const C = session({ id: "cccccccc-0000-0000-0000-000000000003", work_item_id: 50 });
  const d = full();
  d.live_sessions.push(C);
  d.projects.push({ project: DOCS, work_items: [workItem({ id: 50, gh_number: 50, project_id: 2, title: "guia" })] });
  setup(d);
  const user = userEvent.setup();
  await screen.findByText("Sessões vivas · 3");
  const agrupar = screen.getByRole("button", { name: "Agrupar por projeto" });
  expect(agrupar).toHaveAttribute("aria-pressed", "false");
  await user.click(agrupar);
  expect(agrupar).toHaveAttribute("aria-pressed", "true");
  const ct = screen.getByRole("region", { name: "Projeto ctower-agents" });
  expect(within(ct).getAllByRole("button")).toHaveLength(2);
  expect(within(screen.getByRole("region", { name: "Projeto docs-site" })).getByRole("button", { name: /guia/ })).toBeInTheDocument();

  const densidade = screen.getByRole("group", { name: "Densidade da grade" });
  await user.click(within(densidade).getByRole("button", { name: "3 colunas" }));
  expect(within(densidade).getByRole("button", { name: "3 colunas" })).toHaveAttribute("aria-pressed", "true");
  expect(within(ct).getByRole("list")).toHaveAttribute("data-density", "3");
});

test("cartão de quem espera você mostra o pedido do agente e Responder abre o terminal com a faixa", async () => {
  const api = setup(full());
  const user = userEvent.setup();
  const faixa = (await screen.findByText("Precisa de você · 1")).closest("section")!;
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
  await screen.findByText("Precisa de você · 1");
  act(() => api.push("wsl", "needs_human", { session_id: A.id, gh_number: 31 }));
  await user.keyboard("{Control>}j{/Control}");
  const painel = await screen.findByRole("region", { name: "Painel inferior" });
  expect(within(painel).getByText("sessão aguardando você #31")).toBeInTheDocument();
});
