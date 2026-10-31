import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { App } from "../App";
import { FakeDaemonApi } from "../api/fake";
import type { Shell } from "../protocol/gen";
import { terms } from "../test/fakeXterm";
import { dashboard, MIN, session, sync, workItem } from "../test/fixtures";
import { groupSessions } from "./SessionTree";
import { closeCheck, resetShellStore, shellEndText } from "./shells";

// Shell livre no worktree (#423). O xterm é o fake de ../test/fakeXterm.

const NOW = Date.now();
const S = session({ id: "bbbbbbbb-0000-0000-0000-000000000002", work_item_id: 32, started_at: NOW - 84 * MIN });
const WI = workItem({ gh_number: 32, title: "ui: terminal embutido", branch: "feat/32-ui-terminal-embutido" });
const SH: Shell = {
  id: "cccccccc-0000-0000-0000-000000000001",
  work_item_id: 32,
  cwd: WI.worktree_path!,
  program: "/bin/zsh",
  started_at: NOW - 3 * MIN,
  busy: false,
};

beforeEach(() => resetShellStore());

const tablist = () => screen.getByRole("tablist", { name: "Abas abertas" });

async function openSession(api = new FakeDaemonApi(), shells: Shell[] = []) {
  api
    .on("dashboard.get", dashboard({ live_sessions: [S], sync: [sync({ last_ok_at: NOW })] }, [WI]))
    .on("session.peek", { data: "" })
    .on("shell.list", () => shells)
    .on("shell.open", SH)
    .on("shell.close", {});
  terms.length = 0;
  render(<App api={api} />);
  const user = userEvent.setup();
  await user.click(await screen.findByRole("button", { name: /ui: terminal embutido/ }));
  await waitFor(() => expect(api.attached.has(S.id)).toBe(true));
  return { api, user };
}

async function openShell() {
  const ctx = await openSession();
  await ctx.user.click(screen.getByRole("button", { name: "Abrir shell no worktree" }));
  await waitFor(() => expect(ctx.api.attached.has(SH.id)).toBe(true));
  return ctx;
}

test("abrir pelo cabeçalho: aba \"Shell · #32\", cabeçalho enxuto e o shell na árvore", async () => {
  const { api } = await openShell();
  expect(api.callsTo("shell.open")[0].params).toMatchObject({ work_item_id: 32 });
  expect(within(tablist()).getByRole("tab", { name: "Shell · #32" })).toHaveAttribute("aria-selected", "true");
  const barra = screen.getByRole("region", { name: "Shell" });
  expect(within(barra).getByText(SH.cwd)).toBeInTheDocument();
  expect(within(barra).getByText("zsh")).toBeInTheDocument();
  expect(within(barra).getByText("3 min")).toBeInTheDocument();
  // Sem selo de estado, Remote Control, PR nem Encerrar sessão.
  expect(within(barra).queryByText("rodando")).not.toBeInTheDocument();
  expect(within(barra).queryByRole("button", { name: "Encerrar sessão" })).not.toBeInTheDocument();
  expect(within(barra).getByRole("button", { name: "Abrir no VS Code" })).toBeEnabled();
  // Na árvore, filho da linha da sessão do item, um nível adentro.
  const item = screen.getByRole("treeitem", { name: "Shell zsh · #32" });
  expect(item).toHaveAttribute("aria-level", "3");
  expect(item).toHaveAttribute("aria-current", "true");
  // Entrada e saída pelo id do shell.
  act(() => api.output(SH.id, "% \r\n", true));
  const t = terms.find((x) => x.screen.includes("%"))!;
  t.typed("ls\r");
  expect(api.inputs).toEqual([{ env: "wsl", session: SH.id, data: "ls\r" }]);
  // Cada clique, um shell novo.
  api.on("shell.open", { ...SH, id: "cccccccc-0000-0000-0000-000000000002" });
  await userEvent.click(screen.getByRole("button", { name: "Dividir à direita" }));
  await userEvent.click(within(screen.getByRole("region", { name: "Sessão à direita" })).getByRole("button", { name: "Abrir shell no worktree" }));
  await waitFor(() => expect(api.callsTo("shell.open")).toHaveLength(2));
  expect(screen.getAllByRole("treeitem", { name: "Shell zsh · #32" })).toHaveLength(2);
});

