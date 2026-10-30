// Ações no ambiente pela PWA (fase 2 do épico #205, #343): iniciar, retomar,
// encerrar, ligar o Remote Control e abrir, marcar decisões como revisadas e
// retomar pela Recuperação, pelo canal reverso (`hub.env.call`, a `call` da
// `FakeDaemonApi.asWeb()`). Só com o ambiente online.
import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach } from "vitest";
import { App } from "../App";
import type { FakeDaemonApi } from "../api/fake";
import type { Dashboard, Review, Session } from "../protocol/gen";
import type { HubSession } from "../protocol/hub-gen";
import type { Screen } from "../state/nav";
import { hubEnv } from "../test/fixtures";
import { setViewport } from "../test/viewport";
import { ENV, envCalls, hubItem, hubSession, now, webApi } from "../test/web";
import { resetRcRequests } from "./envActions";

afterEach(() => {
  resetRcRequests();
  setViewport(1280);
});

function renderApp(api: FakeDaemonApi, initial?: Screen) {
  const user = userEvent.setup();
  render(<App api={api} initial={initial} />);
  return user;
}

/** O ambiente perdeu o canal com o hub: `online: false` e o `env.online`. */
function offline(api: FakeDaemonApi) {
  api.offline.add(ENV);
  return api.onHub("hub.envs.list", [hubEnv({ last_seen_at: now, online: false })]);
}

/** Sessão do ambiente (`session.get`) a partir da do hub. */
function envSession(s: HubSession, p: Partial<Session> = {}): Session {
  return {
    id: s.id,
    work_item_id: s.work_item_id,
    tool: s.tool,
    tool_session_id: s.tool_session_id,
    transcript_path: null,
    backup_path: null,
    cwd: s.cwd,
    launch_cmd: [],
    state: s.state,
    exit_code: s.exit_code,
    pid: 4242,
    started_at: s.started_at,
    last_event_at: s.last_event_at,
    ended_at: s.ended_at,
    model: s.model,
    mode: s.mode,
    rc_session_id: s.rc_session_id,
    ...p,
  };
}

describe("ações visíveis na web só com o ambiente online", () => {
  it("online: o painel da sessão tem Ligar Remote Control e Encerrar", async () => {
    const user = renderApp(webApi());
    await user.click(await screen.findByRole("button", { name: /WebDaemonApi/ }));
    const actions = await screen.findByRole("region", { name: "Ações da sessão" });
    expect(within(actions).getByRole("button", { name: "Ligar Remote Control" })).toBeInTheDocument();
    expect(within(actions).getByRole("button", { name: "Encerrar" })).toBeInTheDocument();
  });

  it("offline: sem ações no painel, na Recuperação, na Revisão nem no Projeto", async () => {
    const live = hubSession();
    const int = hubSession({ id: "s-int", state: "interrupted", ended_at: now - 60_000 });
    const api = offline(
      webApi({ live_sessions: [live], interrupted_sessions: [int] }, [hubItem({ pending_decisions: 2 })]),
    );
    const user = renderApp(api);
    await user.click(await screen.findByRole("button", { name: /WebDaemonApi/ }));
    expect(await screen.findByText(/Ambiente offline: iniciar, retomar/)).toBeInTheDocument();
    expect(screen.queryByRole("region", { name: "Ações da sessão" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Ligar Remote Control" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Encerrar" })).toBeNull();

    await user.click(screen.getByRole("button", { name: "Recuperação" }));
    expect(await screen.findByRole("heading", { name: "Recuperação · 1" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /^Retomar/ })).toBeNull();

    await user.click(screen.getByRole("button", { name: "Revisão" }));
    expect(await screen.findByRole("heading", { name: "Revisão · 1" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /como revisadas/ })).toBeNull();
    expect(envCalls(api)).toEqual([]);
  });

  it("o env.online derruba as ações em tempo real", async () => {
    let online = true;
    const api = webApi().onHub("hub.envs.list", () => [hubEnv({ last_seen_at: now, online })]);
    const user = renderApp(api);
    await user.click(await screen.findByRole("button", { name: /WebDaemonApi/ }));
    expect(await screen.findByRole("button", { name: "Encerrar" })).toBeInTheDocument();
    online = false;
    act(() => api.hubPush("env.online", { env_id: ENV, online: false }));
    await waitFor(() => expect(screen.queryByRole("button", { name: "Encerrar" })).toBeNull(), { timeout: 3000 });
    expect(screen.getByText(/Ambiente offline/)).toBeInTheDocument();
  });
});

