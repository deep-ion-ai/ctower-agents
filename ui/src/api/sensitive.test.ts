import { beforeEach, describe, expect, it, vi } from "vitest";
import { SENSITIVE_COMMANDS, TauriDaemonApi } from "./daemon";
// Fonte da ponte do desktop: a lista de sensíveis é conferida contra ela.
import gateSrc from "../../src-tauri/src/gate.rs?raw";

const { invoke } = vi.hoisted(() => ({ invoke: vi.fn(async () => ({})) }));
vi.mock("@tauri-apps/api/core", () => ({ invoke, Channel: class {} }));

/** `SENSITIVE_ENV` de `gate.rs`: pares (método, comando dedicado). */
function rustSensitive(): [string, string][] {
  const block = gateSrc.slice(gateSrc.indexOf("pub const SENSITIVE_ENV"));
  const body = block.slice(0, block.indexOf("];"));
  return [...body.matchAll(/\("([^"]+)",\s*"([^"]+)"\)/g)].map((m) => [m[1], m[2]]);
}

describe("métodos sensíveis do ambiente (#231)", () => {
  beforeEach(() => invoke.mockClear());

  it("a tabela da UI é a do gate.rs", () => {
    const rust = rustSensitive();
    expect(rust.length).toBeGreaterThan(0);
    expect([...SENSITIVE_COMMANDS.entries()].sort()).toEqual([...rust].sort());
  });

  it("sensível vai pelo comando dedicado, com o método fixo", async () => {
    await new TauriDaemonApi().call("env-a", "env.update", { force: true });
    expect(invoke).toHaveBeenCalledWith("env_update", { env: "env-a", params: { force: true } });
  });

  it("o resto segue pelo genérico", async () => {
    await new TauriDaemonApi().call("env-a", "env.info");
    expect(invoke).toHaveBeenCalledWith("daemon_call", { env: "env-a", method: "env.info", params: {} });
  });

  it("nome herdado de Object não vira comando dedicado", async () => {
    await new TauriDaemonApi().call("env-a", "constructor");
    expect(invoke).toHaveBeenCalledWith("daemon_call", { env: "env-a", method: "constructor", params: {} });
  });
});
