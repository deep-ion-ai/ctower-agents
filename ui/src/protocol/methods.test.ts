import { describe, expect, it } from "vitest";
import type { DaemonApi } from "../api/daemon";
import { METHOD_SCOPES } from "./gen/methods";

describe("registro de métodos do ambiente (gerado de ct_http::methods)", () => {
  it("os escopos de web e de canal estão em ui, e web é o canal", () => {
    const ui = new Set<string>(METHOD_SCOPES.ui);
    for (const m of [...METHOD_SCOPES.web, ...METHOD_SCOPES.channel]) {
      expect(ui.has(m), m).toBe(true);
    }
    expect(new Set(METHOD_SCOPES.web)).toEqual(new Set(METHOD_SCOPES.channel));
  });

  it("não há método repetido em um escopo", () => {
    for (const list of [METHOD_SCOPES.ui, METHOD_SCOPES.web, METHOD_SCOPES.channel]) {
      expect(new Set(list).size).toBe(list.length);
    }
  });
});

/**
 * Checagem de tipos (roda no `tsc`, não executa): o `call` tipado devolve o
 * resultado do registro. Se a sobrecarga tipada não casar, cai na genérica
 * e `env_name` deixa de existir para o compilador.
 */
export async function tiposDoCallTipado(api: DaemonApi): Promise<string> {
  const nome: string = (await api.call("env", "env.info")).env_name;
  const fila: string[] = (await api.call("env", "projects.list")).map((p) => p.name);
  return `${nome}${fila.length}`;
}
