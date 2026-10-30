import { afterEach, expect, test } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { App } from "../App";
import { FakeDaemonApi } from "../api/fake";
import type { EnvInfo, HubSyncResult, HubSyncStatus } from "../protocol/gen";
import {
  HUB_CONFIG_UNLINKED,
  HUB_DASHBOARD,
  NO_HEALTH,
  NO_PROPOSALS,
  dashboard,
  hubEnv,
  hubInfo,
  hubProjectView,
  hubStatus,
  jiraHubProject,
  RC_AVAILABLE,
} from "../test/fixtures";
import { syncResultText } from "./Sidebar";

afterEach(cleanup);

const NOW = Date.now();

function info(p: Partial<EnvInfo> = {}): EnvInfo {
  return {
    protocol: "2.0",
    version: "0.1.0",
    hostname: "wsl",
    worktrees_root: "/home/wsl/worktrees",
    env_id: "daemon-wsl",
    env_name: "wsl",
    hub_sync: null,
    hub_config: HUB_CONFIG_UNLINKED,
    commit: "",
    update: null,
    remote_control: RC_AVAILABLE,
    docs: { root: null, reachable: false },
    ...p,
  };
}

function status(p: Partial<HubSyncStatus> = {}): HubSyncStatus {
  return {
    pending: 4,
    oldest_pending_at: NOW,
    last_attempt_at: NOW,
    last_ok_at: null,
    last_error: null,
    failures: 0,
    syncing: false,
    prune: { retention_days: 30, pruned_work_items: 0, last_run_at: null, last_pruned: 0, last_error: null },
    ...p,
  };
}

function result(p: Partial<HubSyncResult> = {}): HubSyncResult {
  return {
    ok: true,
    batches: 1,
    sent: 4,
    acked_seq: 4,
    pending: 0,
    error: null,
    joined: false,
    config: null,
    pruned: 0,
    ...p,
  };
}

/** Ambiente que envia ao hub; `sync` responde `hub.sync.now`. */
function setup(sync: () => unknown) {
  let st = status();
  const api = new FakeDaemonApi([{ ...FakeDaemonApi.wslEnv(), name: "wsl" }])
    .on("dashboard.get", dashboard({ projects: [] }))
    .on("env.info", () => info({ hub_sync: st }))
    .on("hub.sync.now", async () => {
      const r = await sync();
      st = status({ pending: (r as HubSyncResult).pending, last_ok_at: NOW });
      return r;
    });
  render(<App api={api} />);
  return { api, side: screen.getByRole("complementary", { name: "Ambientes e projetos" }) };
}

test("enviar ao hub: progresso, resultado e contador atualizado", async () => {
  let release!: () => void;
  const gate = new Promise<void>((r) => (release = r));
  const { api, side } = setup(async () => {
    await gate;
    return result();
  });
  const user = userEvent.setup();
  expect(await within(side).findByText("hub: 4 pendentes")).toBeInTheDocument();
  await user.click(within(side).getByRole("button", { name: "Enviar ao hub o estado de wsl" }));
  const btn = within(side).getByRole("button", { name: "Enviar ao hub o estado de wsl" });
  expect(btn).toBeDisabled();
  expect(btn).toHaveTextContent("Enviando…");
  const infosAntes = api.callsTo("env.info").length;
  release();
  expect(await within(side).findByText("4 mudanças enviadas ao hub")).toBeInTheDocument();
  await waitFor(() => expect(api.callsTo("env.info").length).toBeGreaterThan(infosAntes));
  expect(await within(side).findByText(/hub: enviado/)).toBeInTheDocument();
  expect(btn).toBeEnabled();
  expect(api.callsTo("hub.sync.now")).toHaveLength(1);
});

