import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { App } from "../App";
import { FakeDaemonApi } from "../api/fake";
import type { Review } from "../protocol/gen";
import { dashboard, MIN, project, session, sync, workItem } from "../test/fixtures";
import { prUrl } from "./TerminalScreen";
import { CLOSE_DELAY_MS } from "./TerminalView";
import { matches } from "./SessionTree";
import { terms } from "../test/fakeXterm";
import { RESUMED_PROMPT } from "./actions";
import { RESUME_PROMPT } from "../recovery/RecoveryScreen";

// O xterm é trocado pelo fake de ../test/fakeXterm (setup dos testes).

const NOW = Date.now();
const S = session({
  id: "bbbbbbbb-0000-0000-0000-000000000002",
  work_item_id: 32,
  started_at: NOW - 84 * MIN,
  model: "haiku",
  mode: "subagents",
});
const WI = workItem({ gh_number: 32, title: "ui: terminal embutido", branch: "feat/32-ui-terminal-embutido" });

/** Barra de status do shell: ambiente e worktree da sessão ativa (#316). */
const statusBar = () => screen.getByRole("contentinfo", { name: "Barra de status" });

function review(prs: Review["prs"] = []): Review {
  return { work_item: WI, prs, decisions: [] };
}

async function openTab(api = new FakeDaemonApi()) {
  api
    .on("dashboard.get", dashboard({ live_sessions: [S], sync: [sync({ last_ok_at: NOW })] }, [WI]))
    .on("session.peek", { data: "" })
    .on("session.kill", {})
    .on("review.get", review());
  terms.length = 0;
  render(<App api={api} />);
  const user = userEvent.setup();
  await user.click(await screen.findByRole("button", { name: /ui: terminal embutido/ }));
  await waitFor(() => expect(api.attached.has(S.id)).toBe(true));
  return { api, user };
}

