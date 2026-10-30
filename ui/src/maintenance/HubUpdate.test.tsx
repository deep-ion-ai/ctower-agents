import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { App } from "../App";
import { FakeDaemonApi } from "../api/fake";
import type { EnvInfo } from "../protocol/gen";
import type { HubUpdateStatus, UpdateState } from "../protocol/hub-gen";
import { HUB_CONFIG_UNLINKED, HUB_DASHBOARD, RC_AVAILABLE, dashboard, hubInfo, hubStatus } from "../test/fixtures";

const OLD = "1111111aaaaaaa";
const NEW = "2222222bbbbbbb";
const PREV = "0000000ccccccc";

function upd(u: Partial<HubUpdateStatus> = {}): HubUpdateStatus {
  return {
    installed: { version: "0.14.0", commit: OLD },
    ref: "main",
    available: {
      ref: "main",
      build: { version: "0.15.0", commit: NEW },
      relation: "behind",
      update_available: true,
      commits: ["2222222 hub novo"],
      behind: 1,
      checked_at: Date.now(),
    },
    check_error: null,
    state: null,
    previous: { version: "0.13.0", commit: PREV },
    ...u,
  };
}

function state(s: Partial<UpdateState> = {}): UpdateState {
  return {
    kind: "update",
    phase: "build",
    ref: "main",
    from: { version: "0.14.0", commit: OLD },
    to: { version: "0.15.0", commit: NEW },
    started_at: 1000,
    updated_at: 1000,
    finished_at: null,
    message: null,
    error: null,
    log_tail: [],
    ...s,
  };
}

function envInfo(p: Partial<EnvInfo> = {}): EnvInfo {
  return {
    protocol: "2.4",
    version: "0.14.0",
    commit: OLD,
    hostname: "wsl",
    worktrees_root: "/w",
    env_id: "id-wsl",
    env_name: "wsl",
    hub_sync: null,
    hub_config: HUB_CONFIG_UNLINKED,
    update: null,
    remote_control: RC_AVAILABLE,
    docs: { root: null, reachable: false },
    ...p,
  };
}

interface Opts {
  update?: () => HubUpdateStatus;
  info?: () => ReturnType<typeof hubInfo>;
  env?: EnvInfo;
  section?: "hub-updates" | "env-updates" | "overview";
  prep?: (api: FakeDaemonApi) => void;
}

function setup(o: Opts = {}) {
  const api = new FakeDaemonApi([FakeDaemonApi.wslEnv()])
    .on("dashboard.get", dashboard())
    .on("env.info", () => o.env ?? envInfo())
    .on("env.update", {})
    .withHub()
    .onHub("hub.info", () => (o.info ?? (() => hubInfo({ version: "0.14.0", commit: OLD, protocol: "1.6" })))())
    .onHub("hub.status", hubStatus())
    .onHub("hub.projects.list", [])
    .onHub("hub.envs.list", [])
    .onHub("hub.dashboard.get", HUB_DASHBOARD)
    .onHub("hub.update.get", () => (o.update ?? (() => upd()))())
    .onHub("hub.update.check", () => upd())
    .onHub("hub.update.start", () => upd())
    .onHub("hub.update.rollback", () => upd());
  o.prep?.(api);
  render(<App api={api} initial={{ name: "maintenance", section: o.section ?? "hub-updates" }} />);
  return { api, user: userEvent.setup() };
}

const group = () => screen.getByRole("group", { name: "Atualização do hub" });

test("mostra no ar, protocolo, disponível e os commits novos", async () => {
  setup();
  const g = await waitFor(() => group());
  expect(await within(g).findByText("0.14.0 · 1111111 · protocolo 1.6")).toBeInTheDocument();
  expect(within(g).getByText("0.15.0 · 2222222 · 1 commit(s) novo(s)")).toBeInTheDocument();
  expect(within(g).getByText("2222222 hub novo")).toBeInTheDocument();
  expect(within(g).getByRole("button", { name: "Atualizar o hub" })).toBeEnabled();
});