test("enviar ao hub fora mostra o motivo", async () => {
  const { side } = setup(() =>
    result({ ok: false, sent: 0, batches: 0, acked_seq: null, pending: 4, error: "hub inacessível: conexão recusada" }),
  );
  const user = userEvent.setup();
  await user.click(await within(side).findByRole("button", { name: "Enviar ao hub o estado de wsl" }));
  const msg = await within(side).findByText("falhou: hub inacessível: conexão recusada");
  expect(msg).toHaveAttribute("data-warn", "true");
});

test("erro do daemon vira mensagem legível", async () => {
  const { side } = setup(() => {
    throw new Error("este ambiente não envia ao hub");
  });
  const user = userEvent.setup();
  await user.click(await within(side).findByRole("button", { name: "Enviar ao hub o estado de wsl" }));
  expect(await within(side).findByText("falhou: este ambiente não envia ao hub")).toBeInTheDocument();
});

test("sem envio ao hub não há botão", async () => {
  const api = new FakeDaemonApi().on("dashboard.get", dashboard({ projects: [] })).on("env.info", info());
  render(<App api={api} />);
  await waitFor(() => expect(api.callsTo("env.info").length).toBeGreaterThan(0));
  expect(screen.queryByRole("button", { name: /com o hub/ })).not.toBeInTheDocument();
});

test("docs_root inacessível: selo no ambiente; acessível, nada", async () => {
  const setupDocs = (docs: EnvInfo["docs"]) => {
    const api = new FakeDaemonApi([{ ...FakeDaemonApi.wslEnv(), name: "wsl" }])
      .on("dashboard.get", dashboard({ projects: [] }))
      .on("env.info", () => info({ docs }));
    render(<App api={api} />);
    return { api, side: screen.getByRole("complementary", { name: "Ambientes e projetos" }) };
  };
  const { side } = setupDocs({ root: "/mnt/ct-docs", reachable: false, warning: "docs_unreachable" });
  const selo = await within(side).findByText("docs inacessíveis");
  expect(selo).toHaveAttribute("title", expect.stringContaining("/mnt/ct-docs"));
  cleanup();

  const ok = setupDocs({ root: "/mnt/ct-docs", reachable: true });
  await waitFor(() => expect(ok.api.callsTo("env.info").length).toBeGreaterThan(0));
  expect(await within(ok.side).findByText(/conectado/)).toBeInTheDocument();
  expect(within(ok.side).queryByText("docs inacessíveis")).not.toBeInTheDocument();
});

test("texto do resultado", () => {
  expect(syncResultText(result({ sent: 0, batches: 0 }))).toBe("nada a enviar: o hub já tem tudo");
  expect(syncResultText(result({ sent: 1 }))).toBe("1 mudança enviada ao hub");
  expect(syncResultText(result({ ok: false, sent: 200, pending: 3 }))).toBe("200 enviadas · 3 ainda pendentes");
  expect(syncResultText(result({ ok: false, error: "x" }))).toBe("falhou: x");
});

