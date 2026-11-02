import { act, renderHook } from "@testing-library/react";
import { FakeDaemonApi } from "../api/fake";
import type { PlanReading } from "../protocol/gen";
import { accountName, cacheText, clock, costText, latestPlan, USAGE_RELOAD_MS, useSessionUsage, windowText } from "./usage";

const MIN = 60_000;
const NOW = new Date(2026, 10, 1, 12, 0).getTime();

test("relógio: hora no mesmo dia, dia e hora fora dele", () => {
  expect(clock(new Date(2026, 10, 1, 14, 5).getTime(), NOW)).toBe("14:05");
  expect(clock(new Date(2026, 10, 3, 9, 30).getTime(), NOW)).toBe("03/11 09:30");
});

test("janela do plano: percentual com o reset; renovada sem percentual", () => {
  const at = NOW + 90 * MIN;
  expect(windowText({ state: "ok", used_percentage: 42.4, resets_at: at, read_at: NOW }, NOW)).toBe("42% · renova 13:30");
  const renewed = windowText({ state: "renewed", resets_at: NOW - MIN, read_at: NOW - 10 * MIN }, NOW);
  expect(renewed).not.toMatch(/%/);
  expect(renewed).toBe("renovada 11:59 · aguardando nova leitura");
  expect(windowText({ state: "none" }, NOW)).toBe("—");
});

test("custo e cache de prompt", () => {
  expect(costText(1.234)).toBe("US$ 1,23");
  expect(cacheText({ warm: true, ttl: "5m", expires_at: NOW + 3 * MIN, hit_ratio: 0.87 }, NOW)).toBe(
    "acerto 87% · válido até 12:03 · TTL 5m",
  );
  expect(cacheText({ warm: false, ttl: null, expires_at: NOW - MIN, hit_ratio: null }, NOW)).toBe("expirou 11:59");
  expect(cacheText({ warm: false, ttl: null, expires_at: null, hit_ratio: null }, NOW)).toBe("frio");
  expect(cacheText({ warm: null, ttl: null, expires_at: null, hit_ratio: null }, NOW)).toBeNull();
});

test("plano: a leitura mais recente; conta pelo e-mail ou pela organização", () => {
  const a: PlanReading = { read_at: 1, five_hour: null, seven_day: null };
  const b: PlanReading = { read_at: 2, five_hour: null, seven_day: null };
  expect(latestPlan(a, b)).toBe(b);
  expect(latestPlan(b, a)).toBe(b);
  expect(latestPlan(null, a)).toBe(a);
  expect(latestPlan(null, null)).toBeNull();
  expect(accountName({ key: "k", email: "fulano@example.com", org_name: "conta-a", subscription_type: "max" })).toBe(
    "fulano@example.com",
  );
  expect(accountName({ key: "k", email: null, org_name: "conta-a", subscription_type: null })).toBe("conta-a");
  expect(accountName(null)).toBeNull();
});

test("hook: lê ao montar, relê nos pushes da sessão e no intervalo; a conta só fora da web", async () => {
  vi.useFakeTimers();
  try {
    const api = new FakeDaemonApi()
      .on("session.usage", () => null)
      .on("account.get", () => ({ account: null, statusline: {}, plan: {}, latest: null, now: 0 }));
    const { unmount } = renderHook(() => useSessionUsage(api, "wsl", "s-1", { poll: true, withAccount: true }));
    await act(async () => {});
    expect(api.callsTo("session.usage")).toHaveLength(1);
    expect(api.callsTo("session.usage")[0].params).toEqual({ session_id: "s-1" });
    expect(api.callsTo("account.get")).toHaveLength(1);
    act(() => api.push("wsl", "session_state", { session_id: "s-1", state: "idle" }));
    act(() => api.push("wsl", "session_state", { session_id: "s-2", state: "idle" }));
    expect(api.callsTo("session.usage")).toHaveLength(2);
    await act(async () => vi.advanceTimersByTime(USAGE_RELOAD_MS));
    expect(api.callsTo("session.usage")).toHaveLength(3);
    unmount();
    await act(async () => vi.advanceTimersByTime(USAGE_RELOAD_MS));
    expect(api.callsTo("session.usage")).toHaveLength(3);

    const web = new FakeDaemonApi().on("session.usage", () => null);
    web.web = true;
    renderHook(() => useSessionUsage(web, "wsl", "s-1", { withAccount: true }));
    await act(async () => {});
    expect(web.callsTo("session.usage")).toHaveLength(1);
    expect(web.callsTo("account.get")).toHaveLength(0);
  } finally {
    vi.useRealTimers();
  }
});
