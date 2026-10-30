// Detalhe do ambiente (#23): tudo sobre um ambiente numa tela, e os outros
// lugares (Sala de controle, Configurações › Conexões, Hub › Ambientes,
// Manutenção › Atualizações) como atalhos para ele.
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { App } from "../App";
import { FakeDaemonApi } from "../api/fake";
import type { EnvInfo } from "../protocol/gen";
import type { Screen } from "../state/nav";
import {
  HUB_CONFIG_UNLINKED,
  HUB_DASHBOARD,
  NO_HEALTH,
  NO_PROPOSALS,
  RC_AVAILABLE,
  dashboard,
  hubAssignment,
  hubEnv,
  hubInfo,
  hubProject,
  hubProjectView,
  hubStatus,
} from "../test/fixtures";
import { webApi } from "../test/web";
import { envAt } from "./sections";

const NOW = Date.now();

function info(p: Partial<EnvInfo> = {}): EnvInfo {
  return {
    protocol: "2.0",
    version: "0.3.0",
    hostname: "notebook",
    worktrees_root: "/home/u/worktrees",
    env_id: "daemon-wsl",
    env_name: "wsl-pc",
    hub_sync: {
      pending: 2,
      oldest_pending_at: NOW,
      last_attempt_at: NOW,
      last_ok_at: NOW - 60_000,
      last_error: null,
      failures: 0,
      syncing: false,
      prune: { retention_days: 30, pruned_work_items: 0, last_run_at: null, last_pruned: 0, last_error: null },
    },
    hub_config: { ...HUB_CONFIG_UNLINKED, linked: true, config_rev: 7, pulled_at: NOW - 60_000 },
    commit: "abcdef0123",
    update: null,
    remote_control: RC_AVAILABLE,
    docs: { root: null, reachable: false },
    ...p,
  };
}

function setup(initial?: Screen, p: Partial<EnvInfo> = {}) {
  const api = new FakeDaemonApi([{ ...FakeDaemonApi.wslEnv(), name: "wsl" }])
    .on("dashboard.get", dashboard({ projects: [] }))
    .on("env.info", info(p))
    .on("env.config.get", [])
    .on("github.status", { source: "gh", login: "x", error: null })
    .withHub()
    .onHub("hub.info", hubInfo())
    .onHub("hub.status", hubStatus())
    .onHub("hub.projects.list", [hubProjectView(hubProject(), [hubAssignment()])])
    .onHub("hub.envs.list", [hubEnv({ cleanup: { items: 3, bytes: 2048, checked_at: NOW - 60_000 } })])
    .onHub("hub.dashboard.get", HUB_DASHBOARD)
    .onHub("hub.proposals.list", NO_PROPOSALS)
    .onHub("hub.health.get", NO_HEALTH);
  render(<App api={api} initial={initial} />);
  return { api, user: userEvent.setup() };
}

const sections = () => screen.getByRole("navigation", { name: "Seções do ambiente" });
const body = (name: string) => screen.getByRole("group", { name });
const findBody = (name: string) => screen.findByRole("group", { name });

test("Sala de controle: a linha do ambiente abre o detalhe; o envio ao hub segue na linha", async () => {
  const { user } = setup();
  const side = screen.getByRole("complementary", { name: "Ambientes e projetos" });
  // "Enviar ao hub" (#21) fica fora do botão da linha.
  expect(await within(side).findByRole("button", { name: "Enviar ao hub o estado de wsl" })).toBeInTheDocument();
  await user.click(await within(side).findByRole("button", { name: /^wsl conectado/ }));
  expect(await screen.findByRole("tab", { name: /^wsl/, selected: true })).toBeInTheDocument();
  expect(screen.getByRole("heading", { level: 1, name: "wsl" })).toBeInTheDocument();
  expect(within(sections()).getAllByRole("button").map((b) => b.textContent)).toEqual([
    "Conexão",
    "Hub",
    "Projetos atribuídos",
    "Atualização",
    "Configuração do daemon",
    "Limpeza",
  ]);
  // Conexão: a linha de Configurações › Conexões, só deste ambiente, e o daemon.
  const conn = body("Conexão");
  expect(within(conn).getByRole("button", { name: "Editar wsl" })).toBeInTheDocument();
  expect(within(conn).queryByRole("button", { name: "+ Adicionar ambiente" })).not.toBeInTheDocument();
  expect(await within(conn).findByText("0.3.0 · abcdef0 · protocolo 2.0")).toBeInTheDocument();
});

