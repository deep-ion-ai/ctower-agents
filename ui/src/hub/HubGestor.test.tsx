import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { App } from "../App";
import { FakeDaemonApi } from "../api/fake";
import type { Dashboard, EnvInfo, Session } from "../protocol/gen";
import type { HubHealth, HubProposal, HubProposals, ProposalDecided } from "../protocol/hub-gen";
import {
  HUB_CONFIG_UNLINKED,
  HUB_DASHBOARD,
  NO_HEALTH,
  NO_PROPOSALS,
  dashboard,
  healthSignal,
  hubInfo,
  hubProject,
  hubProjectView,
  hubProposal,
  hubStatus,
  RC_AVAILABLE,
} from "../test/fixtures";
import { renderGestorTemplate, scopeLine } from "../project/templates";
import { DEFAULT_GESTOR_TEMPLATE } from "../settings/defaults";
import { valueText } from "./HubProposals";
import { liveGestorLabel } from "./HubGestor";
import type { HubSection } from "./sections";

const NOW = Date.now();

function info(agent = true): EnvInfo {
  return {
    protocol: "2.1",
    version: "0.1.0",
    commit: "",
    hostname: "wsl",
    worktrees_root: "/w",
    env_id: "daemon-wsl",
    env_name: "wsl-pc",
    hub_sync: null,
    hub_config: { ...HUB_CONFIG_UNLINKED, linked: true, config_rev: 7, pulled_at: NOW, agent_token: agent },
    update: null,
    remote_control: RC_AVAILABLE,
    docs: { root: null, reachable: false },
  };
}

const CT = hubProject();
const DEMO = hubProject({ id: 2, gh_repo: "demo", name: "demo" });

function gestorSession(p: Partial<Session> = {}): Session {
  return {
    id: "s-gestor",
    work_item_id: 900,
    tool: "claude",
    tool_session_id: null,
    transcript_path: null,
    cwd: "/home/u/.local/share/ctower/gestor",
    state: "running",
    exit_code: null,
    pid: 1,
    started_at: NOW - 60_000,
    last_event_at: NOW,
    ended_at: null,
    launch_cmd: [],
    model: null,
    mode: "native",
    ...p,
  } as Session;
}

function setup(
  opts: {
    agent?: boolean;
    /** Sessões vivas do gestor e o escopo de cada uma (por id). */
    live?: Session[];
    scopes?: Dashboard["gestor_scopes"];
    proposals?: () => HubProposals;
    health?: HubHealth;
    section?: HubSection;
  } = {},
) {
  const api = new FakeDaemonApi([{ ...FakeDaemonApi.wslEnv(), name: "WSL" }])
    .on(
      "dashboard.get",
      dashboard({
        projects: [],
        gestor_work_item_id: opts.live ? 900 : null,
        live_sessions: opts.live ?? [],
        gestor_scopes: opts.scopes ?? {},
      }),
    )
    .on("env.info", info(opts.agent ?? true))
    .on("session.start_gestor", gestorSession())
    .withHub("ws://mac-mini:7879/ws")
    .onHub("hub.info", hubInfo())
    .onHub("hub.status", hubStatus())
    .onHub("hub.projects.list", [hubProjectView(CT), hubProjectView(DEMO)])
    .onHub("hub.envs.list", [])
    .onHub("hub.dashboard.get", HUB_DASHBOARD)
    .onHub("hub.proposals.list", (p: { state: string }) =>
      p.state === "pendente" || !opts.proposals ? (opts.proposals?.() ?? NO_PROPOSALS) : opts.proposals(),
    )
    .onHub("hub.health.get", opts.health ?? NO_HEALTH)
    .onHub("github.status", { source: "token", login: "hub-bot", error: null });
  render(<App api={api} initial={{ name: "hub", section: opts.section }} />);
  return { api, user: userEvent.setup() };
}

test("template do gestor: linha de foco por escopo e variáveis", () => {
  const t = renderGestorTemplate(DEFAULT_GESTOR_TEMPLATE, "wsl-pc", { kind: "project", owner: "o", repo: "r" });
  expect(t).toContain("no ambiente wsl-pc.");
  expect(t).toContain("Foco: o/r. Comece por portfolio_project");
  expect(t).not.toContain("{{");
  expect(scopeLine({ kind: "health" })).toContain("portfolio_health");
  expect(renderGestorTemplate("{{scope}} {{x}}", "e", { kind: "portfolio" })).toBe(
    `${scopeLine({ kind: "portfolio" })} {{x}}`,
  );
});

