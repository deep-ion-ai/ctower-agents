import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { App } from "../App";
import { FakeDaemonApi } from "../api/fake";
import type { EnvInfo, UpdateStatus } from "../protocol/gen";
import type { HubEnv, HubStatus } from "../protocol/hub-gen";
import { HUB_BACKUP, HUB_CONFIG_UNLINKED, HUB_DASHBOARD, dashboard, hubEnv, hubInfo, hubStatus, RC_AVAILABLE } from "../test/fixtures";

const NOW = Date.now();
const WSL = FakeDaemonApi.wslEnv();
const MAC = { ...FakeDaemonApi.wslEnv("mac"), name: "mac-mini" };

function update(available: boolean): UpdateStatus {
  return {
    installed: { version: "0.1.0", commit: "1111111aaaa" },
    ref: "main",
    available: {
      ref: "main",
      build: available ? { version: "0.2.0", commit: "2222222bbbb" } : { version: "0.1.0", commit: "1111111aaaa" },
      relation: available ? "behind" : "same",
      update_available: available,
      commits: [],
      behind: available ? 2 : null,
      checked_at: NOW,
    },
    check_error: null,
    state: null,
  };
}

function info(env: string, withUpdate: boolean): EnvInfo {
  return {
    protocol: "2.0",
    version: "0.1.0",
    commit: "1111111aaaa",
    hostname: env,
    worktrees_root: "/w",
    env_id: `daemon-${env}`,
    env_name: env,
    hub_sync: null,
    hub_config: HUB_CONFIG_UNLINKED,
    update: update(withUpdate),
    remote_control: RC_AVAILABLE,
    docs: { root: null, reachable: false },
  };
}

const CLEAN = { checked_at: NOW - 60_000, items: 0, bytes: 0 };

/** wsl com atualização disponível; mac-mini com 2 limpezas pendentes no hub. */
function setup(opts: { hubEnvs?: HubEnv[]; status?: HubStatus; initial?: "overview" | "hub" } = {}) {
  const api = new FakeDaemonApi([WSL, MAC])
    .on("dashboard.get", dashboard({ projects: [] }))
    .on("sync.now", [])
    .on("env.info", (_p: unknown, env: string) => info(env, env === "wsl"))
    .withHub()
    .onHub("hub.info", hubInfo())
    .onHub("hub.status", opts.status ?? hubStatus())
    .onHub("hub.projects.list", [])
    .onHub(
      "hub.envs.list",
      opts.hubEnvs ?? [
        hubEnv({ env_id: "daemon-wsl", name: "wsl", cleanup: CLEAN }),
        hubEnv({ env_id: "daemon-mac", name: "mac", cleanup: { checked_at: NOW - 5 * 60_000, items: 2, bytes: 3 * 1024 ** 3 } }),
      ],
    )
    .onHub("hub.dashboard.get", HUB_DASHBOARD);
  render(<App api={api} initial={opts.initial === "hub" ? { name: "hub", section: "envs" } : { name: "maintenance" }} />);
  return { api, user: userEvent.setup() };
}

const table = () => screen.getByRole("table", { name: "Manutenção por ambiente" });
const row = (name: string) => within(table()).getByRole("row", { name });
const selo = () => within(screen.getByRole("navigation", { name: "Telas" })).getByRole("button", { name: "Manutenção" });

test("dois ambientes, um com atualização e outro com limpeza: a Visão geral mostra os dois e o selo mostra 2", async () => {
  const { user } = setup();
  await waitFor(() => expect(selo()).toHaveAccessibleDescription("2 pendências"));

  const wsl = row("ctower-agents (WSL)");
  expect(within(wsl).getByText("atualização disponível")).toBeInTheDocument();
  expect(within(wsl).getByText("nenhuma pendente")).toBeInTheDocument();
  expect(within(wsl).getByText("conectado")).toBeInTheDocument();

  const mac = row("mac-mini");
  expect(within(mac).queryByText("atualização disponível")).not.toBeInTheDocument();
  expect(within(mac).getByText("em dia")).toBeInTheDocument();
  expect(within(mac).getByText("2 pendente(s) · 3,0 GB")).toBeInTheDocument();
  expect(within(mac).getByText("verificado há 5 min")).toBeInTheDocument();

  // Este app: em dia (o fake não tem versão nova).
  expect(within(row("Este app")).getByText("em dia")).toBeInTheDocument();
  expect(screen.getByText(/^2 pendências:/)).toBeInTheDocument();

  // A célula da limpeza leva à Limpeza daquele ambiente.
  await user.click(within(mac).getByRole("button", { name: /2 pendente\(s\)/ }));
  expect(screen.getByRole("tab", { name: "Manutenção · Limpeza" })).toHaveAttribute("aria-selected", "true");
  expect(screen.getByRole("navigation", { name: "Trilha" })).toHaveTextContent("ManutençãoLimpezamac-mini");
});

