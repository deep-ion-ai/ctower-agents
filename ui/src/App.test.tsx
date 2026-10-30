import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { App } from "./App";
import { FakeDaemonApi } from "./api/fake";
import { dashboard, project, session } from "./test/fixtures";

function setup(d = dashboard()) {
  const api = new FakeDaemonApi().on("dashboard.get", d).on("sync.now", []);
  render(<App api={api} />);
  return api;
}

test("navega entre as telas pela barra de atividades", async () => {
  setup();
  const user = userEvent.setup();
  const nav = screen.getByRole("navigation", { name: "Telas" });
  expect(within(nav).getByRole("button", { name: "Sala de controle" })).toHaveAttribute("aria-current", "page");
  for (const [tela, titulo] of [
    ["Configurações", "Configurações"],
    ["Revisão", "Revisão"],
    ["Projetos", "ctower-agents"],
  ]) {
    await user.click(within(nav).getByRole("button", { name: tela }));
    expect(screen.getByRole("heading", { name: titulo })).toBeInTheDocument();
    expect(within(nav).getByRole("button", { name: tela })).toHaveAttribute("aria-current", "page");
  }
  // Sem sessão aberta, o Terminal mostra o estado vazio.
  await user.click(within(nav).getByRole("button", { name: "Terminal" }));
  expect(screen.getByText("Nenhuma sessão aberta. Abra uma pela Sala de controle.")).toBeInTheDocument();
});

test("contador de interrompidas no item Recuperação", async () => {
  setup(dashboard({ interrupted_sessions: [session({ state: "interrupted" })] }));
  const nav = screen.getByRole("navigation", { name: "Telas" });
  await waitFor(() =>
    expect(within(nav).getByRole("button", { name: "Recuperação" })).toHaveAccessibleDescription("1 interrompida"),
  );
});

test("daemon incompatível mostra aviso", async () => {
  const api = setup();
  act(() => api.setStatus("wsl", false, "daemon com protocolo incompatível (3.0)"));
  const alert = await screen.findByRole("alert");
  expect(alert).toHaveTextContent("incompatível (3.0)");
  // Daemon mais novo: quem atualiza é o app.
  expect(within(alert).getByRole("button", { name: "Atualizar o app" })).toBeInTheDocument();
});

test("daemon defasado: o aviso atualiza o ambiente, com force se recusado", async () => {
  const api = setup();
  api.on("env.update", (p: { force: boolean }) => {
    if (!p.force) throw new Error("Recusado: 2 sessão(ões) viva(s) seriam encerradas");
    return {};
  });
  act(() => api.setStatus("wsl", false, "daemon com protocolo incompatível (1.2)"));
  const alert = await screen.findByRole("alert");
  const user = userEvent.setup();
  await user.click(within(alert).getByRole("button", { name: /Atualizar ctower-agents/ }));
  expect(await within(alert).findByText(/Recusado: 2 sessão/)).toBeInTheDocument();
  await user.click(within(alert).getByRole("button", { name: "Atualizar e encerrar sessões" }));
  expect(await within(alert).findByText(/Atualização iniciada/)).toBeInTheDocument();
  expect(api.callsTo("env.update").map((c) => c.params)).toEqual([{ force: false }, { force: true }]);
});

test("Sincronizar agora chama sync.now nos ambientes conectados", async () => {
  const api = setup();
  await waitFor(() => expect(api.callsTo("dashboard.get").length).toBeGreaterThan(0));
  await userEvent.setup().click(screen.getByRole("button", { name: "Sincronizar agora" }));
  await waitFor(() => expect(api.callsTo("sync.now")).toHaveLength(1));
});

test("push dispara recarga do dashboard com debounce", async () => {
  const api = setup();
  await waitFor(() => expect(api.callsTo("dashboard.get")).toHaveLength(1));
  for (let i = 0; i < 5; i++) api.push("wsl", "session_state", {});
  await waitFor(() => expect(api.callsTo("dashboard.get")).toHaveLength(2));
  await new Promise((r) => setTimeout(r, 400));
  expect(api.callsTo("dashboard.get")).toHaveLength(2);
});

test("exceção ao renderizar uma tela mostra o erro, não a janela em branco", async () => {
  const log = vi.spyOn(console, "error").mockImplementation(() => {});
  const api = new FakeDaemonApi()
    .on("dashboard.get", dashboard({}, []))
    .on("env.info", { protocol: "2.0", version: "0.1.0", hostname: "ct", worktrees_root: "/w" })
    .on("tree.get", { project: project(), sync: null, roots: [{ issue: null }], untracked: [] });
  render(<App api={api} initial={{ name: "project" }} />);
  expect(await screen.findByRole("alert")).toHaveTextContent("Esta tela falhou ao abrir");
  // O menu continua funcionando.
  expect(screen.getByRole("button", { name: "Sala de controle" })).toBeInTheDocument();
  log.mockRestore();
});
