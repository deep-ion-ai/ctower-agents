/**
 * Erro de uma chamada ao daemon ou ao hub, igual na web e no desktop.
 *
 * `code` é o do erro JSON-RPC (ou um código da própria ponte do desktop,
 * `BRIDGE_*` em `src-tauri/src/bridge.rs`, com os mesmos números que o hub
 * usa para os casos equivalentes). Ausente = falha sem código (ex.: um
 * comando do desktop que ainda devolve só texto).
 */
export class ApiError extends Error {
  readonly code: number | undefined;
  readonly data: unknown;

  constructor(message: string, code?: number, data?: unknown) {
    super(message);
    this.name = "ApiError";
    this.code = code;
    this.data = data;
  }

  /** Como antes: a mensagem, sem o prefixo "Error:" (o texto é o que a tela mostra). */
  override toString(): string {
    return this.message;
  }
}

/** Códigos que a UI conhece (JSON-RPC do daemon/hub e da ponte do desktop). */
export const RPC_CODES = {
  /** Ambiente sem canal com o hub, ou desconectado no desktop. */
  ENV_OFFLINE: -32030,
  /** O ambiente não respondeu no prazo. */
  TIMEOUT: -32031,
  /** Log em arquivo desligado no daemon. */
  LOGS_DISABLED: -32040,
  /** Branch de integração do modo autônomo em conflito com a base. */
  AUTONOMY_CONFLICT: -32050,
  /** Limite de trabalho em andamento atingido. */
  WIP_LIMIT: -32051,
  /** Issue sem o rótulo de aprovação exigido pelo projeto. */
  APPROVAL_REQUIRED: -32052,
  /** Já há uma sessão de release viva no projeto. */
  RELEASE_BUSY: -32053,
  /** Método fora do que o lado de lá aceita. */
  METHOD_NOT_FOUND: -32601,
  /** Parâmetros inválidos (ou ambiente desconhecido, no desktop). */
  INVALID_PARAMS: -32602,
  /** Erro genérico do servidor. */
  INTERNAL: -32000,
} as const;

/** Categoria de um erro, para a tela decidir a mensagem. */
export type ErrorKind =
  | "offline"
  | "timeout"
  | "logs_disabled"
  | "autonomy_conflict"
  | "wip_limit"
  | "approval_required"
  | "release_busy"
  | "method_not_found"
  | "invalid_params"
  | "other";

const KIND_BY_CODE: Partial<Record<number, ErrorKind>> = {
  [RPC_CODES.ENV_OFFLINE]: "offline",
  [RPC_CODES.TIMEOUT]: "timeout",
  [RPC_CODES.LOGS_DISABLED]: "logs_disabled",
  [RPC_CODES.AUTONOMY_CONFLICT]: "autonomy_conflict",
  [RPC_CODES.WIP_LIMIT]: "wip_limit",
  [RPC_CODES.APPROVAL_REQUIRED]: "approval_required",
  [RPC_CODES.RELEASE_BUSY]: "release_busy",
  [RPC_CODES.METHOD_NOT_FOUND]: "method_not_found",
  [RPC_CODES.INVALID_PARAMS]: "invalid_params",
};

/**
 * Converte o que chegou numa rejeição (um `ApiError`, o objeto `{ code,
 * message, data }` da ponte do desktop, um texto ou um `Error`) em `ApiError`.
 */
export function toApiError(e: unknown): ApiError {
  if (e instanceof ApiError) return e;
  if (typeof e === "string") return new ApiError(e);
  if (e && typeof e === "object" && "message" in e && typeof (e as { message: unknown }).message === "string") {
    const { message, code, data } = e as { message: string; code?: unknown; data?: unknown };
    return new ApiError(message, typeof code === "number" ? code : undefined, data);
  }
  if (e instanceof Error) return new ApiError(e.message);
  return new ApiError(String(e));
}

/** Código do erro, se houver. */
export function errorCode(e: unknown): number | undefined {
  return toApiError(e).code;
}

/** Categoria do erro pelo código; `other` quando não é um código conhecido. */
export function errorKind(e: unknown): ErrorKind {
  const code = errorCode(e);
  return (code !== undefined && KIND_BY_CODE[code]) || "other";
}
