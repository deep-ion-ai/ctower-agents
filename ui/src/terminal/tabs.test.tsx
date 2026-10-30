import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { App } from "../App";
import { FakeDaemonApi } from "../api/fake";
import type { Shell } from "../protocol/gen";
import { terms } from "../test/fakeXterm";
import { dashboard, MIN, project, session, sync, workItem } from "../test/fixtures";
import { groupSessions } from "./SessionTree";
import { resetShellStore } from "./shells";

// Muitas abas e a sessão do projeto (#427; spec da #87, §5 e §6).

const NOW = Date.now();
const DOCS = project({ id: 2, name: "docs-site", gh_repo: "docs-site" });
const WI = workItem({ gh_number: 32, title: "ui: terminal embutido" });
const WI2 = workItem({ gh_number: 40, title: "revisar guia" });
const WI3 = workItem({ gh_number: 44, project_id: 2, title: "docs novas" });
const W0 = workItem({ gh_number: 0, id: 70, title: null, branch: null, worktree_path: null });
const S = session({ id: "bbbbbbbb-0000-0000-0000-000000000002", work_item_id: 32, started_at: NOW - 10 * MIN });
const S2 = session({ id: "bbbbbbbb-0000-0000-0000-000000000003", work_item_id: 40, started_at: NOW - MIN });
const SD = session({ id: "bbbbbbbb-0000-0000-0000-000000000004", work_item_id: 44 });
const S0 = session({ id: "bbbbbbbb-0000-0000-0000-000000000000", work_item_id: 70 });
const SH: Shell = {
  id: "cccccccc-0000-0000-0000-000000000001",
  work_item_id: 32,
  cwd: WI.worktree_path!,
  program: "/bin/zsh",
  started_at: NOW - 3 * MIN,
  busy: true,
};

beforeEach(() => resetShellStore());

const tablist = () => screen.getByRole("tablist", { name: "Abas abertas" });
const tabNames = () => within(tablist()).getAllByRole("tab").map((t) => t.textContent);
const dash = (live = [S, S2, SD, S0]) =>
  dashboard({
    projects: [
      { project: project(), work_items: [WI, WI2, W0] },
      { project: DOCS, work_items: [WI3] },
    ],
    live_sessions: live,
    sync: [sync({ last_ok_at: NOW })],
  });

async function setup(shells: Shell[] = []) {
  const api = new FakeDaemonApi();
  api
    .on("dashboard.get", dash())
    .on("session.peek", { data: "" })
    .on("session.kill", {})
    .on("recovery.list", [])
    .on("shell.list", () => shells)
    .on("shell.open", SH)
    .on("shell.close", {});
  terms.length = 0;
  render(<App api={api} />);
  const user = userEvent.setup();
  await user.click(await screen.findByRole("button", { name: /ui: terminal embutido/ }));
  await waitFor(() => expect(api.attached.has(S.id)).toBe(true));
  const open = async (name: RegExp, id: string) => {
    await user.click(screen.getByRole("treeitem", { name }));
    await waitFor(() => expect(api.attached.has(id)).toBe(true));
  };
  return { api, user, open };
}

const tabMenu = (name: string | RegExp) => {
  fireEvent.contextMenu(within(tablist()).getByRole("tab", { name }));
  return screen.getByRole("menu", { name: "Ações da aba" });
};

