import { initialNav, navReducer, shellTabId, terminalTabId, type NavState } from "../state/nav";
import { clampWidth, contextShortcut, loadLayout, DEFAULT_LAYOUT, newShellShortcut, shellShortcut } from "./layout";

const key = (k: string, mods: Partial<KeyboardEvent> = {}) => ({
  key: k,
  ctrlKey: false,
  metaKey: false,
  shiftKey: false,
  altKey: false,
  ...mods,
});

test("atalhos: Cmd no macOS, Ctrl fora; no console fora do macOS a tecla fica com o xterm", () => {
  expect(shellShortcut(key("b", { metaKey: true }), false, true)).toBe("side");
  expect(shellShortcut(key("j", { metaKey: true }), true, true)).toBe("panel");
  expect(shellShortcut(key("K", { metaKey: true }), false, true)).toBe("palette");
  expect(shellShortcut(key("b", { ctrlKey: true }), false, true)).toBeNull();
  expect(shellShortcut(key("b", { ctrlKey: true }), false, false)).toBe("side");
  expect(shellShortcut(key("b", { ctrlKey: true }), true, false)).toBeNull();
  expect(shellShortcut(key("b", { ctrlKey: true, shiftKey: true }), false, false)).toBeNull();
  expect(shellShortcut(key("x", { ctrlKey: true }), false, false)).toBeNull();
});

test("largura no intervalo e disposição padrão sem armazenamento", () => {
  expect(clampWidth(10)).toBe(180);
  expect(clampWidth(9999)).toBe(520);
  expect(loadLayout()).toEqual(DEFAULT_LAYOUT);
});

const T1 = { env: "wsl", session: "s1" };
const T2 = { env: "wsl", session: "s2" };

test("abas: uma por tela, a aba vazia do Terminal dá lugar à sessão, fechar ativa a vizinha", () => {
  let s: NavState = initialNav({ name: "control" });
  s = navReducer(s, { type: "open", tab: { screen: { name: "terminal" } } });
  expect(s.editors.map((e) => e.id)).toEqual(["control", "terminal"]);
  s = navReducer(s, { type: "terminal", tab: T1 });
  expect(s.editors.map((e) => e.id)).toEqual(["control", terminalTabId(T1)]);
  s = navReducer(s, { type: "terminal", tab: T2 });
  s = navReducer(s, { type: "open", tab: { screen: { name: "review", env: "wsl", workItemId: 3 } } });
  s = navReducer(s, { type: "open", tab: { screen: { name: "review" } } });
  expect(s.editors.filter((e) => e.id === "review")).toHaveLength(1);
  // A tela Terminal com sessões é a aba da sessão ativa.
  s = navReducer(s, { type: "open", tab: { screen: { name: "terminal" } } });
  expect(s.current).toBe(terminalTabId(T2));
  s = navReducer(s, { type: "close", id: terminalTabId(T2) });
  expect(s.current).toBe(terminalTabId(T1));
  expect(s.active).toEqual(T1);
  for (const e of [...s.editors]) s = navReducer(s, { type: "close", id: e.id });
  expect(s.editors.map((e) => e.id)).toEqual(["control"]);
  expect(s.active).toBeNull();
});

test("navReducer: dividir à direita, trocar os lados e fechar a sessão da divisão (#316)", () => {
  const T1 = { env: "wsl", session: "a" };
  const T2 = { env: "wsl", session: "b" };
  const T3 = { env: "wsl", session: "c" };
  let s: NavState = initialNav({ name: "control" });
  // Sem outra sessão aberta, não divide (nem consigo mesma).
  s = navReducer(s, { type: "terminal", tab: T1 });
  s = navReducer(s, { type: "split", tab: T1 });
  expect(s.split).toBeNull();
  s = navReducer(s, { type: "split", tab: T2 });
  expect(s.split).toBeNull();
  s = navReducer(s, { type: "terminal", tab: T2 });
  s = navReducer(s, { type: "terminal", tab: T3 });
  s = navReducer(s, { type: "split", tab: T1 });
  expect(s.active).toEqual(T3);
  expect(s.split).toEqual(T1);
  // Ativar a da direita troca os lados.
  s = navReducer(s, { type: "activate", id: terminalTabId(T1) });
  expect(s.active).toEqual(T1);
  expect(s.split).toEqual(T3);
  // Outra sessão ativa: a divisão fica.
  s = navReducer(s, { type: "terminal", tab: T2 });
  expect(s.split).toEqual(T3);
  // Sair para outra tela e voltar mantém a divisão.
  s = navReducer(s, { type: "open", tab: { screen: { name: "control" } } });
  s = navReducer(s, { type: "open", tab: { screen: { name: "terminal" } } });
  expect(s.active).toEqual(T2);
  expect(s.split).toEqual(T3);
  // Fechar a sessão da direita desfaz a divisão.
  s = navReducer(s, { type: "close", id: terminalTabId(T3) });
  expect(s.split).toBeNull();
  s = navReducer(s, { type: "split", tab: T1 });
  expect(s.split).toEqual(T1);
  s = navReducer(s, { type: "split", tab: null });
  expect(s.split).toBeNull();
});