test("gestor: escolhe ambiente e escopo, inicia e abre o terminal", async () => {
  const { api, user } = setup();
  await user.click(await screen.findByRole("button", { name: "Abrir sessão do gestor" }));
  const dlg = screen.getByRole("region", { name: "Iniciar o gestor de projetos" });
  await user.click(within(dlg).getByRole("radio", { name: "Um projeto" }));
  await user.selectOptions(within(dlg).getByLabelText("Projeto do escopo"), "deep-ion-ai/demo");
  const prompt = within(dlg).getByLabelText("Prompt inicial (template do gestor)") as HTMLTextAreaElement;
  expect(prompt.value).toContain("Foco: deep-ion-ai/demo.");
  expect(prompt.value).toContain("no ambiente WSL.");
  await user.selectOptions(within(dlg).getByLabelText("Modelo"), "opus");
  await user.click(within(dlg).getByRole("button", { name: "▶ Iniciar gestor" }));
  await waitFor(() => expect(api.callsTo("session.start_gestor")).toHaveLength(1));
  const c = api.callsTo("session.start_gestor")[0];
  expect(c.env).toBe("wsl");
  expect(c.params).toMatchObject({
    scope: { kind: "project", owner: "deep-ion-ai", repo: "demo" },
    model: "opus",
    mode: "native",
  });
  expect(c.params.prompt).toContain("Foco: deep-ion-ai/demo.");
  // Abre o terminal da sessão nova.
  expect(await screen.findByRole("button", { name: "Terminal", description: "1 sessão" })).toHaveAttribute("aria-current", "page");
});

test("gestor: sem token de agente explica como gravar", async () => {
  const { user } = setup({ agent: false });
  await user.click(await screen.findByRole("button", { name: "Abrir sessão do gestor" }));
  expect(screen.getByText(/Nenhum ambiente conectado tem o token de agente/)).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "▶ Iniciar gestor" })).toBeDisabled();
});

test("rótulo da sessão viva: ambiente, escopo e hora de início", () => {
  const at = new Date(2026, 9, 20, 9, 5).getTime();
  const base = { env: "wsl", name: "WSL", session: "s", started_at: at };
  expect(liveGestorLabel({ ...base, scope: { kind: "project", owner: "o", repo: "r" } })).toBe(
    "Abrir gestor · WSL · o/r · 09:05",
  );
  expect(liveGestorLabel({ ...base, scope: { kind: "health" } })).toBe("Abrir gestor · WSL · Saúde e riscos · 09:05");
  // Ambiente que não informa o escopo: só ambiente e hora.
  expect(liveGestorLabel(base)).toBe("Abrir gestor · WSL · 09:05");
});

