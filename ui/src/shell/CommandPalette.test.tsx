import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { App } from "../App";
import { FakeDaemonApi } from "../api/fake";
import type { IssueRow, ProjectTree, TreeNode } from "../protocol/gen";
import { dashboard, project, session, sync, workItem } from "../test/fixtures";
import { webApi } from "../test/web";

const NOW = Date.now();

// Armazenamento em memória: os recentes não dependem do localStorage do Node.
function memoryStorage(): Storage {
  const m = new Map<string, string>();
  return {
    get length() {
      return m.size;
    },
    clear: () => m.clear(),
    getItem: (k) => m.get(k) ?? null,
    key: (i) => [...m.keys()][i] ?? null,
    removeItem: (k) => void m.delete(k),
    setItem: (k, v) => void m.set(k, String(v)),
  };
}

beforeEach(() => vi.stubGlobal("localStorage", memoryStorage()));
afterEach(() => vi.unstubAllGlobals());

const issue = (n: number, title: string, p: Partial<IssueRow> = {}): IssueRow => ({
  gh_number: n,
  title,
  state: "open",
  labels: [],
  parent_number: null,
  has_sub_issues: false,
  url: `https://github.com/deep-ion-ai/ctower-agents/issues/${n}`,
  updated_at: 0,
  ...p,
});

const node = (i: IssueRow, children: TreeNode[] = []): TreeNode => ({
  issue: i,
  work_item: null,
  summary: { pr: null, decisions: { total: 0, pending_autonomous: 0 }, last_activity_at: null, live_envs: [] },
  children,
});

const WI310 = workItem({ gh_number: 310, kind: "epic", title: "Épico: UI no estilo VS Code" });
const WI297 = workItem({ gh_number: 297, title: "ui: paleta de comandos" });
const LIVE = session({ id: "cccccccc-0000-0000-0000-000000000297", work_item_id: 297 });

const TREE: ProjectTree = {
  project: project(),
  sync: sync({ last_ok_at: NOW }),
  roots: [
    node(issue(310, "Épico: UI no estilo VS Code", { has_sub_issues: true }), [
      { ...node(issue(297, "ui: paleta de comandos", { parent_number: 310 })), work_item: WI297 },
    ]),
  ],
  untracked: [],
};

function setup(api = new FakeDaemonApi()) {
  api
    .on("dashboard.get", dashboard({ live_sessions: [LIVE] }, [WI310, WI297]))
    .on("session.peek", { data: "" })
    .on("tree.get", TREE)
    .on("sync.now", [])
    .on("env.info", { protocol: "2.0", version: "0.1.0", hostname: "ct", worktrees_root: "/w" });
  render(<App api={api} />);
  return { api, user: userEvent.setup() };
}

const trigger = () => screen.getByRole("button", { name: "Abrir paleta de comandos" });
const input = () => within(screen.getByRole("dialog", { name: "Paleta de comandos" })).getByRole("combobox");
const options = () => within(screen.getByRole("listbox", { name: "Resultados" })).getAllByRole("option");
const activeOption = () => document.getElementById(input().getAttribute("aria-activedescendant")!)!;

async function open(user: ReturnType<typeof userEvent.setup>) {
  await user.click(trigger());
  expect(input()).toHaveFocus();
}