test("atualizar: fases pelo push, a reconexão e o resultado lido do state.json", async () => {
  let current = upd();
  const { api, user } = setup({ update: () => current });
  const g = await waitFor(() => group());
  await user.click(await within(g).findByRole("button", { name: "Atualizar o hub" }));
  const box = screen.getByRole("group", { name: "Confirmar atualização do hub" });
  expect(box).toHaveTextContent("falha no build não toca no hub em uso");
  await user.click(within(box).getByRole("button", { name: "Atualizar o hub" }));
  await waitFor(() => expect(api.hubCallsTo("hub.update.start")).toHaveLength(1));
  expect(within(g).getByRole("status")).toHaveTextContent("iniciando…");

  act(() => api.hubPush("hub.update.status", upd({ state: state({ phase: "build" }) })));
  expect(within(g).getByRole("status")).toHaveTextContent("compilando…");
  act(() => api.hubPush("hub.update.status", upd({ state: state({ phase: "restart" }) })));
  expect(within(g).getByRole("status")).toHaveTextContent("reiniciando o hub…");
  act(() => api.setHubStatus(false, "conexão perdida"));
  expect(screen.getByRole("group", { name: "Atualização do hub" })).toHaveTextContent("reconectando…");

  // O hub novo sobe e relata pelo state.json (`hub.update.get` ao reconectar).
  current = upd({ installed: { version: "0.15.0", commit: NEW }, available: null, state: state({ phase: "verify" }) });
  act(() => api.setHubStatus(true));
  await waitFor(() =>
    expect(within(group()).getByRole("status")).toHaveTextContent("atualizado para 0.15.0 · 2222222"),
  );
});

test("falha na saúde: rollback automático aparece com o fim do log", async () => {
  const { api, user } = setup();
  const g = await waitFor(() => group());
  await user.click(await within(g).findByRole("button", { name: "Atualizar o hub" }));
  await user.click(
    within(screen.getByRole("group", { name: "Confirmar atualização do hub" })).getByRole("button", {
      name: "Atualizar o hub",
    }),
  );
  await waitFor(() => expect(api.hubCallsTo("hub.update.start")).toHaveLength(1));
  act(() =>
    api.hubPush(
      "hub.update.status",
      upd({
        state: state({
          phase: "rolled_back",
          error: "o cthubd não ficou saudável em 60s",
          log_tail: ["hub.info falhou"],
        }),
      }),
    ),
  );
  expect(within(g).getByRole("status")).toHaveTextContent(
    "falhou e voltou à versão anterior: o cthubd não ficou saudável em 60s",
  );
  expect(within(g).getByText("hub.info falhou")).toBeInTheDocument();
});

test("última atualização do hub aparece sem disparo da tela", async () => {
  setup({ update: () => upd({ state: state({ phase: "failed", error: "Node antigo", finished_at: Date.now() }) }) });
  const g = await waitFor(() => group());
  expect(await within(g).findByRole("status")).toHaveTextContent("última atualização: falhou agora");
});

test("rollback manual pede confirmação do banco quando o hub.db volta do snapshot", async () => {
  const { api, user } = setup({
    prep: (api) =>
      api.onHub("hub.update.rollback", (p: { force?: boolean }) => {
        if (!p.force)
          throw new Error(
            "o rollback troca o banco: hub.db volta ao snapshot de 20261028-1200 UTC (esquema 30 → 29); sai do banco em uso: hub_events +12. O banco atual fica numa cópia .pre-rollback-*. Confirme com force para seguir",
          );
        return upd();
      }),
  });
  const g = await waitFor(() => group());
  await user.click(await within(g).findByRole("button", { name: "Voltar à versão anterior" }));
  const box = screen.getByRole("group", { name: "Confirmar rollback do hub" });
  expect(box).toHaveTextContent("0.13.0 · 0000000");
  await user.click(within(box).getByRole("button", { name: "Voltar à versão anterior" }));
  const db = await screen.findByRole("group", { name: "Confirmar a troca do hub.db" });
  expect(db).toHaveTextContent("sai do banco em uso: hub_events +12");
  expect(db).not.toHaveTextContent("Confirme com force");
  expect(db).toHaveTextContent("ctower hub resync");
  await user.click(within(db).getByRole("button", { name: "Voltar e trocar o banco" }));
  await waitFor(() =>
    expect(api.hubCallsTo("hub.update.rollback").map((c) => c.params)).toEqual([{ force: false }, { force: true }]),
  );
  expect(within(g).getByRole("status")).toHaveTextContent("iniciando…");
});