describe("pushes do ambiente (env.push)", () => {
  it("relêem a visão do hub", async () => {
    const api = webApi();
    renderApp(api);
    await screen.findAllByText("WebDaemonApi");
    const before = api.hubCallsTo("hub.dashboard.get").length;
    act(() => api.push(ENV, "session_started", { session_id: "s-9" }));
    await waitFor(() => expect(api.hubCallsTo("hub.dashboard.get").length).toBeGreaterThan(before), {
      timeout: 2000,
    });
    // Push fora dos que mudam o dashboard: nada.
    const after = api.hubCallsTo("hub.dashboard.get").length;
    act(() => api.push(ENV, "update.status", {}));
    await new Promise((r) => setTimeout(r, 700));
    expect(api.hubCallsTo("hub.dashboard.get").length).toBe(after);
  });
});

describe("ações da sessão pelo canal reverso", () => {
  it("ligar o Remote Control e abrir o link quando o id chega", async () => {
    const live = hubSession();
    let rc: string | null = null;
    const api = webApi({ live_sessions: [live] })
      .on("session.remote_control.enable", () => ({ requested: true, rc_session_id: null }))
      .on("session.get", () => envSession(live, { rc_session_id: rc }));
    const user = renderApp(api);
    await user.click(await screen.findByRole("button", { name: /WebDaemonApi/ }));
    await user.click(await screen.findByRole("button", { name: "Ligar Remote Control" }));
    expect(api.callsTo("session.remote_control.enable")).toEqual([
      { env: ENV, method: "session.remote_control.enable", params: { session_id: "s-201" } },
    ]);
    expect(await screen.findByText(/Conectando ao Remote Control/)).toBeInTheDocument();
    // O id chega (hook ou PTY no ambiente): a leitura seguinte o traz e o link abre sozinho.
    rc = "session_01Ws";
    await waitFor(() => expect(api.opened).toEqual(["https://claude.ai/code/session_01Ws"]), { timeout: 5000 });
    expect(await screen.findByRole("button", { name: "Abrir no Remote Control" })).toBeInTheDocument();
    expect(screen.queryByText(/Conectando/)).toBeNull();
  });

  it("encerrar pede confirmação e chama session.kill", async () => {
    const api = webApi().on("session.kill", {});
    const user = renderApp(api);
    await user.click(await screen.findByRole("button", { name: /WebDaemonApi/ }));
    await user.click(await screen.findByRole("button", { name: "Encerrar" }));
    expect(api.callsTo("session.kill")).toEqual([]);
    await user.click(screen.getByRole("button", { name: "Confirmar encerramento" }));
    await waitFor(() =>
      expect(api.callsTo("session.kill")).toEqual([
        { env: ENV, method: "session.kill", params: { session_id: "s-201" } },
      ]),
    );
  });

  it("retomar no painel de uma sessão encerrada liga o Remote Control", async () => {
    const ended = hubSession({ state: "interrupted", ended_at: now - 60_000 });
    const api = webApi({ live_sessions: [], interrupted_sessions: [ended] })
      .on("session.resume", () => envSession(ended, { id: "s-202", state: "starting" }))
      .on("session.get", () => envSession(ended, { id: "s-202", state: "running" }));
    const user = renderApp(api, { name: "terminal" });
    await act(async () => {});
    // Abre a sessão interrompida pela Recuperação ("Ver sessão").
    await user.click(screen.getByRole("button", { name: "Recuperação" }));
    await user.click(await screen.findByRole("button", { name: "Ver sessão" }));
    await user.click(await screen.findByRole("button", { name: "Retomar" }));
    await waitFor(() => expect(api.callsTo("session.resume")).toHaveLength(1));
    expect(api.callsTo("session.resume")[0].params).toMatchObject({ session_id: "s-201", remote_control: true });
    // A sessão nova ainda não está no hub: o painel a lê no ambiente e espera o Remote Control.
    expect(await screen.findByText(/Conectando ao Remote Control/)).toBeInTheDocument();
    const gets = api.callsTo("session.get");
    expect(gets[gets.length - 1].params).toEqual({ session_id: "s-202" });
  });

  it("retomar pela Recuperação", async () => {
    const int = hubSession({ id: "s-int", state: "interrupted", ended_at: now - 60_000 });
    const api = webApi({ live_sessions: [], interrupted_sessions: [int] })
      .on("session.resume", () => envSession(int, { id: "s-new", state: "starting" }))
      .on("session.get", () => envSession(int, { id: "s-new", state: "running" }));
    const user = renderApp(api, { name: "recovery" });
    await user.click(await screen.findByRole("button", { name: /^Retomar .*#201/ }));
    await waitFor(() => expect(api.callsTo("session.resume")).toHaveLength(1));
    expect(api.callsTo("session.resume")[0]).toMatchObject({
      env: ENV,
      params: { session_id: "s-int", remote_control: true },
    });
    // Abre o painel da sessão retomada.
    expect(await screen.findByRole("region", { name: "Ações da sessão" })).toBeInTheDocument();
  });

  it("erro do ambiente aparece no painel", async () => {
    const api = webApi().on("session.remote_control.enable", () => {
      throw new Error("Remote Control indisponível: ANTHROPIC_API_KEY no ambiente do ctowerd");
    });
    const user = renderApp(api);
    await user.click(await screen.findByRole("button", { name: /WebDaemonApi/ }));
    await user.click(await screen.findByRole("button", { name: "Ligar Remote Control" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("ANTHROPIC_API_KEY");
  });
});

describe("iniciar sessão pela modal de início", () => {
  it("planejamento no ambiente da atribuição, com o Remote Control marcado", async () => {
    const started = envSession(hubSession({ id: "s-plan" }), { state: "starting" });
    const api = webApi()
      .on("env.info", { worktrees_root: "/w", remote_control: { available: true, reason: null } })
      .on("session.start_project", () => started)
      .on("session.get", () => ({ ...started, state: "running" }));
    const user = renderApp(api, { name: "project", owner: "deep-ion-ai", repo: "ctower-agents" });
    await user.click(await screen.findByRole("button", { name: /Sessão de planejamento/ }));
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByLabelText("Ambiente")).toHaveValue(ENV);
    const rc = within(dialog).getByRole("checkbox", { name: "Remote Control" });
    expect(rc).toBeChecked();
    await waitFor(() => expect(api.callsTo("env.info")).toHaveLength(1));
    await user.click(within(dialog).getByRole("button", { name: /Iniciar sessão/ }));
    await waitFor(() => expect(api.callsTo("session.start_project")).toHaveLength(1));
    expect(api.callsTo("session.start_project")[0]).toMatchObject({
      env: ENV,
      params: { owner: "deep-ion-ai", repo: "ctower-agents", remote_control: true },
    });
    // Abre o painel da sessão, que espera o id do Remote Control.
    expect(await screen.findByText(/Conectando ao Remote Control/)).toBeInTheDocument();
  });

  it("sem ambiente online com o projeto: sem o botão de iniciar", async () => {
    const api = offline(webApi());
    renderApp(api, { name: "project", owner: "deep-ion-ai", repo: "ctower-agents" });
    expect(await screen.findByText(/Nenhum ambiente com este projeto está online/)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Sessão de planejamento/ })).toBeNull();
  });
});

describe("Revisão: marcar decisões como revisadas", () => {
  it("usa os ids do ambiente e o PR aberto", async () => {
    let pending = 2;
    const dash = (): Dashboard => ({
      projects: [
        {
          project: {
            id: 7,
            name: "ctower-agents",
            gh_owner: "deep-ion-ai",
            gh_repo: "ctower-agents",
            repo_path: "/r",
            base_branch: "develop",
            protected_branches: [],
            created_at: 0,
            config_rev: null,
            unassigned_at: null,
            location_state: null,
            location_error: null,
            applied_rev: null,
            kind: "github",
            tracker: { kind: "github" },
            forge: { kind: "github" },
            branch_pattern: null,
            attribution: true,
            sync_role: false,
            docs: { enabled: false, project_dirs: [], epic_dirs: [] },
          },
          work_items: [
            {
              title: "WebDaemonApi",
              parent_number: null,
              pending_decisions: pending,
              id: 77,
              project_id: 7,
              gh_number: 201,
              kind: "issue",
              status: "em_revisao",
              branch: "feat/201",
              worktree_path: null,
              created_at: 0,
              closed_at: null,
            },
          ],
        },
      ],
      live_sessions: [],
      interrupted_sessions: [],
      failed_sessions: [],
      sync: [],
      gestor_work_item_id: null,
      gestor_scopes: {},
    });
    const review = {
      work_item: {},
      prs: [
        { number: 300, work_item_id: 77, head: "feat/201", base: "develop", state: "merged", url: "", updated_at: 0, head_sha: null },
        { number: 310, work_item_id: 77, head: "feat/201", base: "develop", state: "open", url: "", updated_at: 0, head_sha: null },
      ],
      decisions: [{ id: 1 }, { id: 2 }],
    } as unknown as Review;
    const api = webApi({ live_sessions: [] }, [hubItem({ status: "em_revisao", pending_decisions: 2 })])
      .on("dashboard.get", dash)
      .on("review.get", review)
      .on("review.mark_reviewed", () => {
        pending = 0;
        return { marked: 2 };
      });
    const user = renderApp(api, { name: "review" });
    await user.click(await screen.findByRole("button", { name: /^Marcar decisões de .*#201/ }));
    expect(await screen.findByText("2 decisões marcadas como revisadas.")).toBeInTheDocument();
    expect(api.callsTo("review.get")[0].params).toEqual({ work_item_id: 77 });
    expect(api.callsTo("review.mark_reviewed")).toEqual([
      { env: ENV, method: "review.mark_reviewed", params: { work_item_id: 77, pr: 310 } },
    ]);
  });
});

describe("no celular", () => {
  it("as ações do painel ficam com alvo de toque de 44 px", async () => {
    setViewport(360);
    const user = renderApp(webApi());
    const bar = await screen.findByRole("navigation", { name: "Telas" });
    expect(bar).toHaveClass("bottombar");
    await user.click(await screen.findByRole("button", { name: /WebDaemonApi/ }));
    const actions = await screen.findByRole("region", { name: "Ações da sessão" });
    // A regra do shell do celular vale para todo botão dentro dele.
    expect(actions.closest('.shell[data-mobile="true"]')).not.toBeNull();
    expect(within(actions).getByRole("button", { name: "Ligar Remote Control" })).toBeInTheDocument();
  });
});

it("Visão geral do projeto na web (#19): saúde e gestor só leitura, sem Sincronizar agora", async () => {
  renderApp(webApi(), { name: "project", owner: "deep-ion-ai", repo: "ctower-agents", view: "overview" });
  expect(await screen.findByRole("heading", { name: "Saúde · 0 sinais" })).toBeInTheDocument();
  const gestor = (await screen.findByRole("heading", { name: "Gestor deste projeto" })).closest("section")!;
  expect(gestor).toHaveTextContent("Iniciar o gestor fica no app desktop");
  expect(within(gestor).queryByRole("button", { name: "Gestor deste projeto" })).not.toBeInTheDocument();
  // `sync.now` fica fora do canal reverso: o cartão nem aparece.
  expect(screen.queryByRole("heading", { name: "Sincronizar" })).not.toBeInTheDocument();
  expect(screen.queryByRole("button", { name: "Sincronizar agora este projeto" })).not.toBeInTheDocument();
});