test("Hub: ligação, envio com 'Enviar ao hub', o que o hub sabe e a ajuda da CLI", async () => {
  setup(envAt("wsl", "hub"));
  const hub = await findBody("Hub");
  expect(await within(hub).findByText(/configuração rev 7/)).toBeInTheDocument();
  expect(within(hub).getByText(/2 pendente\(s\) · último envio/)).toBeInTheDocument();
  expect(within(hub).getByRole("button", { name: "Enviar ao hub o estado de wsl" })).toBeInTheDocument();
  expect(await within(hub).findByText("wsl-pc")).toBeInTheDocument();
  expect(within(hub).getByText(/sem token de agente/)).toBeInTheDocument();
  expect(within(hub).getByText("ctower hub set-agent-token")).toBeInTheDocument();
  expect(within(hub).getByText("cthubd add-env wsl-pc | ctower hub set-token")).toBeInTheDocument();
});

test("Projetos atribuídos: a atribuição do hub leva às Configurações do projeto", async () => {
  const { user } = setup(envAt("wsl", "projects"));
  const list = await screen.findByRole("list", { name: "Projetos atribuídos a wsl" });
  expect(within(list).getByText("aplicada")).toBeInTheDocument();
  await user.click(within(list).getByRole("button", { name: "deep-ion-ai/ctower-agents" }));
  expect(await screen.findByRole("tab", { name: /ctower-agents · Configurações › Ambientes e atribuições/, selected: true })).toBeInTheDocument();
});

test("Limpeza: o resumo do hub e o atalho para Manutenção › Limpeza deste ambiente", async () => {
  const { user } = setup(envAt("wsl", "cleanup"));
  const c = await findBody("Limpeza");
  expect(await within(c).findByText(/3 pendente\(s\) · 2,0 KB/)).toBeInTheDocument();
  await user.click(within(c).getByRole("button", { name: "Abrir a limpeza de wsl ›" }));
  expect(await screen.findByRole("tab", { name: "Manutenção · Limpeza", selected: true })).toBeInTheDocument();
});

test("Atualização: só este ambiente, com canal e Reverter das Configurações", async () => {
  setup(envAt("wsl", "update"));
  const u = await findBody("Atualização");
  expect(await within(u).findByRole("rowgroup", { name: "Atualização de wsl" })).toBeInTheDocument();
  expect(within(u).queryByRole("button", { name: "Atualizar todos" })).not.toBeInTheDocument();
  expect(within(u).getByText("Reverter", { selector: "b" })).toBeInTheDocument();
  expect(within(u).queryByText("Versão e atualização", { selector: "b" })).not.toBeInTheDocument();
});

test("atalhos: Configurações › Conexões e Manutenção › Atualizações levam ao detalhe", async () => {
  const { user } = setup({ name: "settings", section: "connections" });
  await user.click(await screen.findByRole("button", { name: "Detalhes de wsl" }));
  expect(await screen.findByRole("heading", { level: 1, name: "wsl" })).toBeInTheDocument();

  await user.click(screen.getByRole("button", { name: "Configuração do daemon" }));
  expect(await screen.findByRole("tab", { name: "wsl · Configuração do daemon", selected: true })).toBeInTheDocument();
  expect(within(body("Configuração do daemon")).getByText("Token do ambiente", { selector: "b" })).toBeInTheDocument();
});

test("Hub › Ambientes: o nome leva ao detalhe, na seção Hub", async () => {
  const { user } = setup({ name: "hub", section: "envs" });
  await user.click(await screen.findByRole("button", { name: "wsl-pc" }));
  expect(await screen.findByRole("tab", { name: "wsl · Hub", selected: true })).toBeInTheDocument();
});

test("índice e ambiente desconhecido", async () => {
  const { user } = setup({ name: "env" });
  expect(await screen.findByRole("heading", { level: 1, name: "Ambientes" })).toBeInTheDocument();
  await user.click(within(screen.getByRole("list", { name: "Ambientes" })).getByRole("button", { name: "wsl" }));
  expect(await screen.findByRole("heading", { level: 1, name: "wsl" })).toBeInTheDocument();
  render(<App api={new FakeDaemonApi()} initial={envAt("sumiu")} />);
  expect(await screen.findByText("Este ambiente não está (mais) neste app.")).toBeInTheDocument();
});

test("web: sem Atualização, Configuração e Limpeza; a conexão não se edita", async () => {
  const api = webApi().onHub("hub.envs.list", [hubEnv()]);
  render(<App api={api} initial={envAt("daemon-wsl", "cleanup")} />);
  await waitFor(() => expect(within(sections()).getAllByRole("button").map((b) => b.textContent)).toEqual(["Conexão", "Hub", "Projetos atribuídos"]));
  // A seção do desktop pedida cai em Conexão.
  expect(within(body("Conexão")).getByText(/se edita no app desktop/)).toBeInTheDocument();
});
