// Hub › Orçamento do plano (#180): conta sem política (desligado), prévia
// "agora pode usar até X%", ligar, editar e desligar.
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { App } from "../App";
import { FakeDaemonApi } from "../api/fake";
import type {
  AutopilotPolicyView,
  BudgetDecision,
  BudgetPolicy,
  HubPlanAccounts,
} from "../protocol/hub-gen";
import { dashboard, hubEnv, hubInfo, hubProject, hubProjectView, hubStatus, HUB_DASHBOARD } from "../test/fixtures";

const NOW = Date.now();

const DEFAULT: BudgetPolicy = {
  utc_offset_min: -180,
  bands: [
    { label: "Horário comercial", days: [0, 1, 2, 3, 4], start_min: 480, end_min: 1080, five_hour_cap: null, workday: true },
    { label: "Fim de semana", days: [5, 6], start_min: 0, end_min: 1440, five_hour_cap: 95, workday: false },
  ],
  weekly_cap: 80,
  reserve_per_workday: 12,
  workday_cutoff_min: 240,
};

const ACCOUNTS: HubPlanAccounts = {
  accounts: [
    {
      account: { key: "org-a/fulano@example.com", email: "fulano@example.com", org_name: null, subscription_type: "max" },
      plan: { five_hour: { state: "none" }, seven_day: { state: "none" }, read_at: null },
      envs: [],
    },
  ],
  unknown: [],
  now: NOW,
};

function decision(p: BudgetPolicy): BudgetDecision {
  return {
    can_start: true,
    reasons: [],
    band: "Fim de semana",
    five_hour_cap: 95,
    five_hour_used: 40,
    weekly_used: 30,
    weekly: {
      resets_at: NOW + 86_400_000,
      workdays_total: 5,
      workdays_left: 0,
      reserve_limit: 100,
      pace: 0.5,
      pace_limit: p.weekly_cap - 10,
      effective: p.weekly_cap - 10,
    },
    next_open_at: NOW,
    now: NOW,
  };
}

function setup() {
  let stored: BudgetPolicy | null = null;
  const view = (account: string): AutopilotPolicyView => ({
    account,
    policy: stored,
    default: DEFAULT,
    decision: stored ? decision(stored) : null,
  });
  const api = new FakeDaemonApi()
    .on("dashboard.get", dashboard({ projects: [] }))
    .withHub("ws://mac-mini:7879/ws")
    .onHub("hub.info", hubInfo({ protocol: "1.14" }))
    .onHub("hub.status", hubStatus())
    .onHub("hub.projects.list", [hubProjectView(hubProject())])
    .onHub("hub.envs.list", [hubEnv()])
    .onHub("hub.dashboard.get", HUB_DASHBOARD)
    .onHub("hub.plan.accounts", ACCOUNTS)
    .onHub("hub.autopilot.policy.get", (p: { account: string }) => view(p.account))
    .onHub("hub.autopilot.policy.preview", (p: { policy: BudgetPolicy }) => decision(p.policy))
    .onHub("hub.autopilot.policy.set", (p: { account: string; policy: BudgetPolicy | null }) => {
      stored = p.policy;
      return view(p.account);
    });
  render(<App api={api} initial={{ name: "hub", section: "autopilot" }} />);
  return { api, user: userEvent.setup() };
}

test("conta sem política: desligado, prévia e ligar com a política", async () => {
  const { api, user } = setup();
  const panel = await screen.findByRole("region", { name: "Orçamento do piloto automático" });
  expect(await within(panel).findByText("Piloto automático desligado nesta conta (sem política).")).toBeInTheDocument();
  const preview = within(panel).getByRole("status", { name: "Prévia" });
  await waitFor(() => expect(preview).toHaveTextContent("Agora pode usar até 70% da semanal e 95% da janela de 5h"));

  // Mudar o teto refaz a prévia.
  const cap = within(panel).getByLabelText("Teto semanal T (%)");
  await user.clear(cap);
  await user.type(cap, "60");
  await waitFor(() => expect(preview).toHaveTextContent("até 50% da semanal"));

  await user.click(within(panel).getByRole("button", { name: "Ligar com esta política" }));
  expect(await within(panel).findByText("Piloto automático ligado nesta conta.")).toBeInTheDocument();
  const set = api.calls.find((c) => c.method === "hub.autopilot.policy.set");
  expect(set?.params).toMatchObject({ account: "org-a/fulano@example.com", policy: { weekly_cap: 60 } });

  await user.click(within(panel).getByRole("button", { name: "Desligar piloto automático" }));
  expect(await within(panel).findByText("Piloto automático desligado nesta conta.")).toBeInTheDocument();
  const last = api.calls.filter((c) => c.method === "hub.autopilot.policy.set").pop();
  expect(last?.params).toMatchObject({ policy: null });
});

test("faixas: marcar fechada desabilita o teto, e adicionar faixa", async () => {
  const { user } = setup();
  const panel = await screen.findByRole("region", { name: "Orçamento do piloto automático" });
  const weekend = await within(panel).findByRole("group", { name: "Faixa Fim de semana" });
  const capInput = within(weekend).getByLabelText("Teto da 5h (%)");
  expect(capInput).toBeEnabled();
  await user.click(within(weekend).getByLabelText("Fechada"));
  expect(capInput).toBeDisabled();
  await user.click(within(panel).getByRole("button", { name: "Adicionar faixa" }));
  expect(within(panel).getByRole("group", { name: "Faixa Nova faixa" })).toBeInTheDocument();
});
