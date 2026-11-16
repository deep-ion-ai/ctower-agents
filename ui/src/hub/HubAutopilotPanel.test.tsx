// Hub › Painel do piloto (#185): estado, "agora pode usar até X%", fila com
// os motivos, sessões vivas e estacionadas, pausar e retomar (relido pelo
// push `hub.autopilot.changed`), na web também, com a conta anonimizada.
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { App } from "../App";
import { FakeDaemonApi } from "../api/fake";
import type { AutopilotSettings, AutopilotStatus, BudgetDecision, MorningDigest } from "../protocol/hub-gen";
import { deepLinkTab } from "../web/pwa";
import { hubTab } from "./sections";
import { dashboard, hubEnv, hubInfo, hubProject, hubProjectView, hubStatus, HUB_DASHBOARD } from "../test/fixtures";
import { webApi } from "../test/web";
import { pilotState } from "./autopilot";

const NOW = Date.now();
/** Item da fila sem dependências (#215). */
const semDeps = { blocked_by: [], dependencies: [], order: null };

function decision(d: Partial<BudgetDecision> = {}): BudgetDecision {
  return {
    can_start: false,
    reasons: [{ kind: "band_closed", band: "Horário comercial" }],
    band: "Horário comercial",
    five_hour_cap: null,
    five_hour_used: 20,
    weekly_used: 30,
    weekly: {
      resets_at: NOW + 3 * 86_400_000,
      workdays_total: 5,
      workdays_left: 3,
      reserve_limit: 64,
      pace: 0.4,
      pace_limit: 45,
      effective: 45,
    },
    next_open_at: NOW + 4 * 3_600_000,
    now: NOW,
    ...d,
  };
}

function status(account: string, settings: Partial<AutopilotSettings> = {}): AutopilotStatus {
  return {
    settings: {
      paused: false,
      paused_at: null,
      max_sessions_per_env: 1,
      idle_timeout_min: 30,
      max_ci_attempts: 3,
      waiting_timeout_min: 20,
      ...settings,
    },
    last_tick_at: NOW - 30_000,
    now: NOW,
    accounts: [{ account, decision: decision(), exhausted_window: null, exhausted_until: null }],
    projects: [
      {
        owner: "acme",
        repo: "acme-web",
        env_id: "env-auto",
        env_name: "env-auto",
        env_online: true,
        account,
        blocked: "budget:band_closed",
        blocked_message: "a política da conta não permite iniciar agora: faixa fechada (Horário comercial)",
        next: null,
        probe: false,
        queue: [
          { number: 123, title: "ACME-123 login", priority: "P1", model: "opus", skip: null, ...semDeps },
          { number: 124, title: "ACME-124 busca", priority: "P1", model: null, skip: "no_model", ...semDeps },
          { number: 125, title: "ACME-125 cadastro", priority: "P2", model: "opus", skip: "not_approved", ...semDeps },
        ],
        analysis: null,
        analysis_unsupported: false,
      },
    ],
    sessions: [
      {
        session_id: "s-1",
        env_id: "env-auto",
        state: "running",
        owner: "acme",
        repo: "acme-web",
        gh_number: 120,
        started_at: NOW - 600_000,
      },
    ],
    parked: [
      {
        session_id: "s-2",
        env_id: "env-auto",
        account_key: account,
        owner: "acme",
        repo: "acme-web",
        gh_number: 121,
        window: "five_hour",
        resume_at: NOW + 3_600_000,
        parked_at: NOW - 60_000,
      },
    ],
    events: [
      {
        id: 7,
        at: NOW - 120_000,
        kind: "parked",
        scope: "item:acme/acme-web#121",
        reason: "five_hour",
        message: "acme/acme-web#121: limite do plano atingido (janela de 5h)",
        account_key: account,
        env_id: "env-auto",
        owner: "acme",
        repo: "acme-web",
        gh_number: 121,
        session_id: "s-2",
      },
    ],
    wip: [
      {
        owner: "acme",
        repo: "acme-web",
        name: "acme-web",
        branch: "wip/autonoma",
        base: "develop",
        ahead: [
          { number: 118, title: "ACME-118", reason: null },
          { number: 119, title: "ACME-119", reason: null },
        ],
        pr_url: "https://github.example/acme/acme-web/pull/5",
        ci: null,
        conflict: false,
        error: null,
      },
    ],
  };
}

