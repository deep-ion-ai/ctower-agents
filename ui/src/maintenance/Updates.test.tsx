import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { App } from "../App";
import { FakeDaemonApi } from "../api/fake";
import type { EnvInfo, UpdateState, UpdateStatus } from "../protocol/gen";
import { HUB_CONFIG_UNLINKED, dashboard, session, RC_AVAILABLE } from "../test/fixtures";

const OLD = "1111111aaaaaaa";
const NEW = "2222222bbbbbbb";

function status(u: Partial<UpdateStatus> = {}): UpdateStatus {
  return {
    installed: { version: "0.1.0", commit: OLD },
    ref: "main",
    available: {
      ref: "main",
      build: { version: "0.2.0", commit: NEW },
      relation: "behind",
      update_available: true,
      commits: ["2222222 recurso novo", "1a2b3c4 correção"],
      behind: 2,
      checked_at: Date.now(),
    },
    check_error: null,
    state: null,
    ...u,
  };
}

function state(s: Partial<UpdateState> = {}): UpdateState {
  return {
    kind: "update",
    phase: "build",
    ref: "main",
    from: { version: "0.1.0", commit: OLD },
    to: { version: "0.2.0", commit: NEW },
    started_at: 1000,
    updated_at: 1000,
    finished_at: null,
    message: null,
    error: null,
    log_tail: [],
    ...s,
  };
}

function info(env: string, p: Partial<EnvInfo> = {}): EnvInfo {
  return {
    protocol: "2.0",
    version: "0.1.0",
    commit: OLD,
    hostname: env,
    worktrees_root: "/w",
    env_id: `id-${env}`,
    env_name: env,
    hub_sync: null,
    hub_config: HUB_CONFIG_UNLINKED,
    update: status(),
    remote_control: RC_AVAILABLE,
    docs: { root: null, reachable: false },
    ...p,
  };
}

function setup(
  infos: Record<string, EnvInfo>,
  live: Record<string, number> = {},
  prep: (api: FakeDaemonApi) => void = () => {},
  section: "app-updates" | "env-updates" = "env-updates",
) {
  const api = new FakeDaemonApi([
    FakeDaemonApi.wslEnv(),
    { id: "mac", name: "mac-mini", url: "ws://mac:7878/ws", kind: "remote", distro: null, has_token: true },
  ])
    .on("dashboard.get", (_: unknown, env: string) =>
      dashboard({ live_sessions: Array.from({ length: live[env] ?? 0 }, (_, i) => session({ id: `s-${env}-${i}` })) }),
    )
    .on("env.info", (_: unknown, env: string) => infos[env])
    .on("env.update", {})
    .on("env.check_update", (_: unknown, env: string) => infos[env].update)
    .on("github.status", { source: "gh", login: "x", error: null });
  prep(api);
  // Manutenção › Atualizações: Ambientes (todos numa tabela) e Este app (#336).
  render(<App api={api} initial={{ name: "maintenance", section }} />);
  return { api, user: userEvent.setup() };
}

const row = (name: string) => screen.getByRole("rowgroup", { name: `Atualização de ${name}` });

test("mostra instalado, disponível, commits e sessões vivas", async () => {
  setup({ wsl: info("wsl"), mac: info("mac", { update: status({ available: null, check_error: "rede" }) }) }, { wsl: 1 });
  await screen.findByText("0.2.0 · 2222222 · 2 commit(s) novo(s)");
  const wsl = row("ctower-agents (WSL)");
  expect(within(wsl).getByText("0.1.0 · 1111111")).toBeInTheDocument();
  expect(within(wsl).getByText("2222222 recurso novo")).toBeInTheDocument();
  await waitFor(() => expect(within(wsl).getByText("1")).toBeInTheDocument());
  // Todos os ambientes na mesma tabela, sem seletor.
  expect(screen.queryByLabelText("Ambiente da manutenção")).not.toBeInTheDocument();
  const mac = row("mac-mini");
  expect(within(mac).getByText("verificação falhou")).toBeInTheDocument();
  expect(within(mac).getByText("erro na verificação: rede")).toBeInTheDocument();
  expect(within(mac).getByRole("button", { name: "Atualizar mac-mini" })).toBeDisabled();
});

