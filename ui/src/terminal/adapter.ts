import type { DaemonApi } from "../api/daemon";
import type { TermChunk } from "../api/types";
import type { Session, SessionState } from "../protocol/gen";

/** O que o adapter usa do xterm (um fake nos testes). */
export interface TermLike {
  write(data: Uint8Array | string): void;
  reset(): void;
  onData(cb: (data: string) => void): { dispose(): void };
}

export const RESIZE_DEBOUNCE_MS = 150;
/** Intervalo entre as duas metades do resize de redesenho. */
export const REDRAW_DELAY_MS = 50;

export interface TermSize {
  cols: number;
  rows: number;
}

export type TermState =
  | { kind: "attaching" }
  | { kind: "live" }
  /**
   * `recoverable`: a sessão está na Recuperação (pode ser retomada).
   * `clean`: o agente saiu por conta própria, sem erro (`/exit`, código 0),
   * visto pelo push do fim; a aba pode fechar sozinha (#432).
   */
  | { kind: "ended"; message: string; recoverable?: boolean; clean?: boolean };

/** `session.get`: a sessão e se o work item dela já concluiu (#297). */
export type SessionInfo = Session & { work_item_concluded?: boolean };

/**
 * Como a sessão terminou, pelo estado final e o código de saída (#283).
 * `concluded`: a issue da sessão já concluiu, então a Recuperação não a lista
 * (#285) e a aba não a oferece (#297).
 */
export function endOf(
  state: SessionState | null | undefined,
  code: number | null,
  concluded = false,
): { message: string; recoverable: boolean } {
  const how = code === null ? "sinal" : `código ${code}`;
  switch (state) {
    case "interrupted":
      if (concluded)
        return { message: `sessão interrompida (${how}). Issue concluída; a sessão será arquivada pela limpeza`, recoverable: false };
      return { message: `sessão interrompida (${how}). Retome pela Recuperação`, recoverable: true };
    case "archived":
      return { message: "sessão arquivada", recoverable: false };
    default:
      return { message: `sessão encerrada (${how})`, recoverable: false };
  }
}

/**
 * Texto do fim do shell (`TerminalView`): com o código do push
 * `shell_exited`, "shell encerrado (código n)"; `null` = sinal; sem o código
 * (o attach falhou porque o shell já tinha saído), só "shell encerrado".
 */
export function shellEndText(code?: number | null): string {
  if (code === undefined) return "shell encerrado";
  return `shell encerrado (${code === null ? "sinal" : `código ${code}`})`;
}

/** O que roda no PTY: muda só o texto do fim ("sessão encerrada" ou "shell encerrado"). */
export type PtyKind = "session" | "shell";

/**
 * Liga um terminal a uma sessão (ou a um shell, #423): saída do canal → `write` (snapshot limpa
 * antes, para o re-attach após reconexão não duplicar a saída), teclado →
 * `input`, tamanho → `resize` com debounce. Depois que a sessão termina, a
 * entrada é descartada.
 *
 * O PTY recebe o tamanho real antes do attach, e um snapshot com histórico
 * dispara um resize de ida e volta para o agente redesenhar a tela inteira
 * nesse tamanho (o replay é de bytes crus, gerados para outra largura).
 */
export class TerminalAdapter {
  state: TermState = { kind: "attaching" };
  private sub: { dispose(): void } | null = null;
  private resizeTimer: ReturnType<typeof setTimeout> | null = null;
  private redrawTimer: ReturnType<typeof setTimeout> | null = null;
  private size: TermSize | null = null;
  private disposed = false;

  constructor(
    private api: DaemonApi,
    private env: string,
    private session: string,
    private term: TermLike,
    private onState: (s: TermState) => void = () => {},
    private kind: PtyKind = "session",
  ) {}

  async start(size?: TermSize): Promise<void> {
    this.sub = this.term.onData((d) => this.send(d));
    if (size) {
      this.size = size;
      // Sem debounce: o PTY precisa estar no tamanho certo antes do replay.
      await this.api.resize(this.env, this.session, size.cols, size.rows).catch(() => {});
      if (this.disposed) return;
    }
    try {
      await this.api.attach(this.env, this.session, (c) => this.chunk(c));
      if (!this.disposed && this.state.kind === "attaching") this.set({ kind: "live" });
    } catch (e) {
      await this.attachFailed(e);
    }
  }

