import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { App } from "../App";
import { FakeDaemonApi } from "../api/fake";
import type { Decision, Review } from "../protocol/gen";
import type { HubDecision, HubHistory } from "../protocol/hub-gen";
import { dashboard, HUB_DASHBOARD, sync, workItem } from "../test/fixtures";

const NOW = Date.now();
const WI = workItem({ gh_number: 28, status: "em_revisao", title: "ct-server: lacunas", pending_decisions: 2 });
const WI40 = workItem({ gh_number: 40, status: "em_execucao", title: "Fila de envio", pending_decisions: 1 });
const PLAN = workItem({ id: 500, gh_number: 0, status: "em_execucao", title: null, pending_decisions: 1 });
const GESTOR = 900;

function decision(id: number, title: string, wi = WI.id): Decision {
  return {
    id,
    uid: `0190-${id}`,
    work_item_id: wi,
    session_id: null,
    origin: "autonoma",
    title,
    context: "A UI precisa dos tipos do protocolo.",
    decision: `Gerar com ts-rs (${id}).`,
    consequences: "Regenerar ao mudar o Rust.",
    reviewed_at: null,
    reviewed_in_pr: null,
    created_at: NOW,
  };
}

const PR = {
  number: 36,
  work_item_id: WI.id,
  head: "feat/28-ct-server",
  base: "develop",
  state: "open" as const,
  url: "https://github.com/deep-ion-ai/ctower-agents/pull/36",
  updated_at: 0,
  head_sha: null,
};

const TWO = [decision(1, "Tipos com ts-rs"), decision(2, "peek de 4 KiB")];

function setup(review: Review, initial: object = { name: "review", env: "wsl", workItemId: WI.id }) {
  const api = new FakeDaemonApi()
    .on("dashboard.get", dashboard({ sync: [sync({ last_ok_at: NOW })] }, [WI]))
    .on("session.peek", { data: "" })
    .on("review.get", review)
    .on("review.mark_reviewed", { marked: review.decisions.length });
  render(<App api={api} initial={initial as never} />);
  return { api, user: userEvent.setup() };
}

const side = () => screen.findByRole("complementary", { name: "Itens com decisões a revisar" });

