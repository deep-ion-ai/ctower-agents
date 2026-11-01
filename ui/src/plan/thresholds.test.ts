// Limiares configuráveis do plano (#108): validação e leitura das preferências.
import { describe, expect, it } from "vitest";
import { DEFAULT_PLAN_PREFS, planPrefs, staleError, thresholdsError, thresholdsFor, windowThresholds } from "./thresholds";

describe("limiares do plano", () => {
  it("validação: inteiros de 1 a 100, aviso menor que crítico", () => {
    expect(thresholdsError({ warn: 80, critical: 95 })).toBeNull();
    expect(thresholdsError({ warn: 1, critical: 100 })).toBeNull();
    expect(thresholdsError({ warn: 95, critical: 95 })).toBe("O aviso precisa ser menor que o crítico.");
    expect(thresholdsError({ warn: 96, critical: 95 })).toBe("O aviso precisa ser menor que o crítico.");
    expect(thresholdsError({ warn: 0, critical: 95 })).toBe("Use números inteiros de 1 a 100.");
    expect(thresholdsError({ warn: 80, critical: 101 })).toBe("Use números inteiros de 1 a 100.");
    expect(thresholdsError({ warn: 80.5, critical: 95 })).toBe("Use números inteiros de 1 a 100.");
    expect(thresholdsError({ warn: NaN, critical: 95 })).toBe("Use números inteiros de 1 a 100.");
  });

  it("prazo de leitura velha: minutos inteiros de 1 a uma semana", () => {
    expect(staleError(60)).toBeNull();
    expect(staleError(0)).not.toBeNull();
    expect(staleError(7 * 24 * 60 + 1)).not.toBeNull();
  });

  it("ui.json antigo (sem plan): os padrões 80/95 e 60 min", () => {
    expect(planPrefs(undefined)).toEqual(DEFAULT_PLAN_PREFS);
    expect(DEFAULT_PLAN_PREFS.default.five_hour).toEqual({ warn: 80, critical: 95 });
    expect(DEFAULT_PLAN_PREFS.stale_minutes).toBe(60);
    expect(DEFAULT_PLAN_PREFS.notify).toBe(true);
  });

  it("valor inválido volta ao padrão, campo a campo; conta sem janela herda o padrão", () => {
    const p = planPrefs({
      notify: false,
      stale_minutes: -3,
      default: { five_hour: { warn: 70, critical: 90 }, seven_day: { warn: 99, critical: 50 } },
      accounts: { "conta-a": { five_hour: { warn: 50, critical: 60 } } },
    });
    expect(p.notify).toBe(false);
    expect(p.stale_minutes).toBe(60);
    expect(p.default).toEqual({ five_hour: { warn: 70, critical: 90 }, seven_day: { warn: 80, critical: 95 } });
    expect(thresholdsFor(p, "conta-a")).toEqual({ five_hour: { warn: 50, critical: 60 }, seven_day: { warn: 80, critical: 95 } });
    expect(thresholdsFor(p, "conta-b")).toBe(p.default);
  });

  it("um par só vale para as duas janelas", () => {
    expect(windowThresholds({ warn: 50, critical: 60 }, "seven_day")).toEqual({ warn: 50, critical: 60 });
    expect(windowThresholds(DEFAULT_PLAN_PREFS.default, "five_hour")).toEqual({ warn: 80, critical: 95 });
  });
});
