import { describe, expect, it } from "vitest";
import { ApiError, errorCode, errorKind, RPC_CODES, toApiError } from "./errors";

describe("toApiError", () => {
  it("mantém o code e o data do objeto que a ponte do desktop manda", () => {
    const e = toApiError({ code: -32052, message: "aguardando aprovação", data: { label: "aprovada" } });
    expect(e).toBeInstanceOf(ApiError);
    expect(e.code).toBe(-32052);
    expect(e.message).toBe("aguardando aprovação");
    expect(e.data).toEqual({ label: "aprovada" });
  });

  it("texto e Error sem código ficam sem code", () => {
    expect(toApiError("falhou").code).toBeUndefined();
    expect(toApiError("falhou").message).toBe("falhou");
    expect(toApiError(new Error("x")).message).toBe("x");
    expect(toApiError(new Error("x")).code).toBeUndefined();
    expect(toApiError(42).message).toBe("42");
  });

  it("um ApiError passa como está", () => {
    const e = new ApiError("m", -32031);
    expect(toApiError(e)).toBe(e);
  });

  it("toString é só a mensagem (a tela mostra o texto, não o nome da classe)", () => {
    expect(String(new ApiError("ambiente offline: mac"))).toBe("ambiente offline: mac");
  });
});

describe("errorKind", () => {
  it("mapeia os códigos conhecidos, iguais na web e no desktop", () => {
    expect(errorKind({ code: RPC_CODES.ENV_OFFLINE, message: "" })).toBe("offline");
    expect(errorKind(new ApiError("", RPC_CODES.TIMEOUT))).toBe("timeout");
    expect(errorKind(new ApiError("", RPC_CODES.WIP_LIMIT))).toBe("wip_limit");
    expect(errorKind(new ApiError("", RPC_CODES.APPROVAL_REQUIRED))).toBe("approval_required");
    expect(errorKind(new ApiError("", RPC_CODES.RELEASE_BUSY))).toBe("release_busy");
    expect(errorKind(new ApiError("", RPC_CODES.LOGS_DISABLED))).toBe("logs_disabled");
    expect(errorKind(new ApiError("", RPC_CODES.METHOD_NOT_FOUND))).toBe("method_not_found");
  });

  it("código desconhecido ou ausente é other", () => {
    expect(errorKind(new ApiError("x", -99999))).toBe("other");
    expect(errorKind(new Error("x"))).toBe("other");
    expect(errorCode(new Error("x"))).toBeUndefined();
  });
});