test("#297 → Enter abre o item na tela Projeto, com o detalhe aberto", async () => {
  const { user } = setup();
  await screen.findByRole("button", { name: /ui: paleta de comandos/ });
  await open(user);
  await user.type(input(), "#297");
  expect(activeOption()).toHaveTextContent("#297 ui: paleta de comandos");
  expect(activeOption()).toHaveTextContent("Issue");
  await user.keyboard("{Enter}");
  expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  const nav = screen.getByRole("navigation", { name: "Telas" });
  expect(within(nav).getByRole("button", { name: "Projetos" })).toHaveAttribute("aria-current", "page");
  // O item abre em aba própria; a linha da sub-issue (dentro do épico, que abre) fica marcada.
  const detalhe = await screen.findByRole("region", { name: "Detalhe do item" });
  expect(within(detalhe).getByText(/ui: paleta de comandos/)).toBeInTheDocument();
  expect(screen.getByRole("tab", { name: "#297 ui: paleta de comandos" })).toHaveAttribute("aria-selected", "true");
  const side = screen.getByRole("complementary", { name: "Árvore do projeto" });
  expect(await within(side).findByRole("button", { name: /^#297 ui: paleta de comandos/ })).toHaveAttribute(
    "aria-current",
    "true",
  );
});

test("Iniciar sessão: #297 abre a modal de início no item", async () => {
  const { user } = setup();
  await screen.findByRole("button", { name: /ui: paleta de comandos/ });
  await open(user);
  await user.type(input(), ">iniciar 297");
  expect(activeOption()).toHaveTextContent("Iniciar sessão: #297 ui: paleta de comandos");
  await user.keyboard("{Enter}");
  // A modal de início abre no item, sem sair da tela atual.
  const modal = await screen.findByRole("dialog", { name: "#297 ui: paleta de comandos" });
  expect((within(modal).getByLabelText(/Prompt inicial/) as HTMLTextAreaElement).value).toContain("issue #297");
  expect(within(screen.getByRole("navigation", { name: "Telas" })).getByRole("button", { name: "Sala de controle" })).toHaveAttribute(
    "aria-current",
    "page",
  );
  await user.keyboard("{Escape}");
  expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
});

test("teclado: ↑↓ movem a opção ativa (aria-activedescendant), Esc fecha e devolve o foco", async () => {
  const { user } = setup();
  await open(user);
  const dialog = screen.getByRole("dialog", { name: "Paleta de comandos" });
  expect(dialog).toHaveAttribute("aria-modal", "true");
  const opts = options();
  expect(opts[0]).toHaveAttribute("aria-selected", "true");
  expect(input()).toHaveAttribute("aria-activedescendant", opts[0].id);
  await user.keyboard("{ArrowDown}{ArrowDown}");
  expect(input()).toHaveAttribute("aria-activedescendant", opts[2].id);
  expect(opts[2]).toHaveAttribute("aria-selected", "true");
  expect(opts[0]).toHaveAttribute("aria-selected", "false");
  await user.keyboard("{ArrowUp}");
  expect(input()).toHaveAttribute("aria-activedescendant", opts[1].id);
  // Do primeiro, ↑ vai para o último.
  await user.keyboard("{ArrowUp}{ArrowUp}");
  expect(input()).toHaveAttribute("aria-activedescendant", opts[opts.length - 1].id);
  // Tab não sai do campo.
  await user.tab();
  expect(input()).toHaveFocus();
  await user.keyboard("{Escape}");
  expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  expect(trigger()).toHaveFocus();
});

test("Enter num comando: Sincronizar agora chama sync.now; clique fora fecha", async () => {
  const { user, api } = setup();
  await waitFor(() => expect(api.calls.some((c) => c.method === "dashboard.get")).toBe(true));
  await open(user);
  await user.type(input(), "sincronizar");
  await user.keyboard("{Enter}");
  await waitFor(() => expect(api.calls.some((c) => c.method === "sync.now")).toBe(true));
  await open(user);
  await user.click(document.querySelector(".palette-backdrop")!);
  expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
});

test("Ctrl+K (⌘K no macOS) abre a paleta de qualquer tela", async () => {
  const { user } = setup();
  await user.click(screen.getByRole("button", { name: "Hub" }));
  await user.keyboard("{Control>}k{/Control}");
  expect(input()).toHaveFocus();
  await user.keyboard("{Escape}");
  expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
});

test("recentes no topo com a busca vazia", async () => {
  const { user } = setup();
  await open(user);
  await user.type(input(), "revisao");
  await user.keyboard("{Enter}");
  expect(within(screen.getByRole("navigation", { name: "Telas" })).getByRole("button", { name: "Revisão" })).toHaveAttribute(
    "aria-current",
    "page",
  );
  await open(user);
  expect(options()[0]).toHaveTextContent("Revisão");
  expect(options()[0]).toHaveTextContent("recente");
});

test("sem hub e sem ambiente: telas e comandos continuam", async () => {
  const user = userEvent.setup();
  render(<App api={new FakeDaemonApi([])} />);
  await open(user);
  await user.type(input(), "config");
  expect(activeOption()).toHaveTextContent("Configurações");
  await user.clear(input());
  await user.type(input(), ">");
  expect(options().map((o) => o.textContent)).toEqual(
    expect.arrayContaining([expect.stringContaining("Sincronizar agora"), expect.stringContaining("Abrir gestor")]),
  );
  await user.clear(input());
  await user.type(input(), "#1");
  expect(screen.getByRole("status")).toHaveTextContent("Nada encontrado.");
});

test("comandos de projeto (#9): \"novo projeto\" abre o cadastro; \"configurar ctower\", a configuração", async () => {
  const { user } = setup();
  await screen.findByRole("button", { name: /ui: paleta de comandos/ });
  await open(user);
  await user.type(input(), "novo projeto");
  expect(activeOption()).toHaveTextContent("Projeto: Novo…");
  await user.keyboard("{Enter}");
  expect(await screen.findByRole("tab", { name: "Hub · Projetos e atribuições" })).toHaveAttribute("aria-selected", "true");
  await open(user);
  await user.type(input(), "configurar ctower");
  expect(activeOption()).toHaveTextContent("Projeto: Configurações — ctower-agents");
  await user.clear(input());
  await user.type(input(), ">atribuir ctower");
  expect(activeOption()).toHaveTextContent("Projeto: Atribuir ctower-agents a ambiente");
  await user.clear(input());
  await user.type(input(), ">projeto ctower");
  expect(options().map((o) => o.textContent)).toEqual(
    expect.arrayContaining([
      expect.stringContaining("Projeto: Secrets — ctower-agents"),
      expect.stringContaining("Projeto: Regras — ctower-agents"),
    ]),
  );
});

test("comandos de projeto (#17): cada aba e seção abre na aba do próprio projeto", async () => {
  const { user } = setup();
  await screen.findByRole("button", { name: /ui: paleta de comandos/ });
  await open(user);
  await user.type(input(), "configurar ctower");
  expect(activeOption()).toHaveTextContent("Projeto: Configurações — ctower-agents");
  await user.keyboard("{Enter}");
  expect(await screen.findByRole("tab", { name: "ctower-agents · Configurações" })).toHaveAttribute("aria-selected", "true");
  await open(user);
  await user.type(input(), ">regras ctower");
  expect(activeOption()).toHaveTextContent("Projeto: Regras — ctower-agents");
  await user.keyboard("{Enter}");
  // A mesma aba do projeto, com o endereço da seção.
  expect(await screen.findByRole("tab", { name: "ctower-agents · Configurações › Regras do agente" })).toHaveAttribute(
    "aria-selected",
    "true",
  );
  await open(user);
  await user.type(input(), ">projeto sessões ctower");
  expect(activeOption()).toHaveTextContent("Projeto: Sessões — ctower-agents");
  await user.keyboard("{Enter}");
  expect(await screen.findByRole("tab", { name: "ctower-agents · Sessões" })).toHaveAttribute("aria-selected", "true");
  await open(user);
  await user.clear(input());
  await user.type(input(), ">projeto ctower");
  expect(options().map((o) => o.textContent)).toEqual(
    expect.arrayContaining([
      expect.stringContaining("Projeto: Visão geral — ctower-agents"),
      expect.stringContaining("Projeto: Itens — ctower-agents"),
      expect.stringContaining("Projeto: Docs — ctower-agents"),
      expect.stringContaining("Projeto: Excluir — ctower-agents"),
    ]),
  );
});

test("comandos de projeto na web (#9): só os de leitura", async () => {
  const user = userEvent.setup();
  render(<App api={webApi()} />);
  await screen.findByRole("button", { name: /WebDaemonApi/ });
  await open(user);
  await user.type(input(), ">projeto");
  const labels = options().map((o) => o.textContent ?? "");
  expect(labels).toEqual(
    expect.arrayContaining([
      expect.stringContaining("Projeto: Configurações — ctower-agents"),
      expect.stringContaining("Projeto: Regras — ctower-agents"),
    ]),
  );
  for (const l of ["Projeto: Novo", "Projeto: Atribuir", "Projeto: Secrets", "Projeto: Excluir"]) {
    expect(labels.some((t) => t.includes(l))).toBe(false);
  }
});
