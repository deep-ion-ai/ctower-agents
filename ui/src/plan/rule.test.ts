import { describe, expect, it } from "vitest";
import { planView, refreshView, usedPercentage, windowView } from "./rule";

const H = 3_600_000;

describe("regra do reset", () => {
  it("antes do reset, o percentual vale", () => {
    const v = windowView({ used_percentage: 72, resets_at: 12 * H }, 9 * H, 10 * H);
    expect(v).toEqual({ state: "ok", used_percentage: 72, resets_at: 12 * H, read_at: 9 * H });
    expect(usedPercentage(v)).toBe(72);
  });

  it("no reset ou depois, renovada e sem percentual", () => {
    for (const now of [12 * H, 13 * H]) {
      const v = windowView({ used_percentage: 99, resets_at: 12 * H }, 9 * H, now);
      expect(v).toEqual({ state: "renewed", resets_at: 12 * H, read_at: 9 * H });
      expect(usedPercentage(v)).toBeNull();
    }
  });

  it("janelas com resets diferentes", () => {
    const v = planView(
      {
        read_at: 9 * H,
        five_hour: { used_percentage: 90, resets_at: 10 * H },
        seven_day: { used_percentage: 40, resets_at: 100 * H },
      },
      11 * H,
    );
    expect(v.five_hour.state).toBe("renewed");
    expect(v.seven_day.state).toBe("ok");
    expect(v.read_at).toBe(9 * H);
  });

  it("sem leitura", () => {
    expect(planView(null, 0)).toEqual({ five_hour: { state: "none" }, seven_day: { state: "none" }, read_at: null });
  });

  it("reaplica a uma visão antiga", () => {
    const ok = { state: "ok", used_percentage: 50, resets_at: 2 * H, read_at: H } as const;
    expect(refreshView(ok, H)).toBe(ok);
    expect(refreshView(ok, 2 * H)).toEqual({ state: "renewed", resets_at: 2 * H, read_at: H });
  });
});