describe("hub na lateral", () => {
  const linked = (p: Partial<EnvInfo["hub_config"]> = {}) =>
    info({ hub_config: { ...HUB_CONFIG_UNLINKED, linked: true, config_rev: 42, pulled_at: NOW - 3 * 3_600_000, ...p } });

  test("conectado: nome, revisão e avisos; o ambiente mostra a configuração e não vira 'hub'", async () => {
    const api = new FakeDaemonApi([{ ...FakeDaemonApi.wslEnv(), name: "wsl" }])
      .on("dashboard.get", dashboard({ projects: [] }))
      .on("env.info", linked())
      .withHub()
      .onHub("hub.info", hubInfo({ name: "mac-mini", config_rev: 43 }))
      .onHub("hub.status", hubStatus({ warnings: [{ kind: "backup_same_device", message: "mesmo disco" }] }))
      .onHub("hub.dashboard.get", HUB_DASHBOARD);
    render(<App api={api} />);
    const side = screen.getByRole("complementary", { name: "Ambientes e projetos" });
    const user = userEvent.setup();
    const hub = await within(side).findByRole("button", { name: "Hub: conectado · configuração rev 43" });
    expect(within(hub).getByText("mac-mini")).toBeInTheDocument();
    expect(await within(hub).findByText("1 aviso")).toBeInTheDocument();
    expect(await within(side).findByText("configuração rev 42 · há 3 h")).toBeInTheDocument();
    expect(within(side).queryByText("hub", { selector: ".side-env-name *" })).not.toBeInTheDocument();
    await user.click(hub);
    const nav = screen.getByRole("navigation", { name: "Telas" });
    expect(within(nav).getByRole("button", { name: /^Hub/ })).toHaveAttribute("aria-current", "page");
    expect(within(nav).getByRole("button", { name: "Hub" })).toHaveAccessibleDescription("1 aviso");
  });

  test("hub fora: indisponível e a faixa de visão parcial", async () => {
    const api = new FakeDaemonApi([{ ...FakeDaemonApi.wslEnv(), name: "wsl" }])
      .on("dashboard.get", dashboard({ projects: [] }))
      .on("env.info", linked({ last_error: "hub inacessível" }))
      .withHub("ws://mac-mini:7879/ws", false);
    render(<App api={api} />);
    const side = screen.getByRole("complementary", { name: "Ambientes e projetos" });
    expect(await within(side).findByRole("button", { name: /^Hub: indisponível/ })).toBeInTheDocument();
    expect(screen.getByText(/hub indisponível: visão parcial/)).toBeInTheDocument();
    expect(await within(side).findByText("configuração rev 42 · há 3 h · pull falhou")).toHaveAttribute(
      "data-warn",
      "true",
    );
  });

  test("ambiente ligado sem a conexão na UI leva às Configurações; ambiente sem hub avisa", async () => {
    const api = new FakeDaemonApi([
      { ...FakeDaemonApi.wslEnv(), name: "wsl" },
      { ...FakeDaemonApi.wslEnv("mac"), name: "mac" },
    ])
      .on("dashboard.get", dashboard({ projects: [] }))
      .on("env.info", (_: unknown, env: string) => (env === "wsl" ? linked() : info()));
    render(<App api={api} />);
    const side = screen.getByRole("complementary", { name: "Ambientes e projetos" });
    const user = userEvent.setup();
    expect(await within(side).findByText("ambiente não ligado a um hub")).toHaveAttribute("data-warn", "true");
    await user.click(await within(side).findByRole("button", { name: "Hub: conexão não configurada" }));
    expect(screen.getByRole("button", { name: "Configurações" })).toHaveAttribute("aria-current", "page");
  });

  test("sem hub em lugar nenhum: sem seção do hub", async () => {
    const api = new FakeDaemonApi().on("dashboard.get", dashboard({ projects: [] })).on("env.info", info());
    render(<App api={api} />);
    await waitFor(() => expect(api.callsTo("env.info").length).toBeGreaterThan(0));
    expect(screen.queryByRole("button", { name: /^Hub:/ })).not.toBeInTheDocument();
  });
});

