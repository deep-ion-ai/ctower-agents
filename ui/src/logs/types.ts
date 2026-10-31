// Linhas de log da tela Logs (#65): o mesmo formato nos três lugares — o
// arquivo JSONL do ctowerd e do cthubd (`ct_http::logs::LogLine`) e o buffer
// da própria UI (`uiLog.ts`).

export type LogLevel = "error" | "warn" | "info" | "debug" | "trace";

export interface LogLine {
  /** Crescente na fonte (µs nos daemons): o cursor de `before`. */
  id: number;
  /** Momento (ms desde a época). */
  ts: number;
  level: LogLevel | string;
  /** Módulo (`ct_server::hub_sync`) ou parte da UI (`ui.react`). */
  target: string;
  message: string;
  fields?: Record<string, unknown>;
  /** Só da UI: veio do arquivo, da execução anterior do app. */
  previous?: boolean;
}

/** Resposta de `logs.tail` / `hub.logs.tail`. */
export interface TailPage {
  lines: LogLine[];
  more: boolean;
  file: string;
}

/** Push do modo seguir (`logs.lines` / `hub.logs.lines`). */
export interface LinesPush {
  lines: LogLine[];
  missed?: number;
}

export const LEVELS: LogLevel[] = ["error", "warn", "info", "debug", "trace"];

/** `error` = 1 … `trace` = 5; desconhecido fica no fim. */
export function levelRank(level: string): number {
  const i = LEVELS.indexOf(level.toLowerCase() as LogLevel);
  return i < 0 ? LEVELS.length + 1 : i + 1;
}

export const LEVEL_LABEL: Record<LogLevel, string> = {
  error: "erro",
  warn: "aviso",
  info: "info",
  debug: "debug",
  trace: "trace",
};

/** Push do modo seguir do ambiente (`logs.follow`) e do hub (`hub.logs.follow`). */
export const ENV_LOGS_PUSH = "logs.lines";
export const HUB_LOGS_PUSH = "hub.logs.lines";