test("gestor vivo: inicia uma segunda sessão e abre cada uma pela tela Hub", async () => {
  const a = gestorSession({ id: "s-gestor-a", started_at: new Date(2026, 9, 20, 9, 5).getTime() });
  const b = gestorSession({ id: "s-gestor-b", started_at: new Date(2026, 9, 20, 10, 40).getTime() });
  const { api, user } = setup({
    // Fora de ordem: a lista vem da mais antiga para a mais nova.
    live: [b, a],
    scopes: { "s-gestor-a": { kind: "portfolio" }, "s-gestor-b": { kind: "health" } },
  });
  api.on("session.start_gestor", gestorSession({ id: "s-gestor-c" }));
  const lista = await screen.findByRole("list", { name: "Sessões vivas do gestor" });
  expect(within(lista).getAllByRole("button").map((x) => x.textContent)).toEqual([
    "Abrir gestor · WSL · Portfólio · 09:05",
    "Abrir gestor · WSL · Saúde e riscos · 10:40",
  ]);

  // O botão do gestor segue abrindo o diálogo de sessão nova.
  await user.click(screen.getByRole("button", { name: "Abrir sessão do gestor" }));
  const dlg = screen.getByRole("region", { name: "Iniciar o gestor de projetos" });
  await user.click(within(dlg).getByRole("radio", { name: "Saúde e riscos" }));
  await user.click(within(dlg).getByRole("button", { name: "▶ Iniciar gestor" }));
  await waitFor(() => expect(api.callsTo("session.start_gestor")).toHaveLength(1));
  expect(api.callsTo("session.start_gestor")[0].params).toMatchObject({ scope: { kind: "health" } });
  expect(await screen.findByRole("button", { name: "Terminal", description: "1 sessão" })).toHaveAttribute("aria-current", "page");

  // Cada sessão viva abre o próprio terminal.
  for (const [n, label] of [
    [2, "Abrir gestor · WSL · Portfólio · 09:05"],
    [3, "Abrir gestor · WSL · Saúde e riscos · 10:40"],
  ] as const) {
    await user.click(screen.getByRole("button", { name: "Hub" }));
    await user.click(await screen.findByRole("button", { name: label }));
    expect(await screen.findByRole("button", { name: "Terminal", description: `${n} sessões` })).toHaveAttribute("aria-current", "page");
  }
  // A árvore distingue as sessões pelo escopo, no grupo do gestor.
  const grupo = screen.getByRole("treeitem", { name: /^Gestor de projetos, 2 sessões/ });
  expect(within(grupo).getByRole("treeitem", { name: /Gestor de projetos · Portfólio/ })).toBeInTheDocument();
  expect(within(grupo).getByRole("treeitem", { name: /Gestor de projetos · Saúde e riscos/ })).toBeInTheDocument();
});

test("valor da proposta legível", () => {
  expect(valueText("priority", { priority: null })).toBe("não triado");
  expect(valueText("priority", { priority: "P1", before: 12 })).toBe("P1 · antes de #12");
  expect(valueText("status", { status: "pausado" })).toBe("pausado");
});

test("propostas: aprovar uma, rejeitar em lote com nota, histórico e push", async () => {
  let pending: HubProposal[] = [
    hubProposal(),
    hubProposal({ id: 2, kind: "status", number: 12, current: { status: "backlog" }, proposed: { status: "pausado" } }),
    hubProposal({ id: 3, number: 13 }),
  ];
  let past: HubProposal[] = [hubProposal({ id: 9, state: "obsoleta", decision_note: "substituída pela proposta #1" })];
  const { api, user } = setup({
    proposals: () => ({ proposals: [...pending, ...past], truncated: false }),
    section: "proposals",
  });
  // O fake responde a lista inteira às duas chamadas; a seção separa as pendentes.
  api.onHub("hub.proposals.list", (p: { state: string }) => ({
    proposals: p.state === "pendente" ? pending : [...pending, ...past],
    truncated: false,
  }));
  api.onHub("hub.proposals.decide", (p: { id?: number; ids?: number[]; approve: boolean; note?: string }) => {
    if (p.id !== undefined) {
      const x = pending.find((y) => y.id === p.id)!;
      pending = pending.filter((y) => y.id !== p.id);
      past = [{ ...x, state: p.approve ? "aprovada" : "rejeitada", decided_at: NOW }, ...past];
      return past[0];
    }
    const out: ProposalDecided[] = p.ids!.map((id) => ({
      id,
      proposal: { ...pending.find((y) => y.id === id)!, state: "rejeitada" },
      error: null,
    }));
    pending = pending.filter((y) => !p.ids!.includes(y.id));
    return out;
  });
  const sec = await screen.findByRole("region", { name: /Propostas do gestor/ });
  const first = await within(sec).findByRole("row", { name: "Proposta #1" });
  expect(first).toHaveTextContent("deep-ion-ai/ctower-agents");
  expect(first).toHaveTextContent("#215");
  expect(first).toHaveTextContent("não triado → P0");
  const status = within(sec).getByRole("table", { name: "Propostas de status" });
  expect(within(status).getByRole("row", { name: "Proposta #2" })).toHaveTextContent("backlog → pausado");
  expect(first).toHaveTextContent("bloqueia o resto do épico");
  expect(first).toHaveTextContent("wsl-pc");
  expect(within(sec).getByLabelText("3 pendente(s)")).toBeInTheDocument();
  await user.click(within(first).getByRole("button", { name: "Aprovar a proposta #1" }));
  await waitFor(() => expect(api.hubCallsTo("hub.proposals.decide")).toHaveLength(1));
  expect(api.hubCallsTo("hub.proposals.decide")[0].params).toEqual({ id: 1, approve: true });
  expect(await within(sec).findByText("Proposta #1 aprovada.")).toBeInTheDocument();

  await user.click(within(sec).getByRole("checkbox", { name: "Marcar a proposta #2" }));
  await user.click(within(sec).getByRole("checkbox", { name: "Marcar a proposta #3" }));
  await user.type(within(sec).getByLabelText(/Nota/), "fora do trimestre");
  await user.click(within(sec).getByRole("button", { name: "Rejeitar marcadas (2)" }));
  await waitFor(() => expect(api.hubCallsTo("hub.proposals.decide")).toHaveLength(2));
  expect(api.hubCallsTo("hub.proposals.decide")[1].params).toEqual({
    ids: [2, 3],
    approve: false,
    note: "fora do trimestre",
  });
  expect(await within(sec).findByText("Nenhuma proposta pendente.")).toBeInTheDocument();
  expect(within(sec).getByText(/Decididas e obsoletas · 2/)).toBeInTheDocument();
  expect(within(sec).getByText("substituída pela proposta #1")).toBeInTheDocument();

  // Push do hub relê a lista.
  pending = [hubProposal({ id: 4, number: 40 })];
  const before = api.hubCallsTo("hub.proposals.list").length;
  act(() => api.hubPush("hub.proposals.changed"));
  expect(await within(sec).findByRole("row", { name: "Proposta #4" })).toBeInTheDocument();
  expect(api.hubCallsTo("hub.proposals.list").length).toBeGreaterThan(before);
});

