import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { App } from "../App";
import { FakeDaemonApi } from "../api/fake";
import type { ArchivedItem, RecoveryItem } from "../protocol/gen";
import { dashboard, project, session, sync, workItem } from "../test/fixtures";
import { reason, RESUME_PROMPT, TAIL_ENTRIES, worktreeSummary } from "./RecoveryScreen";

const NOW = Date.now();
const WI29 = workItem({ gh_number: 29, title: "ct-client: cliente com reconexão" });
const WI97 = workItem({ gh_number: 97, title: "Paginação de pedidos" });
const S29 = session({ id: "29292929-0000-0000-0000-000000000000", work_item_id: 29, state: "interrupted", tool_session_id: "9f3c" });
const S97 = session({ id: "97979797-0000-0000-0000-000000000000", work_item_id: 97, state: "interrupted", exit_code: 1 });

function item(s: typeof S29, wi: typeof WI29, transcript: RecoveryItem["transcript"]): RecoveryItem {
  return {
    session: s,
    work_item: wi,
    project_name: "ctower-agents",
    last_event: null,
    stopped_for_ms: 130 * 60_000,
    transcript,
    worktree_status: "## feat/29...origin/feat/29 [ahead 3]\n M src/lib.rs",
  };
}

// Fora do dashboard: o nome fica só com o número.
const WI40 = workItem({ gh_number: 40 });
const S40 = session({ id: "40404040-0000-0000-0000-000000000000", work_item_id: 40, state: "archived" });

function archivedItem(transcript: ArchivedItem["transcript"] = "original"): ArchivedItem {
  return { session: S40, work_item: WI40, project_name: "ctower-agents", archived_at: NOW - 3 * 3_600_000, transcript };
}

const TAIL = {
  source: "backup",
  entries: [
    { role: "user", text: "Implemente a reconexão.", ts: null },
    { role: "tool", text: "Bash: cargo test", ts: null },
    { role: "assistant", text: "Testes passando; falta o push.", ts: "2026-10-28T10:00:00Z" },
  ],
};

function setup(initial?: object) {
  const api = new FakeDaemonApi()
    .on(
      "dashboard.get",
      dashboard({ interrupted_sessions: [S29, S97], sync: [sync({ last_ok_at: NOW })] }, [WI29, WI97]),
    )
    .on("recovery.list", [item(S29, WI29, "restauravel_do_backup"), item(S97, WI97, "perdido")])
    .on("session.resume", (p: { session_id: string }) =>
      session({ id: `novo-${p.session_id.slice(0, 2)}`, work_item_id: p.session_id === S29.id ? 29 : 97 }),
    )
    .on("session.archive", {})
    .on("session.peek", { data: "" })
    .on("recovery.archived", [])
    .on("transcript.tail", TAIL)
    .on("tree.get", { project: project(), sync: null, roots: [], untracked: [] });
  render(<App api={api} initial={initial as never} />);
  return { api, user: userEvent.setup() };
}

const side = () => screen.findByRole("complementary", { name: "Sessões interrompidas" });
const detail = (name: RegExp) => screen.findByRole("region", { name });

test("abre sozinha no boot quando há interrompidas", async () => {
  setup();
  const panel = await side();
  await waitFor(() => expect(within(panel).getByRole("button", { name: /Interrompidas/ })).toHaveTextContent("2"));
  expect(screen.getByRole("button", { name: "Recuperação" })).toHaveAttribute("aria-current", "page");
});