test("fechar a aba com um comando rodando pede confirmação e só então chama shell.close", async () => {
  const { api, user } = await openShell();
  api.on("shell.list", [{ ...SH, busy: true }]);
  await user.click(within(tablist()).getByRole("button", { name: "Fechar aba Shell · #32" }));
  const confirma = await screen.findByRole("group", { name: "Confirmar fechamento" });
  expect(confirma).toHaveTextContent("Há um comando rodando no shell. Fechar encerra o processo.");
  expect(api.callsTo("shell.close")).toEqual([]);
  await user.click(within(confirma).getByRole("button", { name: "Cancelar" }));
  expect(within(tablist()).getByRole("tab", { name: "Shell · #32" })).toBeInTheDocument();
  await user.click(screen.getByRole("button", { name: "Fechar shell" }));
  await user.click(within(await screen.findByRole("group", { name: "Confirmar fechamento" })).getByRole("button", { name: "Fechar" }));
  await waitFor(() => expect(within(tablist()).queryByRole("tab", { name: "Shell · #32" })).not.toBeInTheDocument());
  expect(api.callsTo("shell.close").map((c) => c.params)).toEqual([{ shell_id: SH.id }]);
  // A sessão do agente volta a ser a ativa.
  expect(screen.getByRole("region", { name: "Sessão" })).toBeInTheDocument();
});

test("fechar sem comando rodando fecha direto", async () => {
  const { api, user } = await openShell();
  await user.click(screen.getByRole("button", { name: "Fechar shell" }));
  await waitFor(() => expect(api.callsTo("shell.close")).toHaveLength(1));
  expect(within(tablist()).queryByRole("tab", { name: "Shell · #32" })).not.toBeInTheDocument();
  expect(screen.queryByRole("group", { name: "Confirmar fechamento" })).not.toBeInTheDocument();
});

test("fim do processo: \"shell encerrado (código n)\", Abrir novo shell e Fechar", async () => {
  const { api, user } = await openShell();
  act(() => api.push("wsl", "shell_exited", { shell_id: SH.id, code: 0 }));
  const barra = screen.getByRole("region", { name: "Shell" });
  expect(await within(barra).findByText("shell encerrado (código 0)")).toBeInTheDocument();
  expect(screen.queryByRole("treeitem", { name: "Shell zsh · #32" })).not.toBeInTheDocument();
  api.on("shell.open", { ...SH, id: "cccccccc-0000-0000-0000-000000000003" });
  await user.click(within(barra).getByRole("button", { name: "Abrir novo shell" }));
  await waitFor(() => expect(api.attached.has("cccccccc-0000-0000-0000-000000000003")).toBe(true));
  expect(within(tablist()).getAllByRole("tab", { name: "Shell · #32" })).toHaveLength(2);
  // Fechar o encerrado não consulta nem encerra nada.
  await user.click(within(tablist()).getAllByRole("tab", { name: "Shell · #32" })[0]);
  await user.click(within(screen.getByRole("region", { name: "Shell" })).getByRole("button", { name: "Fechar" }));
  expect(within(tablist()).getAllByRole("tab", { name: "Shell · #32" })).toHaveLength(1);
  expect(api.callsTo("shell.close")).toEqual([]);
});

test("app recarregado: a árvore lista o shell vivo e o clique reabre a aba com replay", async () => {
  const { api, user } = await openSession(new FakeDaemonApi(), [SH]);
  const item = await screen.findByRole("treeitem", { name: "Shell zsh · #32" });
  await user.click(item);
  await waitFor(() => expect(api.attached.has(SH.id)).toBe(true));
  expect(within(tablist()).getByRole("tab", { name: "Shell · #32" })).toHaveAttribute("aria-selected", "true");
  act(() => api.output(SH.id, "historico do shell\r\n", true));
  expect(terms.some((t) => t.screen.includes("historico do shell"))).toBe(true);
  expect(api.callsTo("shell.open")).toEqual([]);
});

