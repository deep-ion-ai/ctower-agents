// Bloco do piloto automático da Sala de controle (#359): estado, contas,
// uma linha por projeto, pausar e retomar, push, web e hub antigo.
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { App } from "../App";
import { FakeDaemonApi } from "../api/fake";
import type { AutopilotSettings, AutopilotStatus, BudgetDecision, MorningDigest } from "../protocol/hub-gen";
import { HUB_DASHBOARD, dashboard, hubStatus } from "../test/fixtures";
import { webApi } from "../test/web";
import { nextText, nowText, sinceDigestText } from "./PilotBlock";

const NOW = Date.now();

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

const settings = (s: Partial<AutopilotSettings> = {}): AutopilotSettings => ({
  paused: false,
  paused_at: null,
  max_sessions_per_env: 1,
  idle_timeout_min: 30,
  max_ci_attempts: 3,
  waiting_timeout_min: 20,
  ...s,
});

const queueItem = (number: number, skip: string | null = null) => ({
  number,
  title: `ACME-${number}`,
  priority: "P1" as const,
  model: "opus",
  skip,
  blocked_by: [],
  dependencies: [],
  order: null,
});

function status(account: string, s: Partial<AutopilotSettings> = {}): AutopilotStatus {
  return {
    settings: settings(s),
    last_tick_at: NOW - 30_000,
    now: NOW,
    accounts: [{ account, decision: decision(), exhausted_window: null, exhausted_until: null }],
    projects: [
      {
        owner: "acme",
        repo: "acme-web",
        env_id: "env-auto",
        env_name: "servidor",
        env_online: true,
        account,
        blocked: "budget:band_closed",
        blocked_message: "faixa fechada (Horário comercial)",
        next: null,
        probe: false,
        queue: [queueItem(123), queueItem(124, "no_model")],
        analysis: null,
        analysis_unsupported: false,
      },
    ],
    sessions: [
      { session_id: "s-1", env_id: "env-auto", state: "running", owner: "acme", repo: "acme-web", gh_number: 120, started_at: NOW - 600_000 },
    ],
    parked: [],
    events: [
      {
        id: 7,
        at: NOW - 120_000,
        kind: "started",
        scope: "item:acme/acme-web#120",
        reason: null,
        message: "acme/acme-web#120: sessão iniciada",
        account_key: account,
        env_id: "env-auto",
        owner: "acme",
        repo: "acme-web",
        gh_number: 120,
        session_id: "s-1",
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
        conflict: true,
        error: null,
      },
    ],
  };
}

function digest(): MorningDigest {
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
        merged: [{ number: 118, title: "ACME-118", reason: null }],
        blocked: [{ number: 125, title: "ACME-125", reason: "CI falhou" }],
        decisions_pending: 3,
        decisions_new: 1,
        ahead: 2,
        pr_url: null,
        ci: null,
        conflict: false,
        error: null,
        waiting_blocker: 0,
      },
    ],
    usage: [],
  };
}

/** `hub.autopilot.*` como o hub: pausar e retomar mudam o estado e avisam pelo push. */
function withPilot<T extends FakeDaemonApi>(api: T, account: string, st?: () => AutopilotStatus) {
  let cur = settings();
  const set = (paused: boolean) => {
    cur = { ...cur, paused, paused_at: paused ? NOW : null };
    queueMicrotask(() => api.hubPush("hub.autopilot.changed"));
    return cur;
  };
  return api
    .onHub("hub.autopilot.status", () => ({ ...(st ? st() : status(account)), settings: cur }))
    .onHub("hub.autopilot.digest", () => [digest()])
    .onHub("hub.autopilot.pause", () => set(true))
    .onHub("hub.autopilot.resume", () => set(false));
}

function desktop(account = "org-a/fulano@example.com", st?: () => AutopilotStatus) {
  const api = withPilot(
    new FakeDaemonApi()
      .on("dashboard.get", dashboard())
      .withHub()
      .onHub("hub.dashboard.get", HUB_DASHBOARD)
      .onHub("hub.status", hubStatus()),
    account,
    st,
  );
  render(<App api={api} />);
  return { api, user: userEvent.setup() };
}

test("textos: agora, próxima e contagens do resumo sem repetir a wip", () => {
  const p = status("conta-1").projects[0];
  expect(nowText(status("conta-1").sessions, p)).toBe("Agora: #120 (running)");
  expect(nowText([], p)).toBeNull();
  expect(nextText(p)).toBe("Nada inicia agora: faixa fechada (Horário comercial) · fila: 2");
  expect(nextText({ ...p, next: 123 })).toBe("Próxima: #123 · fila: 2");
  expect(nextText({ ...p, next: 123, probe: true })).toBe("Próxima: #123 (sonda) · fila: 2");
  expect(sinceDigestText(digest().projects[0])).toBe("1 mesclada na wip · 1 bloqueada · 3 decisões para revisar");
});

