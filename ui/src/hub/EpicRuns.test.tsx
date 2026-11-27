// Hub › Execução de épicos (#304): a aprovação do plano (ondas, sobreposição
// de áreas, modelo editável, aprovar com paralelismo e /code-review) e a
// execução (fila de decisões, devolver ao motor, pausar), contra a RPC
// `hub.epic_run.*` falsa.
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { App } from "../App";
import { FakeDaemonApi } from "../api/fake";
import type { EpicNode, EpicPlan, EpicRun, EpicRunView } from "../protocol/hub-gen";
import { dashboard, hubEnv, hubInfo, hubProject, hubProjectView, hubStatus, HUB_DASHBOARD } from "../test/fixtures";

const NOW = Date.now();

function run(r: Partial<EpicRun> = {}): EpicRun {
  return {
    id: 7,
    project_id: 1,
    owner: "acme",
    repo: "acme-web",
    epic: 120,
    env_id: "env-a",
    status: "aguardando_aprovacao",
    plan_version: null,
    max_parallel: 2,
    code_review: false,
    started_by: "desktop",
    plan_session: null,
    plan_model: null,
    plan_effort: null,
    integration_session: null,
    paused_reason: null,
    resume_at: null,
    created_at: NOW,
    updated_at: NOW,
    ended_at: null,
    ...r,
  };
}

const PLAN: EpicPlan = {
  run_id: 7,
  version: 1,
  state: "rascunho",
  summary: "A #121 define o modelo que a #124 usa.",
  nodes: [
    { number: 121, position: 0, model: "sonnet", effort: "medium", areas: ["api/cart/**"], reason: "", fingerprint: null },
    {
      number: 122,
      position: 1,
      model: "sonnet",
      effort: null,
      areas: ["web/checkout/frete.tsx"],
      reason: "",
      fingerprint: null,
    },
    { number: 124, position: 2, model: "opus", effort: "high", areas: ["web/checkout/**"], reason: "", fingerprint: null },
  ],
  edges: [{ number: 124, blocker: 121, reason: "usa o modelo", source: "analysis" }],
  proposals: [{ kind: "nova", title: "Estorno", body: "", from: null }],
  session_id: "s-plano",
  created_at: NOW,
  approved_at: null,
  approved_by: null,
};

function node(number: number, state: EpicNode["state"], n: Partial<EpicNode> = {}): EpicNode {
  return {
    run_id: 7,
    number,
    state,
    prev_state: null,
    session_id: null,
    worktree: null,
    pr: null,
    attempts: 0,
    assumed_since: null,
    reason: null,
    rebase_head: null,
    rebase_at: null,
    updated_at: NOW,
    ...n,
  };
}

function view(v: Partial<EpicRunView> = {}): EpicRunView {
  return {
    run: run(),
    plan: PLAN,
    approved: null,
    nodes: [],
    decisions: [],
    events: [],
    titles: { 120: "Checkout em etapas", 121: "Modelo do carrinho", 122: "Cálculo de frete", 124: "Pagamento" },
    last_tick_at: NOW,
    ...v,
  };
}

function desktop(v: EpicRunView) {
  const api = new FakeDaemonApi()
    .on("dashboard.get", dashboard({ projects: [] }))
    .withHub("ws://mac-mini:7879/ws")
    .onHub("hub.info", hubInfo({ protocol: "1.26" }))
    .onHub("hub.status", hubStatus())
    .onHub("hub.projects.list", [hubProjectView(hubProject())])
    .onHub("hub.envs.list", [hubEnv()])
    .onHub("hub.dashboard.get", HUB_DASHBOARD)
    .onHub("hub.epic_run.list", [v.run])
    .onHub("hub.epic_run.status", v)
    .onHub("hub.epic_run.set_node", PLAN)
    .onHub("hub.epic_run.approve", run({ status: "executando", plan_version: 1 }))
    .onHub("hub.epic_run.decide", {})
    .onHub("hub.epic_run.handback", node(124, "em_pr"))
    .onHub("hub.epic_run.pause", run({ status: "pausado" }));
  render(<App api={api} initial={{ name: "hub", section: "epic-runs" }} />);
  return { api, user: userEvent.setup() };
}

test("aprovação: ondas, sobreposição, modelo editável e aprovar com paralelismo e code-review", async () => {
  const { api, user } = desktop(view());
  const panel = await screen.findByRole("region", { name: "Execução de épicos" });
  expect(await within(panel).findByLabelText("Estado da execução")).toHaveTextContent("Plano aguardando sua aprovação");
  const waves = within(panel).getByRole("list", { name: "Ondas do plano" });
  const w1 = within(waves).getByRole("listitem", { name: "Onda 1" });
  expect(within(w1).getByRole("article", { name: "#121" })).toBeInTheDocument();
  expect(within(w1).getByText(/2 em paralelo/)).toBeInTheDocument();
  const w2 = within(waves).getByRole("listitem", { name: "Onda 2" });
  expect(within(w2).getByRole("article", { name: "#124" })).toHaveTextContent("Depois de #121");
  expect(within(panel).getByRole("article", { name: "#122" })).toHaveTextContent(
    "Área sobreposta com #124: não rodam juntas",
  );
  expect(within(panel).getByText("A #121 define o modelo que a #124 usa.")).toBeInTheDocument();
  expect(within(panel).getByRole("region", { name: "Propostas da sessão de plano" })).toHaveTextContent("Estorno");

  await user.selectOptions(within(panel).getByLabelText("Modelo de #122"), "haiku");
  await waitFor(() => expect(api.hubCallsTo("hub.epic_run.set_node")).toHaveLength(1));
  expect(api.hubCallsTo("hub.epic_run.set_node")[0].params).toEqual({
    run_id: 7,
    number: 122,
    model: "haiku",
    effort: null,
  });

  const par = within(panel).getByLabelText("Sessões em paralelo");
  await user.clear(par);
  await user.type(par, "3");
  await user.click(within(panel).getByLabelText(/Rodar \/code-review na integração/));
  await user.click(within(panel).getByRole("button", { name: "Aprovar e executar" }));
  await waitFor(() => expect(api.hubCallsTo("hub.epic_run.approve")).toHaveLength(1));
  expect(api.hubCallsTo("hub.epic_run.approve")[0].params).toEqual({
    run_id: 7,
    version: 1,
    max_parallel: 3,
    code_review: true,
  });
});