/** `hub.autopilot.*` como o hub: pausar e retomar mudam o estado e avisam pelo push. */
function withAutopilot<T extends FakeDaemonApi>(api: T, account: string) {
  let settings: AutopilotSettings = {
    paused: false,
    paused_at: null,
    max_sessions_per_env: 1,
    idle_timeout_min: 30,
    max_ci_attempts: 3,
    waiting_timeout_min: 20,
  };
  const set = (paused: boolean) => {
    settings = { ...settings, paused, paused_at: paused ? NOW : null };
    queueMicrotask(() => api.hubPush("hub.autopilot.changed"));
    return settings;
  };
  return api
    .onHub("hub.autopilot.status", () => status(account, settings))
    .onHub("hub.autopilot.pause", () => set(true))
    .onHub("hub.autopilot.resume", () => set(false))
    .onHub("hub.autopilot.set", (p: unknown) => {
      settings = { ...settings, ...(p as Partial<AutopilotSettings>) };
      return settings;
    });
}

function desktop() {
  const api = withAutopilot(
    new FakeDaemonApi()
      .on("dashboard.get", dashboard({ projects: [] }))
      .withHub("ws://mac-mini:7879/ws")
      .onHub("hub.info", hubInfo({ protocol: "1.20" }))
      .onHub("hub.status", hubStatus())
      .onHub("hub.projects.list", [hubProjectView(hubProject())])
      .onHub("hub.envs.list", [hubEnv()])
      .onHub("hub.dashboard.get", HUB_DASHBOARD),
    "org-a/fulano@example.com",
  );
  render(<App api={api} initial={{ name: "hub", section: "autopilot-panel" }} />);
  return { api, user: userEvent.setup() };
}

test("estado: pausado, desligado, ligado, aguardando faixa ou orçamento", () => {
  const s = status("conta-1");
  expect(pilotState(s)).toBe("waiting_band");
  expect(pilotState({ ...s, settings: { ...s.settings, paused: true } })).toBe("paused");
  expect(pilotState({ ...s, accounts: [] })).toBe("off");
  const open = { ...s.accounts[0], decision: decision({ can_start: true, reasons: [] }) };
  expect(pilotState({ ...s, accounts: [open] })).toBe("on");
  const over = {
    ...s.accounts[0],
    decision: decision({ reasons: [{ kind: "weekly_over_limit", used: 50, limit: 45 }] }),
  };
  expect(pilotState({ ...s, accounts: [over] })).toBe("waiting_budget");
  const noReading = { ...s.accounts[0], decision: decision({ reasons: [{ kind: "no_reading", window: "five_hour" }] }) };
  const probe = { ...s.projects[0], probe: true };
  expect(pilotState({ ...s, accounts: [noReading], projects: [probe] })).toBe("on");
});

test("painel: estado, limite da semanal, fila com os motivos, wip e sessões", async () => {
  desktop();
  const panel = await screen.findByRole("region", { name: "Painel do piloto automático" });
  expect(await within(panel).findByRole("status", { name: "Estado do piloto" })).toHaveTextContent(
    "Aguardando faixa: a faixa de horário atual não permite iniciar",
  );
  const accounts = within(panel).getByRole("list", { name: "Contas" });
  expect(accounts).toHaveTextContent("Agora pode usar até 45% da semanal (em 30%)");
  expect(accounts).toHaveTextContent("faixa fechada (Horário comercial)");
  expect(accounts).toHaveTextContent("próxima faixa elegível:");

  const queue = within(panel).getByRole("region", { name: "Fila de acme/acme-web" });
  expect(queue).toHaveTextContent("Nada inicia agora: a política da conta não permite iniciar agora");
  const rows = within(queue).getAllByRole("row");
  expect(rows[1]).toHaveTextContent("elegível");
  expect(rows[2]).toHaveTextContent("fora: sem modelo");
  expect(rows[3]).toHaveTextContent("fora: sem rótulo de aprovação");
  expect(queue).toHaveTextContent("wip/autonoma: 2 issues à frente de develop");
  expect(within(queue).getByRole("link", { name: "PR wip/autonoma → develop" })).toHaveAttribute(
    "href",
    "https://github.example/acme/acme-web/pull/5",
  );

  const sessions = within(panel).getByRole("list", { name: "Sessões autônomas" });
  expect(sessions).toHaveTextContent("acme/acme-web#120 · running");
  expect(sessions).toHaveTextContent("acme/acme-web#121 · estacionada");
  expect(sessions).toHaveTextContent("limite da janela de 5h");
  expect(within(panel).getByRole("list", { name: "Eventos recentes" })).toHaveTextContent("limite do plano atingido");
  // No desktop, o atalho para o Orçamento.
  expect(within(panel).getByRole("button", { name: "Orçamento do plano" })).toBeInTheDocument();
});

