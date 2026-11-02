// Regra do reset do plano da conta (#106), igual à do `ct_core::plan`: com
// `agora >= resets_at`, a janela já renovou e o percentual antigo não vale.
// Serve para reaplicar a regra com o relógio da UI sobre uma leitura
// recebida antes (`account.get`, `hub.plan.accounts`, `session.usage`).

import type { PlanReading, PlanView, PlanWindow, WindowView } from "../protocol/gen";

/** Uma janela depois da regra do reset, em `now` (ms). */
export function windowView(w: PlanWindow | null | undefined, readAt: number, now: number): WindowView {
  if (!w) return { state: "none" };
  if (now < w.resets_at) {
    return { state: "ok", used_percentage: w.used_percentage, resets_at: w.resets_at, read_at: readAt };
  }
  return { state: "renewed", resets_at: w.resets_at, read_at: readAt };
}

/** As duas janelas de uma leitura, em `now` (ms). */
export function planView(r: PlanReading | null | undefined, now: number): PlanView {
  if (!r) return { five_hour: { state: "none" }, seven_day: { state: "none" }, read_at: null };
  return {
    five_hour: windowView(r.five_hour, r.read_at, now),
    seven_day: windowView(r.seven_day, r.read_at, now),
    read_at: r.five_hour || r.seven_day ? r.read_at : null,
  };
}

/** Reaplica a regra a uma visão já calculada (ex.: pelo hub, minutos atrás). */
export function refreshView(v: WindowView, now: number): WindowView {
  if (v.state === "ok" && now >= v.resets_at) {
    return { state: "renewed", resets_at: v.resets_at, read_at: v.read_at };
  }
  return v;
}

/** Percentual da janela, ou `null` quando não há número válido. */
export function usedPercentage(v: WindowView): number | null {
  return v.state === "ok" ? v.used_percentage : null;
}
