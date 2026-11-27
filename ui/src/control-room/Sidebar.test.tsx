import { afterEach, expect, test } from "vitest";
import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { App } from "../App";
import { FakeDaemonApi } from "../api/fake";
import type { EnvInfo } from "../protocol/gen";
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

afterEach(cleanup);

function info(p: Partial<EnvInfo> = {}): EnvInfo {
  return {
    protocol: "2.0",
    version: "0.1.0",
    capabilities: ["version-handshake"],
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


describe("Projetos na lateral (#10)", () => {
  const side = () => screen.getByRole("complementary", { name: "Projetos e contas" });
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
    expect(items(menu)).toEqual(["Abrir", "Sessão de planejamento", "Sincronizar este projeto", "Abrir no GitHub"]);
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
    expect(items(menu)).toEqual([
      "Abrir",
      "Configurações",
      "Atribuir a ambiente",
      // Projeto Jira sem o ambiente de sync conectado (#90): desabilitado, com o motivo.
      "Sincronizar este projetoO ambiente de sync daemon-wsl não está conectado com este projeto.",
      "Abrir no GitLab",
      "Abrir no Jira",
    ]);
    expect(within(menu).getByRole("menuitem", { name: "Sincronizar este projeto" })).toBeDisabled();
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