test("painel lateral: item, ambiente, motivo e há quanto tempo; a primeira vem selecionada", async () => {
  const { user } = setup({ name: "recovery" });
  const panel = await side();
  const r29 = await within(panel).findByRole("button", {
    name: "#29 ct-client: cliente com reconexão ctower-agents (WSL) · processo perdido há 2 h 10 min",
  });
  expect(r29).toHaveAttribute("aria-current", "true");
  const r97 = within(panel).getByRole("button", { name: /#97 Paginação de pedidos .* saiu com código 1 há 2 h 10 min/ });
  expect(await detail(/#29 ct-client/)).toBeInTheDocument();
  await user.click(r97);
  expect(await detail(/#97 Paginação/)).toBeInTheDocument();
  expect(screen.getByRole("tab", { name: /Recuperação · #97/ })).toBeInTheDocument();
});

test("retomar com prompt editado abre o terminal da nova sessão", async () => {
  const { api, user } = setup({ name: "recovery", env: "wsl", session: S29.id });
  const card = await detail(/#29 ct-client/);
  expect(within(card).getByText("restaurável do backup")).toBeInTheDocument();
  expect(within(card).getByText("parada há 2 h 10 min")).toBeInTheDocument();
  expect(within(card).getByText("ctower-agents (WSL)")).toBeInTheDocument();
  expect(within(card).getByText("3 commits sem push · 1 arquivo alterado")).toBeInTheDocument();
  const prompt = within(card).getByLabelText("Prompt de retomada") as HTMLTextAreaElement;
  expect(prompt.value).toBe(RESUME_PROMPT);
  await user.clear(prompt);
  await user.type(prompt, "Continue de onde parou.");
  await user.click(within(card).getByRole("button", { name: "Retomar a conversa" }));
  await waitFor(() =>
    expect(api.callsTo("session.resume")[0].params).toEqual({ session_id: S29.id, prompt: "Continue de onde parou." }),
  );
  expect(await screen.findByRole("button", { name: "Terminal", description: "1 sessão" })).toBeInTheDocument();
});

test("transcript perdido: sem Retomar, recomeça com nota na modal de início", async () => {
  const { api, user } = setup({ name: "recovery", env: "wsl", session: S97.id });
  const card = await detail(/#97 Paginação/);
  expect(within(card).getByText("perdido (sem original e sem backup)")).toBeInTheDocument();
  expect(within(card).queryByRole("button", { name: "Retomar a conversa" })).not.toBeInTheDocument();
  expect(within(card).queryByLabelText("Prompt de retomada")).not.toBeInTheDocument();
  await user.click(within(card).getByRole("button", { name: "Recomeçar com nota" }));
  await waitFor(() => expect(api.callsTo("session.archive")[0].params).toEqual({ session_id: S97.id }));
  // A mesma modal de início, com a nota de contexto já no prompt.
  const modal = await screen.findByRole("dialog");
  const prompt = (await within(modal).findByLabelText(/Prompt inicial/)) as HTMLTextAreaElement;
  expect(prompt.value).toContain(`Contexto: a sessão anterior (${S97.id}) foi interrompida e o transcript se perdeu`);
});

test("recomeçar com transcript íntegro: nota sem dizer que se perdeu", async () => {
  const { api, user } = setup({ name: "recovery", env: "wsl", session: S29.id });
  const card = await detail(/#29 ct-client/);
  await user.click(within(card).getByRole("button", { name: "Recomeçar com nota" }));
  await waitFor(() => expect(api.callsTo("session.archive")[0].params).toEqual({ session_id: S29.id }));
  const prompt = (await within(await screen.findByRole("dialog")).findByLabelText(/Prompt inicial/)) as HTMLTextAreaElement;
  expect(prompt.value).toContain(`a sessão anterior (${S29.id}) foi interrompida e esta recomeça sem a conversa dela`);
});

test("descartar arquiva e recarrega a lista", async () => {
  const { api, user } = setup({ name: "recovery" });
  const card = await detail(/#29 ct-client/);
  api.on("recovery.list", [item(S97, WI97, "perdido")]);
  await user.click(within(card).getByRole("button", { name: "Descartar" }));
  expect(api.callsTo("session.archive")[0].params).toEqual({ session_id: S29.id });
  // A que sobrou passa a ser a mostrada.
  expect(await detail(/#97 Paginação/)).toBeInTheDocument();
  expect(within(await side()).getAllByRole("listitem")).toHaveLength(1);
});

test("últimas linhas: fim do transcript da sessão parada, com a origem", async () => {
  const { api } = setup({ name: "recovery", env: "wsl", session: S29.id });
  const card = await detail(/#29 ct-client/);
  const tail = await within(card).findByRole("list", { name: "Últimas linhas do transcript" });
  const lines = within(tail).getAllByRole("listitem");
  expect(lines.map((l) => l.textContent)).toEqual([
    "vocêImplemente a reconexão.",
    "ferramentaBash: cargo test",
    "agenteTestes passando; falta o push.",
  ]);
  expect(within(card).getByText("do backup")).toBeInTheDocument();
  expect(api.callsTo("transcript.tail")[0].params).toEqual({ session_id: S29.id, entries: TAIL_ENTRIES });
});

test("últimas linhas: sem transcript não consulta; erro aparece no cartão", async () => {
  const { api, user } = setup({ name: "recovery", env: "wsl", session: S97.id });
  const card = await detail(/#97 Paginação/);
  expect(within(card).getByText(/Sem transcript nem backup/)).toBeInTheDocument();
  expect(api.callsTo("transcript.tail")).toHaveLength(0);
  api.on("transcript.tail", () => {
    throw new Error("sessão sem transcript nem backup");
  });
  await user.click(within(await side()).getByRole("button", { name: /#29/ }));
  const c29 = await detail(/#29 ct-client/);
  expect(await within(c29).findByText(/Não foi possível ler o transcript: .*sem transcript nem backup/)).toBeInTheDocument();
});

test("descartadas recentemente: no painel e, ao abrir, só leitura com as últimas linhas", async () => {
  const { api, user } = setup({ name: "recovery" });
  api.on("recovery.archived", [archivedItem()]);
  await detail(/#29 ct-client/);
  // Recarrega ao descartar: a sessão passa de uma lista para a outra.
  api.on("recovery.list", [item(S97, WI97, "perdido")]);
  await user.click(within(await detail(/#29 ct-client/)).getByRole("button", { name: "Descartar" }));
  const panel = await side();
  const gone = await within(panel).findByRole("button", { name: "#40 ctower-agents (WSL) · descartada há 3 h" });
  expect(within(panel).getByRole("button", { name: /Descartadas recentemente/ })).toHaveTextContent("1");
  expect(api.callsTo("recovery.archived").length).toBeGreaterThanOrEqual(2);
  await user.click(gone);
  const card = await detail(/^#40$/);
  expect(gone).toHaveAttribute("aria-current", "true");
  expect(within(card).getByText("descartada")).toBeInTheDocument();
  expect(within(card).queryByRole("button", { name: "Retomar a conversa" })).not.toBeInTheDocument();
  expect(within(card).queryByRole("button", { name: "Descartar" })).not.toBeInTheDocument();
  expect(await within(card).findByRole("list", { name: "Últimas linhas do transcript" })).toBeInTheDocument();
  // Recomeçar não arquiva de novo: só abre a modal com a nota.
  const archives = api.callsTo("session.archive").length;
  await user.click(within(card).getByRole("button", { name: "Recomeçar com nota" }));
  expect(within(await screen.findByRole("dialog")).getByRole("heading", { name: "#40" })).toBeInTheDocument();
  expect(api.callsTo("session.archive")).toHaveLength(archives);
});

test("abrir worktree no VS Code", async () => {
  const { api, user } = setup({ name: "recovery" });
  const card = await detail(/#29 ct-client/);
  await user.click(within(card).getByRole("button", { name: "Abrir worktree" }));
  expect(api.vscode).toEqual([{ env: "wsl", path: WI29.worktree_path }]);
});

test("Retomar todas confirma, retoma só as com transcript sem sair da tela e resume", async () => {
  const { api, user } = setup({ name: "recovery" });
  const panel = await side();
  await within(panel).findByRole("button", { name: /#29/ });
  const all = within(panel).getByRole("button", { name: "Retomar todas" });
  expect(all).toHaveAccessibleDescription("1 sessão com transcript; 1 fica de fora");
  await user.click(all);
  const confirm = within(panel).getByRole("alertdialog");
  expect(confirm).toHaveTextContent("Retomar 1 sessão em ctower-agents (WSL)");
  expect(confirm).toHaveTextContent("1 sessão perdida fica de fora");
  expect(api.callsTo("session.resume")).toHaveLength(0);
  await user.click(within(confirm).getByRole("button", { name: "Retomar 1" }));
  await waitFor(() => expect(api.callsTo("session.resume")).toHaveLength(1));
  expect(api.callsTo("session.resume")[0].params).toEqual({ session_id: S29.id, prompt: RESUME_PROMPT });
  expect(await within(panel).findByRole("status")).toHaveTextContent("1 retomada · 1 sem transcript");
  // Continua na tela de Recuperação (sem abrir terminal).
  expect(screen.getByRole("button", { name: "Recuperação" })).toHaveAttribute("aria-current", "page");
});

test("Retomar todas: prompt editado vai para a sua sessão e o erro de uma não para as outras", async () => {
  const { api, user } = setup({ name: "recovery", env: "wsl", session: S29.id });
  api.on("recovery.list", [item(S29, WI29, "original"), item(S97, WI97, "restauravel_do_backup")]);
  api.on("session.resume", (p: { session_id: string }) => {
    if (p.session_id === S29.id) throw new Error("worktree não existe mais");
    return session({ id: "novo-97", work_item_id: 97 });
  });
  const card = await detail(/#29 ct-client/);
  const prompt = within(card).getByLabelText("Prompt de retomada") as HTMLTextAreaElement;
  await user.clear(prompt);
  await user.type(prompt, "Outro prompt.");
  const panel = await side();
  await user.click(within(panel).getByRole("button", { name: "Retomar todas" }));
  await user.click(within(panel).getByRole("button", { name: "Retomar 2" }));
  await waitFor(() => expect(api.callsTo("session.resume")).toHaveLength(2));
  const params = api.callsTo("session.resume").map((c) => c.params);
  expect(params).toContainEqual({ session_id: S29.id, prompt: "Outro prompt." });
  expect(params).toContainEqual({ session_id: S97.id, prompt: RESUME_PROMPT });
  expect(await within(panel).findByRole("status")).toHaveTextContent("1 retomada · 1 erro");
  expect(await screen.findByRole("alert")).toHaveTextContent("worktree não existe mais");
});

test("sem interrompidas", async () => {
  const api = new FakeDaemonApi()
    .on("dashboard.get", dashboard({}, []))
    .on("recovery.list", [])
    .on("session.peek", { data: "" });
  render(<App api={api} initial={{ name: "recovery" }} />);
  expect(await screen.findByText("Nenhuma sessão interrompida.")).toBeInTheDocument();
});

test("resumo do worktree e motivo", () => {
  expect(worktreeSummary(null)).toBe("não encontrado");
  expect(worktreeSummary("## feat/1...origin/feat/1")).toBe("limpo e em dia com a remota");
  expect(worktreeSummary("## feat/1\n?? a.txt\n M b\n… +3 linhas")).toBe("sem branch remota · 5 arquivos alterados");
  expect(worktreeSummary("## feat/1...origin/feat/1 [ahead 1, behind 2]")).toBe("1 commit sem push");
  expect(reason(session({ exit_code: null }))).toBe("processo perdido");
  expect(reason(session({ exit_code: 137 }))).toBe("saiu com código 137");
});
