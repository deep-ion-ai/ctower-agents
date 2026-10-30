import { afterEach, vi } from "vitest";
import { THEME_KEY } from "./theme";

// public/theme-boot.js roda solto no <head>: lido pelo Node e executado aqui.
const fs = (await import("node:fs" as string)) as { readFileSync(p: string, enc: "utf8"): string };
const cwd = (globalThis as unknown as { process: { cwd(): string } }).process.cwd();
const script = fs.readFileSync(`${cwd}/public/theme-boot.js`, "utf8");

const w = window as unknown as { __CT_THEME__?: string };

function boot(systemDark = true) {
  vi.stubGlobal("matchMedia", () => ({ matches: systemDark }));
  new Function(script)();
  return document.documentElement.dataset.theme;
}

afterEach(() => {
  vi.unstubAllGlobals();
  delete w.__CT_THEME__;
});

test("sem nada gravado: escuro", () => {
  expect(boot()).toBe("dark");
});

test("o que o app nativo leu do ui.json vale sem localStorage", () => {
  w.__CT_THEME__ = "light";
  expect(boot()).toBe("light");
  w.__CT_THEME__ = "system";
  expect(boot(false)).toBe("light");
  expect(boot(true)).toBe("dark");
});

test("a última preferência aplicada vence a do início da janela", () => {
  w.__CT_THEME__ = "dark";
  localStorage.setItem(THEME_KEY, "light");
  expect(boot()).toBe("light");
});