test("atualizar acompanha as fases, a reconexão e confirma o commit novo", async () => {
  const infos = { wsl: info("wsl"), mac: info("mac", { update: status({ available: null }) }) };
  const { api, user } = setup(infos);
  const wsl = await waitFor(() => row("ctower-agents (WSL)"));
  await user.click(await within(wsl).findByRole("button", { name: "Atualizar ctower-agents (WSL)" }));
  const box = screen.getByRole("group", { name: "Confirmar atualização" });
  expect(box).not.toHaveTextContent("sessão(ões) viva(s)");
  await user.click(within(box).getByRole("button", { name: "Atualizar" }));
  await waitFor(() => expect(api.callsTo("env.update")).toEqual([{ env: "wsl", method: "env.update", params: { force: false } }]));
  expect(within(wsl).getByRole("status")).toHaveTextContent("iniciando…");

  act(() => api.push("wsl", "update.status", status({ state: state({ phase: "build" }) })));
  expect(within(wsl).getByRole("status")).toHaveTextContent("compilando…");
  act(() => api.setStatus("wsl", false, "conexão perdida"));
  expect(within(wsl).getByRole("status")).toHaveTextContent("reconectando…");

  infos.wsl = info("wsl", {
    version: "0.2.0",
    commit: NEW,
    update: status({ installed: { version: "0.2.0", commit: NEW }, state: state({ phase: "done" }) }),
  });
  act(() => api.setStatus("wsl", true));
  await waitFor(() => expect(within(wsl).getByRole("status")).toHaveTextContent("atualizado para 0.2.0 · 2222222"));
});

test("falha mostra o erro e o fim do log", async () => {
  const infos = { wsl: info("wsl"), mac: info("mac") };
  const { api, user } = setup(infos);
  const wsl = await waitFor(() => row("ctower-agents (WSL)"));
  await user.click(await within(wsl).findByRole("button", { name: "Atualizar ctower-agents (WSL)" }));
  await user.click(within(screen.getByRole("group", { name: "Confirmar atualização" })).getByRole("button", { name: "Atualizar" }));
  act(() =>
    api.push(
      "wsl",
      "update.status",
      status({ state: state({ phase: "failed", error: "cargo falhou", log_tail: ["error[E0425]: x"] }) }),
    ),
  );
  expect(within(wsl).getByRole("status")).toHaveTextContent("falhou: cargo falhou");
  expect(within(wsl).getByText("error[E0425]: x")).toBeInTheDocument();
});

test("atualizar todos: na ordem da lista e force onde há sessões vivas", async () => {
  const { api, user } = setup({ wsl: info("wsl"), mac: info("mac") }, { wsl: 2 });
  await screen.findAllByText("0.2.0 · 2222222 · 2 commit(s) novo(s)");
  await waitFor(() => expect(within(row("ctower-agents (WSL)")).getByText("2")).toBeInTheDocument());
  await user.click(screen.getByRole("button", { name: "Atualizar todos" }));
  const box = screen.getByRole("group", { name: "Confirmar atualização" });
  expect(box).toHaveTextContent("ctower-agents (WSL), mac-mini");
  expect(box).toHaveTextContent("2 sessão(ões) viva(s)");
  await user.click(within(box).getByRole("button", { name: "Atualizar e encerrar sessões" }));
  await waitFor(() => expect(api.callsTo("env.update")).toHaveLength(2));
  expect(api.callsTo("env.update").map((c) => [c.env, c.params.force])).toEqual([
    ["wsl", true],
    ["mac", false],
  ]);
});

test("verificar agora chama env.check_update em todos os conectados", async () => {
  const { api, user } = setup({ wsl: info("wsl"), mac: info("mac") });
  await screen.findAllByText("0.2.0 · 2222222 · 2 commit(s) novo(s)");
  act(() => api.setStatus("mac", false, "conexão perdida"));
  await user.click(screen.getByRole("button", { name: "Verificar agora" }));
  await waitFor(() => expect(api.callsTo("env.check_update").map((c) => c.env)).toEqual(["wsl"]));
  act(() => api.setStatus("mac", true));
  await waitFor(() => expect(screen.getByRole("button", { name: "Verificar agora" })).toBeEnabled());
  await user.click(screen.getByRole("button", { name: "Verificar agora" }));
  await waitFor(() => expect(api.callsTo("env.check_update").map((c) => c.env).sort()).toEqual(["mac", "wsl", "wsl"]));
});

