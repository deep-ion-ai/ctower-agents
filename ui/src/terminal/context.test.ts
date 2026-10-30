import { contextText, contextWindow, DEFAULT_WINDOW, LONG_WINDOW } from "./context";

test("janela pelo alias do modelo", () => {
  expect(contextWindow(null)).toBe(DEFAULT_WINDOW);
  expect(contextWindow("opus")).toBe(DEFAULT_WINDOW);
  expect(contextWindow("sonnet[1m]")).toBe(LONG_WINDOW);
});

test("texto do uso de contexto", () => {
  expect(contextText(null, DEFAULT_WINDOW)).toBeNull();
  expect(contextText(90_000, DEFAULT_WINDOW)).toBe("Contexto 45% · 90k/200k");
  expect(contextText(250_000, LONG_WINDOW)).toBe("Contexto 25% · 250k/1M");
  expect(contextText(210_000, DEFAULT_WINDOW)).toBe("Contexto 100% · 210k/200k");
});
