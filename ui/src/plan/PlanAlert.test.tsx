// Alerta do plano da conta (#108): notificação ao cruzar um limiar (uma por
// conta), limiares das preferências no selo e a confirmação na modal de início.
import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { App } from "../App";
import { FakeDaemonApi } from "../api/fake";
import type { EnvAccount, PlanAccount, PlanReading, StatuslineHealth } from "../protocol/gen";
import { dashboard, project, session, sync } from "../test/fixtures";
import { TowerProvider } from "../state/tower";
import { clockText } from "./format";
import { PlanEnvChoices } from "./PlanStart";
import { PlanProvider } from "./usePlan";
import type { PlanPrefs } from "./thresholds";

// Relógio fixo ao meio-dia local: o reset da janela de 5h (+2h) cai no mesmo
// dia em qualquer fuso e horário em que a suíte rode ("libera às HH:MM").
vi.setSystemTime(new Date(2026, 10, 11, 12, 0));
afterAll(() => vi.useRealTimers());

const NOW = Date.now();
const MIN = 60_000;
const SL: StatuslineHealth = { installed: true, version: 1, current: 1, chained: false, error: null };
const CONTA_A: PlanAccount = { key: "conta-a", email: "fulano@example.com", org_name: null, subscription_type: "max" };
const CONTA_B: PlanAccount = { key: "conta-b", email: "beltrano@example.com", org_name: null, subscription_type: "pro" };
const RESET = NOW + 2 * 60 * MIN;

const reading = (five: number, read_at = NOW - MIN): PlanReading => ({
  read_at,
  five_hour: { used_percentage: five, resets_at: RESET },
  seven_day: { used_percentage: 10, resets_at: NOW + 3 * 24 * 60 * MIN },
});

const envAccount = (account: PlanAccount | null, latest: PlanReading | null): EnvAccount => ({
  account,
  statusline: SL,
  plan: { five_hour: { state: "none" }, seven_day: { state: "none" }, read_at: null },
  latest,
  now: NOW,
});

const statusbar = () => screen.getByRole("contentinfo", { name: "Barra de status" });

/** Dois ambientes (`env-1` e `env-2`); `acc` dá a resposta de cada um. */
function twoEnvs(acc: (env: string) => EnvAccount, plan?: PlanPrefs) {
  const api = new FakeDaemonApi([
    { ...FakeDaemonApi.wslEnv("env-1"), name: "env-1" },
    { ...FakeDaemonApi.wslEnv("env-2"), name: "env-2" },
  ])
    .on("dashboard.get", dashboard())
    .on("account.get", (_: unknown, env: string) => acc(env));
  if (plan) api.uiConfig.plan = plan;
  return api;
}

test("vários ambientes da mesma conta: uma notificação por cruzamento; reler não repete", async () => {
  let pct = 85;
  const api = twoEnvs(() => envAccount(CONTA_A, reading(pct, NOW)));
  render(<App api={api} />);
  await waitFor(() => expect(api.notices).toHaveLength(1));
  expect(api.notices[0]).toEqual({
    title: "Plano da conta perto do limite",
    body: `Conta fulano@example.com: janela de 5h em 85%, libera às ${clockText(RESET)}.`,
  });

  // Mesma leitura relida (push de sessão): nada de novo.
  act(() => api.push("env-1", "session_state", { session_id: "s-1" }));
  await waitFor(() => expect(api.callsTo("account.get").length).toBeGreaterThanOrEqual(4), { timeout: 3000 });
  expect(api.notices).toHaveLength(1);

  // Cruzou o crítico: uma notificação a mais, ainda uma só para a conta.
  pct = 96;
  act(() => api.push("env-2", "session_state", { session_id: "s-2" }));
  await waitFor(() => expect(api.notices).toHaveLength(2), { timeout: 3000 });
  expect(api.notices[1].title).toBe("Plano da conta quase esgotado");
  expect(api.notices[1].body).toContain("96%");
});

test("contas diferentes notificam cada uma; leitura velha não notifica e o selo fica sem tom", async () => {
  const api = twoEnvs((env) =>
    env === "env-1" ? envAccount(CONTA_A, reading(90)) : envAccount(CONTA_B, reading(99, NOW - 3 * 60 * MIN)),
  );
  render(<App api={api} />);
  await waitFor(() => expect(api.notices).toHaveLength(1));
  expect(api.notices[0].body).toContain("fulano@example.com");
  // A conta-b leu há 3 h (prazo padrão: 60 min).
  const conta = await screen.findByRole("listitem", { name: "beltrano@example.com" });
  expect(within(conta).getByText(`5h: 99% · libera às ${clockText(RESET)}`)).toHaveAttribute("data-tone", "none");
  expect(api.notices.some((n) => n.body.includes("beltrano"))).toBe(false);
});

