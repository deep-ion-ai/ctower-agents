import "@testing-library/jest-dom/vitest";
import { cleanup } from "@testing-library/react";
import { afterEach, vi } from "vitest";
import { resetTermPrefs } from "../terminal/prefs";
import { forgetTermSize } from "../terminal/size";
import { resetProjectUi } from "../project/ui-store";
import { resetReviewUi } from "../review/ui-store";
import { THEME_KEY } from "../state/theme";

vi.mock("../terminal/xterm", () => import("./fakeXterm"));

afterEach(() => {
  cleanup();
  forgetTermSize();
  resetTermPrefs();
  resetProjectUi();
  resetReviewUi();
  delete document.documentElement.dataset.theme;
  localStorage.removeItem(THEME_KEY);
  // Layout do shell, recentes da paleta etc.: cada teste começa do padrão,
  // em qualquer ordem (`--sequence.shuffle`).
  localStorage.clear();
});

// Node 25+ tem um localStorage global (vazio sem --localstorage-file) que
// esconde o do jsdom: no lugar dele, um em memória.
if (typeof globalThis.localStorage?.getItem !== "function") {
  const data = new Map<string, string>();
  const memory: Storage = {
    get length() {
      return data.size;
    },
    clear: () => data.clear(),
    getItem: (k) => data.get(k) ?? null,
    key: (i) => [...data.keys()][i] ?? null,
    removeItem: (k) => void data.delete(k),
    setItem: (k, v) => void data.set(k, String(v)),
  };
  Object.defineProperty(globalThis, "localStorage", { value: memory, configurable: true });
}

// jsdom não tem ResizeObserver (usado pelo terminal).
class NoopResizeObserver {
  observe() {}
  unobserve() {}
  disconnect() {}
}
globalThis.ResizeObserver ??= NoopResizeObserver as unknown as typeof ResizeObserver;