test("hub anterior ao 1.6: sem botões e a dica do terminal", async () => {
  setup({
    info: () => hubInfo({ version: "0.13.0", protocol: "1.5" }),
    prep: (api) =>
      api.onHub("hub.update.get", () => {
        throw new Error("método desconhecido: hub.update.get");
      }),
  });
  expect(await screen.findByText(/ainda não atualiza pela UI/)).toHaveTextContent("cthubd update");
  expect(screen.queryByRole("button", { name: "Atualizar o hub" })).not.toBeInTheDocument();
});

test("ambiente à frente do hub: sugere atualizar o hub primeiro", async () => {
  setup({ env: envInfo({ version: "0.15.0" }) });
  expect(await screen.findByText(/à frente do hub \(0\.14\.0\): atualize o hub primeiro/)).toHaveTextContent(
    "ctower-agents (WSL) (0.15.0)",
  );
});

test("ao atualizar ambientes com o hub desatualizado, a confirmação alerta e leva ao hub", async () => {
  const { user } = setup({
    section: "env-updates",
    env: envInfo({
      update: {
        installed: { version: "0.14.0", commit: OLD },
        ref: "main",
        available: {
          ref: "main",
          build: { version: "0.15.0", commit: NEW },
          relation: "behind",
          update_available: true,
          commits: [],
          behind: 1,
          checked_at: Date.now(),
        },
        check_error: null,
        state: null,
      },
    }),
  });
  await user.click(await screen.findByRole("button", { name: "Atualizar ctower-agents (WSL)" }));
  const box = screen.getByRole("group", { name: "Confirmar atualização" });
  await waitFor(() => expect(within(box).getByRole("alert")).toHaveTextContent("Atualize o hub antes dos ambientes"));
  await user.click(within(box).getByRole("button", { name: "Atualizar o hub primeiro ›" }));
  expect(screen.getByRole("tab", { name: "Manutenção · Atualizações · Hub", selected: true })).toBeInTheDocument();
});

test("visão geral: linha do hub com a atualização disponível", async () => {
  setup({ section: "overview" });
  const r = await screen.findByRole("row", { name: "Hub" });
  await waitFor(() => expect(r).toHaveTextContent("atualização disponível 0.15.0"));
});

test("web: a Manutenção não existe; o estado aparece nos dados do hub", async () => {
  const api = new FakeDaemonApi([])
    .asWeb()
    .onHub("hub.info", hubInfo({ version: "0.14.0", commit: OLD, protocol: "1.6" }))
    .onHub("hub.status", hubStatus())
    .onHub("hub.projects.list", [])
    .onHub("hub.envs.list", [])
    .onHub("hub.dashboard.get", HUB_DASHBOARD)
    .onHub("hub.update.get", upd({ state: state({ phase: "build", updated_at: Date.now() }) }));
  render(<App api={api} initial={{ name: "hub" }} />);
  const dl = await screen.findByLabelText("Dados do hub");
  await waitFor(() => expect(dl).toHaveTextContent("em andamento: compilando…"));
  expect(screen.queryByRole("button", { name: "Atualizar o hub" })).not.toBeInTheDocument();
});