test("proposta obsoleta na decisão: avisa o motivo", async () => {
  const p = hubProposal();
  const { api, user } = setup({ proposals: () => ({ proposals: [p], truncated: false }), section: "proposals" });
  api.onHub("hub.proposals.decide", { ...p, state: "obsoleta", decision_note: "valor atual mudou: prioridade P2" });
  const sec = await screen.findByRole("region", { name: /Propostas do gestor/ });
  await user.click(await within(sec).findByRole("button", { name: "Aprovar a proposta #1" }));
  expect(await within(sec).findByRole("alert")).toHaveTextContent("ficou obsoleta: valor atual mudou: prioridade P2");
});

test("saúde: sinais por gravidade e link para o projeto", async () => {
  const health: HubHealth = {
    generated_at: NOW,
    signals: [
      healthSignal(),
      healthSignal({ kind: "nao_triado", severity: "info", number: null, priority: null, value: "12 issues" }),
      healthSignal({
        kind: "ambiente_mudo",
        severity: "aviso",
        owner: null,
        repo: null,
        number: null,
        env: "mac",
        value: "2 h",
      }),
    ],
    counts: { critico: 1, aviso: 1, info: 1 },
  };
  const { user } = setup({ health, section: "health" });
  const sec = await screen.findByRole("region", { name: /Saúde e riscos/ });
  expect(sec).toHaveTextContent("1 crítico(s), 1 aviso(s)");
  expect(within(sec).getByText("bloqueado")).toBeInTheDocument();
  expect(within(sec).getAllByText("8 dias")).toHaveLength(1);
  expect(within(sec).getByText("mac")).toBeInTheDocument();
  // Informativos só sob demanda.
  expect(within(sec).queryByText("issues não triadas")).toBeNull();
  await user.click(within(sec).getByLabelText("mostrar informativos"));
  expect(within(sec).getByText("issues não triadas")).toBeInTheDocument();
  // Contador da seção no painel lateral: só aviso e crítico.
  // (A contagem por projeto fica no índice de Projetos desde a #18: ProjectIndex.test.tsx.)
  expect(screen.getByRole("button", { name: "Saúde dos projetos" })).toHaveAccessibleDescription("2 sinais");
  await user.click(within(sec).getByRole("button", { name: "deep-ion-ai/ctower-agents#176" }));
  expect(await screen.findByRole("heading", { level: 1, name: /ctower-agents/ })).toBeInTheDocument();
});

