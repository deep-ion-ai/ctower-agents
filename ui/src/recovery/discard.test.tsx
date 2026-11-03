// "Descartar" uma interrompida sem retomar (#138): a confirmação compartilhada,
// na Sala de controle, na paleta e no "Descartar todas" da Recuperação.
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { App } from "../App";
import { FakeDaemonApi } from "../api/fake";
import type { Dashboard, RecoveryItem, Session } from "../protocol/gen";
import { dashboard as base, HUB_DASHBOARD, project, session, sync, workItem } from "../test/fixtures";
import { DISCARD_REASON, discardSummary } from "./discard";

const NOW = Date.now();
const ACME = project({ name: "acme-web", gh_owner: "acme", gh_repo: "acme-web" });
const WI12 = workItem({ gh_number: 12, title: "Tela de login" });
const WI13 = workItem({ gh_number: 13, title: "Recuperar senha" });
const S1 = session({ id: "s-1", work_item_id: 12, state: "interrupted", last_event_at: NOW - 60_000 });
const S2 = session({ id: "s-2", work_item_id: 13, state: "interrupted", exit_code: 1, last_event_at: NOW - 60_000 });

const dash = (interrupted: Session[]): Dashboard =>
  base(
    {
      projects: [{ project: ACME, work_items: [WI12, WI13] }],
      interrupted_sessions: interrupted,
      sync: [sync({ last_ok_at: NOW, last_attempt_at: NOW })],
    },
    [],
  );

function recoveryItem(s: Session, wi: typeof WI12): RecoveryItem {
  return {
    session: s,
    work_item: wi,
    project_name: "acme-web",
    last_event: null,
    stopped_for_ms: 60_000,
    transcript: "original",
    worktree_status: null,
  };
}

/** Ambiente com as interrompidas; o dashboard segue `state.interrupted` (o archive tira dela). */
function setup({ hub = false, interrupted = [S1] }: { hub?: boolean; interrupted?: Session[] } = {}) {
  const state = { interrupted: [...interrupted] };
  const api = new FakeDaemonApi()
    .on("dashboard.get", () => dash(state.interrupted))
    .on("session.archive", (p: { session_id: string }) => {
      state.interrupted = state.interrupted.filter((s) => s.id !== p.session_id);
      return {};
    })
    .on("session.peek", { data: "" })
    .on("recovery.list", () =>
      state.interrupted.map((s) => recoveryItem(s, s.work_item_id === 12 ? WI12 : WI13)),
    )
    .on("recovery.archived", [])
    .on("transcript.tail", { source: "original", entries: [] });
  if (hub) {
    api
      .withHub()
      .onHub("hub.info", { protocol: "1.10", version: "0.3.0", commit: "", name: "hub", config_rev: 1, backup: null })
      .onHub("hub.dashboard.get", HUB_DASHBOARD)
      .onHub("hub.status", { warnings: [] })
      .onHub("hub.projects.list", [])
      .onHub("hub.envs.list", [])
      .onHub("hub.workitem.set_status", {});
  }
  return { api, state, user: userEvent.setup() };
}

/** No boot com interrompidas a Recuperação abre sozinha: volta para a Sala de controle. */
const strip = async () => {
  await screen.findByRole("complementary", { name: "Sessões interrompidas" });
  await userEvent.click(screen.getByRole("button", { name: "Sala de controle" }));
  return (await screen.findByText(/Precisa de você ·/)).closest("section")!;
};
const confirm = () => screen.findByRole("alertdialog", { name: /Descartar/ });

test("Sala de controle: Descartar ao lado de Recuperar arquiva sem mexer no item; a interrompida some", async () => {
  const { api, user } = setup();
  render(<App api={api} />);
  const faixa = await strip();
  expect(within(faixa).getByText("acme-web #12 Tela de login")).toBeInTheDocument();
  expect(within(faixa).getByRole("button", { name: "Recuperar" })).toBeInTheDocument();
  await user.click(within(faixa).getByRole("button", { name: "Descartar: acme-web #12 Tela de login" }));

  const dlg = await confirm();
  expect(dlg).toHaveTextContent("A conversa não será retomada por aqui");
  expect(dlg).toHaveTextContent("descartadas recentemente");
  // Padrão: não mexer no work item. Sem hub, o status nem pode mudar.
  expect(within(dlg).getByRole("radio", { name: "Não mexer no item" })).toBeChecked();
  expect(within(dlg).getByRole("radio", { name: "Marcar o item como concluído" })).toBeDisabled();
  expect(dlg).toHaveTextContent("hub, que está fora do ar");

  await user.click(within(dlg).getByRole("button", { name: "Descartar" }));
  await waitFor(() => expect(api.callsTo("session.archive")).toHaveLength(1));
  expect(api.callsTo("session.archive")[0].params).toEqual({ session_id: "s-1" });
  expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument();

  // O push `session_archived` relê o dashboard: sai da faixa.
  api.push("wsl", "session_archived", { session_id: "s-1" });
  await waitFor(() => expect(screen.queryByText("acme-web #12 Tela de login")).not.toBeInTheDocument());
});