test("recusa do daemon aparece na tela", async () => {
  const { api, user } = await openSession();
  api.on("shell.open", () => {
    throw new Error("limite de 8 shells abertos neste ambiente; feche um antes");
  });
  await user.click(screen.getByRole("button", { name: "Abrir shell no worktree" }));
  expect(await screen.findByRole("alert")).toHaveTextContent(
    "Não foi possível abrir o shell: limite de 8 shells abertos neste ambiente; feche um antes",
  );
});

test("sessão do projeto (#0): \"Shell · sessão do projeto\" e o aviso do clone principal", async () => {
  const api = new FakeDaemonApi();
  const W0 = workItem({ gh_number: 0, title: null, branch: null, worktree_path: null });
  const S0 = session({ id: "dddddddd-0000-0000-0000-000000000001", work_item_id: 0 });
  api
    .on("dashboard.get", dashboard({ live_sessions: [S0], sync: [sync({ last_ok_at: NOW })] }, [W0]))
    .on("session.peek", { data: "" })
    .on("shell.list", [])
    .on("shell.open", { ...SH, work_item_id: 0, cwd: "/home/user/ctower-agents" });
  render(<App api={api} />);
  const user = userEvent.setup();
  await user.click(await screen.findByRole("button", { name: /sessão do projeto/i }));
  await user.click(await screen.findByRole("button", { name: "Abrir shell no worktree" }));
  const barra = await screen.findByRole("region", { name: "Shell" });
  expect(within(barra).getByText("Shell · sessão do projeto")).toBeInTheDocument();
  expect(within(barra).getByText("clone principal")).toBeInTheDocument();
  expect(screen.getByRole("treeitem", { name: "Shell zsh · sessão do projeto" })).toBeInTheDocument();
});

test("árvore: o shell de um item sem sessão fica sob a linha do próprio item", () => {
  const d = dashboard({}, [WI]);
  const groups = groupSessions([{ id: "wsl", name: "WSL" }], { wsl: d }, [], () => ({}), {}, { wsl: [SH] });
  expect(groups).toHaveLength(1);
  const [row] = groups[0].sessions;
  expect(row).toMatchObject({ tab: null, title: "#32 ui: terminal embutido", state: { text: "sem sessão" } });
  expect(row.shells).toEqual([{ tab: { env: "wsl", shell: SH.id, workItemId: 32 }, label: "zsh · #32" }]);
});

test("busy e texto do fim", () => {
  expect(closeCheck([SH], SH.id)).toBe("close");
  expect(closeCheck([{ ...SH, busy: true }], SH.id)).toBe("busy");
  expect(closeCheck([], SH.id)).toBe("close");
  expect(closeCheck(null, SH.id)).toBe("unknown");
  expect(shellEndText(2)).toBe("shell encerrado (código 2)");
  expect(shellEndText(null)).toBe("shell encerrado (sinal)");
  expect(shellEndText()).toBe("shell encerrado");
});

test("cores das abas (#73): data-kind por tipo e o ícone de terminal só nas abas de sessão", async () => {
  await openShell();
  const etab = (name: string | RegExp) => within(tablist()).getByRole("tab", { name }).closest(".etab")!;
  const sessao = etab(/ui: terminal embutido/);
  const shell = etab("Shell · #32");
  expect(sessao).toHaveAttribute("data-kind", "terminal");
  expect(shell).toHaveAttribute("data-kind", "shell");
  // Ícone entre o ponto de estado e o título, na cor do texto (currentColor).
  const icone = sessao.querySelector('svg[data-icon="terminal"]')!;
  expect(icone).toHaveAttribute("stroke", "currentColor");
  expect(icone.previousElementSibling).toHaveClass("sp-dot");
  expect(icone.nextElementSibling).toHaveClass("etab-title");
  expect(shell.querySelector('svg[data-icon="prompt"]')).not.toBeNull();
  expect(shell.querySelector('svg[data-icon="terminal"]')).toBeNull();
  // As demais abas: tipo "screen" e nenhum ícone novo.
  const telas = [...tablist().querySelectorAll(".etab")].filter((e) => e !== sessao && e !== shell);
  expect(telas.length).toBeGreaterThan(0);
  for (const t of telas) {
    expect(t).toHaveAttribute("data-kind", "screen");
    expect(t.querySelector("svg[data-icon]:not([data-icon=close]):not([data-icon=pin])")).toBeNull();
  }
});