const item = (n: number) => ({
  id: `project:o/r#${n}`,
  title: `#${n}`,
  screen: { name: "project" as const, owner: "o", repo: "r", item: { ghNumber: n } },
});
const ids = (s: NavState) => s.editors.map((e) => e.id);
const previews = (s: NavState) => s.editors.flatMap((e) => (e.kind === "screen" && e.preview ? [e.id] : []));

test("abas provisórias: o item novo substitui a provisória no lugar dela; a aba da tela não é provisória", () => {
  let s: NavState = initialNav({ name: "control" });
  s = navReducer(s, { type: "open", tab: { screen: { name: "project", owner: "o", repo: "r" } } });
  s = navReducer(s, { type: "open", tab: item(1) });
  expect(previews(s)).toEqual([item(1).id]);
  s = navReducer(s, { type: "open", tab: { screen: { name: "review" } } });
  s = navReducer(s, { type: "open", tab: item(2) });
  // No lugar da #1 (antes da Revisão), ativa.
  expect(ids(s)).toEqual(["control", "project", item(2).id, "review"]);
  expect(s.current).toBe(item(2).id);
  expect(previews(s)).toEqual([item(2).id]);
});

test("fixar: a aba fica; reabrir item já aberto só ativa; trocar o endereço da atual mantém o estado", () => {
  let s: NavState = initialNav({ name: "control" });
  s = navReducer(s, { type: "open", tab: item(1) });
  s = navReducer(s, { type: "pin", id: item(1).id, pinned: true });
  expect(previews(s)).toEqual([]);
  s = navReducer(s, { type: "open", tab: item(2) });
  expect(ids(s)).toEqual(["control", item(1).id, item(2).id]);
  // Reabrir a fixada: ativa, sem nova aba e sem perder a fixação.
  s = navReducer(s, { type: "open", tab: item(1) });
  expect(ids(s)).toHaveLength(3);
  expect(s.current).toBe(item(1).id);
  expect(previews(s)).toEqual([item(2).id]);
  // Trocar o endereço da aba atual (`id: nav.current`) não cria aba nem a torna provisória.
  s = navReducer(s, { type: "open", tab: { ...item(1), screen: { ...item(1).screen, item: { ghNumber: 1 } } } });
  expect(ids(s)).toHaveLength(3);
  expect(previews(s)).toEqual([item(2).id]);
  s = navReducer(s, { type: "activate", id: item(2).id });
  s = navReducer(s, { type: "open", tab: { id: s.current, screen: item(9).screen } });
  expect(ids(s)).toEqual(["control", item(1).id, item(2).id]);
  expect(previews(s)).toEqual([item(2).id]);
  // A aba principal de uma tela e as de sessão não se fixam.
  const before = s;
  expect(navReducer(s, { type: "pin", id: "control", pinned: false })).toBe(before);
  // Desafixar: vira a provisória e a anterior sai (só uma).
  s = navReducer(s, { type: "pin", id: item(1).id, pinned: false });
  expect(ids(s)).toEqual(["control", item(1).id]);
  expect(previews(s)).toEqual([item(1).id]);
  // Fechar continua valendo para a provisória.
  s = navReducer(s, { type: "close", id: item(1).id });
  expect(ids(s)).toEqual(["control"]);
});

test("atalho do painel de contexto (#426): Ctrl+Alt+B, ⌘⌥B no macOS, sem colidir com o painel lateral", () => {
  expect(contextShortcut({ ...key("b", { ctrlKey: true, altKey: true }), code: "KeyB" }, false)).toBe(true);
  // Option+B no macOS vira "∫": vale a tecla física.
  expect(contextShortcut({ ...key("∫", { metaKey: true, altKey: true }), code: "KeyB" }, true)).toBe(true);
  expect(contextShortcut(key("b", { ctrlKey: true, altKey: true }), true)).toBe(false);
  expect(contextShortcut(key("b", { ctrlKey: true }), false)).toBe(false);
  expect(contextShortcut(key("b", { ctrlKey: true, altKey: true, shiftKey: true }), false)).toBe(false);
  // O ⌘B/Ctrl+B do painel lateral não reage com Alt.
  expect(shellShortcut(key("b", { ctrlKey: true, altKey: true }), false, false)).toBeNull();
  expect(shellShortcut(key("∫", { metaKey: true, altKey: true }), false, true)).toBeNull();
});

const SH = { env: "wsl", shell: "sh1", workItemId: 31 };

test("atalho do shell novo: Ctrl+Shift+` pela tecla física, também no macOS", () => {
  expect(newShellShortcut({ ...key("~", { ctrlKey: true, shiftKey: true }), code: "Backquote" })).toBe(true);
  expect(newShellShortcut(key("`", { ctrlKey: true, shiftKey: true }))).toBe(true);
  expect(newShellShortcut(key("`", { ctrlKey: true }))).toBe(false);
  expect(newShellShortcut(key("`", { metaKey: true, shiftKey: true }))).toBe(false);
});

