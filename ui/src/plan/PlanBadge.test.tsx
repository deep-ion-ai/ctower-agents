// Selo do plano na barra de status e lista de contas na Sala de controle (#107).
import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { App } from "../App";
import { FakeDaemonApi } from "../api/fake";
import type { EnvAccount, PlanAccount, PlanReading, StatuslineHealth } from "../protocol/gen";
import type { HubPlanAccounts } from "../protocol/hub-gen";
import { dashboard } from "../test/fixtures";
import { ENV, webApi } from "../test/web";
import { clockText } from "./format";

// Relógio fixo ao meio-dia local: o reset da janela de 5h (+2h) cai no mesmo
// dia em qualquer fuso e horário em que a suíte rode ("libera às HH:MM").
vi.setSystemTime(new Date(2026, 10, 11, 12, 0));
afterAll(() => vi.useRealTimers());

const NOW = Date.now();
const MIN = 60_000;
const SL: StatuslineHealth = { installed: true, version: 1, current: 1, chained: false, error: null };
const CONTA: PlanAccount = { key: "org-a:fulano@example.com", email: "fulano@example.com", org_name: null, subscription_type: "max" };
const RESET = NOW + 2 * 60 * MIN;

const reading = (five: number, seven: number, read_at = NOW - 3 * MIN): PlanReading => ({
  read_at,
  five_hour: { used_percentage: five, resets_at: RESET },
  seven_day: { used_percentage: seven, resets_at: NOW + 3 * 24 * 60 * MIN },
});

const envAccount = (account: PlanAccount | null, latest: PlanReading | null, statusline = SL): EnvAccount => ({
  account,
  statusline,
  // O selo usa `latest` com o relógio da UI; a visão do ambiente não entra.
  plan: { five_hour: { state: "none" }, seven_day: { state: "none" }, read_at: null },
  latest,
  now: NOW,
});

const statusbar = () => screen.getByRole("contentinfo", { name: "Barra de status" });
const plano = () => screen.getByRole("region", { name: /^Plano das contas/ });

function desktop(acc: EnvAccount) {
  const api = new FakeDaemonApi().on("dashboard.get", dashboard()).on("account.get", () => acc);
  render(<App api={api} />);
  return api;
}

test("barra de status: as duas janelas, o reset e a idade da leitura; o push de sessão relê", async () => {
  let acc = envAccount(CONTA, reading(72, 64));
  const api = new FakeDaemonApi().on("dashboard.get", dashboard()).on("account.get", () => acc);
  render(<App api={api} />);
  const sb = statusbar();
  expect(await within(sb).findByText(`5h 72% · libera às ${clockText(RESET)} · sem. 64%`)).toBeInTheDocument();
  expect(within(sb).getByText("· atualizado há 3 min")).toBeInTheDocument();
  expect(within(sb).getByTitle(/O percentual é da conta toda/)).toBeInTheDocument();

  // Lista de contas na Sala de controle, com os ambientes.
  const conta = within(plano()).getByRole("listitem", { name: "fulano@example.com" });
  expect(within(conta).getByText(`5h: 72% · libera às ${clockText(RESET)}`)).toBeInTheDocument();
  expect(within(conta).getByText("ctower-agents (WSL)")).toBeInTheDocument();

  // Leitura nova: chega com o push de sessão, sem recarregar a tela.
  acc = envAccount(CONTA, reading(96, 64, NOW));
  act(() => api.push("wsl", "session_state", { session_id: "s-1" }));
  const badge = await within(sb).findByText(/^5h 96%/, undefined, { timeout: 3000 });
  expect(badge.closest(".plan-badge")).toHaveAttribute("data-tone", "critical");
});

test("janela já renovada: sem o percentual antigo", async () => {
  desktop(
    envAccount(CONTA, {
      read_at: NOW - 6 * 60 * MIN,
      five_hour: { used_percentage: 99, resets_at: NOW - 60 * MIN },
      seven_day: null,
    }),
  );
  const sb = statusbar();
  expect(await within(sb).findByText(`5h renovada às ${clockText(NOW - 60 * MIN)} · sem leitura nova`)).toBeInTheDocument();
  expect(sb).not.toHaveTextContent("99");
});

test("sem leitura: diz isso, sem número", async () => {
  desktop(envAccount(CONTA, null));
  expect(await within(statusbar()).findByText("sem leitura do plano")).toBeInTheDocument();
});

test("conta desconhecida: sem selo, com a explicação; instalar o statusline", async () => {
  const api = desktop(envAccount(null, null, { ...SL, installed: false, version: null }));
  api.on("env.statusline.install", "installed");
  const user = userEvent.setup();
  expect(await within(statusbar()).findByText("Plano: conta desconhecida")).toBeInTheDocument();
  const item = within(plano()).getByRole("listitem", { name: "Conta desconhecida" });
  expect(within(item).getByText(/sem login no Claude Code/)).toBeInTheDocument();
  expect(within(item).getByText("statusline não instalado: sem leitura do plano")).toBeInTheDocument();
  expect(item.querySelector(".plan-badge, .plan-lines")).toBeNull();
  await user.click(within(item).getByRole("button", { name: "Instalar o statusline em ctower-agents (WSL)" }));
  expect(await within(item).findByText("statusline instalado")).toBeInTheDocument();
  expect(api.callsTo("env.statusline.install")).toHaveLength(1);
});

test("com hub: a conta vem de hub.plan.accounts", async () => {
  const h: HubPlanAccounts = {
    accounts: [
      {
        account: CONTA,
        plan: {
          five_hour: { state: "ok", used_percentage: 40, resets_at: RESET, read_at: NOW - MIN },
          seven_day: { state: "ok", used_percentage: 20, resets_at: RESET + MIN, read_at: NOW - MIN },
          read_at: NOW - MIN,
        },
        envs: [{ env_id: "d-wsl", name: "wsl no hub", statusline: SL }],
      },
    ],
    unknown: [],
    now: NOW,
  };
  const api = new FakeDaemonApi()
    .withHub()
    .on("dashboard.get", dashboard())
    .on("env.info", { env_id: "d-wsl" })
    .onHub("hub.plan.accounts", h);
  render(<App api={api} />);
  expect(await within(statusbar()).findByText(`5h 40% · libera às ${clockText(RESET)} · sem. 20%`)).toBeInTheDocument();
  expect(api.callsTo("account.get")).toHaveLength(0);
  await waitFor(() =>
    expect(within(plano()).getByRole("listitem", { name: "fulano@example.com" })).toHaveTextContent("ctower-agents (WSL)"),
  );
});

test("web: o selo com o que o hub tem, rotulado pelos ambientes", async () => {
  const api = webApi().onHub("hub.plan.accounts", {
    accounts: [
      {
        account: { key: "conta-1", email: null, org_name: null, subscription_type: null },
        plan: {
          five_hour: { state: "ok", used_percentage: 85, resets_at: RESET, read_at: NOW - MIN },
          seven_day: { state: "none" },
          read_at: NOW - MIN,
        },
        envs: [{ env_id: ENV, name: "env-1", statusline: SL }],
      },
    ],
    unknown: [],
    now: NOW,
  } satisfies HubPlanAccounts);
  render(<App api={api} />);
  const badge = await within(statusbar()).findByText(`5h 85% · libera às ${clockText(RESET)}`);
  expect(badge.closest(".plan-badge")).toHaveAttribute("data-tone", "warn");
  expect(within(plano()).getByRole("listitem", { name: /^Conta de / })).toBeInTheDocument();
  expect(api.callsTo("account.get")).toHaveLength(0);
});