test("aba com barra da sessão e saída do terminal", async () => {
  const { api } = await openTab();
  expect(screen.getByRole("treeitem", { name: /#32 ui: terminal embutido/ })).toHaveAttribute("aria-current", "true");
  const barra = screen.getByRole("region", { name: "Sessão" });
  expect(within(barra).getByText("feat/32-ui-terminal-embutido")).toBeInTheDocument();
  expect(within(statusBar()).getByText(WI.worktree_path!)).toBeInTheDocument();
  expect(within(statusBar()).getByText("ctower-agents (WSL)")).toBeInTheDocument();
  expect(within(barra).getByText("claude · 1 h 24 min")).toBeInTheDocument();
  expect(within(barra).getByText("Haiku · subagents")).toBeInTheDocument();
  expect(within(barra).getByText("rodando")).toBeInTheDocument();
  act(() => api.output(S.id, "pronto\r\n", true));
  expect(terms[0].screen).toBe("pronto\r\n");
  terms[0].typed("y");
  expect(api.inputs).toEqual([{ env: "wsl", session: S.id, data: "y" }]);
});

test("sessão encerrada com a aba aberta mostra o código e bloqueia a entrada", async () => {
  const { api } = await openTab();
  // Encerrada, a sessão sai do dashboard: a aba mantém o contexto.
  api.on("dashboard.get", dashboard({ sync: [sync({ last_ok_at: NOW })] }, [WI]));
  act(() => api.push("wsl", "session_exited", { session_id: S.id, exit_code: 1 }));
  expect(await screen.findByText("sessão encerrada (código 1)")).toBeInTheDocument();
  // Encerrada, sai da árvore sozinha; o painel segue mostrando o estado final (#305).
  expect(screen.queryByRole("treeitem", { name: /#32 ui: terminal embutido/ })).not.toBeInTheDocument();
  expect(screen.getByText("Nenhuma sessão aberta. Abra uma pela Sala de controle.")).toBeInTheDocument();
  expect(terms[0].screen).toContain("sessão encerrada (código 1)");
  terms[0].typed("x");
  expect(api.inputs).toEqual([]);
  expect(screen.getByRole("button", { name: "Encerrar sessão" })).toBeDisabled();
  await waitFor(() => expect(api.callsTo("dashboard.get").length).toBeGreaterThan(1));
  expect(screen.queryByRole("treeitem", { name: /#32 ui: terminal embutido/ })).not.toBeInTheDocument();
  expect(screen.getByText("feat/32-ui-terminal-embutido")).toBeInTheDocument();
});

test("aba de sessão encerrada reaberta mostra título, metadados e o fim, sem o erro de attach (#283)", async () => {
  const { api, user } = await openTab();
  const fim = { ...S, state: "exited" as const, exit_code: null, ended_at: S.started_at + 30 * MIN };
  api.on("dashboard.get", dashboard({ sync: [sync({ last_ok_at: NOW })] }, [WI])).on("session.get", fim);
  api.attach = async () => {
    throw "sessão não está rodando";
  };
  act(() => api.push("wsl", "session_exited", { session_id: S.id, exit_code: null, state: "exited" }));
  expect(await screen.findByText("sessão encerrada (sinal)")).toBeInTheDocument();
  await waitFor(() => expect(api.callsTo("dashboard.get").length).toBeGreaterThan(1));
  // Sair da tela e voltar remonta a aba: a sessão já não está no dashboard.
  await user.click(screen.getByRole("button", { name: "Sala de controle" }));
  await user.click(screen.getByRole("button", { name: /^Terminal/ }));
  const barra = screen.getByRole("region", { name: "Sessão" });
  expect(await within(barra).findByText("feat/32-ui-terminal-embutido")).toBeInTheDocument();
  expect(screen.queryByRole("treeitem", { name: /#32 ui: terminal embutido/ })).not.toBeInTheDocument();
  expect(within(statusBar()).getByText(WI.worktree_path!)).toBeInTheDocument();
  expect(within(barra).getByText("claude · 30 min")).toBeInTheDocument();
  expect(within(barra).getByText("Haiku · subagents")).toBeInTheDocument();
  expect(await within(barra).findByText("sessão encerrada (sinal)")).toBeInTheDocument();
  expect(screen.queryByText(/não foi possível anexar/)).not.toBeInTheDocument();
  expect(screen.queryByRole("button", { name: "Abrir Recuperação" })).not.toBeInTheDocument();
});

test("saída anormal mostra o código e leva à Recuperação (#283)", async () => {
  const { api, user } = await openTab();
  api.on("recovery.list", []);
  act(() => api.push("wsl", "session_exited", { session_id: S.id, exit_code: 137, state: "interrupted" }));
  expect(await screen.findByText("sessão interrompida (código 137). Retome pela Recuperação")).toBeInTheDocument();
  await user.click(screen.getByRole("button", { name: "Abrir Recuperação" }));
  expect(await screen.findByText("Nenhuma sessão interrompida.")).toBeInTheDocument();
});

/** Uma ação do menu ⋯ do cabeçalho da sessão (#424). */
async function menuAction(user: ReturnType<typeof userEvent.setup>, name: string, bar = "Sessão") {
  await user.click(within(screen.getByRole("region", { name: bar })).getByRole("button", { name: "Mais ações" }));
  await user.click(screen.getByRole("menuitem", { name }));
}

test("abrir no VS Code e abrir PR", async () => {
  const { api, user } = await openTab();
  await menuAction(user, "Abrir no VS Code");
  expect(api.vscode).toEqual([{ env: "wsl", path: WI.worktree_path }]);
  await menuAction(user, "Abrir PR");
  await waitFor(() =>
    expect(api.opened).toEqual([
      "https://github.com/deep-ion-ai/ctower-agents/compare/develop...feat%2F32-ui-terminal-embutido?expand=1",
    ]),
  );
});

test("encerrar pede confirmação na própria tela", async () => {
  const { api, user } = await openTab();
  await user.click(screen.getByRole("button", { name: "Encerrar sessão" }));
  await user.click(screen.getByRole("button", { name: "Cancelar" }));
  expect(api.callsTo("session.kill")).toHaveLength(0);
  await user.click(screen.getByRole("button", { name: "Encerrar sessão" }));
  await user.click(screen.getByRole("button", { name: "Confirmar" }));
  await waitFor(() => expect(api.callsTo("session.kill")).toEqual([{ env: "wsl", method: "session.kill", params: { session_id: S.id } }]));
});

test("attach perdido após reconexão encerra a aba com o motivo", async () => {
  const { api } = await openTab();
  act(() => api.push("wsl", "attach_lost", { session_id: S.id, reason: "sessão não está rodando" }));
  expect(
    await screen.findByText("sessão indisponível após reconectar: sessão não está rodando. Veja a Recuperação"),
  ).toBeInTheDocument();
});

test("ambiente caído aparece na barra da sessão (Review Focus 1)", async () => {
  const { api } = await openTab();
  act(() => api.setStatus("wsl", false, "conexão perdida"));
  expect(await screen.findByText("ambiente desconectado · reconectando…")).toBeInTheDocument();
  act(() => api.setStatus("wsl", true));
  const barra = screen.getByRole("region", { name: "Sessão" });
  expect(await within(barra).findByText("rodando")).toBeInTheDocument();
});

test("prUrl prefere o PR aberto", () => {
  const pr = (number: number, state: "open" | "closed" | "merged") => ({
    number,
    work_item_id: 32,
    head: "h",
    base: "develop",
    state,
    url: `https://github.com/o/r/pull/${number}`,
    updated_at: 0,
    head_sha: null,
  });
  expect(prUrl(review([pr(1, "closed"), pr(2, "open")]), { gh_owner: "o", gh_repo: "r" }, "develop")).toBe("https://github.com/o/r/pull/2");
  expect(prUrl(review([pr(1, "merged")]), { gh_owner: "o", gh_repo: "r" }, "develop")).toBe("https://github.com/o/r/pull/1");
  expect(prUrl({ ...review(), work_item: { ...WI, branch: null } }, { gh_owner: "o", gh_repo: "r" }, "develop")).toBeNull();
});

test("Ctrl+F abre a busca: digita, navega, diferencia maiúsculas, conta e fecha com Esc", async () => {
  const { user } = await openTab();
  const t = terms[0];
  expect(screen.queryByRole("search", { name: "Buscar no terminal" })).not.toBeInTheDocument();
  act(() => t.uiAction("search"));
  const barra = screen.getByRole("search", { name: "Buscar no terminal" });
  const campo = within(barra).getByRole("searchbox", { name: "Texto da busca" });
  expect(campo).toHaveFocus();
  await user.type(campo, "ok");
  expect(t.searches).toEqual(["next o inc", "next ok inc"]);
  act(() => t.results(1, 4));
  expect(within(barra).getByText("2 de 4")).toBeInTheDocument();
  await user.keyboard("{Enter}");
  await user.keyboard("{Shift>}{Enter}{/Shift}");
  expect(t.searches.slice(2)).toEqual(["next ok", "previous ok"]);
  await user.click(within(barra).getByRole("button", { name: "Diferenciar maiúsculas" }));
  expect(t.searches[t.searches.length - 1]).toBe("next ok Aa inc");
  act(() => t.results(-1, 0));
  expect(within(barra).getByText("nenhuma")).toBeInTheDocument();
  const focos = t.focused;
  campo.focus();
  await user.keyboard("{Escape}");
  expect(screen.queryByRole("search", { name: "Buscar no terminal" })).not.toBeInTheDocument();
  expect(t.searches[t.searches.length - 1]).toBe("clear");
  expect(t.focused).toBe(focos + 1);
});

test("zoom pelo teclado muda a fonte e reajusta o PTY", async () => {
  await openTab();
  const t = terms[0];
  expect(t.font.size).toBe(13);
  act(() => t.uiAction("zoomIn"));
  expect(t.font.size).toBe(14);
  act(() => t.uiAction("zoomOut"));
  act(() => t.uiAction("zoomOut"));
  expect(t.font.size).toBe(12);
  act(() => t.uiAction("zoomReset"));
  expect(t.font.size).toBe(13);
});

const DOCS = project({ id: 2, name: "docs-site", gh_repo: "docs-site" });
const WI2 = workItem({ gh_number: 40, title: "revisar guia", status: "aguardando_voce" });
const WI3 = workItem({ gh_number: 44, project_id: 2, kind: "epic", title: "docs novas" });
const S2 = session({ id: "bbbbbbbb-0000-0000-0000-000000000003", work_item_id: 40, started_at: NOW - MIN });
const S3 = session({ id: "bbbbbbbb-0000-0000-0000-000000000004", work_item_id: 44, state: "interrupted" });
const SG = session({ id: "bbbbbbbb-0000-0000-0000-000000000005", work_item_id: 900 });

/** Dois projetos, uma sessão aguardando, um épico interrompido e o gestor do docs-site. */
async function openTree() {
  const api = new FakeDaemonApi();
  api
    .on(
      "dashboard.get",
      dashboard({
        projects: [
          { project: project(), work_items: [WI, WI2] },
          { project: DOCS, work_items: [WI3] },
        ],
        live_sessions: [S, S2, SG],
        interrupted_sessions: [S3],
        gestor_work_item_id: 900,
        gestor_scopes: { [SG.id]: { kind: "project", owner: "deep-ion-ai", repo: "docs-site" } },
        sync: [sync({ last_ok_at: NOW })],
      }),
    )
    .on("session.peek", { data: "" })
    .on("review.get", review())
    .on("recovery.list", []);
  terms.length = 0;
  render(<App api={api} />);
  const user = userEvent.setup();
  // Com sessão interrompida, o app abre na Recuperação.
  await user.click(await screen.findByRole("button", { name: "Sala de controle" }));
  await user.click(await screen.findByRole("button", { name: /ui: terminal embutido/ }));
  await waitFor(() => expect(api.attached.has(S.id)).toBe(true));
  return { api, user, tree: screen.getByRole("tree", { name: "Projetos" }), side: screen.getByRole("complementary", { name: "Sessões por projeto" }) };
}

const group = (name: RegExp) => screen.getByRole("treeitem", { name });

test("árvore agrupa as sessões por projeto, com o gestor no projeto do escopo", async () => {
  const { side } = await openTree();
  const ct = group(/^ctower-agents, 2 sessões, 1 aguardando$/);
  expect(ct).toHaveAttribute("aria-expanded", "true");
  expect(within(ct).getByRole("treeitem", { name: /#32 ui: terminal embutido rodando/ })).toBeInTheDocument();
  expect(within(ct).getByRole("treeitem", { name: /#40 revisar guia aguardando você/ })).toBeInTheDocument();
  const docs = group(/^docs-site, 2 sessões$/);
  expect(within(docs).getByRole("treeitem", { name: /#44 docs novas interrompida · recuperável épico/ })).toBeInTheDocument();
  expect(within(docs).getByRole("treeitem", { name: /Gestor de projetos · deep-ion-ai\/docs-site/ })).toBeInTheDocument();
  expect(within(side).getByText("2 projetos · 4 sessões")).toBeInTheDocument();
  expect(within(ct).getByText("1 aguardando")).toBeInTheDocument();
});

test("selecionar outra sessão abre o terminal dela e mantém o anterior montado e oculto", async () => {
  const { api, user } = await openTree();
  await user.click(screen.getByRole("treeitem", { name: /#40 revisar guia/ }));
  await waitFor(() => expect(api.attached.has(S2.id)).toBe(true));
  expect(screen.getByRole("treeitem", { name: /#40 revisar guia/ })).toHaveAttribute("aria-current", "true");
  expect(screen.getByRole("treeitem", { name: /#32 ui: terminal embutido/ })).not.toHaveAttribute("aria-current");
  expect(api.attached.has(S.id)).toBe(true);
  expect(terms).toHaveLength(2);
  act(() => api.output(S.id, "buffer\r\n", true));
  await user.click(screen.getByRole("treeitem", { name: /#32 ui: terminal embutido/ }));
  expect(terms[0].screen).toBe("buffer\r\n");
});

test("recolher, expandir e o estado recolhido sobrevive a sair da tela", async () => {
  const { user } = await openTree();
  await user.click(screen.getByRole("button", { name: "Recolher tudo" }));
  expect(group(/^ctower-agents/)).toHaveAttribute("aria-expanded", "false");
  expect(group(/^docs-site/)).toHaveAttribute("aria-expanded", "false");
  expect(screen.queryByRole("treeitem", { name: /#40/ })).not.toBeInTheDocument();
  await user.click(screen.getByRole("button", { name: "Expandir tudo" }));
  expect(group(/^docs-site/)).toHaveAttribute("aria-expanded", "true");
  await user.click(within(group(/^docs-site/)).getByText("docs-site"));
  expect(group(/^docs-site/)).toHaveAttribute("aria-expanded", "false");
  await user.click(screen.getByRole("button", { name: "Sala de controle" }));
  await user.click(screen.getByRole("button", { name: /^Terminal/ }));
  expect(group(/^docs-site/)).toHaveAttribute("aria-expanded", "false");
  expect(group(/^ctower-agents/)).toHaveAttribute("aria-expanded", "true");
  expect(screen.getByRole("treeitem", { name: /#32 ui: terminal embutido/ })).toHaveAttribute("aria-current", "true");
});

test("filtro por título, branch ou #issue", async () => {
  const { user } = await openTree();
  const campo = screen.getByRole("searchbox", { name: "Filtrar sessões" });
  await user.type(campo, "#44");
  expect(screen.getAllByRole("treeitem", { name: /#/ }).map((e) => e.getAttribute("aria-level"))).toEqual(["2"]);
  expect(screen.queryByRole("treeitem", { name: /^ctower-agents/ })).not.toBeInTheDocument();
  await user.clear(campo);
  await user.type(campo, "feat/40");
  expect(screen.getByRole("treeitem", { name: /#40 revisar guia/ })).toBeInTheDocument();
  expect(screen.queryByRole("treeitem", { name: /#32/ })).not.toBeInTheDocument();
  await user.clear(campo);
  await user.type(campo, "nada disso");
  expect(screen.getByText("Nenhuma sessão encontrada.")).toBeInTheDocument();
});

test("matches aceita o número da issue com ou sem #", () => {
  const s = { tab: { env: "wsl", session: "x" }, key: "s:wsl|x", workItemId: 7, title: "#7 algo", branch: "feat/7-algo", ghNumber: 7, epic: false, parent: null, state: { text: "", tone: "" }, remote: false, shells: [] };
  expect(matches(s, "7")).toBe(true);
  expect(matches(s, "#7")).toBe(true);
  expect(matches(s, "#70")).toBe(false);
  expect(matches(s, "ALGO")).toBe(true);
});

test("árvore navegável pelo teclado", async () => {
  const { api, user } = await openTree();
  const atual = screen.getByRole("treeitem", { name: /#32 ui: terminal embutido/ });
  expect(atual).toHaveAttribute("tabindex", "0");
  act(() => atual.focus());
  await user.keyboard("{ArrowDown}");
  const proxima = screen.getByRole("treeitem", { name: /#40 revisar guia/ });
  expect(proxima).toHaveFocus();
  await user.keyboard("{Enter}");
  await waitFor(() => expect(api.attached.has(S2.id)).toBe(true));
  expect(proxima).toHaveAttribute("aria-current", "true");
  await user.keyboard("{ArrowLeft}");
  const ct = group(/^ctower-agents/);
  expect(ct).toHaveFocus();
  await user.keyboard("{ArrowLeft}");
  expect(ct).toHaveAttribute("aria-expanded", "false");
  await user.keyboard("{ArrowRight}");
  expect(ct).toHaveAttribute("aria-expanded", "true");
  await user.keyboard("{ArrowRight}");
  expect(screen.getByRole("treeitem", { name: /#32 ui: terminal embutido/ })).toHaveFocus();
  await user.keyboard("{End}");
  expect(screen.getByRole("treeitem", { name: /#44 docs novas/ })).toHaveFocus();
  await user.keyboard("{Home}");
  expect(ct).toHaveFocus();
});

test("sem sessões no ambiente, mostra o estado vazio", async () => {
  const api = new FakeDaemonApi();
  api.on("dashboard.get", dashboard({ sync: [sync({ last_ok_at: NOW })] }));
  render(<App api={api} initial={{ name: "terminal" }} />);
  expect(await screen.findByText("Nenhuma sessão aberta. Abra uma pela Sala de controle.")).toBeInTheDocument();
});

test("a árvore fica no painel lateral do shell, com as classes próprias (#305, #316)", async () => {
  await openTab();
  const lateral = screen.getByRole("region", { name: "Painel lateral" });
  const arvore = within(lateral).getByRole("complementary", { name: "Sessões por projeto" });
  // `.tree-title` e `.tree-row` são do project.css (flex: 1 1 260px, borda): a árvore usa nomes próprios.
  expect(within(arvore).getByRole("tree", { name: "Projetos" })).toHaveClass("tree");
  // Sem a barra lateral duplicada dentro da área de editor.
  const editor = screen.getByRole("tabpanel", { name: "Área de editor" });
  expect(within(editor).queryByRole("tree")).not.toBeInTheDocument();
});

const visibleTerms = () => screen.getAllByRole("region", { name: "Terminal da sessão" });

test("dividir à direita mostra duas sessões lado a lado sem remontar os consoles; ativar a da direita troca os lados (#316)", async () => {
  const { api, user } = await openTree();
  await user.click(screen.getByRole("treeitem", { name: /#40 revisar guia/ }));
  await waitFor(() => expect(api.attached.has(S2.id)).toBe(true));
  expect(visibleTerms()).toHaveLength(1);
  const esquerda = screen.getByRole("region", { name: "Sessão" });
  await user.click(within(esquerda).getByRole("button", { name: "Dividir à direita" }));
  const direita = screen.getByRole("region", { name: "Sessão à direita" });
  expect(within(esquerda).getByText("#40 revisar guia")).toBeInTheDocument();
  expect(within(direita).getByText("#32 ui: terminal embutido")).toBeInTheDocument();
  expect(visibleTerms()).toHaveLength(2);
  // Os dois consoles seguem os mesmos (sem remontar nem reanexar) e recebem a saída.
  expect(terms).toHaveLength(2);
  act(() => api.output(S.id, "à direita\r\n", true));
  expect(terms[0].screen).toBe("à direita\r\n");
  // A divisão já existe: o painel da esquerda não oferece outra.
  expect(within(esquerda).queryByRole("button", { name: "Dividir à direita" })).not.toBeInTheDocument();

  // Ativar a sessão da direita pela aba troca os lados.
  await user.click(screen.getByRole("tab", { name: /#32 ui: terminal embutido/ }));
  expect(within(screen.getByRole("region", { name: "Sessão" })).getByText("#32 ui: terminal embutido")).toBeInTheDocument();
  expect(within(screen.getByRole("region", { name: "Sessão à direita" })).getByText("#40 revisar guia")).toBeInTheDocument();
  expect(terms).toHaveLength(2);
  expect(terms[0].screen).toBe("à direita\r\n");

  // Sair da tela e voltar mantém a divisão.
  await user.click(screen.getByRole("button", { name: "Sala de controle" }));
  await user.click(screen.getByRole("button", { name: /^Terminal/ }));
  expect(screen.getByRole("region", { name: "Sessão à direita" })).toBeInTheDocument();
  expect(visibleTerms()).toHaveLength(2);

  await user.click(
    within(screen.getByRole("region", { name: "Sessão à direita" })).getByRole("button", { name: "Fechar divisão" }),
  );
  expect(screen.queryByRole("region", { name: "Sessão à direita" })).not.toBeInTheDocument();
  expect(visibleTerms()).toHaveLength(1);
});

test("dividir à direita fica desabilitado com uma sessão só", async () => {
  await openTab();
  expect(within(screen.getByRole("region", { name: "Sessão" })).getByRole("button", { name: "Dividir à direita" })).toBeDisabled();
});

test("fechar a aba da sessão da direita desfaz a divisão", async () => {
  const { api, user } = await openTree();
  await user.click(screen.getByRole("treeitem", { name: /#40 revisar guia/ }));
  await waitFor(() => expect(api.attached.has(S2.id)).toBe(true));
  await user.click(within(screen.getByRole("region", { name: "Sessão" })).getByRole("button", { name: "Dividir à direita" }));
  await user.click(screen.getByRole("button", { name: /^Fechar aba #32/ }));
  expect(screen.queryByRole("region", { name: "Sessão à direita" })).not.toBeInTheDocument();
  expect(visibleTerms()).toHaveLength(1);
});

test("/exit (código 0) na sessão da direita fecha a aba e desfaz a divisão; código ≠ 0 mantém a aba (#432)", async () => {
  const { api, user } = await openTree();
  await user.click(screen.getByRole("treeitem", { name: /#40 revisar guia/ }));
  await waitFor(() => expect(api.attached.has(S2.id)).toBe(true));
  await user.click(within(screen.getByRole("region", { name: "Sessão" })).getByRole("button", { name: "Dividir à direita" }));
  expect(screen.getByRole("region", { name: "Sessão à direita" })).toBeInTheDocument();

  act(() => api.push("wsl", "session_exited", { session_id: S.id, exit_code: 0, state: "exited" }));
  expect(screen.getByRole("tab", { name: /#32 ui: terminal embutido/ })).toBeInTheDocument();
  await waitFor(() => expect(screen.queryByRole("tab", { name: /#32 ui: terminal embutido/ })).not.toBeInTheDocument(), {
    timeout: CLOSE_DELAY_MS + 1000,
  });
  expect(screen.queryByRole("region", { name: "Sessão à direita" })).not.toBeInTheDocument();
  expect(visibleTerms()).toHaveLength(1);

  // A ativa sai com erro: a aba fica com o código.
  act(() => api.push("wsl", "session_exited", { session_id: S2.id, exit_code: 2, state: "exited" }));
  expect(await screen.findByText("sessão encerrada (código 2)")).toBeInTheDocument();
  await act(() => new Promise((r) => setTimeout(r, CLOSE_DELAY_MS + 100)));
  expect(screen.getByRole("tab", { name: /#40 revisar guia/ })).toBeInTheDocument();
});

test("pedido pendente aparece abaixo do console, com o texto do agente, e Responder leva o foco ao console (#316)", async () => {
  const { api, user } = await openTree();
  // A sessão rodando não tem faixa.
  expect(screen.queryByRole("group", { name: "Pedido pendente" })).not.toBeInTheDocument();
  await user.click(screen.getByRole("treeitem", { name: /#40 revisar guia/ }));
  await waitFor(() => expect(api.attached.has(S2.id)).toBe(true));
  const faixa = screen.getByRole("group", { name: "Pedido pendente" });
  expect(within(faixa).getByText("Permissão ou pergunta pendente no terminal")).toBeInTheDocument();
  act(() =>
    api.push("wsl", "needs_human", { session_id: S2.id, work_item_id: 40, message: "Claude needs your permission to use Bash" }),
  );
  expect(await within(faixa).findByText("Claude needs your permission to use Bash")).toBeInTheDocument();
  const t = terms[1];
  const antes = t.focused;
  await user.click(within(faixa).getByRole("button", { name: "Responder no terminal" }));
  expect(t.focused).toBe(antes + 1);
  // Sem opções que o protocolo não sabe responder.
  expect(within(faixa).queryByRole("button", { name: "Permitir" })).not.toBeInTheDocument();
});

test("botão Buscar do cabeçalho abre a busca do console", async () => {
  const { user } = await openTab();
  await user.click(within(screen.getByRole("region", { name: "Sessão" })).getByRole("button", { name: "Buscar no terminal" }));
  expect(screen.getByRole("searchbox", { name: "Texto da busca" })).toHaveFocus();
});

test("barra de status mostra a sessão ativa só na tela Terminal", async () => {
  const { user } = await openTab();
  expect(within(statusBar()).getByText(WI.worktree_path!)).toBeInTheDocument();
  await user.click(screen.getByRole("button", { name: "Sala de controle" }));
  expect(within(statusBar()).queryByText(WI.worktree_path!)).not.toBeInTheDocument();
});

test("Nova sessão abre a modal de planejamento no projeto da sessão ativa", async () => {
  const { user } = await openTab();
  await user.click(screen.getByRole("button", { name: "Nova sessão" }));
  expect(screen.getByRole("dialog")).toHaveAccessibleName("Sessão de planejamento · ctower-agents");
});

const REQ = {
  request_id: "req-1",
  session_id: S.id,
  work_item_id: 32,
  tool_name: "Bash",
  tool_input: { command: "git push origin feat/32", description: "push" },
  created_at: NOW,
  expires_at: NOW + 120_000,
};

test("pedido de permissão: faixa com Negar, Sempre nesta sessão e Permitir; Responder no terminal manda ask (#365)", async () => {
  const api = new FakeDaemonApi().on("session.permission.watch", []).on("session.permission.respond", {});
  const { user } = await openTab(api);
  // A UI acompanha os pedidos ao conectar: sem isso o daemon não os segura.
  expect(api.callsTo("session.permission.watch")).toHaveLength(1);
  expect(screen.queryByRole("group", { name: "Pedido de permissão" })).not.toBeInTheDocument();
  act(() => api.push("wsl", "permission_request", REQ));
  const faixa = await screen.findByRole("group", { name: "Pedido de permissão" });
  expect(within(faixa).getByText("Bash: git push origin feat/32")).toBeInTheDocument();
  expect(within(faixa).getByText(/terminal em [12]:\d\d/)).toBeInTheDocument();
  await user.click(within(faixa).getByRole("button", { name: "Sempre nesta sessão" }));
  expect(api.callsTo("session.permission.respond").map((c) => c.params)).toEqual([
    { request_id: "req-1", decision: "allow_session" },
  ]);
  await waitFor(() => expect(screen.queryByRole("group", { name: "Pedido de permissão" })).not.toBeInTheDocument());

  // "Responder no terminal" devolve o pedido ao menu e foca o console.
  act(() => api.push("wsl", "permission_request", { ...REQ, request_id: "req-2" }));
  const outra = await screen.findByRole("group", { name: "Pedido de permissão" });
  const antes = terms[0].focused;
  await user.click(within(outra).getByRole("button", { name: "Responder no terminal" }));
  const resp = api.callsTo("session.permission.respond");
  expect(resp[resp.length - 1].params).toEqual({ request_id: "req-2", decision: "ask" });
  expect(terms[0].focused).toBe(antes + 1);
});

test("pedido pendente vem do watch ao conectar e sai no permission_resolved (expirado)", async () => {
  const api = new FakeDaemonApi().on("session.permission.watch", [REQ]);
  await openTab(api);
  const faixa = await screen.findByRole("group", { name: "Pedido de permissão" });
  expect(within(faixa).getByRole("button", { name: "Permitir" })).toBeInTheDocument();
  act(() => api.push("wsl", "permission_resolved", { request_id: "req-1", session_id: S.id, decision: "expired" }));
  await waitFor(() => expect(screen.queryByRole("group", { name: "Pedido de permissão" })).not.toBeInTheDocument());
});

test("pedido que já não está pendente sai da faixa e mostra o erro", async () => {
  const api = new FakeDaemonApi().on("session.permission.watch", [REQ]).on("session.permission.respond", () => {
    throw "pedido de permissão não está mais pendente";
  });
  const { user } = await openTab(api);
  const faixa = await screen.findByRole("group", { name: "Pedido de permissão" });
  await user.click(within(faixa).getByRole("button", { name: "Negar" }));
  await waitFor(() => expect(screen.queryByRole("group", { name: "Pedido de permissão" })).not.toBeInTheDocument());
});

test("barra de status mostra o uso de contexto e relê no session_state (#365)", async () => {
  let tokens = 90_000;
  const api = new FakeDaemonApi().on("session.context", () => ({ tokens, model: "claude-haiku-5-5", ts: null }));
  await openTab(api);
  expect(await within(statusBar()).findByText("Contexto 45% · 90k/200k")).toBeInTheDocument();
  expect(api.callsTo("session.context")[0].params).toEqual({ session_id: S.id });
  tokens = 150_000;
  act(() => api.push("wsl", "session_state", { session_id: S.id, work_item_id: 32, state: "idle" }));
  expect(await within(statusBar()).findByText("Contexto 75% · 150k/200k")).toBeInTheDocument();
});

test("sem session.context no daemon, a barra de status segue sem o uso de contexto", async () => {
  await openTab();
  expect(within(statusBar()).getByText(WI.worktree_path!)).toBeInTheDocument();
  expect(within(statusBar()).queryByText(/^Contexto/)).not.toBeInTheDocument();
});

test("árvore põe as sessões das sub-issues sob o épico pai, com a do épico junto (#365)", async () => {
  const EP = workItem({ gh_number: 50, kind: "epic", title: "épico grande" });
  const SUB = workItem({ gh_number: 51, title: "parte um", parent_number: 50 });
  const SUB2 = workItem({ gh_number: 52, title: "parte dois", parent_number: 60 });
  const SE = session({ id: "cccccccc-0000-0000-0000-000000000050", work_item_id: 50 });
  const SS = session({ id: "cccccccc-0000-0000-0000-000000000051", work_item_id: 51 });
  const SS2 = session({ id: "cccccccc-0000-0000-0000-000000000052", work_item_id: 52 });
  const api = new FakeDaemonApi();
  api
    .on(
      "dashboard.get",
      dashboard({ live_sessions: [S, SE, SS, SS2], sync: [sync({ last_ok_at: NOW })] }, [WI, EP, SUB, SUB2]),
    )
    .on("session.peek", { data: "" })
    .on("review.get", review());
  terms.length = 0;
  render(<App api={api} />);
  const user = userEvent.setup();
  await user.click(await screen.findByRole("button", { name: /ui: terminal embutido/ }));
  const ep = await screen.findByRole("treeitem", { name: "#50 épico grande, épico, 2 sessões" });
  expect(ep).toHaveAttribute("aria-level", "2");
  const dentro = within(ep).getAllByRole("treeitem");
  expect(dentro.map((x) => x.getAttribute("aria-level"))).toEqual(["3", "3"]);
  expect(dentro[0]).toHaveAccessibleName(/#50 épico grande/);
  expect(dentro[1]).toHaveAccessibleName(/#51 parte um/);
  // Épico fora do dashboard: o nó usa só o número.
  expect(screen.getByRole("treeitem", { name: "Épico #60, épico, 1 sessão" })).toBeInTheDocument();
  // A issue sem pai segue no nível do projeto.
  expect(screen.getByRole("treeitem", { name: /#32 ui: terminal embutido/ })).toHaveAttribute("aria-level", "2");
  // Recolher o épico pelo teclado: ← no nó, e ← numa sessão volta ao nó.
  act(() => within(ep).getAllByRole("treeitem")[1].focus());
  await user.keyboard("{ArrowLeft}");
  expect(ep).toHaveFocus();
  await user.keyboard("{ArrowLeft}");
  expect(ep).toHaveAttribute("aria-expanded", "false");
  expect(within(ep).queryAllByRole("treeitem")).toHaveLength(0);
  await user.keyboard("{ArrowRight}");
  expect(ep).toHaveAttribute("aria-expanded", "true");
});

// Ações do console (#424): Retomar e Reiniciar na mesma aba, menu ⋯ e paleta.
const RS = session({
  id: "dddddddd-0000-0000-0000-000000000032",
  work_item_id: 32,
  tool_session_id: "tool-32",
  model: "haiku",
  mode: "subagents",
});
const RS2 = session({ id: "dddddddd-0000-0000-0000-000000000040", work_item_id: 40 });
const NEW = session({ id: "dddddddd-0000-0000-0000-0000000000ff", work_item_id: 32, tool_session_id: "tool-32" });

/** #32 à direita (coluna 2) de #40, com as duas abas na faixa: #32 primeiro. */
async function openSplit() {
  const api = new FakeDaemonApi();
  const live = (sessions: typeof RS[]) =>
    api.on("dashboard.get", dashboard({ live_sessions: sessions, sync: [sync({ last_ok_at: NOW })] }, [WI, WI2]));
  live([RS, RS2]).on("session.peek", { data: "" }).on("session.kill", {}).on("review.get", review());
  terms.length = 0;
  render(<App api={api} />);
  const user = userEvent.setup();
  await user.click(await screen.findByRole("button", { name: /ui: terminal embutido/ }));
  await user.click(await screen.findByRole("treeitem", { name: /#40 revisar guia/ }));
  await waitFor(() => expect(api.attached.has(RS2.id)).toBe(true));
  await user.click(within(screen.getByRole("region", { name: "Sessão" })).getByRole("button", { name: "Dividir à direita" }));
  expect(within(screen.getByRole("region", { name: "Sessão à direita" })).getByText("#32 ui: terminal embutido")).toBeInTheDocument();
  return { api, user, live };
}

/** Ids das abas de sessão, na ordem da faixa. */
const tabIds = () =>
  screen
    .getAllByRole("tab")
    .map((t) => t.id)
    .filter((id) => id.startsWith("etab-terminal:"));
const swapped = (ids: string[]) => ids.map((id) => id.replace(RS.id, NEW.id));

test("Retomar: session.resume direto e a sessão nova fica na mesma aba e na mesma coluna", async () => {
  const { api, user, live } = await openSplit();
  const antes = tabIds();
  live([RS2]);
  act(() => api.push("wsl", "session_exited", { session_id: RS.id, exit_code: 0, state: "exited" }));
  const direita = screen.getByRole("region", { name: "Sessão à direita" });
  const retomar = await within(direita).findByRole("button", { name: "Retomar" });
  // A viva não oferece Retomar.
  expect(within(screen.getByRole("region", { name: "Sessão" })).queryByRole("button", { name: "Retomar" })).toBeNull();
  api.on("session.resume", () => {
    live([NEW, RS2]);
    return NEW;
  });
  await user.click(retomar);
  await waitFor(() => expect(api.attached.has(NEW.id)).toBe(true));
  act(() => api.push("wsl", "session_resumed", { session_id: NEW.id }));
  expect(api.callsTo("session.resume").map((c) => c.params)).toEqual([
    // Saiu normalmente: o prompt neutro, sem dizer que foi interrompida.
    { session_id: RS.id, prompt: RESUMED_PROMPT },
  ]);
  // Mesma posição na faixa, ainda à direita, e a ativa segue a #40.
  expect(tabIds()).toEqual(swapped(antes));
  expect(await within(screen.getByRole("region", { name: "Sessão à direita" })).findByText("#32 ui: terminal embutido")).toBeInTheDocument();
  expect(within(screen.getByRole("region", { name: "Sessão" })).getByText("#40 revisar guia")).toBeInTheDocument();
  expect(screen.queryByRole("tab", { name: /#32.*encerrada/ })).toBeNull();
});

test("sessão encerrada sem o id da ferramenta não oferece Retomar", async () => {
  const { api } = await openTab();
  api.on("dashboard.get", dashboard({ live_sessions: [], sync: [sync({ last_ok_at: NOW })] }, [WI]));
  act(() => api.push("wsl", "session_exited", { session_id: S.id, exit_code: 0, state: "exited" }));
  expect(await screen.findByText("sessão encerrada (código 0)")).toBeInTheDocument();
  expect(screen.queryByRole("button", { name: "Retomar" })).toBeNull();
});

test("Retomar: erro do daemon no cabeçalho, com o link para a Recuperação", async () => {
  const { api, user, live } = await openSplit();
  live([RS2]);
  act(() => api.push("wsl", "session_exited", { session_id: RS.id, exit_code: 0, state: "exited" }));
  api.on("session.resume", () => {
    throw "transcript da sessão foi perdido e não há backup";
  });
  api.on("recovery.list", []);
  const direita = screen.getByRole("region", { name: "Sessão à direita" });
  await user.click(await within(direita).findByRole("button", { name: "Retomar" }));
  const erro = await within(direita).findByRole("alert");
  expect(erro).toHaveTextContent("Não foi possível retomar: transcript da sessão foi perdido e não há backup");
  await user.click(within(erro).getByRole("button", { name: "Abrir Recuperação" }));
  expect(await screen.findByText("Nenhuma sessão interrompida.")).toBeInTheDocument();
});

test("Reiniciar a sessão viva: confirma, modal com modelo e modo, kill antes do start e a mesma aba", async () => {
  const { api, user, live } = await openSplit();
  const antes = tabIds();
  const order: string[] = [];
  api.on("session.kill", () => {
    order.push("kill");
    return {};
  });
  api.on("session.start", () => {
    order.push("start");
    live([NEW, RS2]);
    return NEW;
  });
  await menuAction(user, "Reiniciar", "Sessão à direita");
  const confirma = screen.getByRole("group", { name: "Confirmar reinício" });
  expect(confirma).toHaveTextContent("Reiniciar encerra o processo atual do agente");
  expect(api.callsTo("session.kill")).toHaveLength(0);
  await user.click(within(confirma).getByRole("button", { name: "Continuar" }));
  const modal = screen.getByRole("dialog");
  expect(modal).toHaveAccessibleName("#32 ui: terminal embutido");
  expect(within(modal).getByLabelText("Modelo")).toHaveValue("haiku");
  expect(within(modal).getByLabelText("Modo")).toHaveValue("subagents");
  expect(within(modal).getByText(/a sessão atual é encerrada/)).toBeInTheDocument();
  // A sessão substituída não conta como "já existe uma sessão".
  expect(within(modal).queryByText(/Já existe uma sessão/)).toBeNull();
  await user.click(within(modal).getByRole("button", { name: /Iniciar sessão/ }));
  await waitFor(() => expect(api.attached.has(NEW.id)).toBe(true));
  act(() => api.push("wsl", "session_started", { session_id: NEW.id }));
  expect(order).toEqual(["kill", "start"]);
  expect(api.callsTo("session.kill")[0].params).toEqual({ session_id: RS.id });
  expect(api.callsTo("session.start")[0].params).toMatchObject({ gh_number: 32, model: "haiku", mode: "subagents" });
  expect(tabIds()).toEqual(swapped(antes));
  expect(await within(screen.getByRole("region", { name: "Sessão à direita" })).findByText("#32 ui: terminal embutido")).toBeInTheDocument();
});

test("Reiniciar a sessão encerrada abre a modal direto, sem kill", async () => {
  const { api, user, live } = await openSplit();
  live([RS2]);
  act(() => api.push("wsl", "session_exited", { session_id: RS.id, exit_code: 0, state: "exited" }));
  await within(screen.getByRole("region", { name: "Sessão à direita" })).findByRole("button", { name: "Retomar" });
  api.on("session.start", () => {
    live([NEW, RS2]);
    return NEW;
  });
  await menuAction(user, "Reiniciar", "Sessão à direita");
  expect(screen.queryByRole("group", { name: "Confirmar reinício" })).toBeNull();
  const modal = screen.getByRole("dialog");
  expect(within(modal).queryByText(/a sessão atual é encerrada/)).toBeNull();
  await user.click(within(modal).getByRole("button", { name: /Iniciar sessão/ }));
  await waitFor(() => expect(api.attached.has(NEW.id)).toBe(true));
  act(() => api.push("wsl", "session_started", { session_id: NEW.id }));
  expect(api.callsTo("session.kill")).toHaveLength(0);
  expect(await within(screen.getByRole("region", { name: "Sessão à direita" })).findByText("#32 ui: terminal embutido")).toBeInTheDocument();
});

test("Abrir issue no GitHub pelo menu ⋯ e pela paleta, aplicada à sessão ativa", async () => {
  const { api, user } = await openTab();
  await menuAction(user, "Abrir issue no GitHub");
  expect(api.opened).toEqual(["https://github.com/deep-ion-ai/ctower-agents/issues/32"]);
  await user.click(screen.getByRole("button", { name: "Abrir paleta de comandos" }));
  const paleta = screen.getByRole("dialog", { name: "Paleta de comandos" });
  await user.type(within(paleta).getByRole("combobox"), "Sessão: Abrir issue");
  await user.keyboard("{Enter}");
  expect(api.opened).toEqual([
    "https://github.com/deep-ion-ai/ctower-agents/issues/32",
    "https://github.com/deep-ion-ai/ctower-agents/issues/32",
  ]);
});

test("sessão do projeto (#0): Abrir repositório no GitHub, sem PR nem item; Reiniciar via start_project", async () => {
  const W0 = workItem({ gh_number: 0, id: 70, title: null, branch: null, worktree_path: null });
  const S0 = session({ id: "eeeeeeee-0000-0000-0000-000000000000", work_item_id: 70, model: "opus" });
  const N0 = session({ id: "eeeeeeee-0000-0000-0000-0000000000ff", work_item_id: 70 });
  const api = new FakeDaemonApi();
  api
    .on("dashboard.get", dashboard({ live_sessions: [S0], sync: [sync({ last_ok_at: NOW })] }, [W0]))
    .on("session.peek", { data: "" })
    .on("session.kill", {})
    .on("session.start_project", () => {
      api.on("dashboard.get", dashboard({ live_sessions: [N0], sync: [sync({ last_ok_at: NOW })] }, [W0]));
      return N0;
    });
  terms.length = 0;
  render(<App api={api} />);
  const user = userEvent.setup();
  await user.click(await screen.findByRole("button", { name: /sessão do projeto/ }));
  await waitFor(() => expect(api.attached.has(S0.id)).toBe(true));
  const barra = screen.getByRole("region", { name: "Sessão" });
  await user.click(within(barra).getByRole("button", { name: "Mais ações" }));
  const menu = screen.getByRole("menu", { name: "Ações da sessão" });
  expect(within(menu).getAllByRole("menuitem").map((m) => m.textContent)).toEqual([
    "Abrir repositório no GitHub",
    "Abrir no VS Code",
    "Reiniciar",
  ]);
  await user.click(within(menu).getByRole("menuitem", { name: "Abrir repositório no GitHub" }));
  expect(api.opened).toEqual(["https://github.com/deep-ion-ai/ctower-agents"]);
  await menuAction(user, "Reiniciar");
  await user.click(within(screen.getByRole("group", { name: "Confirmar reinício" })).getByRole("button", { name: "Continuar" }));
  const modal = screen.getByRole("dialog");
  expect(modal).toHaveAccessibleName("Sessão de planejamento · ctower-agents");
  expect(within(modal).getByLabelText("Modelo")).toHaveValue("opus");
  await user.click(within(modal).getByRole("button", { name: /Iniciar sessão/ }));
  await waitFor(() => expect(api.attached.has(N0.id)).toBe(true));
  expect(api.callsTo("session.start_project")[0].params).toMatchObject({ project_id: 1, model: "opus" });
  expect(api.callsTo("session.kill")[0].params).toEqual({ session_id: S0.id });
});

test("gestor: sem Abrir issue; Reiniciar abre a modal do gestor e chama start_gestor com o escopo", async () => {
  const G = session({ id: "ffffffff-0000-0000-0000-000000000900", work_item_id: 900, mode: "subagents" });
  const NG = session({ id: "ffffffff-0000-0000-0000-0000000009ff", work_item_id: 900 });
  const scope = { kind: "project" as const, owner: "deep-ion-ai", repo: "docs-site" };
  const api = new FakeDaemonApi();
  api
    .on(
      "dashboard.get",
      dashboard({
        live_sessions: [G],
        gestor_work_item_id: 900,
        gestor_scopes: { [G.id]: scope },
        sync: [sync({ last_ok_at: NOW })],
      }),
    )
    .on("session.peek", { data: "" })
    .on("session.kill", {})
    .on("session.start_gestor", () => {
      api.on(
        "dashboard.get",
        dashboard({ live_sessions: [NG], gestor_work_item_id: 900, sync: [sync({ last_ok_at: NOW })] }),
      );
      return NG;
    });
  terms.length = 0;
  render(<App api={api} />);
  const user = userEvent.setup();
  await user.click(await screen.findByRole("button", { name: /Gestor de projetos/ }));
  await waitFor(() => expect(api.attached.has(G.id)).toBe(true));
  const barra = screen.getByRole("region", { name: "Sessão" });
  await user.click(within(barra).getByRole("button", { name: "Mais ações" }));
  expect(within(screen.getByRole("menu")).getAllByRole("menuitem").map((m) => m.textContent)).toEqual([
    "Revisar decisões do gestor",
    "Reiniciar",
  ]);
  await user.click(screen.getByRole("menuitem", { name: "Reiniciar" }));
  await user.click(within(screen.getByRole("group", { name: "Confirmar reinício" })).getByRole("button", { name: "Continuar" }));
  const modal = screen.getByRole("dialog");
  expect(modal).toHaveAccessibleName("Gestor de projetos");
  expect(within(modal).getByText("ctower-agents (WSL) · deep-ion-ai/docs-site")).toBeInTheDocument();
  expect((within(modal).getByLabelText(/Prompt inicial/) as HTMLTextAreaElement).value).toContain("Foco: deep-ion-ai/docs-site.");
  expect(within(modal).getByLabelText("Modo")).toHaveValue("subagents");
  await user.click(within(modal).getByRole("button", { name: /Iniciar sessão/ }));
  await waitFor(() => expect(api.attached.has(NG.id)).toBe(true));
  expect(api.callsTo("session.start_gestor")[0].params).toMatchObject({ scope, mode: "subagents" });
  // A nova ocupa a aba da antiga: uma aba de sessão só.
  expect(tabIds()).toEqual([`etab-terminal:wsl:${NG.id}`]);
});

test("Retomar a interrompida recuperável usa o prompt da Recuperação", async () => {
  const { api, user, live } = await openSplit();
  live([RS2]);
  api.on("recovery.list", []);
  act(() => api.push("wsl", "session_exited", { session_id: RS.id, exit_code: 137, state: "interrupted" }));
  const direita = screen.getByRole("region", { name: "Sessão à direita" });
  await within(direita).findByText(/sessão interrompida \(código 137\)/);
  api.on("session.resume", () => {
    live([NEW, RS2]);
    return NEW;
  });
  await user.click(within(direita).getByRole("button", { name: "Retomar" }));
  await waitFor(() => expect(api.attached.has(NEW.id)).toBe(true));
  expect(api.callsTo("session.resume")[0].params).toEqual({ session_id: RS.id, prompt: RESUME_PROMPT });
});

// Prompts prontos (#425; spec da #87, §2.4).
const MULTI = { id: "meu-1", name: "Duas linhas", text: "Olhe a issue {{number}}\nna branch {{branch}}" };
const PASTE = "\x1b[200~Olhe a issue 32\nna branch feat/32-ui-terminal-embutido\x1b[201~";

test("prompt pronto: colagem com as variáveis e depois o Enter, por session.input; o foco volta ao console", async () => {
  const api = new FakeDaemonApi();
  api.uiConfig.prompts = [...api.uiConfig.prompts, MULTI];
  const { user } = await openTab(api);
  const barra = screen.getByRole("region", { name: "Sessão" });
  const antes = terms[0].focused;
  await user.click(within(barra).getByRole("button", { name: "Enviar prompt pronto" }));
  const menu = screen.getByRole("menu", { name: "Prompts prontos" });
  expect(within(menu).getAllByRole("menuitem").map((m) => m.textContent)).toEqual([
    "Continuar",
    "Rodar os testes e corrigir",
    "Registrar as decisões no ledger",
    "Abrir o PR",
    "Resumir o estado atual",
    "Duas linhas",
  ]);
  await user.click(within(menu).getByRole("menuitem", { name: "Duas linhas" }));
  await waitFor(() =>
    expect(api.inputs).toEqual([
      { env: "wsl", session: S.id, data: PASTE },
      { env: "wsl", session: S.id, data: "\r" },
    ]),
  );
  await waitFor(() => expect(terms[0].focused).toBe(antes + 1));
});

test("prompt pronto pela paleta: Sessão: Enviar prompt — Continuar", async () => {
  const { api, user } = await openTab();
  await user.click(screen.getByRole("button", { name: "Abrir paleta de comandos" }));
  const paleta = screen.getByRole("dialog", { name: "Paleta de comandos" });
  await user.type(within(paleta).getByRole("combobox"), "Sessão: Enviar prompt — Continuar");
  await user.keyboard("{Enter}");
  await waitFor(() =>
    expect(api.inputs).toEqual([
      { env: "wsl", session: S.id, data: "\x1b[200~continue\x1b[201~" },
      { env: "wsl", session: S.id, data: "\r" },
    ]),
  );
});

test("prompt pronto com pedido de permissão pendente: o menu avisa e não envia; a paleta não oferece", async () => {
  const api = new FakeDaemonApi().on("session.permission.watch", [REQ]);
  const { user } = await openTab(api);
  await screen.findByRole("group", { name: "Pedido de permissão" });
  await user.click(within(screen.getByRole("region", { name: "Sessão" })).getByRole("button", { name: "Enviar prompt pronto" }));
  const menu = screen.getByRole("menu", { name: "Prompts prontos" });
  expect(within(menu).getByRole("note")).toHaveTextContent("O agente está esperando a resposta do pedido de permissão");
  const continuar = within(menu).getByRole("menuitem", { name: "Continuar" });
  expect(continuar).toBeDisabled();
  await user.click(continuar);
  await user.keyboard("{Escape}");
  await user.click(screen.getByRole("button", { name: "Abrir paleta de comandos" }));
  const paleta = screen.getByRole("dialog", { name: "Paleta de comandos" });
  await user.type(within(paleta).getByRole("combobox"), "Enviar prompt");
  expect(within(paleta).queryByText(/Enviar prompt — Continuar/)).toBeNull();
  expect(api.inputs).toEqual([]);
});

test("prompt pronto: botão desligado com a sessão encerrada", async () => {
  const { api } = await openTab();
  const barra = screen.getByRole("region", { name: "Sessão" });
  expect(within(barra).getByRole("button", { name: "Enviar prompt pronto" })).toBeEnabled();
  api.on("dashboard.get", dashboard({ sync: [sync({ last_ok_at: NOW })] }, [WI]));
  act(() => api.push("wsl", "session_exited", { session_id: S.id, exit_code: 0 }));
  await waitFor(() => expect(within(barra).getByRole("button", { name: "Enviar prompt pronto" })).toBeDisabled());
  expect(within(barra).getByRole("button", { name: "Enviar prompt pronto" })).toHaveAttribute("title", "A sessão está encerrada");
});