  /**
   * O attach falhou: se a sessão já terminou, mostra como (código, motivo) em
   * vez do erro cru; o erro só aparece se a sessão ainda consta como viva ou
   * não pôde ser lida.
   */
  private async attachFailed(e: unknown) {
    if (this.kind === "shell") {
      // O shell sai do daemon quando termina: sem replay nem código.
      if (this.disposed) return;
      this.end(/não está rodando/.test(String(e)) ? shellEndText() : `não foi possível anexar: ${String(e)}`);
      return;
    }
    const s = await this.api
      .call<SessionInfo>(this.env, "session.get", { session_id: this.session })
      .catch(() => null);
    if (this.disposed) return;
    if (s && (s.state === "exited" || s.state === "interrupted" || s.state === "archived")) {
      const { message, recoverable } = endOf(s.state, s.exit_code, s.work_item_concluded);
      this.end(message, recoverable);
    } else {
      this.end(`não foi possível anexar: ${String(e)}`);
    }
  }

  chunk(c: TermChunk) {
    if (this.disposed) return;
    if (c.snapshot) this.term.reset();
    this.term.write(c.bytes);
    if (this.state.kind === "attaching") this.set({ kind: "live" });
    if (c.snapshot && c.bytes.length > 0) this.redraw();
  }

  /** Resize de ida e volta (`rows-1` → `rows`): o agente redesenha a tela. */
  private redraw() {
    const size = this.size;
    if (!size || size.rows < 2) return;
    if (this.redrawTimer) clearTimeout(this.redrawTimer);
    this.api.resize(this.env, this.session, size.cols, size.rows - 1).catch(() => {});
    this.redrawTimer = setTimeout(() => {
      this.redrawTimer = null;
      if (this.state.kind === "ended" || this.disposed) return;
      const now = this.size ?? size;
      this.api.resize(this.env, this.session, now.cols, now.rows).catch(() => {});
    }, REDRAW_DELAY_MS);
  }

  send(data: string) {
    if (this.state.kind === "ended" || this.disposed) return;
    this.api.input(this.env, this.session, data).catch(() => {
      // Desconectado: o status do ambiente já aparece; a tecla se perde.
    });
  }

  resize(cols: number, rows: number) {
    this.size = { cols, rows };
    if (this.resizeTimer) clearTimeout(this.resizeTimer);
    this.resizeTimer = setTimeout(() => {
      this.resizeTimer = null;
      if (this.state.kind === "ended" || this.disposed) return;
      this.api.resize(this.env, this.session, cols, rows).catch(() => {});
    }, RESIZE_DEBOUNCE_MS);
  }

  /** A sessão saiu: avisa no terminal e para de aceitar entrada. */
  async exited(code: number | null, state?: SessionState | null) {
    if (this.kind === "shell") {
      this.end(shellEndText(code));
      return;
    }
    let concluded = false;
    if (state === "interrupted") {
      // O push não diz se a issue concluiu: pergunta ao daemon (#297).
      const s = await this.api
        .call<SessionInfo>(this.env, "session.get", { session_id: this.session })
        .catch(() => null);
      concluded = s?.work_item_concluded === true;
    }
    if (this.disposed) return;
    const { message, recoverable } = endOf(state, code, concluded);
    this.end(message, recoverable, state === "exited" && code === 0);
  }

  end(message: string, recoverable = false, clean = false) {
    if (this.state.kind === "ended") return;
    this.term.write(`\r\n\x1b[2m[${message}]\x1b[0m\r\n`);
    this.set(clean ? { kind: "ended", message, recoverable, clean } : { kind: "ended", message, recoverable });
  }

  dispose() {
    if (this.disposed) return;
    this.disposed = true;
    this.sub?.dispose();
    if (this.resizeTimer) clearTimeout(this.resizeTimer);
    if (this.redrawTimer) clearTimeout(this.redrawTimer);
    if (this.state.kind !== "ended") this.api.detach(this.env, this.session).catch(() => {});
  }

  private set(s: TermState) {
    this.state = s;
    this.onState(s);
  }
}
