import {
  closeTargets,
  initialNav,
  navReducer,
  shellTabId,
  terminalTabId,
  type NavState,
  type Pane,
  type TabMeta,
} from "./nav";

// Muitas abas (#427): fechar em lote e a ordem por projeto.

const A = { env: "wsl", session: "a" };
const B = { env: "wsl", session: "b" };
const C = { env: "wsl", session: "c" };
const D = { env: "wsl", session: "d" };
const SH = { env: "wsl", shell: "x", workItemId: 7 };

const P1 = { key: "wsl|p1", name: "ctower-agents" };
const P2 = { key: "wsl|p2", name: "docs-site" };
const m = (project: TabMeta["project"], ended = false): TabMeta => ({ title: "t", project, env: "WSL", ended });

function open(...panes: Pane[]): NavState {
  let s = initialNav({ name: "control" });
  for (const p of panes) s = navReducer(s, { type: "terminal", tab: p });
  return s;
}

const ids = (s: NavState) => s.editors.map((e) => e.id);

test("Fechar outras: só a aba clicada fica, e ativa", () => {
  const s = open(A, B, C);
  const out = closeTargets(s.editors, { how: "others", id: terminalTabId(B) }, {});
  expect(out).toEqual(["control", terminalTabId(A), terminalTabId(C)]);
  const n = navReducer(s, { type: "closeMany", ids: out, keep: terminalTabId(B) });
  expect(ids(n)).toEqual([terminalTabId(B)]);
  expect(n.current).toBe(terminalTabId(B));
  expect(n.active).toEqual(B);
});

test("Fechar à direita: as depois da clicada; a ativa fechada passa para a clicada", () => {
  const s = open(A, B, C, D);
  expect(s.current).toBe(terminalTabId(D));
  const out = closeTargets(s.editors, { how: "right", id: terminalTabId(B) }, {});
  expect(out).toEqual([terminalTabId(C), terminalTabId(D)]);
  const n = navReducer(s, { type: "closeMany", ids: out, keep: terminalTabId(B) });
  expect(ids(n)).toEqual(["control", terminalTabId(A), terminalTabId(B)]);
  expect(n.current).toBe(terminalTabId(B));
  // A ativa sobrevivendo, segue ativa.
  const k = navReducer(open(A, B, C), { type: "activate", id: terminalTabId(A) });
  const r = navReducer(k, { type: "closeMany", ids: closeTargets(k.editors, { how: "right", id: terminalTabId(B) }, {}), keep: terminalTabId(B) });
  expect(r.current).toBe(terminalTabId(A));
});

test("Fechar encerradas: sessões encerradas e shells encerrados; a interrompida recuperável fica", () => {
  const s = open(A, B, C, SH);
  const meta = {
    [terminalTabId(A)]: m(P1, true),
    // Interrompida recuperável: a tela Terminal não a marca como encerrada.
    [terminalTabId(B)]: m(P1, false),
    [shellTabId(SH)]: m(P1, true),
  };
  const out = closeTargets(s.editors, { how: "ended" }, meta);
  expect(out).toEqual([terminalTabId(A), shellTabId(SH)]);
  // O lote fecha o shell encerrado direto, sem o pedido do cabeçalho.
  const n = navReducer(s, { type: "closeMany", ids: out });
  expect(ids(n)).toEqual(["control", terminalTabId(B), terminalTabId(C)]);
  expect(n.closing).toBeNull();
});

test("Fechar as do projeto: as abas de sessão e shell daquele projeto", () => {
  const s = open(A, B, C, SH);
  const meta = {
    [terminalTabId(A)]: m(P1),
    [terminalTabId(B)]: m(P2),
    [terminalTabId(C)]: m(P1),
    [shellTabId(SH)]: m(P1),
  };
  const out = closeTargets(s.editors, { how: "project", project: P1.key }, meta);
  expect(out).toEqual([terminalTabId(A), terminalTabId(C), shellTabId(SH)]);
  const n = navReducer(s, { type: "closeMany", ids: out });
  expect(ids(n)).toEqual(["control", terminalTabId(B)]);
  expect(n.active).toEqual(B);
});

test("fechar todas em lote volta para a Sala de controle e desfaz a divisão", () => {
  let s = open(A, B);
  s = navReducer(s, { type: "split", tab: A });
  expect(s.split).toEqual(A);
  s = navReducer(s, { type: "closeMany", ids: ids(s) });
  expect(ids(s)).toEqual(["control"]);
  expect(s.active).toBeNull();
  expect(s.split).toBeNull();
});

test("ordem por projeto: a aba nova entra depois da última do mesmo projeto", () => {
  const groups = {
    [terminalTabId(A)]: P1.key,
    [terminalTabId(B)]: P2.key,
    [terminalTabId(C)]: P1.key,
    [shellTabId(SH)]: P2.key,
  };
  let s = initialNav({ name: "control" });
  for (const p of [A, B, C, SH] as Pane[]) s = navReducer(s, { type: "terminal", tab: p, groups });
  expect(ids(s)).toEqual(["control", terminalTabId(A), terminalTabId(C), terminalTabId(B), shellTabId(SH)]);
  // Sem projeto conhecido, no fim.
  s = navReducer(s, { type: "terminal", tab: D, groups });
  expect(ids(s)[ids(s).length - 1]).toBe(terminalTabId(D));
  // Já aberta: não muda de lugar.
  s = navReducer(s, { type: "terminal", tab: A, groups });
  expect(ids(s)[1]).toBe(terminalTabId(A));
});

// Antigo Hub › Projetos e atribuições (#18): o endereço vira o lugar novo ao abrir.

test("endereço antigo do Hub › Projetos abre o projeto (Configurações) ou o índice, na aba deles", () => {
  const project = { owner: "deep-ion-ai", repo: "ctower-agents" };
  // Com projeto e `assign`: a aba do projeto, em Configurações › Ambientes, com o pedido.
  let s = navReducer(initialNav({ name: "control" }), {
    type: "open",
    // O `id` velho (a aba da seção, ou o `nav.current` de quem trocava só o endereço) não vale.
    tab: { id: "hub:projects", screen: { name: "hub", section: "projects", project, assign: true } },
  });
  expect(ids(s)).toEqual(["control", "project:deep-ion-ai/ctower-agents"]);
  expect(s.editors[1]).toMatchObject({
    screen: { name: "project", ...project, view: "settings", section: "envs", assign: true },
  });
  // Sem projeto, com `register`: o índice de Projetos com o pedido de cadastro.
  s = navReducer(s, { type: "open", tab: { screen: { name: "hub", section: "projects", register: { url: "u" } } } });
  expect(s.current).toBe("project");
  expect(s.editors.find((e) => e.id === "project")).toMatchObject({ screen: { name: "project", register: { url: "u" } } });
  // A aba inicial também (endereço de uma versão anterior).
  const init = initialNav({ name: "hub", section: "projects", project });
  expect(init.editors).toEqual([
    { kind: "screen", id: "project:deep-ion-ai/ctower-agents", screen: { name: "project", ...project, view: "settings" } },
  ]);
  // As outras seções do Hub seguem iguais.
  expect(initialNav({ name: "hub", section: "envs" }).editors[0]).toMatchObject({
    id: "hub",
    screen: { name: "hub", section: "envs" },
  });
});
