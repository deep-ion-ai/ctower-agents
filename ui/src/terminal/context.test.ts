import type { SessionUsage } from "../protocol/gen";
import { contextText, contextTokens, contextWindow, DEFAULT_WINDOW, LONG_WINDOW } from "./context";

/** Leitura de uso fictícia. */
function usageReading(p: Partial<SessionUsage> = {}): SessionUsage {
  return {
    tool_session_id: "s-1",
    written_at: 0,
    model: null,
    effort: null,
    cost: null,
    context_window: null,
    prompt_cache: null,
    plan: null,
    ...p,
  };
}

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

test("com leitura de uso, a janela é a real: 1M sem [1m] no alias (#110)", () => {
  const reading = usageReading({
    context_window: { context_window_size: 1_000_000, used_percentage: 25, used_tokens: 250_000 },
  });
  expect(contextWindow("opus", reading)).toBe(LONG_WINDOW);
  expect(contextText(250_000, contextWindow("opus", reading))).toBe("Contexto 25% · 250k/1M");
  // Sem leitura (ou sem o tamanho nela), o palpite pelo alias.
  expect(contextWindow("opus", null)).toBe(DEFAULT_WINDOW);
  expect(contextWindow("opus", usageReading())).toBe(DEFAULT_WINDOW);
});

test("tokens: a fonte mais recente entre o transcript e a leitura", () => {
  const reading = usageReading({
    written_at: Date.parse("2026-11-01T12:00:00Z"),
    context_window: { context_window_size: 1_000_000, used_percentage: 12, used_tokens: 120_000 },
  });
  expect(contextTokens(null, null)).toBeNull();
  expect(contextTokens({ tokens: 90_000, model: null, ts: null }, null)).toBe(90_000);
  expect(contextTokens(null, reading)).toBe(120_000);
  expect(contextTokens({ tokens: 90_000, model: null, ts: null }, reading)).toBe(120_000);
  expect(contextTokens({ tokens: 90_000, model: null, ts: "2026-11-01T11:59:00Z" }, reading)).toBe(120_000);
  expect(contextTokens({ tokens: 130_000, model: null, ts: "2026-11-01T12:01:00Z" }, reading)).toBe(130_000);
});