test("painel: a sonda sem leitura do plano e o erro da branch de integração", async () => {
  const base = status("org-a/fulano@example.com");
  const s: AutopilotStatus = {
    ...base,
    projects: [{ ...base.projects[0], blocked: null, blocked_message: null, next: 123, probe: true }],
    wip: base.wip.map((w) => ({ ...w, error: "push de wip/autonoma recusado (403)" })),
  };
  const api = new FakeDaemonApi()
    .on("dashboard.get", dashboard({ projects: [] }))
    .withHub("ws://mac-mini:7879/ws")
    .onHub("hub.info", hubInfo({ protocol: "1.20" }))
    .onHub("hub.status", hubStatus())
    .onHub("hub.projects.list", [hubProjectView(hubProject())])
    .onHub("hub.envs.list", [hubEnv()])
    .onHub("hub.dashboard.get", HUB_DASHBOARD)
    .onHub("hub.autopilot.status", () => s);
  render(<App api={api} initial={{ name: "hub", section: "autopilot-panel" }} />);
  const queue = await screen.findByRole("region", { name: "Fila de acme/acme-web" });
  expect(queue).toHaveTextContent("Próxima a iniciar: #123 (sonda: a conta não tem leitura do plano");
  expect(queue).toHaveTextContent("erro na branch: push de wip/autonoma recusado (403)");
});

test("pausar tudo e retomar: chama o hub e relê pelo push", async () => {
  const { api, user } = desktop();
  const panel = await screen.findByRole("region", { name: "Painel do piloto automático" });
  await user.click(await within(panel).findByRole("button", { name: "Pausar tudo" }));
  expect(api.calls.some((c) => c.method === "hub.autopilot.pause")).toBe(true);
  await waitFor(() =>
    expect(within(panel).getByRole("status", { name: "Estado do piloto" })).toHaveTextContent("Pausado"),
  );
  const reads = api.calls.filter((c) => c.method === "hub.autopilot.status").length;
  expect(reads).toBeGreaterThan(1);

  await user.click(within(panel).getByRole("button", { name: "Retomar" }));
  expect(api.calls.some((c) => c.method === "hub.autopilot.resume")).toBe(true);
  await waitFor(() =>
    expect(within(panel).getByRole("status", { name: "Estado do piloto" })).toHaveTextContent("Aguardando faixa"),
  );
  expect(within(panel).getByRole("button", { name: "Pausar tudo" })).toBeInTheDocument();

  // Mudança vinda de fora (outra UI, o agendador): o push relê.
  const before = api.calls.filter((c) => c.method === "hub.autopilot.status").length;
  api.hubPush("hub.autopilot.changed");
  await waitFor(() =>
    expect(api.calls.filter((c) => c.method === "hub.autopilot.status").length).toBeGreaterThan(before),
  );
});

test("web (PWA): o painel aparece, com a conta anonimizada, e pausa", async () => {
  const api = withAutopilot(webApi(), "conta-1");
  render(<App api={api} initial={{ name: "hub", section: "autopilot-panel" }} />);
  const user = userEvent.setup();
  const panel = await screen.findByRole("region", { name: "Painel do piloto automático" });
  const accounts = await within(panel).findByRole("list", { name: "Contas" });
  expect(accounts).toHaveTextContent("conta-1");
  expect(panel).not.toHaveTextContent("example.com");
  // O Orçamento (política, com a chave da conta) fica só no desktop.
  expect(within(panel).queryByRole("button", { name: "Orçamento do plano" })).toBeNull();
  await user.click(within(panel).getByRole("button", { name: "Pausar tudo" }));
  await waitFor(() =>
    expect(within(panel).getByRole("status", { name: "Estado do piloto" })).toHaveTextContent("Pausado"),
  );
  expect(api.calls.some((c) => c.method === "hub.autopilot.pause")).toBe(true);
});

