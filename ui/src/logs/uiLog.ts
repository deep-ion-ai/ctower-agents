// Log da própria UI (#65): buffer em anel em memória com `console.error` e
// `console.warn`, erros não tratados, `unhandledrejection`, falhas do
// `ErrorBoundary` e erros das chamadas ao protocolo (`logCalls`). No app
// desktop as linhas vão também para um arquivo (`ui_log_append` do Tauri) e,
// na abertura, as da execução anterior voltam ao buffer (`previous`). Na web
// fica só a memória. Tudo passa pela máscara antes de guardar.
import type { DaemonApi } from "../api/daemon";
import { mask, maskLine } from "./mask";
import type { LogLevel, LogLine } from "./types";

/** Linhas guardadas em memória. */
export const UI_LOG_CAPACITY = 2000;
/** Espera para juntar as linhas antes de gravar no arquivo. */
export const UI_LOG_FLUSH_MS = 1000;
/** Tamanho máximo de uma mensagem (argumentos enormes do console). */
const MAX_MESSAGE = 4000;

/** O que o app desktop oferece para guardar o log em arquivo. */
export interface UiLogPersist {
  uiLogAppend(lines: string[]): Promise<void>;
  uiLogRead(): Promise<string[]>;
}

type Sub = (lines: LogLine[]) => void;

export class UiLogStore {
  private lines: LogLine[] = [];
  private subs = new Set<Sub>();
  private next = 1;
  private pending: string[] = [];
  private timer: ReturnType<typeof setTimeout> | null = null;
  private persist: UiLogPersist | null = null;

  constructor(private readonly capacity = UI_LOG_CAPACITY) {}

  /** Guarda uma linha (mascarada) e avisa quem acompanha. */
  add(level: LogLevel, target: string, message: string, fields?: Record<string, unknown>): LogLine {
    const text = message.length > MAX_MESSAGE ? `${message.slice(0, MAX_MESSAGE)}…` : message;
    const line = maskLine({ id: this.next++, ts: Date.now(), level, target, message: text, fields });
    if (!line.fields || Object.keys(line.fields).length === 0) delete line.fields;
    this.push([line]);
    if (this.persist) {
      this.pending.push(JSON.stringify(line));
      this.timer ??= setTimeout(() => void this.flush(), UI_LOG_FLUSH_MS);
    }
    return line;
  }

  private push(lines: LogLine[]) {
    this.lines.push(...lines);
    if (this.lines.length > this.capacity) this.lines.splice(0, this.lines.length - this.capacity);
    for (const cb of this.subs) cb(lines);
  }

  /** Grava as linhas pendentes no arquivo; falha não vira log (nem laço). */
  async flush(): Promise<void> {
    if (this.timer) clearTimeout(this.timer);
    this.timer = null;
    const batch = this.pending.splice(0);
    if (!batch.length || !this.persist) return;
    try {
      await this.persist.uiLogAppend(batch);
    } catch {
      /* sem arquivo: fica a memória */
    }
  }

  /**
   * Liga o arquivo do desktop: lê o que ficou da execução anterior (antes
   * das linhas desta) e passa a gravar as novas, inclusive as que chegaram
   * antes de ligar.
   */
  async attach(persist: UiLogPersist): Promise<void> {
    let previous: LogLine[] = [];
    try {
      previous = (await persist.uiLogRead())
        .map((raw) => {
          try {
            const l = JSON.parse(raw) as LogLine;
            return typeof l.message === "string" && typeof l.ts === "number" ? l : null;
          } catch {
            return null;
          }
        })
        .filter((l): l is LogLine => l !== null)
        .map((l) => ({ ...maskLine(l), previous: true }));
    } catch {
      /* arquivo ilegível: segue sem a execução anterior */
    }
    const current = this.lines;
    // Ids da execução anterior abaixo dos desta (que começam em 1).
    const base = -previous.length;
    this.lines = [...previous.map((l, i) => ({ ...l, id: base + i })), ...current].slice(-this.capacity);
    this.persist = persist;
    this.pending.push(...current.map((l) => JSON.stringify(l)));
    if (this.pending.length) this.timer ??= setTimeout(() => void this.flush(), UI_LOG_FLUSH_MS);
    for (const cb of this.subs) cb([]);
  }