test("propostas de prioridade: tabela na ordem resultante, com sobe/desce e lote", async () => {
  const pending: HubProposal[] = [
    // Mais nova primeiro, como o hub devolve; a tabela reordena.
    hubProposal({
      id: 7,
      number: 30,
      title: null,
      created_at: NOW - 1000,
      current: { priority: null },
      proposed: { priority: "P1" },
    }),
    hubProposal({
      id: 6,
      number: 12,
      created_at: NOW - 2000,
      current: { priority: "P1" },
      proposed: { priority: "P2" },
      justification: "x".repeat(300),
    }),
    hubProposal({
      id: 5,
      number: 11,
      created_at: NOW - 3000,
      current: { priority: "P1" },
      proposed: { priority: "P0" },
    }),
  ];
  const { api, user } = setup({ proposals: () => ({ proposals: pending, truncated: false }), section: "proposals" });
  const item = (position: number, number: number, priority: "P0" | "P1" | "P2" | null) => ({
    position,
    owner: "deep-ion-ai",
    repo: "ctower-agents",
    number,
    title: `Issue ${number}`,
    kind: "issue" as const,
    status: "backlog" as const,
    priority,
    rank: 1,
  });
  api.onHub("hub.ranking.get", {
    items: [item(1, 10, "P0"), item(2, 11, "P1"), item(3, 12, "P1"), item(4, 30, null)],
    truncated: false,
  });
  api.onHub("hub.proposals.decide", (p: { ids: number[] }) =>
    p.ids.map((id) => ({ id, proposal: { ...pending.find((y) => y.id === id)!, state: "aprovada" }, error: null })),
  );
  const sec = await screen.findByRole("region", { name: /Propostas do gestor/ });
  const table = await within(sec).findByRole("table", { name: "Propostas de prioridade" });
  // Resultado: P0 #10, P0 #11 (=), P1 #30 (sobe, sai de não triado), P2 #12 (desce).
  await waitFor(() => expect(within(table).getByRole("row", { name: "Proposta #5" })).toHaveTextContent("2 → 2"));
  const rows = within(table).getAllByRole("row").slice(1);
  expect(rows.map((r) => r.getAttribute("aria-label"))).toEqual(["Proposta #5", "Proposta #7", "Proposta #6"]);
  expect(rows[0]).toHaveTextContent("=");
  expect(rows[1]).toHaveTextContent("4 → 3 ↑1");
  expect(rows[1]).toHaveTextContent("Issue 30");
  expect(rows[2]).toHaveTextContent("3 → 4 ↓1");
  expect(within(rows[1]).getByLabelText("sobe 1")).toHaveAttribute("data-move", "up");
  expect(within(rows[2]).getByLabelText("desce 1")).toHaveAttribute("data-move", "down");
  // Justificativa longa vem resumida, expansível.
  expect(within(rows[2]).getByText(/^x+…$/)).toBeInTheDocument();
  expect(api.hubCallsTo("hub.ranking.get")[0].params).toEqual({ limit: 200 });

  await user.click(within(sec).getByRole("button", { name: "Marcar todas" }));
  await user.click(within(sec).getByRole("button", { name: "Aprovar marcadas (3)" }));
  await waitFor(() => expect(api.hubCallsTo("hub.proposals.decide")).toHaveLength(1));
  const call = api.hubCallsTo("hub.proposals.decide")[0].params as { ids: number[]; approve: boolean };
  expect(call.approve).toBe(true);
  expect([...call.ids].sort()).toEqual([5, 6, 7]);
  expect(await within(sec).findByText("3 proposta(s) aprovada(s).")).toBeInTheDocument();
});

test("propostas de prioridade: sem ranking, a tabela sai sem posições e avisa", async () => {
  setup({ proposals: () => ({ proposals: [hubProposal()], truncated: false }), section: "proposals" });
  const sec = await screen.findByRole("region", { name: /Propostas do gestor/ });
  expect(await within(sec).findByText(/Ranking atual indisponível/)).toBeInTheDocument();
  expect(within(sec).getByRole("row", { name: "Proposta #1" })).toHaveTextContent("—");
});