test("hub anterior ao agendador: avisa para atualizar", async () => {
  const api = new FakeDaemonApi()
    .on("dashboard.get", dashboard({ projects: [] }))
    .withHub("ws://mac-mini:7879/ws")
    .onHub("hub.info", hubInfo({ protocol: "1.18" }))
    .onHub("hub.status", hubStatus())
    .onHub("hub.projects.list", [])
    .onHub("hub.envs.list", [])
    .onHub("hub.dashboard.get", HUB_DASHBOARD)
    .onHub("hub.autopilot.status", () => {
      throw new Error("método desconhecido: hub.autopilot.status");
    });
  render(<App api={api} initial={{ name: "hub", section: "autopilot-panel" }} />);
  expect(await screen.findByRole("alert")).toHaveTextContent("anterior ao protocolo 1.19");
});

function digest(account: string): MorningDigest {
  return {
    id: 3,
    local_day: 20_401,
    at: NOW - 3_600_000,
    since: NOW - 25 * 3_600_000,
    projects: [
      {
        owner: "acme",
        repo: "acme-web",
        name: "acme-web",
        branch: "wip/autonoma",
        base: "develop",
        merged: [
          { number: 123, title: "ACME-123", reason: null },
          { number: 124, title: "ACME-124", reason: null },
        ],
        blocked: [{ number: 125, title: "ACME-125", reason: "piloto automático: o CI do PR #9 falhou depois de 3 tentativas de correção" }],
        decisions_pending: 3,
        decisions_new: 1,
        ahead: 2,
        pr_url: "https://github.example/acme/acme-web/pull/5",
        ci: null,
        conflict: false,
        error: null,
        waiting_blocker: 0,
      },
    ],
    usage: [{ account, weekly_used: 35, resets_at: NOW + 86_400_000, weekly_before: 20, delta: 15 }],
  };
}

test("resumo da manhã: card na Visão geral com contagens, PR e uso; relido pelo push", async () => {
  let d: MorningDigest[] = [];
  const api = new FakeDaemonApi()
    .on("dashboard.get", dashboard({ projects: [] }))
    .withHub("ws://mac-mini:7879/ws")
    .onHub("hub.info", hubInfo({ protocol: "1.20" }))
    .onHub("hub.status", hubStatus())
    .onHub("hub.projects.list", [hubProjectView(hubProject())])
    .onHub("hub.envs.list", [hubEnv()])
    .onHub("hub.dashboard.get", HUB_DASHBOARD)
    .onHub("hub.autopilot.digest", () => d);
  render(<App api={api} initial={{ name: "hub" }} />);
  await waitFor(() => expect(api.calls.some((c) => c.method === "hub.autopilot.digest")).toBe(true));
  // Sem resumo ainda: o card não aparece.
  expect(screen.queryByRole("region", { name: "Resumo da manhã" })).toBeNull();
  d = [digest("org-a/fulano@example.com")];
  api.hubPush("hub.autopilot.changed");
  const card = await screen.findByRole("region", { name: "Resumo da manhã" });
  const projects = within(card).getByRole("list", { name: "Projetos do resumo" });
  expect(projects).toHaveTextContent(
    "acme-web: 2 mescladas na wip · 1 bloqueada · 3 decisões para revisar · 2 à frente da base",
  );
  expect(projects).toHaveTextContent("Mescladas na wip/autonoma: #123, #124");
  // O motivo da bloqueada (#184).
  expect(projects).toHaveTextContent(
    "Bloqueadas: #125 (piloto automático: o CI do PR #9 falhou depois de 3 tentativas de correção)",
  );
  expect(projects).toHaveTextContent("CI sem dados no espelho");
  expect(within(card).getByRole("link", { name: "PR wip/autonoma → develop" })).toHaveAttribute(
    "href",
    "https://github.example/acme/acme-web/pull/5",
  );
  expect(within(card).getByRole("list", { name: "Uso da semanal no período" })).toHaveTextContent(
    "+15% no período, semanal em 35%",
  );
});

test("resumo da manhã na web: a conta chega anonimizada; o link da notificação abre a Visão geral", async () => {
  const api = webApi().onHub("hub.autopilot.digest", () => [digest("conta-1")]);
  render(<App api={api} initial={{ name: "hub" }} />);
  const card = await screen.findByRole("region", { name: "Resumo da manhã" });
  expect(card).toHaveTextContent("conta-1: +15% no período");
  expect(card).not.toHaveTextContent("example.com");
  expect(deepLinkTab("?go=hub")).toEqual(hubTab("overview"));
  expect(deepLinkTab("?go=hub:autopilot-panel")).toEqual(hubTab("autopilot-panel"));
});