test("ajustes de atualização levam às Configurações › Ambiente › Atualização do daemon", async () => {
  const { user } = setup({ wsl: info("wsl"), mac: info("mac") });
  await user.click(await screen.findByRole("button", { name: "Ajustes de atualização ›" }));
  expect(screen.getByRole("tab", { name: "Configurações" })).toHaveAttribute("aria-selected", "true");
  expect(await screen.findByRole("heading", { name: "Atualização do daemon" })).toBeInTheDocument();
  // E de volta: a ponte nas Configurações leva à Manutenção.
  await user.click(screen.getByRole("button", { name: "Ver e atualizar em Manutenção ›" }));
  expect(screen.getByRole("tab", { name: "Manutenção · Atualizações · Ambientes", selected: true })).toBeInTheDocument();
});

test("última atualização que falhou aparece sem disparo da tela", async () => {
  setup({
    wsl: info("wsl", { update: status({ state: state({ phase: "rolled_back", error: "não subiu", finished_at: Date.now() }) }) }),
    mac: info("mac"),
  });
  const wsl = await waitFor(() => row("ctower-agents (WSL)"));
  expect(await within(wsl).findByRole("status")).toHaveTextContent(
    "última atualização: falhou e voltou à versão anterior agora",
  );
});

test("app: versão nova, notas, confirmação e progresso do download", async () => {
  const { api, user } = setup({ wsl: info("wsl"), mac: info("mac") }, {}, (api) => {
    api.uiUpdate = { current: "0.1.0", available: { version: "0.2.0", notes: "- recurso novo", date: null }, error: null };
    api.uiUpdateInstall = () => new Promise(() => {});
  }, "app-updates");
  // Aviso no cabeçalho, em qualquer tela.
  expect(await screen.findByRole("button", { name: "App 0.2.0 disponível" })).toBeInTheDocument();
  const app = screen.getByRole("group", { name: "Atualização do app" });
  expect(within(app).getByText("0.1.0")).toBeInTheDocument();
  expect(within(app).getByText("0.2.0")).toBeInTheDocument();
  expect(within(app).getByText("- recurso novo")).toBeInTheDocument();
  await user.click(within(app).getByRole("button", { name: "Atualizar o app" }));
  const box = within(app).getByRole("group", { name: "Confirmar atualização do app" });
  expect(box).toHaveTextContent("Instalar o Control Tower 0.2.0");
  await user.click(within(box).getByRole("button", { name: "Atualizar e reiniciar" }));
  expect(within(app).getByRole("status")).toHaveTextContent("baixando a versão nova…");
  act(() => api.uiProgress({ downloaded: 45, total: 100 }));
  expect(within(app).getByRole("status")).toHaveTextContent("baixando a versão nova… 45%");
});

test("app: em dia, erro de verificação e falha na instalação", async () => {
  const { api, user } = setup({ wsl: info("wsl"), mac: info("mac") }, {}, (api) => {
    api.uiUpdate = { current: "0.1.0", available: null, error: "sem token do GitHub" };
  }, "app-updates");
  const app = await waitFor(() => screen.getByRole("group", { name: "Atualização do app" }));
  expect(await within(app).findByText("verificação falhou")).toBeInTheDocument();
  expect(within(app).getByText("erro na verificação: sem token do GitHub")).toBeInTheDocument();
  expect(within(app).getByRole("button", { name: "Atualizar o app" })).toBeDisabled();
  expect(screen.queryByRole("button", { name: /disponível$/ })).not.toBeInTheDocument();

  api.uiUpdate = { current: "0.1.0", available: { version: "0.3.0", notes: null, date: null }, error: null };
  api.uiUpdateInstall = async () => {
    throw new Error("assinatura inválida");
  };
  await user.click(screen.getByRole("button", { name: "Verificar agora" }));
  await user.click(await within(app).findByRole("button", { name: "Atualizar o app" }));
  await user.click(within(app).getByRole("button", { name: "Atualizar e reiniciar" }));
  expect(await within(app).findByText("Error: assinatura inválida")).toBeInTheDocument();
});
