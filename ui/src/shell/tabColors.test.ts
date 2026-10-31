import type { TabColors } from "../api/types";
import type { EditorTab } from "../state/nav";
import {
  applyTabColors,
  contrastRatio,
  emptyTabColors,
  hasTabColors,
  kindContrast,
  normalizeHex,
  tabColorVars,
  tabKind,
  withTabColor,
} from "./tabColors";

test("HEX: #RGB e #RRGGBB, com ou sem #, sem diferenciar maiúsculas; grava #rrggbb", () => {
  expect(normalizeHex("#7FD4BC")).toBe("#7fd4bc");
  expect(normalizeHex("7fd4bc")).toBe("#7fd4bc");
  expect(normalizeHex("#AbC")).toBe("#aabbcc");
  expect(normalizeHex("f0a")).toBe("#ff00aa");
  expect(normalizeHex("  #123456 ")).toBe("#123456");
  for (const bad of ["", "#", "#12", "#1234", "#12345g", "red", "##123", "#1234567", "rgb(0,0,0)"])
    expect(normalizeHex(bad)).toBeNull();
});

test("contraste WCAG", () => {
  expect(contrastRatio("#000000", "#ffffff")).toBeCloseTo(21, 5);
  expect(contrastRatio("#ffffff", "#000000")).toBeCloseTo(21, 5);
  expect(contrastRatio("#777", "#777")).toBeCloseTo(1, 5);
  expect(contrastRatio("#767676", "#ffffff")).toBeCloseTo(4.54, 2);
});

test("contraste do tipo usa as cores padrão do tema nos campos vazios", () => {
  const empty = emptyTabColors();
  // --muted sobre --bg-side de cada tema.
  expect(kindContrast(empty, "dark", "terminal")).toBeCloseTo(contrastRatio("#9aa3b2", "#12151b"), 5);
  expect(kindContrast(undefined, "light", "shell")).toBeCloseTo(contrastRatio("#5a6372", "#f0f2f5"), 5);
  const c = withTabColor(empty, "dark", "issue", "bg", "#9aa3b2");
  expect(kindContrast(c, "dark", "issue")).toBeCloseTo(1, 5);
  expect(kindContrast(c, "light", "issue")).toBeGreaterThan(4.5);
});

const cfg: TabColors = {
  dark: { terminal: { fg: "#7fd4bc", bg: "#13241f" }, shell: { fg: null }, issue: { bg: "bogus" }, screen: {} },
  light: { terminal: { fg: "#1a7a5e", bg: "#ddf3ec" }, shell: {}, issue: {}, screen: { fg: "#123456" } },
};

test("variáveis do tema em uso: só as cores válidas; ausente, null ou inválida ficam com o tema", () => {
  expect(tabColorVars(cfg, "dark")).toEqual({ "--tab-terminal-fg": "#7fd4bc", "--tab-terminal-bg": "#13241f" });
  expect(tabColorVars(cfg, "light")).toEqual({
    "--tab-terminal-fg": "#1a7a5e",
    "--tab-terminal-bg": "#ddf3ec",
    "--tab-screen-fg": "#123456",
  });
  expect(tabColorVars(undefined, "dark")).toEqual({});
});

test("applyTabColors põe as do tema e tira as que deixaram de valer", () => {
  const el = document.createElement("div");
  applyTabColors(cfg, "light", el);
  expect(el.style.getPropertyValue("--tab-screen-fg")).toBe("#123456");
  expect(el.style.getPropertyValue("--tab-terminal-bg")).toBe("#ddf3ec");
  applyTabColors(cfg, "dark", el);
  expect(el.style.getPropertyValue("--tab-screen-fg")).toBe("");
  expect(el.style.getPropertyValue("--tab-terminal-bg")).toBe("#13241f");
  applyTabColors(undefined, "dark", el);
  expect(el.style.getPropertyValue("--tab-terminal-bg")).toBe("");
});

test("withTabColor muda só o tema e o campo pedidos; null limpa", () => {
  const a = withTabColor(undefined, "dark", "terminal", "fg", "#7fd4bc");
  expect(a.dark.terminal).toEqual({ fg: "#7fd4bc" });
  expect(a.light).toEqual(emptyTabColors().light);
  const b = withTabColor(a, "dark", "terminal", "bg", "#13241f");
  expect(b.dark.terminal).toEqual({ fg: "#7fd4bc", bg: "#13241f" });
  const c = withTabColor(b, "dark", "terminal", "fg", null);
  expect(c.dark.terminal).toEqual({ bg: "#13241f" });
  expect(hasTabColors(c, "dark")).toBe(true);
  expect(hasTabColors(c, "light")).toBe(false);
  expect(hasTabColors(emptyTabColors())).toBe(false);
});

test("tipo da aba: sessão, shell, issue/épico (projeto com item) e demais telas", () => {
  const t = (x: EditorTab) => tabKind(x);
  expect(t({ kind: "terminal", id: "a", env: "wsl", session: "s" })).toBe("terminal");
  expect(t({ kind: "shell", id: "b", env: "wsl", shell: "x", workItemId: 1 })).toBe("shell");
  expect(
    t({ kind: "screen", id: "c", screen: { name: "project", owner: "acme", repo: "acme-web", item: { ghNumber: 12 } } }),
  ).toBe("issue");
  expect(t({ kind: "screen", id: "d", screen: { name: "project", env: "wsl", projectId: 1, item: { ghNumber: 3 } } })).toBe(
    "issue",
  );
  expect(t({ kind: "screen", id: "e", screen: { name: "project", owner: "acme", repo: "acme-web" } })).toBe("screen");
  expect(t({ kind: "screen", id: "project", screen: { name: "project" } })).toBe("screen");
  expect(t({ kind: "screen", id: "control", screen: { name: "control" } })).toBe("screen");
  expect(t({ kind: "screen", id: "settings", screen: { name: "settings" } })).toBe("screen");
});