test("a célula da versão leva a Atualizações › Ambientes; a do app, a Este app", async () => {
  const { user } = setup();
  await waitFor(() => expect(selo()).toHaveAccessibleDescription("2 pendências"));
  await user.click(within(row("ctower-agents (WSL)")).getByRole("button", { name: /atualização disponível/ }));
  expect(screen.getByRole("tab", { name: "Manutenção · Atualizações · Ambientes" })).toHaveAttribute("aria-selected", "true");
  await user.click(screen.getByRole("tab", { name: "Manutenção · Visão geral" }));
  await user.click(within(row("Este app")).getByRole("button", { name: /0\.1\.0/ }));
  expect(screen.getByRole("tab", { name: "Manutenção · Atualizações · Este app" })).toHaveAttribute("aria-selected", "true");
});

test("ambiente desconectado: o último dado do hub e a indicação de desconectado", async () => {
  const { api } = setup();
  await waitFor(() => expect(selo()).toHaveAccessibleDescription("2 pendências"));
  act(() => api.setStatus("mac", false));

  const mac = row("mac-mini");
  await waitFor(() => expect(within(mac).getByText("desconectado")).toBeInTheDocument());
  expect(within(mac).getByText("sem conexão")).toBeInTheDocument();
  // A limpeza é a do hub: continua lá e continua contando.
  expect(within(mac).getByText("2 pendente(s) · 3,0 GB")).toBeInTheDocument();
  expect(within(mac).getByText(/último envio ao hub há/)).toBeInTheDocument();
  expect(selo()).toHaveAccessibleDescription("2 pendências");

  // Desconectado, a atualização do wsl deixa de contar (não dá para atualizar).
  act(() => api.setStatus("wsl", false));
  await waitFor(() => expect(selo()).toHaveAccessibleDescription("1 pendência"));
});

test("ambiente que só o hub conhece aparece, sem link para a limpeza; o mesmo ambiente com as duas conta 2", async () => {
  setup({
    hubEnvs: [
      hubEnv({ env_id: "daemon-wsl", name: "wsl", cleanup: { ...CLEAN, items: 1, bytes: 2048 } }),
      hubEnv({ env_id: "daemon-mac", name: "mac", cleanup: CLEAN }),
      hubEnv({ env_id: "daemon-note", name: "notebook", cleanup: { ...CLEAN, items: 3, bytes: 4096 } }),
    ],
  });
  // wsl: atualização + limpeza (2); notebook: limpeza (1).
  await waitFor(() => expect(selo()).toHaveAccessibleDescription("3 pendências"));
  const note = row("notebook");
  expect(within(note).getByText("não configurado nesta UI")).toBeInTheDocument();
  expect(within(note).getByText("fora desta UI")).toBeInTheDocument();
  expect(within(note).queryByRole("button")).not.toBeInTheDocument();
});

test("backup do hub: avisos de backup em destaque e o link para Backup e snapshots", async () => {
  const { user } = setup({
    status: hubStatus({
      warnings: [
        { kind: "backup_stale", message: "último snapshot há 2 dias" },
        { kind: "push_quarantine", message: "linhas em quarentena" },
      ],
      backup: { ...HUB_BACKUP, newest_at: NOW - 2 * 24 * 3600_000 },
    }),
  });
  const backup = await screen.findByRole("region", { name: "Backup do hub" });
  await waitFor(() => expect(within(backup).getByText("último snapshot há 2 dias", { selector: "li" })).toBeInTheDocument());
  expect(within(backup).queryByText(/quarentena/)).not.toBeInTheDocument();
  await user.click(within(backup).getByRole("button", { name: "Backup e snapshots ›" }));
  expect(screen.getByRole("tab", { name: "Manutenção · Backup e snapshots" })).toHaveAttribute("aria-selected", "true");
});

test("hub anterior ao #263 (sem `remote` no backup): a Visão geral abre", async () => {
  const { remote: _, ...old } = { ...HUB_BACKUP, newest_at: NOW - 3600_000 };
  setup({ status: hubStatus({ backup: old as typeof HUB_BACKUP }) });
  const backup = await screen.findByRole("region", { name: "Backup do hub" });
  await waitFor(() => expect(within(backup).getByText(/último snapshot há 1 h/)).toBeInTheDocument());
  expect(within(backup).queryByText(/remoto/)).not.toBeInTheDocument();
});

test("Hub › Ambientes: a limpeza pendente leva à Limpeza daquele ambiente", async () => {
  const { user } = setup({ initial: "hub" });
  await user.click(await screen.findByRole("button", { name: "2 pendente(s) · 3,0 GB" }));
  expect(screen.getByRole("tab", { name: "Manutenção · Limpeza" })).toHaveAttribute("aria-selected", "true");
  expect(screen.getByRole("navigation", { name: "Trilha" })).toHaveTextContent("ManutençãoLimpezamac-mini");
});
