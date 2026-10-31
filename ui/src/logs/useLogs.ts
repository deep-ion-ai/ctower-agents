// Estado da tela Logs (#65) para uma fonte: as últimas linhas (`logs.tail`),
// as mais antigas sob demanda (`before`) e o modo seguir (`logs.follow` +
// push), com pausa. A UI lê o próprio buffer (`uiLog`).
import { useCallback, useEffect, useRef, useState } from "react";
import type { DaemonApi } from "../api/daemon";
import { useTower } from "../state/tower";
import { appendLines, matches, sourceKey, tailParams, type LogFilter, type LogSource } from "./sources";
import { ENV_LOGS_PUSH, HUB_LOGS_PUSH, type LinesPush, type LogLine, type TailPage } from "./types";
import { uiLog } from "./uiLog";

/** Linhas pedidas por página. */
export const PAGE = 200;
/** Linhas mantidas na tela; as mais antigas saem (e voltam por "Carregar anteriores"). */
export const MAX_SHOWN = 5000;

/**
 * Quantas telas seguem cada fonte: a conexão (do app com o daemon) é uma
 * só, então `follow` vai na primeira e `unfollow` na última.
 */
const followers = new Map<string, number>();

function followCall(api: DaemonApi, s: LogSource, on: boolean): Promise<unknown> {
  if (s.kind === "hub") return api.hubCall(on ? "hub.logs.follow" : "hub.logs.unfollow");
  if (s.kind === "env") return api.call(s.env, on ? "logs.follow" : "logs.unfollow");
  return Promise.resolve();
}

async function acquire(api: DaemonApi, s: LogSource) {
  const k = sourceKey(s);
  const n = followers.get(k) ?? 0;
  followers.set(k, n + 1);
  if (n === 0) await followCall(api, s, true);
}

function release(api: DaemonApi, s: LogSource) {
  const k = sourceKey(s);
  const n = (followers.get(k) ?? 1) - 1;
  if (n > 0) {
    followers.set(k, n);
    return;
  }
  followers.delete(k);
  void followCall(api, s, false).catch(() => {});
}

/** Só para os testes. */
export const resetFollowers = () => followers.clear();

export interface LogsState {
  lines: LogLine[];
  /** Há linhas mais antigas para "Carregar anteriores". */
  more: boolean;
  loading: boolean;
  /** Mensagem do último erro da fonte. */
  error: string | null;
  /** Arquivo do daemon (`logs.tail`). */
  file: string | null;
  /** Linhas que chegaram com a pausa ligada. */
  pending: number;
  /** Linhas perdidas no modo seguir (o daemon mandou mais que a conexão levou). */
  missed: number;
  loadOlder(): Promise<void>;
  reload(): void;
}

/**
 * `ready` = a fonte está alcançável agora (hub ou ambiente conectado); muda
 * de `false` para `true` ao reconectar, o que recarrega e volta a seguir.
 */
export function useLogs(source: LogSource, filter: LogFilter, opts: { follow: boolean; paused: boolean; ready: boolean }): LogsState {
  const { api } = useTower();
  const key = sourceKey(source);
  const [lines, setLines] = useState<LogLine[]>([]);
  const [more, setMore] = useState(false);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [file, setFile] = useState<string | null>(null);
  const [pending, setPending] = useState<LogLine[]>([]);
  const [missed, setMissed] = useState(0);
  const [nonce, setNonce] = useState(0);
  const filterRef = useRef(filter);
  filterRef.current = filter;
  const pausedRef = useRef(opts.paused);
  pausedRef.current = opts.paused;
  // Fonte da página atual: resposta de uma fonte antiga não vale.
  const gen = useRef(0);

  const tail = useCallback(
    (params: Record<string, unknown>): Promise<TailPage> =>
      source.kind === "hub"
        ? api.hubCall<TailPage>("hub.logs.tail", params)
        : api.call<TailPage>((source as { env: string }).env, "logs.tail", params),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [api, key],
  );

  // Primeira página (e a cada troca de fonte, filtro ou reconexão).
  useEffect(() => {
    const g = ++gen.current;
    setPending([]);
    setMissed(0);
    if (source.kind === "ui") {
      setLines(uiLog.snapshot().filter((l) => matches(l, filter)).slice(-MAX_SHOWN));
      setMore(false);
      setError(null);
      setFile(null);
      setLoading(false);
      return;
    }
    if (!opts.ready) {
      setLoading(false);
      return;
    }
    setLoading(true);
    tail(tailParams(filter, { limit: PAGE }))
      .then((p) => {
        if (g !== gen.current) return;
        setLines(p.lines);
        setMore(p.more);
        setFile(p.file);
        setError(null);
      })
      .catch((e: unknown) => {
        if (g !== gen.current) return;
        setLines([]);
        setMore(false);
        setError(e instanceof Error ? e.message : String(e));
      })
      .finally(() => {
        if (g === gen.current) setLoading(false);
      });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key, filter.level, filter.q, opts.ready, nonce, tail]);

  // Modo seguir.
  useEffect(() => {
    if (!opts.follow) return;
    const add = (incoming: LogLine[]) => {
      const fresh = incoming.filter((l) => matches(l, filterRef.current));
      if (!fresh.length) return;
      if (pausedRef.current) setPending((p) => appendLines(p, fresh, MAX_SHOWN).lines);
      else setLines((cur) => appendLines(cur, fresh, MAX_SHOWN).lines);
    };
    if (source.kind === "ui") {
      return uiLog.subscribe((l) => {
        // `[]`: o buffer mudou por inteiro (chegou a execução anterior).
        if (l.length) add(l);
        else setLines(uiLog.snapshot().filter((x) => matches(x, filterRef.current)).slice(-MAX_SHOWN));
      });
    }
    if (!opts.ready) return;
    const onPush = (data: LinesPush) => {
      if (data.missed) setMissed((m) => m + (data.missed ?? 0));
      if (Array.isArray(data.lines)) add(data.lines);
    };
    const off =
      source.kind === "hub"
        ? api.onHubPush((e) => {
            if (e.event === HUB_LOGS_PUSH) onPush(e.data as LinesPush);
          })
        : api.onPush((e) => {
            if (e.event === ENV_LOGS_PUSH && e.env === (source as { env: string }).env) onPush(e.data as LinesPush);
          });
    let held = true;
    acquire(api, source).catch((e: unknown) => {
      held = false;
      release(api, source);
      setError(e instanceof Error ? e.message : String(e));
    });
    return () => {
      off();
      if (held) release(api, source);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [api, key, opts.follow, opts.ready]);

  // Despausar: as linhas guardadas entram na lista.
  useEffect(() => {
    if (opts.paused || !pending.length) return;
    setLines((cur) => appendLines(cur, pending, MAX_SHOWN).lines);
    setPending([]);
  }, [opts.paused, pending]);

  const loadOlder = useCallback(async () => {
    if (source.kind === "ui" || !lines.length) return;
    const g = gen.current;
    setLoading(true);
    try {
      const p = await tail(tailParams(filterRef.current, { limit: PAGE, before: lines[0].id }));
      if (g !== gen.current) return;
      setLines((cur) => [...p.lines, ...cur]);
      setMore(p.more);
    } catch (e) {
      if (g === gen.current) setError(e instanceof Error ? e.message : String(e));
    } finally {
      if (g === gen.current) setLoading(false);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key, lines, tail]);

  return {
    lines,
    more,
    loading,
    error,
    file,
    pending: pending.length,
    missed,
    loadOlder,
    reload: () => setNonce((n) => n + 1),
  };
}
