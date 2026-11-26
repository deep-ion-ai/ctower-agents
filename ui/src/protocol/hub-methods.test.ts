import { describe, expect, it } from "vitest";
import type { DaemonApi } from "../api/daemon";
import { HUB_METHOD_SCOPES } from "./hub-gen/methods";

describe("registro de métodos do hub (gerado de crates/cthubd/src/methods.rs)", () => {
  it("a web é um subconjunto da UI do app, sem método repetido", () => {
    const ui = new Set<string>(HUB_METHOD_SCOPES.ui);
    for (const m of HUB_METHOD_SCOPES.web) {
      expect(ui.has(m), m).toBe(true);
    }
    expect(new Set(HUB_METHOD_SCOPES.web).size).toBe(HUB_METHOD_SCOPES.web.length);
    expect(new Set(HUB_METHOD_SCOPES.ui).size).toBe(HUB_METHOD_SCOPES.ui.length);
  });

  it("a web não tem métodos de escrita de configuração nem sensíveis", () => {
    const web = new Set<string>(HUB_METHOD_SCOPES.web);
    for (const m of [
      "hub.projects.delete",
      "hub.projects.add",
      "hub.secrets.set",
      "hub.secrets.delete",
      "github.set_token",
      "github.clear_token",
    ]) {
      expect(web.has(m), m).toBe(false);
    }
  });
});

/**
 * Checagem de tipos (roda no `tsc`, não executa): o `hubCall` tipado devolve o
 * resultado do registro. Se a sobrecarga tipada não casar, cai na genérica e
 * `name` deixa de ser `string` para o compilador.
 */
export async function tiposDoHubCall(api: DaemonApi): Promise<string> {
  const nome: string = (await api.hubCall("hub.info")).name;
  const contas: number = (await api.hubCall("hub.plan.accounts")).accounts.length;
  return `${nome}${contas}`;
}
