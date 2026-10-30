import { act, renderHook } from "@testing-library/react";
import { afterEach, vi } from "vitest";
import { THEME_KEY, applyTheme, resolveTheme, useTheme } from "./theme";

/** `prefers-color-scheme` do sistema, com troca disparando `change`. */
function fakeSystem(dark: boolean) {
  const subs = new Set<() => void>();
  const mq = {
    get matches() {
      return dark;
    },
    addEventListener: (_: string, cb: () => void) => subs.add(cb),
    removeEventListener: (_: string, cb: () => void) => subs.delete(cb),
  };
  vi.stubGlobal("matchMedia", () => mq);
  return {
    subs,
    set(d: boolean) {
      dark = d;
      for (const cb of subs) cb();
    },
  };
}

afterEach(() => vi.unstubAllGlobals());

test("claro e escuro são fixos; sistema segue o prefers-color-scheme", () => {
  fakeSystem(false);
  expect(resolveTheme("light")).toBe("light");
  expect(resolveTheme("dark")).toBe("dark");
  expect(resolveTheme("system")).toBe("light");
  fakeSystem(true);
  expect(resolveTheme("system")).toBe("dark");
});

test("sem matchMedia, sistema cai no escuro", () => {
  expect(resolveTheme("system")).toBe("dark");
});

test("aplicar põe data-theme no <html> e guarda a preferência para o boot", () => {
  expect(applyTheme("light")).toBe("light");
  expect(document.documentElement.dataset.theme).toBe("light");
  expect(localStorage.getItem(THEME_KEY)).toBe("light");
});

test("seguindo o sistema, acompanha a troca; ao escolher um tema fixo, para de ouvir", () => {
  const sys = fakeSystem(true);
  const { rerender } = renderHook(({ pref }) => useTheme(pref), {
    initialProps: { pref: "system" as "system" | "light" | null },
  });
  expect(document.documentElement.dataset.theme).toBe("dark");
  act(() => sys.set(false));
  expect(document.documentElement.dataset.theme).toBe("light");
  rerender({ pref: "light" });
  expect(sys.subs.size).toBe(0);
  act(() => sys.set(true));
  expect(document.documentElement.dataset.theme).toBe("light");
});

test("antes de carregar (null), não mexe no tema do boot", () => {
  document.documentElement.dataset.theme = "light";
  renderHook(() => useTheme(null));
  expect(document.documentElement.dataset.theme).toBe("light");
  expect(localStorage.getItem(THEME_KEY)).toBeNull();
});