test("ordem por projeto, tooltip com projeto e ambiente e Fechar as do projeto", async () => {
  const { api, user, open } = await setup();
  await open(/#44 docs novas/, SD.id);
  await open(/#40 revisar guia/, S2.id);
  // A #40 é do ctower-agents: entra ao lado da #32, antes da do docs-site.
  expect(tabNames()).toEqual(["Sala de controle", "#32 ui: terminal embutido", "#40 revisar guia", "#44 docs novas"]);
  expect(within(tablist()).getByRole("tab", { name: "#44 docs novas" })).toHaveAttribute(
    "title",
    "#44 docs novas\ndocs-site · ctower-agents (WSL)",
  );
  const menu = tabMenu("#40 revisar guia");
  expect(within(menu).getAllByRole("menuitem").map((m) => m.textContent)).toEqual([
    "Fechar",
    "Fechar outras",
    "Fechar à direita",
    "Fechar encerradas",
    "Fechar as do projeto ctower-agents",
  ]);
  expect(within(menu).getByRole("menuitem", { name: "Fechar encerradas" })).toBeDisabled();
  await user.click(within(menu).getByRole("menuitem", { name: "Fechar as do projeto ctower-agents" }));
  expect(tabNames()).toEqual(["Sala de controle", "#44 docs novas"]);
  // Fechar a aba não encerra a sessão do agente.
  expect(api.callsTo("session.kill")).toHaveLength(0);
  // A aba de tela não tem projeto.
  const m2 = tabMenu("Sala de controle");
  expect(within(m2).getByRole("menuitem", { name: "Fechar as do projeto" })).toBeDisabled();
});

test("Fechar outras e à direita pelo menu da aba", async () => {
  const { user, open } = await setup();
  await open(/#40 revisar guia/, S2.id);
  await open(/#44 docs novas/, SD.id);
  await user.click(within(tabMenu("#40 revisar guia")).getByRole("menuitem", { name: "Fechar à direita" }));
  expect(tabNames()).toEqual(["Sala de controle", "#32 ui: terminal embutido", "#40 revisar guia"]);
  expect(within(tablist()).getByRole("tab", { name: "#40 revisar guia" })).toHaveAttribute("aria-selected", "true");
  await user.click(within(tabMenu("#32 ui: terminal embutido")).getByRole("menuitem", { name: "Fechar outras" }));
  expect(tabNames()).toEqual(["#32 ui: terminal embutido"]);
  expect(within(tablist()).getByRole("tab", { name: "#32 ui: terminal embutido" })).toHaveAttribute("aria-selected", "true");
});

test("Fechar encerradas: a encerrada sai (com o título do item até lá), a interrompida recuperável fica", async () => {
  const { api, user, open } = await setup();
  await open(/#40 revisar guia/, S2.id);
  await open(/#44 docs novas/, SD.id);
  const side = screen.getByRole("complementary", { name: "Sessões por projeto" });
  expect(within(side).getByRole("button", { name: "Fechar encerradas" })).toBeDisabled();
  // A #40 sai normalmente; a #44 é interrompida (recuperável).
  api.on("dashboard.get", dash([S]));
  act(() => api.push("wsl", "session_exited", { session_id: S2.id, exit_code: 0 }));
  act(() => api.push("wsl", "session_exited", { session_id: SD.id, exit_code: 137, state: "interrupted" }));
  await waitFor(() => expect(api.callsTo("dashboard.get").length).toBeGreaterThan(1));
  // Fora do dashboard, a aba segue com o título do item (antes: "Sessão").
  expect(tabNames()).toEqual(["Sala de controle", "#32 ui: terminal embutido", "#40 revisar guia", "#44 docs novas"]);
  await waitFor(() => expect(within(side).getByRole("button", { name: "Fechar encerradas" })).toBeEnabled());
  await user.click(within(side).getByRole("button", { name: "Fechar encerradas" }));
  expect(tabNames()).toEqual(["Sala de controle", "#32 ui: terminal embutido", "#44 docs novas"]);
  expect(within(side).getByRole("button", { name: "Fechar encerradas" })).toBeDisabled();
});

test("árvore: Fechar as abas deste projeto pelo menu do grupo", async () => {
  const { user, open } = await setup();
  await open(/#44 docs novas/, SD.id);
  const ct = screen.getByRole("treeitem", { name: /^ctower-agents, / });
  fireEvent.contextMenu(within(ct).getByText("ctower-agents"));
  const menu = screen.getByRole("menu", { name: "Ações do projeto ctower-agents" });
  await user.click(within(menu).getByRole("menuitem", { name: "Fechar as abas deste projeto" }));
  expect(tabNames()).toEqual(["Sala de controle", "#44 docs novas"]);
  // Pelo teclado (Shift+F10) no grupo, sem abas dele: desabilitado.
  act(() => ct.focus());
  await user.keyboard("{Shift>}{F10}{/Shift}");
  const m2 = screen.getByRole("menu", { name: "Ações do projeto ctower-agents" });
  expect(within(m2).getByRole("menuitem", { name: "Fechar as abas deste projeto" })).toBeDisabled();
});

test("fechar várias com um shell ocupado pede uma confirmação só e chama shell.close", async () => {
  const { api, user } = await setup([SH]);
  await user.click(screen.getByRole("button", { name: "Abrir shell no worktree" }));
  await waitFor(() => expect(api.attached.has(SH.id)).toBe(true));
  await user.click(within(tabMenu("#32 ui: terminal embutido")).getByRole("menuitem", { name: "Fechar outras" }));
  const ask = await screen.findByRole("group", { name: "Confirmar fechamento das abas" });
  expect(ask).toHaveTextContent("1 shell tem um comando rodando. Fechar encerra os processos.");
  // Cancelar mantém tudo.
  await user.click(within(ask).getByRole("button", { name: "Cancelar" }));
  expect(tabNames()).toContain("Shell · #32");
  await user.click(within(tabMenu("#32 ui: terminal embutido")).getByRole("menuitem", { name: "Fechar outras" }));
  await user.click(
    within(await screen.findByRole("group", { name: "Confirmar fechamento das abas" })).getByRole("button", { name: "Fechar 2 abas" }),
  );
  await waitFor(() => expect(tabNames()).toEqual(["#32 ui: terminal embutido"]));
  expect(api.callsTo("shell.close")[0].params).toEqual({ shell_id: SH.id });
});

test("sem shell ocupado, fecha direto (o shell vivo recebe shell.close)", async () => {
  const { api, user } = await setup([{ ...SH, busy: false }]);
  await user.click(screen.getByRole("button", { name: "Abrir shell no worktree" }));
  await waitFor(() => expect(api.attached.has(SH.id)).toBe(true));
  await user.click(within(tabMenu("#32 ui: terminal embutido")).getByRole("menuitem", { name: "Fechar à direita" }));
  await waitFor(() => expect(tabNames()).toEqual(["Sala de controle", "#32 ui: terminal embutido"]));
  expect(screen.queryByRole("group", { name: "Confirmar fechamento das abas" })).not.toBeInTheDocument();
  expect(api.callsTo("shell.close")).toHaveLength(1);
});

test("abas que não cabem: o ⌄ abre a lista filtrável com o projeto e ativa pelo teclado", async () => {
  const sw = Object.getOwnPropertyDescriptor(HTMLElement.prototype, "scrollWidth");
  const cw = Object.getOwnPropertyDescriptor(HTMLElement.prototype, "clientWidth");
  Object.defineProperty(HTMLElement.prototype, "scrollWidth", {
    configurable: true,
    get() {
      return this.classList.contains("etabs") ? 900 : 0;
    },
  });
  Object.defineProperty(HTMLElement.prototype, "clientWidth", {
    configurable: true,
    get() {
      return this.classList.contains("etabs") ? 400 : 0;
    },
  });
  try {
    const { user, open } = await setup();
    await open(/#44 docs novas/, SD.id);
    const btn = screen.getByRole("button", { name: "Lista de abas abertas (3)" });
    await user.click(btn);
    const list = screen.getByRole("dialog", { name: "Abas abertas" });
    expect(within(list).getByRole("option", { name: /#32 ui: terminal embutido/ })).toHaveTextContent("ctower-agents");
    expect(within(list).getByRole("option", { name: /#44 docs novas/ })).toHaveTextContent("docs-site");
    await user.keyboard("terminal{Enter}");
    expect(screen.queryByRole("dialog", { name: "Abas abertas" })).not.toBeInTheDocument();
    expect(within(tablist()).getByRole("tab", { name: "#32 ui: terminal embutido" })).toHaveAttribute("aria-selected", "true");
    await user.click(btn);
    await user.keyboard("{Escape}");
    expect(btn).toHaveFocus();
  } finally {
    if (sw) Object.defineProperty(HTMLElement.prototype, "scrollWidth", sw);
    if (cw) Object.defineProperty(HTMLElement.prototype, "clientWidth", cw);
  }
});

test("sessão do projeto (#0): aba \"Sessão do projeto · <projeto>\" e a linha no topo do grupo", async () => {
  const { open } = await setup();
  const ct = screen.getByRole("treeitem", { name: /^ctower-agents, / });
  const rows = within(ct).getAllByRole("treeitem").map((r) => r.textContent);
  expect(rows[0]).toMatch(/^Sessão do projeto/);
  expect(rows[1]).toMatch(/^#32/);
  await open(/^Sessão do projeto/, S0.id);
  expect(within(tablist()).getByRole("tab", { name: "Sessão do projeto · ctower-agents" })).toHaveAttribute(
    "aria-selected",
    "true",
  );
});

test("groupSessions põe o #0 antes das issues do projeto", () => {
  const d = dash();
  const info = (t: { session: string }) => {
    const s = d.live_sessions.find((x) => x.id === t.session)!;
    const p = d.projects.find((x) => x.work_items.some((w) => w.id === s.work_item_id))!;
    return { s, wi: p.work_items.find((w) => w.id === s.work_item_id), project: p.project };
  };
  const groups = groupSessions([{ id: "wsl", name: "WSL" }], { wsl: d }, [], info, {});
  expect(groups.find((g) => g.name === "ctower-agents")!.sessions.map((s) => s.ghNumber)).toEqual([0, 32, 40]);
});
