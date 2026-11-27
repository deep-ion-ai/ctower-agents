import { describe, expect, it } from "vitest";
import type { EnvStatus } from "../api/types";
import { mergeHubStatus } from "./hubStatus";

const conn = { url: "ws://hub.example.com:7879/ws", has_token: true };
const st = (connected: boolean, since: number): EnvStatus => ({
  env: "hub",
  connected,
  error: null,
  since,
});

describe("mergeHubStatus", () => {
  it("leitura atrasada não apaga o 'conectado' que chegou por evento", () => {
    // reloadHub começa em 100; o evento chega em 105; a leitura (feita
    // antes do handshake) volta vazia depois dele.
    expect(mergeHubStatus(st(true, 105), null, conn, 100)).toEqual(st(true, 105));
  });

  it("leitura mais nova que o evento vale", () => {
    expect(mergeHubStatus(st(true, 105), st(false, 110), conn, 100)).toEqual(st(false, 110));
    expect(mergeHubStatus(st(false, 110), st(true, 105), conn, 100)).toEqual(st(false, 110));
  });

  it("status anterior à releitura não sobrevive", () => {
    // De uma conexão anterior (outro hub, ou antes de remover): vale a leitura.
    expect(mergeHubStatus(st(true, 50), null, conn, 100)).toBeNull();
    expect(mergeHubStatus(st(true, 50), st(false, 101), conn, 100)).toEqual(st(false, 101));
  });

  it("sem conexão configurada não há status", () => {
    expect(mergeHubStatus(st(true, 105), st(true, 106), null, 100)).toBeNull();
  });
});