test("contagem no botão, marcar e lista vazia depois", async () => {
  const { api, user } = setup({ work_item: WI, prs: [PR], decisions: TWO });
  const card = await screen.findByRole("article", { name: "Tipos com ts-rs" });
  expect(within(card).getByText("autônoma")).toBeInTheDocument();
  // A primeira vem aberta; as outras, recolhidas.
  expect(within(card).getByText("Gerar com ts-rs (1).")).toBeVisible();
  expect(within(card).getByText("Contexto")).toBeVisible();
  expect(screen.getByText(/PR #36 aberto/)).toBeInTheDocument();
  expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent("#28 ct-server: lacunas");
  api.on("review.get", { work_item: WI, prs: [PR], decisions: [] });
  await user.click(screen.getByRole("button", { name: "Marcar as 2 como revisadas" }));
  await waitFor(() =>
    expect(api.callsTo("review.mark_reviewed")[0].params).toEqual({ work_item_id: WI.id, pr: 36 }),
  );
  expect(await screen.findByText("Nenhuma decisão pendente.")).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Marcar como revisadas" })).toBeDisabled();
  await user.click(screen.getByRole("button", { name: "Abrir PR #36" }));
  expect(api.opened).toEqual([PR.url]);
});

test("sem PR no espelho não dá para marcar", async () => {
  setup({ work_item: WI, prs: [], decisions: [decision(1, "Tipos com ts-rs")] });
  expect(await screen.findByText(/Sem PR no espelho do GitHub/)).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Marcar 1 como revisada" })).toBeDisabled();
  // Sem PR: o botão abre a criação (compare) e não há "Comentar no PR".
  expect(screen.getByRole("button", { name: "Abrir PR no GitHub" })).toBeEnabled();
  expect(screen.queryByRole("button", { name: "Comentar no PR" })).not.toBeInTheDocument();
});

test("decisão recolhível e Revisada grava só aquela decisão no ledger", async () => {
  const { api, user } = setup({ work_item: WI, prs: [PR], decisions: TWO });
  const second = await screen.findByRole("article", { name: "peek de 4 KiB" });
  const toggle = within(second).getByRole("button", { name: "peek de 4 KiB" });
  expect(toggle).toHaveAttribute("aria-expanded", "false");
  expect(within(second).getByText("Gerar com ts-rs (2).")).not.toBeVisible();
  await user.click(toggle);
  expect(toggle).toHaveAttribute("aria-expanded", "true");
  expect(within(second).getByText("Gerar com ts-rs (2).")).toBeVisible();

  api.on("review.get", { work_item: WI, prs: [PR], decisions: [TWO[1]] });
  const first = screen.getByRole("article", { name: "Tipos com ts-rs" });
  await user.click(within(first).getByRole("button", { name: "Revisada" }));
  await waitFor(() =>
    expect(api.callsTo("review.mark_reviewed")[0].params).toEqual({ work_item_id: WI.id, pr: 36, decision_ids: [1] }),
  );
  await waitFor(() => expect(screen.queryByRole("article", { name: "Tipos com ts-rs" })).not.toBeInTheDocument());
  expect(screen.getByRole("button", { name: "Marcar 1 como revisada" })).toBeEnabled();
});

test("Comentar no PR: texto da decisão editável, envio e link do comentário", async () => {
  const { api, user } = setup({ work_item: WI, prs: [PR], decisions: TWO });
  api.on("github.pr.comment", { id: 99, url: "https://github.com/deep-ion-ai/ctower-agents/pull/36#issuecomment-99" });
  const second = await screen.findByRole("article", { name: "peek de 4 KiB" });
  await user.click(within(second).getByRole("button", { name: "Comentar no PR" }));
  const box = within(second).getByRole("textbox", { name: "Comentário no PR" });
  expect(box).toHaveValue(
    "**Decisão autônoma: peek de 4 KiB**\n\n**Contexto:** A UI precisa dos tipos do protocolo.\n\n" +
      "**Decisão:** Gerar com ts-rs (2).\n\n**Consequências:** Regenerar ao mudar o Rust.",
  );
  await user.clear(box);
  await user.type(box, "Prefiro 8 KiB.");
  await user.click(within(second).getByRole("button", { name: "Enviar comentário" }));
  await waitFor(() =>
    expect(api.callsTo("github.pr.comment")[0].params).toEqual({ project_id: 1, pr: 36, body: "Prefiro 8 KiB." }),
  );
  expect(await within(second).findByText(/Comentário publicado no PR/)).toBeInTheDocument();
  expect(within(second).queryByRole("textbox")).not.toBeInTheDocument();
  await user.click(within(second).getByRole("button", { name: "Ver no GitHub" }));
  expect(api.opened).toEqual(["https://github.com/deep-ion-ai/ctower-agents/pull/36#issuecomment-99"]);
});

test("Comentar no PR: erro do GitHub mantém o texto", async () => {
  const { api, user } = setup({ work_item: WI, prs: [PR], decisions: TWO });
  api.on("github.pr.comment", () => {
    throw new Error("token sem permissão");
  });
  const first = await screen.findByRole("article", { name: "Tipos com ts-rs" });
  await user.click(within(first).getByRole("button", { name: "Comentar no PR" }));
  await user.click(within(first).getByRole("button", { name: "Enviar comentário" }));
  expect(await within(first).findByRole("alert")).toHaveTextContent("token sem permissão");
  expect((within(first).getByRole("textbox", { name: "Comentário no PR" }) as HTMLTextAreaElement).value).toContain(
    "Tipos com ts-rs",
  );
});

test("painel lateral: com PR aberto, sem PR (gestor e planejamento) e seleção", async () => {
  const reviews: Record<number, Review> = {
    [WI.id]: { work_item: WI, prs: [PR], decisions: TWO },
    [WI40.id]: { work_item: WI40, prs: [], decisions: [decision(3, "Fila em disco", WI40.id)] },
    [PLAN.id]: { work_item: PLAN, prs: [], decisions: [decision(4, "Ordem do backlog", PLAN.id)] },
    [GESTOR]: { work_item: { ...WI, id: GESTOR }, prs: [], decisions: [decision(5, "Prioridade do #310", GESTOR)] },
  };
  const api = new FakeDaemonApi()
    .on("dashboard.get", dashboard({ gestor_work_item_id: GESTOR }, [WI, WI40, PLAN]))
    .on("session.peek", { data: "" })
    .on("review.get", (p: { work_item_id: number }) => reviews[p.work_item_id]);
  render(<App api={api} initial={{ name: "review" }} />);
  const user = userEvent.setup();
  const panel = await side();

  // O grupo só aparece quando o review.get do item com PR responde.
  const withPr = await within(panel).findByRole("region", { name: /Com PR aberto/ });
  const noPr = within(panel).getByRole("region", { name: /Sem PR/ });
  expect(await within(withPr).findByRole("button", { name: /#28 ct-server: lacunas PR #36 → develop/ })).toHaveAccessibleDescription(
    "2 pendentes",
  );
  expect(await within(noPr).findByRole("button", { name: /Gestor de projetos decisões autônomas do gestor/ })).toBeInTheDocument();
  expect(within(noPr).getByRole("button", { name: /Planejamento · ctower-agents sessões sem issue associada/ })).toBeInTheDocument();
  expect(within(noPr).getByRole("button", { name: /#40 Fila de envio sem PR aberto/ })).toHaveAccessibleDescription("1 pendente");

  // Sem seleção, o editor mostra o primeiro (com PR aberto).
  expect(await screen.findByRole("article", { name: "Tipos com ts-rs" })).toBeInTheDocument();
  expect(within(withPr).getByRole("button", { name: /#28/ })).toHaveAttribute("aria-current", "true");

  await user.click(within(noPr).getByRole("button", { name: /Gestor de projetos/ }));
  expect(await screen.findByRole("article", { name: "Prioridade do #310" })).toBeInTheDocument();
  expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent("Gestor de projetos");
  expect(screen.getByRole("tab", { name: /Revisão · Gestor de projetos/ })).toBeInTheDocument();
});

test("gestor: marca sem PR, uma decisão ou todas, e esvazia a lista", async () => {
  let pending = [decision(5, "Prioridade do #310", GESTOR), decision(6, "Ordem do #311", GESTOR)];
  const api = new FakeDaemonApi()
    .on("dashboard.get", dashboard({ gestor_work_item_id: GESTOR }, []))
    .on("session.peek", { data: "" })
    .on("review.get", () => ({ work_item: { ...WI, id: GESTOR }, prs: [], decisions: pending }))
    .on("review.mark_reviewed", (p: { decision_ids?: number[] }) => {
      const ids = p.decision_ids ?? pending.map((d) => d.id);
      pending = pending.filter((d) => !ids.includes(d.id));
      return { marked: ids.length };
    });
  render(<App api={api} initial={{ name: "review", env: "wsl", workItemId: GESTOR }} />);
  const user = userEvent.setup();
  expect(await screen.findByText(/O gestor não abre PR/)).toBeInTheDocument();
  expect(screen.queryByRole("button", { name: /Abrir PR/ })).not.toBeInTheDocument();

  const card = await screen.findByRole("article", { name: "Prioridade do #310" });
  // O gestor não abre PR: sem "Comentar no PR".
  expect(within(card).queryByRole("button", { name: "Comentar no PR" })).not.toBeInTheDocument();
  await user.click(within(card).getByRole("button", { name: "Revisada" }));
  await waitFor(() => expect(api.callsTo("review.mark_reviewed")[0].params).toEqual({ work_item_id: GESTOR, decision_ids: [5] }));
  await waitFor(() => expect(screen.queryByRole("article", { name: "Prioridade do #310" })).not.toBeInTheDocument());

  await user.click(screen.getByRole("button", { name: "Marcar 1 como revisada" }));
  await waitFor(() => expect(api.callsTo("review.mark_reviewed")[1].params).toEqual({ work_item_id: GESTOR }));
  expect(await screen.findByText("Nenhuma decisão pendente.")).toBeInTheDocument();
});

test("sem nada para revisar", async () => {
  const api = new FakeDaemonApi().on("dashboard.get", dashboard({}, [])).on("session.peek", { data: "" });
  render(<App api={api} initial={{ name: "review" }} />);
  expect(await screen.findByText(/Nada para revisar/)).toBeInTheDocument();
  expect(within(await side()).getByText("Nenhum item com decisões a revisar.")).toBeInTheDocument();
});

function hubDecision(id: number, title: string, d: Partial<HubDecision>): HubDecision {
  return { ...decision(id, title), env_id: "wsl", ...d };
}

test("filtros Autônomas, Orientadas e Revisadas (histórico do hub)", async () => {
  // Hub anterior ao 1.2: sem `sub_work_items`.
  const history = {
    work_item: { id: WI.id, gh_number: 28 },
    decisions: [
      hubDecision(1, "Tipos com ts-rs", {}),
      hubDecision(7, "Usar tokens do tema", { origin: "orientada", reviewed_at: NOW }),
      hubDecision(8, "Painel por portal", { reviewed_at: NOW, reviewed_in_pr: 30 }),
    ],
  } as unknown as HubHistory;
  const api = new FakeDaemonApi()
    .withHub()
    .onHub("hub.dashboard.get", HUB_DASHBOARD)
    .on("dashboard.get", dashboard({}, [WI]))
    .on("session.peek", { data: "" })
    .on("review.get", { work_item: WI, prs: [PR], decisions: TWO })
    .onHub("hub.workitem.history", history);
  render(<App api={api} initial={{ name: "review", env: "wsl", workItemId: WI.id }} />);
  const user = userEvent.setup();
  const filters = await screen.findByRole("group", { name: "Filtrar decisões" });
  const auto = within(filters).getByRole("button", { name: "Autônomas · 2" });
  expect(auto).toHaveAttribute("aria-pressed", "true");
  const guided = await within(filters).findByRole("button", { name: "Orientadas · 1" });
  await user.click(guided);
  expect(guided).toHaveAttribute("aria-pressed", "true");
  expect(await screen.findByRole("article", { name: "Usar tokens do tema" })).toBeInTheDocument();
  expect(screen.queryByRole("article", { name: "Tipos com ts-rs" })).not.toBeInTheDocument();
  // Orientada não tem "Revisada".
  expect(screen.queryByRole("button", { name: "Revisada" })).not.toBeInTheDocument();

  await user.click(within(filters).getByRole("button", { name: "Revisadas · 1" }));
  const done = await screen.findByRole("article", { name: "Painel por portal" });
  expect(within(done).getByText(/no PR #30/)).toBeInTheDocument();
  expect(api.hubCallsTo("hub.workitem.history")[0].params).toEqual({
    owner: "deep-ion-ai",
    repo: "ctower-agents",
    number: 28,
    include_sub_issues: true,
  });
});

test("épico: decisões das sub-issues no histórico do hub vêm rotuladas", async () => {
  const EPIC = workItem({ gh_number: 310, kind: "epic", status: "em_revisao", title: "UI estilo VS Code", pending_decisions: 0 });
  const history = {
    work_item: { id: 70, gh_number: 310 },
    sub_work_items: [{ id: 71, gh_number: 316 }],
    decisions: [
      hubDecision(7, "Faixa de permissão", { origin: "orientada", work_item_id: 71 }),
      hubDecision(8, "Abas no NavProvider", { origin: "orientada", work_item_id: 70 }),
    ],
  } as unknown as HubHistory;
  const api = new FakeDaemonApi()
    .withHub()
    .onHub("hub.dashboard.get", HUB_DASHBOARD)
    .on("dashboard.get", dashboard({}, [EPIC]))
    .on("session.peek", { data: "" })
    .on("review.get", { work_item: EPIC, prs: [], decisions: [] })
    .onHub("hub.workitem.history", history);
  render(<App api={api} initial={{ name: "review", env: "wsl", workItemId: EPIC.id }} />);
  const user = userEvent.setup();
  const filters = await screen.findByRole("group", { name: "Filtrar decisões" });
  await user.click(await within(filters).findByRole("button", { name: "Orientadas · 2" }));
  const sub = await screen.findByRole("article", { name: "Faixa de permissão" });
  expect(within(sub).getByText(/sub-issue #316/)).toBeInTheDocument();
  const own = screen.getByRole("article", { name: "Abas no NavProvider" });
  expect(within(own).queryByText(/sub-issue/)).not.toBeInTheDocument();
});

test("sem hub, Orientadas e Revisadas ficam indisponíveis", async () => {
  setup({ work_item: WI, prs: [PR], decisions: TWO });
  const filters = await screen.findByRole("group", { name: "Filtrar decisões" });
  expect(within(filters).getByRole("button", { name: "Orientadas" })).toBeDisabled();
  expect(within(filters).getByRole("button", { name: "Revisadas" })).toBeDisabled();
});