test("limiares das preferências: o selo usa os da conta; notificação desligada não notifica", async () => {
  const api = twoEnvs(() => envAccount(CONTA_A, reading(72)), {
    notify: false,
    stale_minutes: 60,
    default: { five_hour: { warn: 80, critical: 95 }, seven_day: { warn: 80, critical: 95 } },
    accounts: { "conta-a": { five_hour: { warn: 60, critical: 70 }, seven_day: { warn: 80, critical: 95 } } },
  });
  render(<App api={api} />);
  const badge = await within(statusbar()).findByText(/^5h 72%/);
  expect(badge.closest(".plan-badge")).toHaveAttribute("data-tone", "critical");
  await new Promise((r) => setTimeout(r, 50));
  expect(api.notices).toEqual([]);
});

describe("modal de início", () => {
  const LOCAL = { name: "project", env: "wsl", projectId: 1 } as const;

  function setup(pct: number) {
    const api = new FakeDaemonApi()
      .on("dashboard.get", dashboard())
      .on("tree.get", { project: project(), sync: sync(), roots: [], untracked: [] })
      .on("env.info", { protocol: "2.0", version: "0.1.0", hostname: "ct", worktrees_root: "/home/user/worktrees" })
      .on("account.get", envAccount(CONTA_A, reading(pct)))
      .on("session.start_project", session({ id: "eeeeeeee-0000-0000-0000-000000000003", work_item_id: 900 }));
    render(<App api={api} initial={LOCAL} />);
    return { api, user: userEvent.setup() };
  }

  const open = async (user: ReturnType<typeof userEvent.setup>) => {
    // O plano chega antes de abrir a modal.
    await within(statusbar()).findByText(/^5h /);
    await user.click(await screen.findByRole("button", { name: "Iniciar sessão de planejamento" }));
    return screen.getByRole("dialog");
  };

  test("crítico: aviso com percentual e reset; iniciar pede confirmação, sem bloquear", async () => {
    const { api, user } = setup(97);
    const modal = await open(user);
    const uso = within(modal).getByRole("list", { name: "Uso do plano por ambiente" });
    expect(uso).toHaveTextContent(`fulano@example.com · 5h 97% · libera às ${clockText(RESET)}`);
    const note = within(modal).getByRole("status", { name: "Plano da conta" });
    expect(note).toHaveTextContent(`Conta fulano@example.com: janela de 5h em 97%, libera às ${clockText(RESET)}.`);
    expect(note).toHaveTextContent("iniciar pede confirmação");

    await user.click(within(modal).getByRole("button", { name: /^Iniciar sessão/ }));
    expect(within(modal).getByRole("alert")).toHaveTextContent("Iniciar mesmo assim?");
    expect(api.callsTo("session.start_project")).toHaveLength(0);
    // Voltar desfaz o passo; confirmar inicia.
    await user.click(within(modal).getByRole("button", { name: "Voltar" }));
    expect(api.callsTo("session.start_project")).toHaveLength(0);
    await user.click(within(modal).getByRole("button", { name: /^Iniciar sessão/ }));
    await user.click(within(modal).getByRole("button", { name: "Iniciar mesmo assim" }));
    await waitFor(() => expect(api.callsTo("session.start_project")).toHaveLength(1));
  });

  test("aviso: só o texto, inicia direto", async () => {
    const { api, user } = setup(85);
    const modal = await open(user);
    const note = within(modal).getByRole("status", { name: "Plano da conta" });
    expect(note).toHaveTextContent("janela de 5h em 85%");
    expect(note).not.toHaveTextContent("confirmação");
    await user.click(within(modal).getByRole("button", { name: /^Iniciar sessão/ }));
    await waitFor(() => expect(api.callsTo("session.start_project")).toHaveLength(1));
    expect(screen.queryByRole("button", { name: "Iniciar mesmo assim" })).not.toBeInTheDocument();
  });

  test("abaixo do aviso: sem aviso", async () => {
    const { user } = setup(40);
    const modal = await open(user);
    expect(within(modal).queryByRole("status", { name: "Plano da conta" })).not.toBeInTheDocument();
  });
});

test("uso da conta de cada ambiente: escolher um de outra conta com janela livre", async () => {
  const api = twoEnvs((env) => (env === "env-1" ? envAccount(CONTA_A, reading(97)) : envAccount(CONTA_B, reading(20))));
  const chosen: string[] = [];
  render(
    <TowerProvider api={api}>
      <PlanProvider>
        <PlanEnvChoices
          envs={[
            { env: "env-1", name: "env-1" },
            { env: "env-2", name: "env-2" },
          ]}
          chosen="env-1"
          onChoose={(e) => chosen.push(e)}
          now={NOW}
        />
      </PlanProvider>
    </TowerProvider>,
  );
  const um = await screen.findByRole("button", { name: /^env-1: fulano@example.com · 5h 97%/ });
  expect(um).toHaveAttribute("aria-pressed", "true");
  expect(um.closest("li")).toHaveAttribute("data-tone", "critical");
  const dois = screen.getByRole("button", { name: /^env-2: beltrano@example.com · 5h 20%/ });
  expect(dois.closest("li")).toHaveAttribute("data-tone", "normal");
  await userEvent.setup().click(dois);
  expect(chosen).toEqual(["env-2"]);
});