test("abas de shell (#423): no split ao lado da sessão, trocando de lado como o terminal", () => {
  let s: NavState = initialNav({ name: "control" });
  s = navReducer(s, { type: "terminal", tab: T1 });
  s = navReducer(s, { type: "terminal", tab: SH });
  expect(s.editors.map((e) => e.id)).toEqual(["control", terminalTabId(T1), shellTabId(SH)]);
  expect(s.editors[2]).toEqual({ kind: "shell", id: shellTabId(SH), ...SH });
  expect(s.active).toEqual(SH);
  // O agente à esquerda, o shell à direita.
  s = navReducer(s, { type: "activate", id: terminalTabId(T1) });
  s = navReducer(s, { type: "split", tab: SH });
  expect(s.split).toEqual(SH);
  // Ativar o shell da direita troca os lados.
  s = navReducer(s, { type: "activate", id: shellTabId(SH) });
  expect(s.active).toEqual(SH);
  expect(s.split).toEqual(T1);
});

test("fechar a aba de shell: o pedido ativa a aba e espera o cabeçalho; com force, fecha e sai do split", () => {
  let s: NavState = initialNav({ name: "control" });
  s = navReducer(s, { type: "terminal", tab: T1 });
  s = navReducer(s, { type: "terminal", tab: SH });
  s = navReducer(s, { type: "activate", id: terminalTabId(T1) });
  s = navReducer(s, { type: "split", tab: SH });
  s = navReducer(s, { type: "activate", id: "control" });
  // × na aba do shell: nada fecha ainda; a aba fica ativa e marcada.
  s = navReducer(s, { type: "close", id: shellTabId(SH) });
  expect(s.editors).toHaveLength(3);
  expect(s.current).toBe(shellTabId(SH));
  expect(s.closing).toBe(shellTabId(SH));
  s = navReducer(s, { type: "cancelClose" });
  expect(s.closing).toBeNull();
  s = navReducer(s, { type: "close", id: shellTabId(SH), force: true });
  expect(s.editors.map((e) => e.id)).toEqual(["control", terminalTabId(T1)]);
  expect(s.active).toEqual(T1);
  expect(s.split).toBeNull();
  expect(s.closing).toBeNull();
  // A sessão do agente segue fechando direto.
  s = navReducer(s, { type: "close", id: terminalTabId(T1) });
  expect(s.editors.map((e) => e.id)).toEqual(["control"]);
  expect(s.active).toBeNull();
});

test("navReducer: replace troca a sessão na mesma aba, posição e coluna (Retomar e Reiniciar, #424)", () => {
  const A = { env: "wsl", session: "a" };
  const B = { env: "wsl", session: "b" };
  const C = { env: "wsl", session: "c" };
  const N = { env: "wsl", session: "nova" };
  let s: NavState = initialNav({ name: "control" });
  s = navReducer(s, { type: "terminal", tab: A });
  s = navReducer(s, { type: "terminal", tab: B });
  s = navReducer(s, { type: "terminal", tab: C });
  s = navReducer(s, { type: "activate", id: terminalTabId(B) });
  s = navReducer(s, { type: "split", tab: C });
  const ids = (x: NavState) => x.editors.map((e) => e.id);

  // A ativa (coluna 1): a nova fica no mesmo lugar, ativa e com a mesma divisão.
  let r = navReducer(s, { type: "replace", old: B, tab: N });
  expect(ids(r)).toEqual(["control", terminalTabId(A), terminalTabId(N), terminalTabId(C)]);
  expect(r.current).toBe(terminalTabId(N));
  expect(r.active).toEqual(N);
  expect(r.split).toEqual(C);

  // A da direita (coluna 2): segue na coluna 2, sem roubar o foco.
  r = navReducer(s, { type: "replace", old: C, tab: N });
  expect(ids(r)).toEqual(["control", terminalTabId(A), terminalTabId(B), terminalTabId(N)]);
  expect(r.current).toBe(terminalTabId(B));
  expect(r.active).toEqual(B);
  expect(r.split).toEqual(N);

  // Uma aba de fundo: troca no lugar e a ativa fica como estava.
  r = navReducer(s, { type: "replace", old: A, tab: N });
  expect(ids(r)).toEqual(["control", terminalTabId(N), terminalTabId(B), terminalTabId(C)]);
  expect(r.current).toBe(terminalTabId(B));

  // A antiga já fechada: a nova abre como uma aba comum.
  r = navReducer(navReducer(s, { type: "close", id: terminalTabId(A) }), { type: "replace", old: A, tab: N });
  expect(ids(r)).toEqual(["control", terminalTabId(B), terminalTabId(C), terminalTabId(N)]);
  expect(r.active).toEqual(N);
});
