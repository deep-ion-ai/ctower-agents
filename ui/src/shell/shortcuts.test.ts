import { isHelpKey, shortcutGroups } from "./shortcuts";

const key = (p: Partial<Parameters<typeof isHelpKey>[0]> = {}) =>
  isHelpKey({ key: "?", ctrlKey: false, shiftKey: true, altKey: false, metaKey: false, ...p });

test("? abre a folha fora de campos de texto e do console; com Ctrl, Alt ou ⌘ sozinhos, não", () => {
  expect(key()).toBe(true);
  expect(key({ target: document.body })).toBe(true);
  // AltGr (Ctrl+Alt) em layouts que digitam o ? assim.
  expect(key({ ctrlKey: true, altKey: true })).toBe(true);
  expect(key({ ctrlKey: true })).toBe(false);
  expect(key({ altKey: true })).toBe(false);
  expect(key({ metaKey: true })).toBe(false);
  expect(key({ repeat: true })).toBe(false);
  expect(key({ key: "/" })).toBe(false);
  const editable = document.createElement("div");
  editable.setAttribute("contenteditable", "true");
  const typing = [
    document.createElement("input"),
    document.createElement("textarea"),
    document.createElement("select"),
    editable,
  ];
  for (const el of typing) expect(key({ target: el })).toBe(false);
  // O xterm digita num textarea, mas o alvo pode ser outro elemento do console.
  const term = document.createElement("div");
  term.className = "xterm";
  const inner = document.createElement("span");
  term.append(inner);
  expect(key({ target: inner })).toBe(false);
  const off = document.createElement("div");
  off.setAttribute("contenteditable", "false");
  expect(key({ target: off })).toBe(true);
});

type Groups = ReturnType<typeof shortcutGroups>;
const titles = (g: Groups) => g.map((x) => x.title);
const keysOf = (g: Groups) => g.flatMap((x) => x.rows.flatMap((r) => (r.gesture ? [] : r.keys)));

test("desktop: atalhos por tela, com Ctrl fora do macOS e ⌘ no macOS", () => {
  const pc = shortcutGroups({ web: false, mobile: false, mac: false });
  expect(titles(pc)).toEqual([
    "Geral",
    "Paleta de comandos",
    "Abas e painel lateral",
    "Terminal",
    "Árvore de sessões",
    "Projetos",
    "Iniciar sessão",
  ]);
  expect(keysOf(pc)).toEqual(
    expect.arrayContaining(["?", "Ctrl+K", "Ctrl+B", "Ctrl+J", "Ctrl+Alt+B", "Shift+F10", "Alt+↑", "Alt+↓", "Ctrl+Shift+`"]),
  );
  // Fora do macOS, Ctrl+B/J/K ficam com o console.
  expect(pc[0].note).toMatch(/console/);
  const mac = shortcutGroups({ web: false, mobile: false, mac: true });
  expect(keysOf(mac)).toEqual(expect.arrayContaining(["⌘K", "⌘B", "⌘⌥B", "⌥↑", "Ctrl+Shift+`"]));
  expect(mac[0].note).toBeUndefined();
});

test("web no desktop: sem Terminal e sem reordenar projetos", () => {
  const g = shortcutGroups({ web: true, mobile: false, mac: false });
  expect(titles(g)).not.toContain("Terminal");
  expect(titles(g)).not.toContain("Árvore de sessões");
  expect(keysOf(g)).not.toContain("Alt+↑");
  expect(keysOf(g)).not.toContain("Ctrl+Shift+`");
  expect(keysOf(g)).toEqual(expect.arrayContaining(["Ctrl+K", "Shift+F10"]));
});

test("celular: só gestos, sem tecla", () => {
  const g = shortcutGroups({ web: true, mobile: true, mac: false });
  expect(g.length).toBeGreaterThan(0);
  expect(g.every((x) => x.rows.every((r) => r.gesture))).toBe(true);
  expect(keysOf(g)).toEqual([]);
});
