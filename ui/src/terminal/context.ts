// Uso de contexto da sessão ativa na barra de status (#365): o daemon lê os
// tokens da última resposta no transcript (`session.context`). Com a leitura
// do statusline (`session.usage`, #110), a janela é a real dela; sem leitura,
// um palpite da UI pelo modelo pedido ao iniciar.
import { useEffect, useState } from "react";
import type { DaemonApi } from "../api/daemon";
import type { ContextUsage, SessionUsage } from "../protocol/gen";

/** Janela padrão do Claude Code. */
export const DEFAULT_WINDOW = 200_000;
/** Janela estendida (`[1m]` no alias do modelo, ex. `sonnet[1m]`). */
export const LONG_WINDOW = 1_000_000;

/**
 * Tamanho da janela: o real da leitura de uso, quando há; senão o palpite
 * pelo modelo da sessão (alias ou id), e sem modelo o padrão.
 */
export function contextWindow(model: string | null | undefined, reading?: SessionUsage | null): number {
  const real = reading?.context_window?.context_window_size;
  if (real && real > 0) return real;
  return model && /\[1m\]/i.test(model) ? LONG_WINDOW : DEFAULT_WINDOW;
}

const k = (n: number) => (n >= 1_000_000 ? `${+(n / 1_000_000).toFixed(1)}M` : `${Math.round(n / 1000)}k`);

/** "45% · 90k/200k"; `null` sem tokens. */
export function contextFigures(tokens: number | null, window: number): string | null {
  if (tokens === null) return null;
  const pct = Math.min(100, Math.round((tokens / window) * 100));
  return `${pct}% · ${k(tokens)}/${k(window)}`;
}

/** "Contexto 45% · 90k/200k"; `null` sem resposta no transcript. */
export function contextText(tokens: number | null, window: number): string | null {
  const f = contextFigures(tokens, window);
  return f === null ? null : `Contexto ${f}`;
}

/**
 * Tokens no contexto: os da fonte mais recente entre o transcript e a
 * leitura de uso (a sem carimbo perde para a com carimbo).
 */
export function contextTokens(ctx: ContextUsage | null, reading: SessionUsage | null): number | null {
  const fromReading = reading?.context_window?.used_tokens ?? null;
  const fromTranscript = ctx?.tokens ?? null;
  if (fromReading === null) return fromTranscript;
  if (fromTranscript === null) return fromReading;
  const at = ctx?.ts ? Date.parse(ctx.ts) : NaN;
  return Number.isFinite(at) && at > reading!.written_at ? fromTranscript : fromReading;
}

/**
 * Tokens da sessão: lidos ao trocar de sessão e a cada mudança de estado dela
 * (push `session_state`, ex. o fim de uma resposta no Stop). Daemon anterior
 * ao método ou sessão sem transcript: `null`.
 */
export function useContextUsage(api: DaemonApi, env: string | null, session: string | null): ContextUsage | null {
  const [usage, setUsage] = useState<ContextUsage | null>(null);
  useEffect(() => {
    setUsage(null);
    if (!env || !session) return;
    let alive = true;
    const load = () =>
      api.call<ContextUsage>(env, "session.context", { session_id: session }).then(
        (u) => alive && setUsage(u),
        () => alive && setUsage(null),
      );
    void load();
    const off = api.onPush((e) => {
      if (e.env === env && e.event === "session_state" && e.data?.session_id === session) void load();
    });
    return () => {
      alive = false;
      off();
    };
  }, [api, env, session]);
  return usage;
}