test("Cancelar não arquiva", async () => {
  const { api, user } = setup();
  render(<App api={api} />);
  await user.click(within(await strip()).getByRole("button", { name: /^Descartar:/ }));
  await user.click(within(await confirm()).getByRole("button", { name: "Cancelar" }));
  expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument();
  expect(api.callsTo("session.archive")).toHaveLength(0);
});

test("com o hub: marcar o item como concluído muda o status pelo hub, depois de arquivar", async () => {
  const { api, user } = setup({ hub: true });
  render(<App api={api} />);
  await user.click(within(await strip()).getByRole("button", { name: /^Descartar:/ }));
  const dlg = await confirm();
  const done = within(dlg).getByRole("radio", { name: "Marcar o item como concluído" });
  await waitFor(() => expect(done).toBeEnabled());
  await user.click(done);
  await user.click(within(dlg).getByRole("button", { name: "Descartar" }));
  await waitFor(() => expect(api.hubCallsTo("hub.workitem.set_status")).toHaveLength(1));
  expect(api.callsTo("session.archive")[0].params).toEqual({ session_id: "s-1" });
  expect(api.hubCallsTo("hub.workitem.set_status")[0].params).toEqual({
    owner: "acme",
    repo: "acme-web",
    number: 12,
    status: "concluido",
    reason: DISCARD_REASON,
  });
});

test("com o hub, sem escolher: o status não muda", async () => {
  const { api, user } = setup({ hub: true });
  render(<App api={api} />);
  await user.click(within(await strip()).getByRole("button", { name: /^Descartar:/ }));
  await user.click(within(await confirm()).getByRole("button", { name: "Descartar" }));
  await waitFor(() => expect(api.callsTo("session.archive")).toHaveLength(1));
  expect(api.hubCallsTo("hub.workitem.set_status")).toHaveLength(0);
});

test("erro ao arquivar fica na confirmação", async () => {
  const { api, user } = setup();
  api.on("session.archive", () => {
    throw new Error("sessão não está interrompida");
  });
  render(<App api={api} />);
  await user.click(within(await strip()).getByRole("button", { name: /^Descartar:/ }));
  await user.click(within(await confirm()).getByRole("button", { name: "Descartar" }));
  const result = await screen.findByRole("list", { name: "Resultado de cada sessão" });
  expect(result).toHaveTextContent("erro: Error: sessão não está interrompida");
  expect(screen.getByRole("status")).toHaveTextContent("0 descartadas · 1 erro");
});

test("Recuperação: Descartar todas do ambiente mostra o resultado de cada uma", async () => {
  const { api, user } = setup({ interrupted: [S1, S2] });
  api.on("session.archive", (p: { session_id: string }) => {
    if (p.session_id === "s-2") throw new Error("falhou");
    return {};
  });
  render(<App api={api} initial={{ name: "recovery" } as never} />);
  const panel = await screen.findByRole("complementary", { name: "Sessões interrompidas" });
  await waitFor(() => expect(within(panel).getByRole("button", { name: /Interrompidas/ })).toHaveTextContent("2"));
  await user.click(within(panel).getByRole("button", { name: "Descartar todas" }));
  const dlg = await confirm();
  expect(within(dlg).getByRole("heading")).toHaveTextContent("Descartar 2 sessões?");
  await user.click(within(dlg).getByRole("button", { name: "Descartar 2" }));
  const result = await within(dlg).findByRole("list", { name: "Resultado de cada sessão" });
  const rows = within(result).getAllByRole("listitem");
  expect(rows.map((r) => r.textContent)).toEqual(["#12 Tela de logindescartada", "#13 Recuperar senhaerro: Error: falhou"]);
  expect(api.callsTo("session.archive").map((c) => c.params)).toEqual([{ session_id: "s-1" }, { session_id: "s-2" }]);
  await user.click(within(dlg).getByRole("button", { name: "Fechar" }));
  expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument();
});

test("Recuperação: o Descartar do detalhe passa pela mesma confirmação", async () => {
  const { api, user } = setup();
  render(<App api={api} initial={{ name: "recovery", env: "wsl", session: "s-1" } as never} />);
  const card = await screen.findByRole("region", { name: /#12 Tela de login/ });
  await user.click(within(card).getByRole("button", { name: "Descartar" }));
  expect(api.callsTo("session.archive")).toHaveLength(0);
  await user.click(within(await confirm()).getByRole("button", { name: "Descartar" }));
  await waitFor(() => expect(api.callsTo("session.archive")[0].params).toEqual({ session_id: "s-1" }));
  expect(await screen.findByText("Nenhuma sessão interrompida.")).toBeInTheDocument();
});

test("discardSummary", () => {
  const t = { env: "wsl", session: "s-1", name: "x" };
  expect(discardSummary([{ target: t, ok: true }])).toBe("1 descartada");
  expect(discardSummary([{ target: t, ok: true }, { target: t, ok: false, error: "x" }])).toBe("1 descartada · 1 erro");
});
