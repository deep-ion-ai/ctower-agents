import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { App } from "../App";
import { FakeDaemonApi } from "../api/fake";
import type { IssueRow, ProjectTree, TreeNode, WorkItem } from "../protocol/gen";
import {
  HUB_DASHBOARD,
  NO_HEALTH,
  NO_PROPOSALS,
  dashboard,
  hubInfo,
  hubStatus,
  project,
  session,
  sync,
  workItem,
} from "../test/fixtures";
import { rememberTermSize } from "../terminal/size";
import { DEFAULT_UI_CONFIG } from "../settings/defaults";
import { renderTemplate } from "./templates";

const NOW = Date.now();

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

const node = (i: IssueRow, wi: WorkItem | null = null, children: TreeNode[] = []): TreeNode => ({
  issue: i,
  work_item: wi,
  summary: { pr: null, decisions: { total: 0, pending_autonomous: 0 }, last_activity_at: null, live_envs: [] },
  children,
});

const WI28 = workItem({ gh_number: 28, status: "concluido", branch: "feat/28-ct-server" });
const WI30 = workItem({ gh_number: 30, status: "em_execucao", branch: "feat/30-ui-esqueleto" });
const LIVE = session({ id: "cccccccc-0000-0000-0000-000000000030", work_item_id: 30 });

const TREE: ProjectTree = {
  project: project(),
  sync: sync({ last_ok_at: NOW }),
  roots: [
    node(issue(4, "Plano 2 — Sessões", { has_sub_issues: true, state: "closed" }), null, [
      node(issue(5, "ct-pty: ring buffer", { parent_number: 4, state: "closed" })),
    ]),
    node(issue(27, "Plano 4 — UI Tauri", { has_sub_issues: true }), null, [
      node(issue(28, "ct-server: lacunas", { parent_number: 27 }), WI28),
      {
        ...node(issue(30, "ui: esqueleto Tauri", { parent_number: 27 }), WI30),
        summary: { pr: null, decisions: { total: 0, pending_autonomous: 0 }, last_activity_at: NOW, live_envs: [] },
      },
      node(issue(33, "ui: Projeto, Recuperação e Revisão", { parent_number: 27 })),
    ]),
    node(issue(16, "Pré-aceitar confiança", { state: "closed" })),
  ],
  untracked: [],
};

function setup(tree: unknown = TREE) {
  const api = new FakeDaemonApi()
    .on("dashboard.get", dashboard({ live_sessions: [LIVE], sync: [sync({ last_ok_at: NOW })] }, [WI28, WI30]))
    .on("session.peek", { data: "" })
    .on("tree.get", tree)
    .on("env.info", { protocol: "2.0", version: "0.1.0", hostname: "ct", worktrees_root: "/home/user/worktrees" })
    .on("session.start", session({ id: "dddddddd-0000-0000-0000-000000000033", work_item_id: 33 }));
  render(<App api={api} initial={{ name: "project" }} />);
  return { api, user: userEvent.setup() };
}

// Painel lateral (árvore) e a modal de início.
const side = () => screen.getByRole("complementary", { name: "Árvore do projeto" });
const rowBtn = (name: RegExp | string) => within(side()).getByRole("button", { name });
const dialog = () => screen.getByRole("dialog");
const play = (n: number) => within(side()).getByRole("button", { name: `Iniciar sessão para #${n}` });

// Daemon anterior à #46: o tree.get não traz `summary`. A tela não pode quebrar.
const semSummary = (n: TreeNode): unknown => {
  const { summary: _omit, children, ...rest } = n;
  return { ...rest, children: children.map(semSummary) };
};

