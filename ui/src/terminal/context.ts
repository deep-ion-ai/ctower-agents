// Uso de contexto da sessão ativa na barra de status (#365): o daemon lê os
// tokens da última resposta no transcript (`session.context`); o tamanho da
// janela fica com a UI, pelo modelo pedido ao iniciar.
import { useEffect, useState } from "react";
import type { DaemonApi } from "../api/daemon";
import type { ContextUsage } from "../protocol/gen";

/** Janela padrão do Claude Code. */
export const DEFAULT_WINDOW = 200_000;
/** Janela estendida (`[1m]` no alias do modelo, ex. `sonnet[1m]`). */
export const LONG_WINDOW = 1_000_000;

/** Tamanho da janela pelo modelo da sessão (alias ou id); sem modelo, o padrão. */
export function contextWindow(model: string | null | undefined): number {
  return model && /\[1m\]/i.test(model) ? LONG_WINDOW : DEFAULT_WINDOW;
}

const k = (n: number) => (n >= 1_000_000 ? `${+(n / 1_000_000).toFixed(1)}M` : `${Math.round(n / 1000)}k`);

/** "Contexto 45% · 90k/200k"; `null` sem resposta no transcript. */
export function contextText(tokens: number | null, window: number): string | null {
  if (tokens === null) return null;
  const pct = Math.min(100, Math.round((tokens / window) * 100));
  return `Contexto ${pct}% · ${k(tokens)}/${k(window)}`;
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