  /** Cópia do buffer, da mais antiga para a mais nova. */
  snapshot(): LogLine[] {
    return this.lines.slice();
  }

  /** Linhas novas (`[]` = o buffer mudou por inteiro: releia o `snapshot`). */
  subscribe(cb: Sub): () => void {
    this.subs.add(cb);
    return () => this.subs.delete(cb);
  }

  /** Só para os testes. */
  reset() {
    this.lines = [];
    this.pending = [];
    this.persist = null;
    if (this.timer) clearTimeout(this.timer);
    this.timer = null;
    this.next = 1;
  }
}

/** O log da UI deste processo. */
export const uiLog = new UiLogStore();

/** Texto de um argumento do console ou de um erro. */
export function describe(v: unknown): string {
  if (v instanceof Error) return v.message || v.name;
  if (typeof v === "string") return v;
  if (v === undefined) return "undefined";
  try {
    return JSON.stringify(v) ?? String(v);
  } catch {
    return String(v);
  }
}

function errorFields(args: unknown[]): Record<string, unknown> | undefined {
  const err = args.find((a): a is Error => a instanceof Error);
  return err?.stack ? { stack: mask(err.stack) } : undefined;
}

/** O console original, antes do `installUiLog` (para quem registra por conta própria). */
export const rawConsole: Pick<Console, "error" | "warn"> = {
  error: (...a: unknown[]) => console.error(...a),
  warn: (...a: unknown[]) => console.warn(...a),
};

let installed = false;

/**
 * Passa a registrar `console.error/warn`, `error` e `unhandledrejection` da
 * janela. Com `persist` (desktop), grava também em arquivo. Uma vez por
 * processo; chamado só nas entradas (`main.tsx`, `main.web.tsx`).
 */
export function installUiLog(opts: { persist?: UiLogPersist; target?: Window } = {}): void {
  if (installed) return;
  installed = true;
  const w = opts.target ?? window;
  const origError = console.error.bind(console);
  const origWarn = console.warn.bind(console);
  rawConsole.error = origError;
  rawConsole.warn = origWarn;
  console.error = (...args: unknown[]) => {
    origError(...args);
    uiLog.add("error", "ui.console", args.map(describe).join(" "), errorFields(args));
  };
  console.warn = (...args: unknown[]) => {
    origWarn(...args);
    uiLog.add("warn", "ui.console", args.map(describe).join(" "), errorFields(args));
  };
  w.addEventListener("error", (e: ErrorEvent) => {
    const where = e.filename ? { file: e.filename, line: e.lineno, col: e.colno } : {};
    uiLog.add("error", "ui.window", `Erro não tratado: ${e.message || describe(e.error)}`, {
      ...where,
      ...(errorFields([e.error]) ?? {}),
    });
  });
  w.addEventListener("unhandledrejection", (e: PromiseRejectionEvent) => {
    uiLog.add("error", "ui.window", `Promessa rejeitada sem tratamento: ${describe(e.reason)}`, errorFields([e.reason]));
  });
  uiLog.add("info", "ui", "app aberto", { agent: navigator.userAgent });
  if (opts.persist) void uiLog.attach(opts.persist);
}

/**
 * Registra no log da UI as chamadas ao protocolo que falham (`call` e
 * `hubCall`), sem mudar o resultado. Altera a própria `api` e a devolve.
 */
export function logCalls<T extends DaemonApi>(api: T): T {
  const call = api.call.bind(api);
  const hubCall = api.hubCall.bind(api);
  api.call = (<R>(env: string, method: string, params?: object) =>
    call<R>(env, method, params).catch((e: unknown) => {
      uiLog.add("warn", "ui.protocol", `${env} › ${method}: ${describe(e)}`, { env, method });
      throw e;
    })) as T["call"];
  api.hubCall = (<R>(method: string, params?: object) =>
    hubCall<R>(method, params).catch((e: unknown) => {
      uiLog.add("warn", "ui.protocol", `hub › ${method}: ${describe(e)}`, { env: "hub", method });
      throw e;
    })) as T["hubCall"];
  return api;
}