test("bloco no fim da Sala: estado, conta, linha do projeto, resumo e último evento", async () => {
  desktop();
  const block = await screen.findByRole("region", { name: "Piloto automático" });
  expect(within(block).getByRole("status", { name: "Estado do piloto" })).toHaveTextContent(
    "Aguardando faixa: a faixa de horário atual não permite iniciar",
  );
  expect(block).toHaveTextContent("Último ciclo do agendador agora");
  const contas = within(block).getByRole("list", { name: "Contas do piloto" });
  expect(contas).toHaveTextContent("Agora pode usar até 45% da semanal (em 30%)");
  expect(contas).toHaveTextContent("não inicia agora: faixa fechada (Horário comercial)");
  const linha = await waitFor(() => {
    const li = within(within(block).getByRole("list", { name: "Projetos autônomos" })).getByText("acme-web").closest("li");
    expect(li).toHaveTextContent("Desde o resumo");
    return li as HTMLElement;
  });
  expect(linha).toHaveTextContent("servidor · online");
  expect(linha).toHaveTextContent("Agora: #120 (running) · Nada inicia agora: faixa fechada (Horário comercial) · fila: 2");
  expect(linha).toHaveTextContent("wip/autonoma: 2 issues à frente de develop · em conflito com a base");
  expect(linha).toHaveTextContent("(CI sem dados no espelho)");
  expect(within(linha).getByRole("link", { name: "PR" })).toHaveAttribute("href", "https://github.example/acme/acme-web/pull/5");
  expect(linha).toHaveTextContent("Desde o resumo: 1 mesclada na wip · 1 bloqueada · 3 decisões para revisar");
  expect(block).toHaveTextContent("Resumo da manhã:");
  expect(block).toHaveTextContent("Último evento: acme/acme-web#120: sessão iniciada (há 2 min)");
  // No fim da página, antes do rodapé de infraestrutura.
  const infra = screen.getByRole("contentinfo", { name: "Infraestrutura" });
  expect(block.compareDocumentPosition(infra) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
});

test("conta com o limite do plano atingido mostra até quando e as estacionadas", async () => {
  const conta = "conta-1";
  desktop(conta, () => {
    const s = status(conta);
    s.accounts = [{ account: conta, decision: decision(), exhausted_window: "five_hour", exhausted_until: NOW + 3_600_000 }];
    s.parked = [
      { session_id: "s-2", env_id: "env-auto", account_key: conta, owner: "acme", repo: "acme-web", gh_number: 121, window: "five_hour", resume_at: NOW + 3_600_000, parked_at: NOW },
    ];
    return s;
  });
  const contas = await screen.findByRole("list", { name: "Contas do piloto" });
  expect(contas).toHaveTextContent("limite do plano atingido (janela de 5h) até");
  expect(contas).toHaveTextContent("estacionadas: #121");
});

test("Pausar tudo e Retomar a partir da Sala; o push relê o estado", async () => {
  const { api, user } = desktop();
  const block = await screen.findByRole("region", { name: "Piloto automático" });
  await user.click(within(block).getByRole("button", { name: "Pausar tudo" }));
  expect(api.hubCallsTo("hub.autopilot.pause")).toHaveLength(1);
  expect(await within(block).findByRole("button", { name: "Retomar" })).toBeInTheDocument();
  expect(within(block).getByRole("status", { name: "Estado do piloto" })).toHaveTextContent("Pausado");
  await user.click(within(block).getByRole("button", { name: "Retomar" }));
  expect(api.hubCallsTo("hub.autopilot.resume")).toHaveLength(1);
  expect(await within(block).findByRole("button", { name: "Pausar tudo" })).toBeInTheDocument();
});

test("o push hub.autopilot.changed atualiza o bloco", async () => {
  let next: number | null = null;
  const { api } = desktop("conta-1", () => {
    const s = status("conta-1");
    s.projects[0] = { ...s.projects[0], next };
    return s;
  });
  const block = await screen.findByRole("region", { name: "Piloto automático" });
  expect(block).toHaveTextContent("Nada inicia agora");
  next = 123;
  api.hubPush("hub.autopilot.changed");
  await waitFor(() => expect(block).toHaveTextContent("Próxima: #123 · fila: 2"));
});

test("link Painel do piloto abre a seção do hub", async () => {
  const { user } = desktop();
  const block = await screen.findByRole("region", { name: "Piloto automático" });
  await user.click(within(block).getByRole("button", { name: "Painel do piloto ›" }));
  expect(await screen.findByRole("tab", { name: /Painel do piloto/, selected: true })).toBeInTheDocument();
});

test("web: as contas aparecem anonimizadas", async () => {
  render(<App api={withPilot(webApi(), "conta-1")} />);
  const block = await screen.findByRole("region", { name: "Piloto automático" });
  expect(within(block).getByRole("list", { name: "Contas do piloto" })).toHaveTextContent("conta-1");
  expect(block).not.toHaveTextContent("example.com");
});

test("sem hub, ou com hub anterior ao status, o bloco não aparece", async () => {
  const sem = new FakeDaemonApi().on("dashboard.get", dashboard());
  const { unmount } = render(<App api={sem} />);
  await screen.findByRole("heading", { name: "Sala de controle" });
  expect(screen.queryByRole("region", { name: "Piloto automático" })).toBeNull();
  unmount();

  const velho = new FakeDaemonApi()
    .on("dashboard.get", dashboard())
    .withHub()
    .onHub("hub.dashboard.get", HUB_DASHBOARD)
    .onHub("hub.status", hubStatus());
  render(<App api={velho} />);
  await screen.findByRole("heading", { name: "Sala de controle" });
  await waitFor(() => expect(velho.hubCallsTo("hub.autopilot.status").length).toBeGreaterThan(0));
  expect(screen.queryByRole("region", { name: "Piloto automático" })).toBeNull();
});