test("ajustes do agendador: tentativas do CI e “Aguardando você” pelo hub.autopilot.set (#184)", async () => {
  const { api, user } = desktop();
  const panel = await screen.findByRole("region", { name: "Painel do piloto automático" });
  expect(
    await within(panel).findByText(/devolve a falha do CI ao agente até 3 vezes/),
  ).toBeInTheDocument();
  await user.click(within(panel).getByText("Ajustes do agendador"));
  const attempts = within(panel).getByLabelText("Tentativas de correção do CI (0 = bloqueia na primeira falha)");
  const waiting = within(panel).getByLabelText("Minutos em “Aguardando você” (0 = nunca)");
  expect(attempts).toHaveValue(3);
  expect(waiting).toHaveValue(20);
  await user.clear(attempts);
  await user.type(attempts, "11");
  await user.click(within(panel).getByRole("button", { name: "Salvar ajustes" }));
  expect(await within(panel).findByRole("alert")).toHaveTextContent("de 0 a 10");
  await user.clear(attempts);
  await user.type(attempts, "5");
  await user.clear(waiting);
  await user.type(waiting, "0");
  await user.click(within(panel).getByRole("button", { name: "Salvar ajustes" }));
  await waitFor(() =>
    expect(api.calls.find((c) => c.method === "hub.autopilot.set")?.params).toEqual({
      max_ci_attempts: 5,
      waiting_timeout_min: 0,
    }),
  );
  expect(await within(panel).findByText(/Ajustes do piloto automático salvos/)).toBeInTheDocument();
  expect(within(panel).getByText(/não é bloqueada/)).toBeInTheDocument();
});

test("fila com dependências (#215): bloqueada por, ordem, análise e correção manual", async () => {
  const { api, user } = desktop();
  const base = status("org-a/fulano@example.com");
  const withDeps: AutopilotStatus = {
    ...base,
    projects: [
      {
        ...base.projects[0],
        analysis: {
          state: "reanalysing",
          session_id: "s-analise",
          opened_at: NOW - 3_600_000,
          planned_at: NOW - 3_000_000,
          pending: [125],
          failed_at: null,
        },
        queue: [
          {
            number: 123,
            title: "ACME-123 login",
            priority: "P1",
            model: "opus",
            skip: "blocked_by",
            blocked_by: [120],
            dependencies: [{ blocker: 120, reason: "usa a tabela nova", source: "analysis", removed: false, state: "pending" }],
            order: 2,
          },
          {
            number: 124,
            title: "ACME-124 busca",
            priority: "P1",
            model: "opus",
            skip: null,
            blocked_by: [],
            dependencies: [{ blocker: 121, reason: "", source: "manual", removed: true, state: "pending" }],
            order: 1,
          },
        ],
      },
    ],
  };
  api.onHub("hub.autopilot.status", () => withDeps).onHub("hub.autopilot.deps.set", () => ({ dependencies: [] }));
  const panel = await screen.findByRole("region", { name: "Painel do piloto automático" });
  const queue = await within(panel).findByRole("region", { name: "Fila de acme/acme-web" });
  await waitFor(() => expect(queue).toHaveTextContent("Análise de dependências: reanalisando #125."));
  const rows = within(queue).getAllByRole("row");
  expect(rows[1]).toHaveTextContent("fora: bloqueada por #120");
  expect(rows[1]).toHaveTextContent("#120 (pendente)");
  expect(rows[2]).toHaveTextContent("#121 (removida, manual)");

  await user.click(within(queue).getByRole("button", { name: "Remover a dependência de #123 em #120" }));
  await user.click(within(queue).getByRole("button", { name: "Restaurar a dependência de #124 em #121" }));
  const form = within(queue).getByRole("form", { name: "Adicionar dependência em acme/acme-web" });
  await user.type(within(form).getByLabelText("Issue #"), "124");
  await user.type(within(form).getByLabelText("bloqueada por #"), "123");
  await user.type(within(form).getByLabelText("motivo"), "mesma tela");
  await user.click(within(form).getByRole("button", { name: "Adicionar dependência" }));
  await waitFor(() => expect(api.hubCallsTo("hub.autopilot.deps.set")).toHaveLength(3));
  expect(api.hubCallsTo("hub.autopilot.deps.set").map((c) => c.params)).toEqual([
    { owner: "acme", repo: "acme-web", number: 123, blocker: 120, action: "remove" },
    { owner: "acme", repo: "acme-web", number: 124, blocker: 121, action: "reset" },
    { owner: "acme", repo: "acme-web", number: 124, blocker: 123, action: "add", reason: "mesma tela" },
  ]);
});
