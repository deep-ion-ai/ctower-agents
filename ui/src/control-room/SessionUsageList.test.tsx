// Visão "Uso por sessão" na Sala de controle (#109), com dados fictícios.
import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { App } from "../App";
import { FakeDaemonApi } from "../api/fake";
import type { Dashboard, EnvAccount, PlanAccount, SessionUsage, StatuslineHealth } from "../protocol/gen";
import type { HubPlanAccounts } from "../protocol/hub-gen";
import { dashboard, project, session, sync, workItem } from "../test/fixtures";
import { ENV, webApi } from "../test/web";

const NOW = Date.now();
const MIN = 60_000;
const SL: StatuslineHealth = { installed: true, version: 1, current: 1, chained: false, error: null };
const CONTA: PlanAccount = { key: "conta-a", email: "fulano@example.com", org_name: null, subscription_type: "max" };

const env = (id: string) => ({ ...FakeDaemonApi.wslEnv(id), name: id });

const usage = (cost: number, pct: number): SessionUsage => ({
  tool_session_id: "t-1",
  written_at: NOW - MIN,
  model: { id: "claude-opus", display_name: "Opus" },
  effort: "high",
  cost: { total_cost_usd: cost, total_duration_ms: null, total_api_duration_ms: null },
  context_window: { context_window_size: 200_000, used_percentage: pct, used_tokens: null },
  prompt_cache: null,
  plan: null,
});

const account = (a: PlanAccount | null): EnvAccount => ({
  account: a,
  statusline: SL,
  plan: { five_hour: { state: "none" }, seven_day: { state: "none" }, read_at: null },
  latest: a ? { read_at: NOW - MIN, five_hour: { used_percentage: 72, resets_at: NOW + 60 * MIN }, seven_day: null } : null,
  now: NOW,
});

// env-1 (conta-a): s-1 esperando você há 12 min, s-2 rodando e mais cara.
// env-2 (conta desconhecida): s-3 sem leitura.
const S1 = session({ id: "s-1", work_item_id: 31, state: "idle", started_at: NOW - 60 * MIN, last_event_at: NOW - 12 * MIN });
const S2 = session({ id: "s-2", work_item_id: 32, state: "running", started_at: NOW - 30 * MIN });
const S3 = session({ id: "s-3", work_item_id: 33, state: "idle", started_at: NOW - 90 * MIN, last_event_at: NOW - 40 * MIN });

const fresh = [sync({ last_ok_at: NOW, last_attempt_at: NOW })];

function env1(): Dashboard {
  const d = dashboard({ live_sessions: [S1, S2], sync: fresh });
  return {
    ...d,
    projects: [
      {
        project: project({ name: "acme-web" }),
        work_items: [
          workItem({ gh_number: 31, status: "aguardando_voce", title: "tela de login" }),
          workItem({ gh_number: 32, status: "em_execucao", title: "API de pedidos" }),
        ],
      },
    ],
  };
}

function setup() {
  let readings: Record<string, SessionUsage | null> = { "s-1": usage(1.5, 42), "s-2": usage(3.25, 80), "s-3": null };
  const api = new FakeDaemonApi([env("env-1"), env("env-2")])
    .on("dashboard.get", (_: unknown, e: string) =>
      e === "env-1"
        ? env1()
        : dashboard({ live_sessions: [S3], sync: fresh }, [workItem({ gh_number: 33, status: "em_execucao", title: "relatório mensal" })]),
    )
    .on("account.get", (_: unknown, e: string) => account(e === "env-1" ? CONTA : null))
    .on("session.usage", (p: { session_id: string }) => readings[p.session_id] ?? null);
  render(<App api={api} />);
  return { api, set: (r: typeof readings) => (readings = r) };
}

async function openView() {
  const user = userEvent.setup();
  const lateral = await screen.findByRole("complementary", { name: "Ambientes e projetos" });
  await user.click(within(lateral).getByRole("button", { name: "Uso por sessão" }));
  return user;
}

const titles = (g: HTMLElement) =>
  within(g)
    .getAllByRole("row")
    .slice(1)
    .map((r) => within(r).getAllByRole("cell")[0].textContent);