describe("Projetos na lateral (#10)", () => {
  const side = () => screen.getByRole("complementary", { name: "Ambientes e projetos" });
  const items = (menu: HTMLElement) => within(menu).getAllByRole("menuitem").map((b) => b.textContent);

  test("sem hub: menu com Abrir, planejamento e GitHub; sem '+'", async () => {
    const api = new FakeDaemonApi([{ ...FakeDaemonApi.wslEnv(), name: "wsl" }])
      .on("dashboard.get", dashboard())
      .on("env.info", info());
    render(<App api={api} />);
    const user = userEvent.setup();
    const linha = await within(side()).findByRole("button", { name: "ctower-agents" });
    expect(within(side()).queryByRole("button", { name: "Novo projeto" })).not.toBeInTheDocument();

    // Botão direito: o foco vai para a primeira ação; Esc devolve à linha.
    fireEvent.contextMenu(linha.closest("li")!, { clientX: 10, clientY: 10 });
    let menu = screen.getByRole("menu", { name: "Ações do projeto ctower-agents" });
    expect(items(menu)).toEqual(["Abrir", "Sessão de planejamento", "Abrir no GitHub"]);
    expect(within(menu).getByRole("menuitem", { name: "Abrir" })).toHaveFocus();
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("menu")).not.toBeInTheDocument();
    expect(linha).toHaveFocus();

    // Shift+F10 na linha focada.
    await user.keyboard("{Shift>}{F10}{/Shift}");
    menu = screen.getByRole("menu", { name: "Ações do projeto ctower-agents" });
    await user.click(within(menu).getByRole("menuitem", { name: "Abrir no GitHub" }));
    expect(api.opened).toEqual(["https://github.com/deep-ion-ai/ctower-agents"]);

    fireEvent.contextMenu(linha.closest("li")!, { clientX: 10, clientY: 10 });
    await user.click(screen.getByRole("menuitem", { name: "Sessão de planejamento" }));
    expect(await screen.findByRole("dialog")).toBeInTheDocument();
  });

  function setupHub() {
    const jira = jiraHubProject();
    const api = new FakeDaemonApi([{ ...FakeDaemonApi.wslEnv(), name: "wsl" }])
      .on("dashboard.get", dashboard({ projects: [] }))
      .on("env.info", info())
      .withHub()
      .onHub("hub.info", hubInfo())
      .onHub("hub.status", hubStatus())
      .onHub("hub.projects.list", [hubProjectView(jira)])
      .onHub("hub.envs.list", [hubEnv()])
      .onHub("hub.dashboard.get", { ...HUB_DASHBOARD, projects: [{ project: jira, work_items: [] }] })
      .onHub("hub.proposals.list", NO_PROPOSALS)
      .onHub("hub.health.get", NO_HEALTH);
    render(<App api={api} />);
    return { api, user: userEvent.setup() };
  }

  test("hub no ar: '+' abre o cadastro em branco", async () => {
    const { user } = setupHub();
    await user.click(await within(side()).findByRole("button", { name: "Novo projeto" }));
    expect(await screen.findByRole("form", { name: "Novo projeto" })).toBeInTheDocument();
  });

  test("hub no ar: menu completo, links do GitLab e do Jira e Configurações", async () => {
    const { api, user } = setupHub();
    const linha = await within(side()).findByRole("button", { name: "app" });
    fireEvent.contextMenu(linha.closest("li")!, { clientX: 10, clientY: 10 });
    const menu = screen.getByRole("menu", { name: "Ações do projeto app" });
    // Sem ambiente conectado com o projeto: sem sessão de planejamento.
    expect(items(menu)).toEqual(["Abrir", "Configurações", "Atribuir a ambiente", "Abrir no GitLab", "Abrir no Jira"]);
    await user.click(within(menu).getByRole("menuitem", { name: "Abrir no GitLab" }));
    fireEvent.contextMenu(linha.closest("li")!, { clientX: 10, clientY: 10 });
    await user.click(screen.getByRole("menuitem", { name: "Abrir no Jira" }));
    expect(api.opened).toEqual(["https://gitlab.example.com/grupo/projeto/app", "https://jira.example.com/browse/PROJ"]);

    fireEvent.contextMenu(linha.closest("li")!, { clientX: 10, clientY: 10 });
    await user.click(screen.getByRole("menuitem", { name: "Configurações" }));
    // A aba Configurações do próprio projeto (#17), não o Hub.
    expect(await screen.findByRole("tab", { name: /app · Configurações/, selected: true })).toBeInTheDocument();
  });

  test("hub no ar: Atribuir a ambiente abre o formulário do projeto", async () => {
    const { user } = setupHub();
    const linha = await within(side()).findByRole("button", { name: "app" });
    linha.focus();
    await user.keyboard("{Shift>}{F10}{/Shift}");
    await user.click(screen.getByRole("menuitem", { name: "Atribuir a ambiente" }));
    expect(await screen.findByRole("form", { name: "Atribuir app" })).toBeInTheDocument();
  });
});