test("aprovação: nó sem modelo trava o Aprovar", async () => {
  const p = { ...PLAN, nodes: PLAN.nodes.map((n) => (n.number === 124 ? { ...n, model: null, effort: null } : n)) };
  desktop(view({ plan: p }));
  const panel = await screen.findByRole("region", { name: "Execução de épicos" });
  expect(await within(panel).findByRole("button", { name: "Aprovar e executar" })).toBeDisabled();
});

test("execução: decisões com as opções, aguardar com bloqueante, devolver ao motor e pausar", async () => {
  const approved = { ...PLAN, state: "aprovado" as const };
  const v = view({
    run: run({ status: "executando", plan_version: 1, max_parallel: 3 }),
    plan: approved,
    nodes: [
      node(121, "mesclada", { pr: 131 }),
      node(122, "bloqueada", { session_id: "s-122" }),
      node(124, "assumida", { session_id: "s-124", assumed_since: NOW - 4 * 60_000 }),
    ],
    decisions: [
      {
        id: 9,
        run_id: 7,
        number: 122,
        origin: "plano_errado",
        text: "#122: falta a tabela de endereços. Sugestão: esperar a #127",
        options: ["aguardar", "replanejar"],
        answer: null,
        created_at: NOW,
        answered_at: null,
        answered_by: null,
      },
    ],
    events: [{ id: 1, run_id: 7, at: NOW, kind: "decision_requested", number: 122, reason: null, message: "#122 pediu decisão" }],
  });
  const { api, user } = desktop(v);
  const panel = await screen.findByRole("region", { name: "Execução de épicos" });
  const queue = await within(panel).findByRole("region", { name: "Decisões pendentes" });
  expect(queue).toHaveTextContent("a sessão informou que o plano está errado");
  expect(queue).toHaveTextContent("Sugestão: esperar a #127");
  expect(within(queue).getByRole("button", { name: /Replanejar/ })).toBeInTheDocument();
  await user.type(within(queue).getByLabelText("Issue que a sub-issue espera"), "127");
  await user.click(within(queue).getByRole("button", { name: /Aguardar/ }));
  await waitFor(() => expect(api.hubCallsTo("hub.epic_run.decide")).toHaveLength(1));
  expect(api.hubCallsTo("hub.epic_run.decide")[0].params).toEqual({ decision_id: 9, answer: "aguardar", blocker: 127 });

  expect(within(panel).getByRole("article", { name: "#121" })).toHaveTextContent("Mesclada na wip · PR #131");
  const assumed = within(panel).getByRole("article", { name: "#124" });
  expect(assumed).toHaveTextContent("Assumida por você");
  await user.click(within(assumed).getByRole("button", { name: "Devolver ao motor" }));
  await waitFor(() => expect(api.hubCallsTo("hub.epic_run.handback")).toHaveLength(1));
  expect(api.hubCallsTo("hub.epic_run.handback")[0].params).toEqual({ run_id: 7, number: 124 });
  expect(within(panel).getByRole("region", { name: "Eventos do motor" })).toHaveTextContent("#122 pediu decisão");

  await user.click(within(panel).getByRole("button", { name: "Pausar" }));
  await waitFor(() => expect(api.hubCallsTo("hub.epic_run.pause")).toHaveLength(1));
});

test("integração: o PR do épico fecha as sub-issues e o épico; aguardando o seu merge", async () => {
  const v = view({
    run: run({ status: "aguardando_merge", plan_version: 1, integration_session: "s-int" }),
    plan: { ...PLAN, state: "aprovado" },
    nodes: [node(121, "mesclada"), node(122, "mesclada"), node(124, "mesclada")],
  });
  desktop(v);
  const panel = await screen.findByRole("region", { name: "Execução de épicos" });
  expect(await within(panel).findByLabelText("Estado da execução")).toHaveTextContent("Aguardando o seu merge");
  const pr = within(panel).getByRole("region", { name: "PR do épico" });
  expect(pr).toHaveTextContent("Closes #121");
  expect(pr).toHaveTextContent("Closes #124");
  expect(pr).toHaveTextContent("Closes #120");
  expect(pr).toHaveTextContent("não pede este merge");
  expect(within(panel).getByRole("button", { name: "Abrir sessão de integração" })).toBeInTheDocument();
});