test("daemon antigo, sem summary no tree.get: a árvore aparece mesmo assim", async () => {
  setup({ ...TREE, roots: TREE.roots.map(semSummary) });
  expect(await screen.findByText("Plano 4 — UI Tauri")).toBeInTheDocument();
  // Sem dados de resumo: o nome da linha diz "sem atividade".
  expect(rowBtn(/^#28 ct-server: lacunas, concluído, sem atividade/)).toBeInTheDocument();
});

test("painel lateral: épico com progresso n/m, sub-issues, estado na linha e ▶ só no que pode iniciar", async () => {
  const { user } = setup();
  expect(await screen.findByText("Plano 4 — UI Tauri")).toBeInTheDocument();
  expect(screen.getByRole("heading", { level: 1, name: "ctower-agents" })).toBeInTheDocument();
  // Só itens em aberto por padrão: o épico #4 (todo fechado) fica de fora.
  const epicos = within(side()).getByRole("region", { name: /Épicos/ });
  expect(within(epicos).getByRole("button", { name: /^Épico #27 Plano 4 — UI Tauri, backlog, 1 de 3 concluídas/ })).toBeInTheDocument();
  expect(within(epicos).getByText("1/3")).toBeInTheDocument();
  expect(within(side()).queryByText("Plano 2 — Sessões")).not.toBeInTheDocument();
  expect(rowBtn(/^#28 ct-server: lacunas, concluído/)).toBeInTheDocument();
  // Concluído não inicia; os abertos têm ▶ (inclusive o épico).
  expect(within(side()).queryByRole("button", { name: "Iniciar sessão para #28" })).not.toBeInTheDocument();
  expect(play(33)).toBeInTheDocument();
  expect(play(27)).toBeInTheDocument();
  // Sessão viva: dita no nome da linha.
  expect(rowBtn(/^#30 ui: esqueleto Tauri, em execução.*sessão viva em ctower-agents \(WSL\)$/)).toBeInTheDocument();
  // Avulsa fechada só aparece com "mostrar concluídos" (nos filtros).
  expect(within(side()).queryByText("Pré-aceitar confiança")).not.toBeInTheDocument();
  await user.click(within(side()).getByRole("button", { name: "Filtros" }));
  await user.click(screen.getByRole("checkbox", { name: "mostrar concluídos" }));
  const avulsas = within(side()).getByRole("region", { name: /Issues avulsas/ });
  expect(within(avulsas).getByRole("button", { name: /^#16 Pré-aceitar confiança, concluído/ })).toBeInTheDocument();
  expect(within(side()).queryByRole("button", { name: "Iniciar sessão para #16" })).not.toBeInTheDocument();
});

test("▶ na linha abre a modal com prévia e manda número, prompt e o modo padrão", async () => {
  const { api, user } = setup();
  await screen.findByText("ui: Projeto, Recuperação e Revisão");
  await user.click(play(33));
  const modal = dialog();
  expect(modal).toHaveAttribute("aria-modal", "true");
  expect(modal).toHaveAccessibleName("#33 ui: Projeto, Recuperação e Revisão");
  expect(within(modal).getByText("issue")).toBeInTheDocument();
  expect(within(modal).getByText("feat/33-ui-projeto-recuperacao-e-revisao")).toBeInTheDocument();
  expect(within(modal).getByText("criados ao iniciar")).toBeInTheDocument();
  await waitFor(() =>
    expect(
      within(modal).getByText("/home/user/worktrees/ctower-agents/feat__33-ui-projeto-recuperacao-e-revisao"),
    ).toBeInTheDocument(),
  );
  const prompt = within(modal).getByLabelText(/Prompt inicial/) as HTMLTextAreaElement;
  // Projeto GitHub (#351): o template de sempre, sem aviso do Jira.
  expect(within(modal).getByLabelText("Prompt inicial · template de issue")).toBe(prompt);
  expect(prompt.value).toBe(
    renderTemplate(DEFAULT_UI_CONFIG.templates.issue, {
      kind: "issue",
      number: 33,
      owner: "deep-ion-ai",
      repo: "ctower-agents",
      branch: "feat/33-ui-projeto-recuperacao-e-revisao",
      protected: ["develop", "main"],
    }),
  );
  expect(within(modal).queryByRole("status", { name: "Prontidão do Jira e do GitLab" })).not.toBeInTheDocument();
  expect(prompt.value).toContain("Trabalhe na issue #33 do repositório deep-ion-ai/ctower-agents");
  expect(prompt.value).toContain("nunca em develop, main");
  await user.type(prompt, " Extra.");
  await user.click(within(modal).getByRole("button", { name: /^Iniciar sessão/ }));
  await waitFor(() => expect(api.callsTo("session.start")).toHaveLength(1));
  const { params } = api.callsTo("session.start")[0];
  // Modelo padrão do Claude Code: sem `model`; modo padrão das Configurações.
  expect(Object.keys(params).sort()).toEqual(["gh_number", "mode", "project_id", "prompt"]);
  expect(params.mode).toBe("native");
  expect(params.gh_number).toBe(33);
  expect(params.prompt.endsWith(" Extra.")).toBe(true);
  // Sessão criada: vai para o terminal dela, com o aviso "Ver terminal".
  expect(await screen.findByRole("button", { name: "Terminal", description: "1 sessão" })).toHaveAttribute("aria-current", "page");
  expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  expect(screen.getByText("Sessão de #33 iniciada em ctower-agents (WSL)")).toBeInTheDocument();
});

test("modelo e modo: padrão das Configurações pré-selecionado e escolha enviada", async () => {
  const api = new FakeDaemonApi()
    .on("dashboard.get", dashboard({ live_sessions: [LIVE], sync: [sync({ last_ok_at: NOW })] }, [WI28, WI30]))
    .on("session.peek", { data: "" })
    .on("tree.get", TREE)
    .on("env.info", { protocol: "2.0", version: "0.1.0", hostname: "ct", worktrees_root: "/home/user/worktrees" })
    .on("session.start", session({ id: "dddddddd-0000-0000-0000-000000000033", work_item_id: 33 }));
  api.uiConfig = { ...api.uiConfig, session: { model: "sonnet", mode: "subagents", remote_control: false } };
  render(<App api={api} initial={{ name: "project" }} />);
  const user = userEvent.setup();

  await screen.findByText("ui: Projeto, Recuperação e Revisão");
  await user.click(play(33));
  const modelo = within(dialog()).getByLabelText("Modelo") as HTMLSelectElement;
  const modo = within(dialog()).getByLabelText("Modo") as HTMLSelectElement;
  await waitFor(() => expect(modelo.value).toBe("sonnet"));
  expect(modo.value).toBe("subagents");
  expect((within(dialog()).getByLabelText("Ferramenta") as HTMLSelectElement).value).toBe("claude");
  await user.selectOptions(modelo, "opus");
  await user.selectOptions(modo, "native");
  await user.click(within(dialog()).getByRole("button", { name: /^Iniciar sessão/ }));
  await waitFor(() => expect(api.callsTo("session.start")).toHaveLength(1));
  expect(api.callsTo("session.start")[0].params).toMatchObject({ gh_number: 33, model: "opus", mode: "native" });
});

/** Como `setup`, com o `env.info` de um daemon 2.3 e a disponibilidade do Remote Control. */
function setupRc(rc: { available: boolean; reason: string | null }, remote = false) {
  const api = new FakeDaemonApi()
    .on("dashboard.get", dashboard({ live_sessions: [LIVE], sync: [sync({ last_ok_at: NOW })] }, [WI28, WI30]))
    .on("session.peek", { data: "" })
    .on("tree.get", TREE)
    .on("env.info", { protocol: "2.3", version: "0.1.0", hostname: "ct", worktrees_root: "/w", remote_control: rc })
    .on("session.start", session({ id: "dddddddd-0000-0000-0000-000000000033", work_item_id: 33 }));
  api.uiConfig = { ...api.uiConfig, session: { ...api.uiConfig.session, remote_control: remote } };
  render(<App api={api} initial={{ name: "project" }} />);
  return { api, user: userEvent.setup() };
}

test("Remote Control na modal: pré-marcado pela preferência e enviado como remote_control (#197)", async () => {
  const { api, user } = setupRc({ available: true, reason: null }, true);
  await screen.findByText("ui: Projeto, Recuperação e Revisão");
  await user.click(play(33));
  const rc = (await within(dialog()).findByLabelText("Remote Control")) as HTMLInputElement;
  await waitFor(() => expect(rc.checked).toBe(true));
  expect(within(dialog()).getByText(/claude\.ai\/code/)).toBeInTheDocument();
  // Fica na tela Projeto para abrir a modal de novo.
  await user.click(within(dialog()).getByLabelText("Abrir o terminal ao iniciar"));
  await user.click(within(dialog()).getByRole("button", { name: /^Iniciar sessão/ }));
  await waitFor(() => expect(api.callsTo("session.start")).toHaveLength(1));
  expect(api.callsTo("session.start")[0].params).toMatchObject({ gh_number: 33, remote_control: true });

  // Desmarcado: o parâmetro nem vai.
  await user.click(play(33));
  await user.click(await within(dialog()).findByLabelText("Remote Control"));
  await user.click(within(dialog()).getByRole("button", { name: /^Iniciar sessão/ }));
  await waitFor(() => expect(api.callsTo("session.start")).toHaveLength(2));
  expect(api.callsTo("session.start")[1].params).not.toHaveProperty("remote_control");
});

test("Remote Control indisponível no ambiente: opção desabilitada com o motivo; daemon 2.2 não mostra a opção", async () => {
  const { user } = setupRc(
    { available: false, reason: "Remote Control indisponível: ANTHROPIC_API_KEY definida (ambiente do ctowerd)" },
    true,
  );
  await screen.findByText("ui: Projeto, Recuperação e Revisão");
  await user.click(play(33));
  const rc = (await within(dialog()).findByLabelText("Remote Control")) as HTMLInputElement;
  expect(rc).toBeDisabled();
  expect(rc.checked).toBe(false);
  expect(within(dialog()).getByText(/ANTHROPIC_API_KEY definida/)).toBeInTheDocument();
});

test("daemon 2.2 (sem remote_control no env.info): a modal não mostra a opção", async () => {
  const { user } = setup();
  await screen.findByText("ui: Projeto, Recuperação e Revisão");
  await user.click(play(33));
  await within(dialog()).findByLabelText("Modelo");
  expect(within(dialog()).queryByLabelText("Remote Control")).not.toBeInTheDocument();
});

test("modelo padrão do Claude Code: a sessão de planejamento vai sem `model`", async () => {
  const { api, user } = setup();
  api.on("session.start_project", session({ id: "eeeeeeee-0000-0000-0000-000000000003", work_item_id: 900 }));
  await screen.findByText("Plano 4 — UI Tauri");
  await user.click(within(side()).getByRole("button", { name: "Iniciar sessão de planejamento" }));
  expect((within(dialog()).getByLabelText("Modelo") as HTMLSelectElement).value).toBe("");
  await user.selectOptions(within(dialog()).getByLabelText("Modo"), "subagents");
  await user.click(within(dialog()).getByRole("button", { name: /^Iniciar sessão/ }));
  await waitFor(() => expect(api.callsTo("session.start_project")).toHaveLength(1));
  const { params } = api.callsTo("session.start_project")[0];
  expect(params).not.toHaveProperty("model");
  expect(params.mode).toBe("subagents");
});

test("épico: template de épico, branch wip/, escopo e atalho para sub-issue a fazer que troca o alvo", async () => {
  const { api, user } = setup();
  await screen.findByText("Plano 4 — UI Tauri");
  await user.click(play(27));
  const modal = dialog();
  expect(within(modal).getByText("épico")).toBeInTheDocument();
  expect(within(modal).getAllByText("wip/27-plano-4-ui-tauri").length).toBeGreaterThan(0);
  expect((within(modal).getByLabelText(/Prompt inicial/) as HTMLTextAreaElement).value).toMatch(/^Trabalhe no épico #27/);
  // Só a #33 está a fazer (#28 concluída, #30 em execução).
  const subs = within(modal).getByRole("group", { name: "Sub-issues a fazer" });
  expect(within(subs).getAllByRole("button")).toHaveLength(1);
  await user.click(within(subs).getByRole("button", { name: /#33/ }));
  expect(modal).toHaveAccessibleName("#33 ui: Projeto, Recuperação e Revisão");
  expect((within(modal).getByLabelText(/Prompt inicial/) as HTMLTextAreaElement).value).toMatch(/^Trabalhe na issue #33/);
  await user.click(within(modal).getByRole("button", { name: /^Iniciar sessão/ }));
  await waitFor(() => expect(api.callsTo("session.start")[0]?.params.gh_number).toBe(33));
});

test("erro ao iniciar aparece na modal, que continua aberta", async () => {
  const { api, user } = setup();
  api.on("session.start", () => {
    throw "worktree já existe com outra branch";
  });
  await screen.findByText("ui: Projeto, Recuperação e Revisão");
  await user.click(play(33));
  await user.click(within(dialog()).getByRole("button", { name: /^Iniciar sessão/ }));
  expect(await within(dialog()).findByRole("alert")).toHaveTextContent("worktree já existe com outra branch");
});

test("épico fechado só com 'mostrar concluídos': vem depois e começa recolhido", async () => {
  const { user } = setup();
  await screen.findByText("Plano 4 — UI Tauri");
  expect(screen.queryByText("Plano 2 — Sessões")).not.toBeInTheDocument();
  await user.click(within(side()).getByRole("button", { name: "Filtros" }));
  await user.click(screen.getByRole("checkbox", { name: "mostrar concluídos" }));
  const titulos = within(side())
    .getAllByText(/^Plano \d/)
    .map((e) => e.textContent);
  expect(titulos).toEqual(["Plano 4 — UI Tauri", "Plano 2 — Sessões"]);
  expect(within(side()).getByText("Pré-aceitar confiança")).toBeInTheDocument();
  expect(within(side()).queryByText("ct-pty: ring buffer")).not.toBeInTheDocument();
  const abrir = screen.getByRole("button", { name: "Expandir épico #4" });
  expect(abrir).toHaveAttribute("aria-expanded", "false");
  await user.click(abrir);
  expect(within(side()).getByText("ct-pty: ring buffer")).toBeInTheDocument();
  await user.click(screen.getByRole("button", { name: "Recolher épico #27" }));
  expect(within(side()).queryByText("ui: esqueleto Tauri")).not.toBeInTheDocument();
});

test("busca por número e chips de status filtram a árvore e expandem os épicos", async () => {
  const { user } = setup();
  await screen.findByText("Plano 4 — UI Tauri");
  await user.click(screen.getByRole("button", { name: "Recolher épico #27" }));
  await user.type(screen.getByRole("searchbox", { name: "Buscar por número ou título" }), "#30");
  expect(screen.getByRole("button", { name: "Recolher épico #27" })).toHaveAttribute("aria-expanded", "true");
  expect(within(side()).getByText("ui: esqueleto Tauri")).toBeInTheDocument();
  expect(within(side()).queryByText("ct-server: lacunas")).not.toBeInTheDocument();
  await user.clear(screen.getByRole("searchbox"));
  const filtros = within(side()).getByRole("button", { name: "Filtros" });
  expect(filtros).toHaveAttribute("aria-expanded", "false");
  await user.click(filtros);
  await user.click(screen.getByRole("button", { name: "backlog" }));
  expect(screen.getByRole("button", { name: "backlog" })).toHaveAttribute("aria-pressed", "true");
  expect(within(side()).getByText("ui: Projeto, Recuperação e Revisão")).toBeInTheDocument();
  expect(within(side()).queryByText("ui: esqueleto Tauri")).not.toBeInTheDocument();
  await user.click(screen.getByRole("button", { name: "backlog" }));
  await user.type(screen.getByRole("searchbox"), "zzz");
  expect(screen.getByText(/Nada casa com a busca e os filtros/)).toBeInTheDocument();
});

test("filtros e épicos abertos sobrevivem a abrir um item noutra aba", async () => {
  const { user } = setup();
  await screen.findByText("Plano 4 — UI Tauri");
  await user.click(screen.getByRole("button", { name: "Recolher épico #27" }));
  await user.type(screen.getByRole("searchbox"), "Plano");
  await user.click(rowBtn(/^Épico #27/));
  expect(await screen.findByRole("tab", { name: "#27 Plano 4 — UI Tauri" })).toHaveAttribute("aria-selected", "true");
  expect(screen.getByRole("searchbox")).toHaveValue("Plano");
});

test("sessão de planejamento: sem issue, no clone principal, com o template do projeto", async () => {
  const { api, user } = setup();
  api.on("session.start_project", session({ id: "eeeeeeee-0000-0000-0000-000000000000", work_item_id: 900 }));
  await screen.findByText("Plano 4 — UI Tauri");
  await user.click(within(side()).getByRole("button", { name: "Iniciar sessão de planejamento" }));
  const modal = dialog();
  expect(modal).toHaveAccessibleName("Sessão de planejamento · ctower-agents");
  expect(within(modal).getByText("planejamento")).toBeInTheDocument();
  expect(within(modal).getByText("clone principal, sem branch nova")).toBeInTheDocument();
  expect(within(modal).getByText("/home/user/projetos/ctower-agents")).toBeInTheDocument();
  const prompt = within(modal).getByLabelText(/Prompt inicial/) as HTMLTextAreaElement;
  expect(prompt.value).toContain("clone principal de deep-ion-ai/ctower-agents");
  expect(prompt.value).toContain("nem mescle em develop, main");
  await user.click(within(modal).getByRole("button", { name: /^Iniciar sessão/ }));
  await waitFor(() =>
    expect(api.callsTo("session.start_project")[0]?.params).toEqual({
      project_id: 1,
      prompt: prompt.value,
      mode: "native",
    }),
  );
  expect(api.callsTo("session.start")).toHaveLength(0);
  expect(await screen.findByRole("button", { name: "Terminal", description: "1 sessão" })).toBeInTheDocument();
});

test("com sessão do projeto viva, a modal avisa e 'Abrir a existente' abre o terminal dela", async () => {
  const wi0 = workItem({ gh_number: 0, title: null, branch: null, worktree_path: null });
  const s0 = session({ id: "ffffffff-0000-0000-0000-000000000000", work_item_id: wi0.id });
  const api = new FakeDaemonApi()
    .on("dashboard.get", dashboard({ live_sessions: [s0], sync: [sync({ last_ok_at: NOW })] }, [wi0]))
    .on("session.peek", { data: "" })
    .on("tree.get", TREE)
    .on("env.info", { protocol: "2.0", version: "0.1.0", hostname: "ct", worktrees_root: "/w" });
  render(<App api={api} initial={{ name: "project" }} />);
  const user = userEvent.setup();
  await screen.findByText("Plano 4 — UI Tauri");
  await user.click(screen.getByRole("button", { name: /Sessão de planejamento/ }));
  expect(within(dialog()).getByText(/Já existe uma sessão para este item/)).toHaveTextContent("ctower-agents (WSL) · rodando");
  await user.click(within(dialog()).getByRole("button", { name: "Abrir a existente" }));
  expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  expect(await screen.findByRole("treeitem", { name: /Sessão do projeto/ })).toBeInTheDocument();
});

describe("Quadro local", () => {
  afterEach(() => window.sessionStorage.clear());
  const abrir = async () => {
    const r = setup();
    await screen.findByText("Plano 4 — UI Tauri");
    await r.user.click(screen.getByRole("button", { name: "Quadro" }));
    return r;
  };
  const col = (nome: RegExp) => screen.getByRole("region", { name: nome });

  test("colunas por status, cards do épico e das filhas, ponto de sessão viva e '—' sem atividade", async () => {
    await abrir();
    const exec = col(/^Coluna em execução/);
    const filha = within(exec).getByRole("button", { name: /ui: esqueleto Tauri/ });
    expect(within(filha).getByRole("img", { name: /sessão viva em/ })).toBeInTheDocument();
    expect(within(filha).getByText("épico #27")).toBeInTheDocument();
    expect(within(filha).getByTitle("última atividade")).not.toHaveTextContent("—");
    expect(within(col(/^Coluna concluído/)).getByRole("button", { name: /ct-server: lacunas/ })).toBeInTheDocument();
    const backlog = within(col(/^Coluna backlog/));
    const semAtividade = backlog.getByRole("button", { name: /ui: Projeto, Recuperação e Revisão/ });
    expect(within(semAtividade).getByTitle("última atividade")).toHaveTextContent("—");
    expect(backlog.getByRole("button", { name: /^#27 ÉPICO/ })).toBeInTheDocument();
  });

  test("clicar no card abre o item em aba própria e marca a linha da árvore", async () => {
    const { user } = await abrir();
    await user.click(within(col(/^Coluna em execução/)).getByRole("button", { name: /ui: esqueleto Tauri/ }));
    const item = await screen.findByRole("region", { name: "Detalhe do item" });
    expect(within(item).getByRole("heading", { level: 1, name: /#30 ui: esqueleto Tauri/ })).toBeInTheDocument();
    expect(screen.getByRole("tab", { name: "#30 ui: esqueleto Tauri" })).toHaveAttribute("aria-selected", "true");
    expect(rowBtn(/^#30 ui: esqueleto Tauri/)).toHaveAttribute("aria-current", "true");
  });

  test("chip filtra as colunas", async () => {
    const { user } = await abrir();
    await user.click(within(side()).getByRole("button", { name: "Filtros" }));
    await user.click(screen.getByRole("button", { name: "backlog" }));
    expect(screen.getAllByRole("region", { name: /^Coluna / })).toHaveLength(1);
    expect(screen.queryByRole("button", { name: /ui: esqueleto Tauri/ })).not.toBeInTheDocument();
  });
});

describe("item aberto como aba", () => {
  test("épico: cabeçalho, progresso, sub-issues com status, sessão e ▶, e a trilha", async () => {
    const { api, user } = setup();
    await screen.findByText("Plano 4 — UI Tauri");
    await user.click(rowBtn(/^Épico #27/));
    const item = await screen.findByRole("region", { name: "Detalhe do item" });
    expect(within(item).getByRole("heading", { level: 1, name: /#27 Plano 4 — UI Tauri/ })).toBeInTheDocument();
    expect(within(item).getByText("épico")).toBeInTheDocument();
    expect(within(item).getByRole("progressbar", { name: "Progresso do épico" })).toHaveAttribute("aria-valuenow", "1");
    expect(within(item).getByText("1 de 3 sub-issues concluídas")).toBeInTheDocument();
    const subs = within(item).getByRole("region", { name: "Sub-issues" });
    const linhas = within(subs).getAllByRole("row").slice(1);
    expect(linhas).toHaveLength(3);
    const l30 = linhas.find((r) => r.textContent?.includes("#30"))!;
    expect(within(l30).getByRole("button", { name: /Abrir terminal de #30/ })).toBeInTheDocument();
    expect(within(subs).queryByRole("button", { name: "Iniciar sessão para #28" })).not.toBeInTheDocument();
    // ▶ na sub-issue abre a modal nela.
    await user.click(within(subs).getByRole("button", { name: "Iniciar sessão para #33" }));
    expect(dialog()).toHaveAccessibleName("#33 ui: Projeto, Recuperação e Revisão");
    await user.keyboard("{Escape}");
    // "Iniciar sessão" do cabeçalho: o épico.
    await user.click(within(item).getByRole("button", { name: "Iniciar sessão" }));
    expect(dialog()).toHaveAccessibleName("#27 Plano 4 — UI Tauri");
    await user.click(within(dialog()).getByRole("button", { name: "Cancelar" }));
    // Abrir uma sub-issue pela tabela abre a aba dela; a trilha volta ao épico.
    await user.click(within(subs).getByRole("button", { name: /#33 ui: Projeto/ }));
    const sub = await screen.findByRole("region", { name: "Detalhe do item" });
    expect(within(sub).getByRole("heading", { level: 1, name: /#33/ })).toBeInTheDocument();
    await user.click(within(screen.getByRole("list", { name: "Trilha" })).getByRole("button", { name: "Épico #27" }));
    expect(await screen.findByRole("tab", { name: "#27 Plano 4 — UI Tauri" })).toHaveAttribute("aria-selected", "true");
    expect(api.callsTo("session.start")).toHaveLength(0);
  });

  test("menu de contexto da linha: Iniciar sessão abre a modal", async () => {
    const { user } = setup();
    await screen.findByText("ui: Projeto, Recuperação e Revisão");
    const linha = rowBtn(/^#33 /);
    fireEvent.contextMenu(linha.closest("li")!, { clientX: 10, clientY: 10 });
    const menu = screen.getByRole("menu", { name: "Ações do item" });
    expect(within(menu).getAllByRole("menuitem").map((b) => b.textContent)).toEqual([
      "Abrir",
      "Iniciar sessão",
      "Abrir no GitHub",
    ]);
    expect(within(menu).getByRole("menuitem", { name: "Abrir" })).toHaveFocus();
    await user.keyboard("{ArrowDown}");
    expect(within(menu).getByRole("menuitem", { name: "Iniciar sessão" })).toHaveFocus();
    await user.keyboard("{Enter}");
    expect(dialog()).toHaveAccessibleName("#33 ui: Projeto, Recuperação e Revisão");
  });
});

test("PR mesclado na base com a issue aberta: aviso na linha, no menu e no item (#375)", async () => {
  const merged = (n: number, base: string): TreeNode => ({
    ...node(issue(n, `item ${n}`), workItem({ gh_number: n, status: "em_execucao", branch: `fix/${n}-x` })),
    summary: {
      pr: { number: n + 1, work_item_id: n, head: `fix/${n}-x`, base, state: "merged", url: `https://github.com/deep-ion-ai/ctower-agents/pull/${n + 1}`, updated_at: 0, head_sha: null },
      decisions: { total: 0, pending_autonomous: 0 },
      last_activity_at: null,
      live_envs: [],
    },
  });
  const { api, user } = setup({ ...TREE, roots: [merged(279, "develop"), merged(290, "wip/27-ui")] });
  const linha = await waitFor(() => rowBtn(/^#279 item 279, em execução, PR #280 mesclado, PR mesclado, issue aberta/));
  expect(within(linha).getByTitle(/PR #280 não fechou a issue/)).toBeInTheDocument();
  // PR mesclado na branch do épico não é aviso.
  expect(rowBtn(/^#290 /)).not.toHaveAccessibleName(/issue aberta/);
  fireEvent.contextMenu(linha.closest("li")!, { clientX: 10, clientY: 10 });
  const menu = screen.getByRole("menu", { name: "Ações do item" });
  await user.click(within(menu).getByRole("menuitem", { name: "Fechar a issue no GitHub (PR mesclado)" }));
  expect(api.opened).toEqual(["https://github.com/deep-ion-ai/ctower-agents/issues/279"]);
  await user.click(rowBtn(/^#279 /));
  const item = await screen.findByRole("region", { name: "Detalhe do item" });
  expect(within(item).getByText("PR mesclado, issue aberta")).toBeInTheDocument();
  await user.click(within(item).getByRole("button", { name: "Fechar a issue no GitHub ↗" }));
  expect(api.opened).toEqual([
    "https://github.com/deep-ion-ai/ctower-agents/issues/279",
    "https://github.com/deep-ion-ai/ctower-agents/issues/279",
  ]);
});

describe("modal de início", () => {
  test("foco preso, Esc fecha e devolve o foco ao botão de origem", async () => {
    const { user } = setup();
    await screen.findByText("ui: Projeto, Recuperação e Revisão");
    const origem = play(33);
    await user.click(origem);
    const modal = dialog();
    // O foco entra na modal (primeiro controle) e não sai com Tab.
    expect(modal.contains(document.activeElement)).toBe(true);
    for (let i = 0; i < 15; i++) {
      await user.tab();
      expect(modal.contains(document.activeElement)).toBe(true);
    }
    await user.tab({ shift: true });
    expect(modal.contains(document.activeElement)).toBe(true);
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(origem).toHaveFocus();
  });

  test("⌘↵ (Ctrl+Enter) inicia de dentro do prompt", async () => {
    const { api, user } = setup();
    await screen.findByText("ui: Projeto, Recuperação e Revisão");
    await user.click(play(33));
    await user.click(within(dialog()).getByLabelText(/Prompt inicial/));
    await user.keyboard("{Control>}{Enter}{/Control}");
    await waitFor(() => expect(api.callsTo("session.start")).toHaveLength(1));
  });

  test("Restaurar template volta ao prompt do template; sem 'Abrir o terminal', fica na tela e o aviso oferece Ver terminal", async () => {
    const { api, user } = setup();
    await screen.findByText("ui: Projeto, Recuperação e Revisão");
    await user.click(play(33));
    const prompt = within(dialog()).getByLabelText(/Prompt inicial/) as HTMLTextAreaElement;
    const original = prompt.value;
    expect(within(dialog()).queryByRole("button", { name: "Restaurar template" })).not.toBeInTheDocument();
    await user.type(prompt, " mais");
    await user.click(within(dialog()).getByRole("button", { name: "Restaurar template" }));
    expect(prompt.value).toBe(original);
    const abrir = within(dialog()).getByRole("checkbox", { name: "Abrir o terminal ao iniciar" });
    expect(abrir).toBeChecked();
    await user.click(abrir);
    await user.click(within(dialog()).getByRole("button", { name: /^Iniciar sessão/ }));
    await waitFor(() => expect(api.callsTo("session.start")).toHaveLength(1));
    expect(await screen.findByText("Sessão de #33 iniciada em ctower-agents (WSL)")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Projetos" })).toHaveAttribute("aria-current", "page");
    await user.click(screen.getByRole("button", { name: "Ver terminal" }));
    expect(screen.getByRole("button", { name: "Terminal", description: "1 sessão" })).toHaveAttribute("aria-current", "page");
  });
});

test("resposta antiga de tree.get não sobrescreve a mais nova", async () => {
  const velha: ProjectTree = { ...TREE, roots: [node(issue(99, "árvore velha"))] };
  let soltar: (t: ProjectTree) => void = () => {};
  let chamadas = 0;
  const api = new FakeDaemonApi()
    // Objeto novo a cada chamada: o push muda o dashboard e dispara outro carregamento.
    .on("dashboard.get", () => dashboard({ live_sessions: [], sync: [sync({ last_ok_at: NOW })] }, [WI28, WI30]))
    .on("session.peek", { data: "" })
    .on("env.info", { protocol: "2.0", version: "0.1.0", hostname: "ct", worktrees_root: "/w" })
    .on("tree.get", () => (++chamadas === 1 ? new Promise<ProjectTree>((r) => (soltar = r)) : TREE));
  render(<App api={api} initial={{ name: "project" }} />);
  await waitFor(() => expect(chamadas).toBeGreaterThan(0));
  // Um segundo carregamento (push do dashboard) resolve antes do primeiro.
  await act(async () => api.push("wsl", "session_state", {}));
  await waitFor(() => expect(chamadas).toBeGreaterThan(1));
  expect(await screen.findByText("Plano 4 — UI Tauri")).toBeInTheDocument();
  await act(async () => soltar(velha));
  expect(screen.getByText("Plano 4 — UI Tauri")).toBeInTheDocument();
  expect(screen.queryByText("árvore velha")).not.toBeInTheDocument();
});

test("a sessão nasce com o último tamanho medido do terminal", async () => {
  rememberTermSize({ cols: 132, rows: 41 });
  const { api, user } = setup();
  api.on("session.start_project", session({ id: "eeeeeeee-0000-0000-0000-000000000001", work_item_id: 900 }));
  await screen.findByText("Plano 4 — UI Tauri");
  await user.click(within(side()).getByRole("button", { name: "Iniciar sessão de planejamento" }));
  await user.click(within(dialog()).getByRole("button", { name: /^Iniciar sessão/ }));
  await waitFor(() =>
    expect(api.callsTo("session.start_project")[0]?.params).toMatchObject({ cols: 132, rows: 41 }),
  );
});

describe("sem projetos (#6)", () => {
  // Nenhum projeto no ambiente; o hub, quando há, também está vazio.
  function setupVazio(hub: "no ar" | "fora" | "sem") {
    const api = new FakeDaemonApi()
      .on("dashboard.get", dashboard({ projects: [] }))
      .on("env.info", { protocol: "2.0", version: "0.1.0", hostname: "ct", worktrees_root: "/w" });
    if (hub !== "sem")
      api
        .withHub("ws://mac-mini:7879/ws", hub === "no ar")
        .onHub("hub.info", hubInfo())
        .onHub("hub.status", hubStatus())
        .onHub("hub.projects.list", [])
        .onHub("hub.envs.list", [])
        .onHub("hub.dashboard.get", HUB_DASHBOARD)
        .onHub("hub.proposals.list", NO_PROPOSALS)
        .onHub("hub.health.get", NO_HEALTH);
    render(<App api={api} initial={{ name: "project" }} />);
    return userEvent.setup();
  }

  test("hub no ar: o texto aponta para o Hub e '+ Novo projeto' abre o formulário de cadastro", async () => {
    const user = setupVazio("no ar");
    const novo = await screen.findByRole("button", { name: "+ Novo projeto" });
    expect(screen.getByText(/O cadastro fica no Hub, em Projetos e atribuições/)).toBeInTheDocument();
    expect(screen.queryByText(/Cadastre um em Configurações/)).not.toBeInTheDocument();
    await user.click(novo);
    expect(await screen.findByRole("form", { name: "Novo projeto" })).toBeInTheDocument();
  });

  test.each(["fora", "sem"] as const)("hub %s: 'Configurar o hub' leva a Configurações › Conexões", async (hub) => {
    const user = setupVazio(hub);
    const configurar = await screen.findByRole("button", { name: "Configurar o hub" });
    expect(screen.queryByRole("button", { name: "+ Novo projeto" })).not.toBeInTheDocument();
    await user.click(configurar);
    expect(await screen.findByRole("tab", { name: /Configurações/, selected: true })).toBeInTheDocument();
  });
});

test("Editar template na modal abre a aba do template usado e fecha a modal; no épico, as regras dele (#11)", async () => {
  const { user } = setup();
  await screen.findByText("Plano 4 — UI Tauri");
  await user.click(play(27));
  expect(within(dialog()).getByRole("button", { name: "Regras do projeto ›" })).toBeInTheDocument();
  expect(within(dialog()).getByRole("button", { name: "Regras do épico #27 ›" })).toBeInTheDocument();
  await user.click(within(dialog()).getByRole("button", { name: "Editar template ›" }));
  expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  expect(screen.getByRole("tab", { name: "Template · Épico" })).toHaveAttribute("aria-selected", "true");
  expect(screen.getByRole("heading", { name: "Template: Épico" })).toBeInTheDocument();
});
