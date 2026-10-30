import { TERM_THEME } from "./theme";

// O vitest não carrega CSS como texto (`?raw` vem vazio): lê o arquivo pelo Node.
const fs = (await import("node:fs" as string)) as { readFileSync(p: string, enc: "utf8"): string };
const cwd = (globalThis as unknown as { process: { cwd(): string } }).process.cwd();
const css = fs.readFileSync(`${cwd}/src/theme.css`, "utf8");
const token = (name: string) => css.match(new RegExp(`--${name}:\\s*(#[0-9a-f]{6})`, "i"))?.[1];

test("paleta ANSI completa", () => {
  const ansi = ["black", "red", "green", "yellow", "blue", "magenta", "cyan", "white"] as const;
  for (const c of ansi) {
    const bright = `bright${c[0].toUpperCase()}${c.slice(1)}` as keyof typeof TERM_THEME;
    expect(TERM_THEME[c], c).toMatch(/^#[0-9a-f]{6}$/);
    expect(TERM_THEME[bright], bright).toMatch(/^#[0-9a-f]{6}$/);
  }
});

test.each([
  ["background", "term-bg"],
  ["foreground", "term-fg"],
  ["selectionBackground", "term-selection"],
  ["black", "surface-3"],
  ["red", "error"],
  ["green", "ok"],
  ["yellow", "attention"],
  ["blue", "running"],
  ["magenta", "epic"],
  ["white", "text-2"],
  ["brightBlack", "faint"],
  ["brightRed", "error-soft"],
  ["brightGreen", "done"],
  ["brightWhite", "text"],
] as const)("%s vem de --%s", (color, name) => {
  expect(TERM_THEME[color]).toBe(token(name));
});

const { readdirSync, statSync } = (await import("node:fs" as string)) as {
  readdirSync(d: string): string[];
  statSync(p: string): { isDirectory(): boolean };
};

// Tema claro (#290): o terminal fica escuro, e todo token de cor tem par claro.
const light = css.slice(css.indexOf(':root[data-theme="light"]'));
const block = (src: string) => src.slice(src.indexOf("{"), src.indexOf("}"));
const names = (src: string) => [...block(src).matchAll(/(--[\w-]+):\s*#/g)].map((m) => m[1]);

test("tema claro não redefine os tokens do terminal", () => {
  expect(names(light).filter((n) => n.startsWith("--term-"))).toEqual([]);
});

test("tema claro cobre todos os tokens de cor do escuro", () => {
  const dark = names(css).filter((n) => !n.startsWith("--term-"));
  expect(new Set(names(light))).toEqual(new Set(dark));
});

test("sem cor fixa nos CSS fora de theme.css", () => {
  const root = `${cwd}/src`;
  const files: string[] = [];
  const walk = (d: string) => {
    for (const f of readdirSync(d)) {
      const p = `${d}/${f}`;
      if (statSync(p).isDirectory()) walk(p);
      else if (p.endsWith(".css") && !p.endsWith("/theme.css")) files.push(p);
    }
  };
  walk(root);
  const colors = files.flatMap((f) =>
    fs
      .readFileSync(f, "utf8")
      .replace(/\/\*[\s\S]*?\*\//g, "")
      .split("\n")
      .filter((l) => /#[0-9a-f]{3,8}\b|rgba?\(|hsla?\(/i.test(l))
      .map((l) => `${f.slice(root.length)}: ${l.trim()}`),
  );
  expect(colors).toEqual([]);
});