test("agrupa por conta com o selo, custo estimado, contexto, tempo parada e ordenação", async () => {
  const { api } = setup();
  const user = await openView();
  expect(screen.getByRole("heading", { level: 1, name: "Uso por sessão" })).toBeInTheDocument();

  const contaA = await screen.findByRole("region", { name: "Conta fulano@example.com" });
  await waitFor(() => expect(within(contaA).getByText(/US\$ 3,25/)).toBeInTheDocument());
  // Selo da conta no cabeçalho e os ambientes dela.
  expect(within(contaA).getByText(/^5h 72%/)).toBeInTheDocument();
  expect(within(contaA).getByText("Ambientes: env-1")).toBeInTheDocument();
  expect(within(contaA).getByRole("columnheader", { name: "Custo estimado" })).toHaveAttribute(
    "title",
    expect.stringMatching(/não é a cobrança/),
  );

  // Padrão: maior custo primeiro.
  expect(titles(contaA)).toEqual([expect.stringContaining("API de pedidos"), expect.stringContaining("tela de login")]);
  const [, caro, barato] = within(contaA).getAllByRole("row");
  expect(within(caro).getByText("rodando", { selector: ".badge" })).toBeInTheDocument();
  expect(caro).toHaveTextContent("80% de 200k");
  expect(within(barato).getByText("aguardando você")).toBeInTheDocument();
  expect(barato).toHaveTextContent("US$ 1,50 (estimativa)");
  expect(barato).toHaveTextContent("42% de 200k");
  expect(barato).toHaveTextContent("12 min");
  expect(barato).toHaveTextContent("Opus · esforço high");
  expect(barato).toHaveTextContent("acme-web #31");

  // Conta desconhecida à parte; sem leitura, sem número.
  const desconhecida = screen.getByRole("region", { name: "Conta Conta desconhecida" });
  const [, s3] = within(desconhecida).getAllByRole("row");
  await waitFor(() => expect(within(s3).getAllByText("sem leitura")).toHaveLength(2));
  expect(s3).not.toHaveTextContent("US$");
  expect(s3).toHaveTextContent("40 min");

  // Por tempo parada: a que espera há mais tempo primeiro, a que roda no fim.
  await user.click(screen.getByRole("button", { name: "Tempo parada" }));
  expect(titles(contaA)).toEqual([expect.stringContaining("tela de login"), expect.stringContaining("API de pedidos")]);

  // Só as sessões listadas são lidas.
  expect(new Set(api.callsTo("session.usage").map((c) => c.params.session_id))).toEqual(new Set(["s-1", "s-2", "s-3"]));
});

test("push da sessão relê só a leitura dela", async () => {
  const { api, set } = setup();
  await openView();
  const contaA = await screen.findByRole("region", { name: "Conta fulano@example.com" });
  await waitFor(() => expect(within(contaA).getByText(/US\$ 1,50/)).toBeInTheDocument());
  const before = api.callsTo("session.usage").length;

  set({ "s-1": usage(2, 50), "s-2": usage(3.25, 80), "s-3": null });
  act(() => api.push("env-1", "session_state", { session_id: "s-1" }));
  await waitFor(() => expect(within(contaA).getByText(/US\$ 2,00/)).toBeInTheDocument(), { timeout: 3000 });
  const after = api.callsTo("session.usage").slice(before);
  expect(after.map((c) => c.params.session_id)).toEqual(["s-1"]);
});

test("web: a leitura vem pelo canal do hub, agrupada pela conta do hub", async () => {
  const api = webApi()
    .onHub("hub.plan.accounts", {
      accounts: [
        {
          account: { key: "conta-1", email: null, org_name: null, subscription_type: null },
          plan: { five_hour: { state: "none" }, seven_day: { state: "none" }, read_at: null },
          envs: [{ env_id: ENV, name: "env-1", statusline: SL }],
        },
      ],
      unknown: [],
      now: NOW,
    } satisfies HubPlanAccounts)
    .on("session.usage", usage(0.75, 10));
  render(<App api={api} />);
  await openView();
  const conta = await screen.findByRole("region", { name: /^Conta Conta de / });
  await waitFor(() => expect(conta).toHaveTextContent("US$ 0,75 (estimativa)"));
  expect(conta).toHaveTextContent("10% de 200k");
  expect(api.callsTo("session.usage").map((c) => c.params.session_id)).toContain("s-201");
});
