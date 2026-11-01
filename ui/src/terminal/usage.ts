// Leitura de uso da sessão (`session.usage`, #106) para a seção Sessão do
// card Contexto e a janela real na barra de status (#110). A leitura é a
// última que o statusline gravou; a regra do reset do plano é reaplicada
// com o relógio da UI (`../plan/rule`).
import { useEffect, useState } from "react";
import type { DaemonApi } from "../api/daemon";
import type { EnvAccount, PlanAccount, PlanReading, SessionUsage, UsagePromptCache, WindowView } from "../protocol/gen";

/** Releitura periódica enquanto a tela está aberta (além dos pushes da sessão). */
export const USAGE_RELOAD_MS = 30_000;

/** Pushes da sessão que relêem a leitura. */
const USAGE_PUSHES: ReadonlySet<string> = new Set(["session_state", "session_exited"]);

export interface SessionReading {
  /** `null` = sem arquivo de uso (statusline ausente) ou ambiente anterior ao método. */
  usage: SessionUsage | null;
  /** Conta do ambiente (`account.get`, só no app desktop); `null` sem ela. */
  account: PlanAccount | null;
  /** Leitura bruta mais recente do plano pelas sessões do ambiente. */
  accountPlan: PlanReading | null;
}

const EMPTY: SessionReading = { usage: null, account: null, accountPlan: null };

/**
 * Lê a leitura de uso ao trocar de sessão, a cada push da sessão
 * (`session_state`, `session_exited`) e, com `poll`, a cada
 * `USAGE_RELOAD_MS`. Com `withAccount`, também a conta do ambiente
 * (`account.get`; na web o canal não a expõe e fica `null`).
 */
export function useSessionUsage(
  api: DaemonApi,
  env: string | null,
  session: string | null,
  opts: { poll?: boolean; withAccount?: boolean } = {},
): SessionReading {
  const [reading, setReading] = useState<SessionReading>(EMPTY);
  const { poll = false, withAccount = false } = opts;
  useEffect(() => {
    setReading(EMPTY);
    if (!env || !session) return;
    let alive = true;
    const load = () => {
      api.call<SessionUsage | null>(env, "session.usage", { session_id: session }).then(
        (u) => alive && setReading((r) => ({ ...r, usage: u ?? null })),
        // Falha passageira: fica a última leitura.
        () => {},
      );
      if (withAccount && !api.web)
        api.call<EnvAccount>(env, "account.get", {}).then(
          (a) => alive && setReading((r) => ({ ...r, account: a?.account ?? null, accountPlan: a?.latest ?? null })),
          () => {},
        );
    };
    load();
    const off = api.onPush((e) => {
      if (e.env === env && USAGE_PUSHES.has(e.event) && e.data?.session_id === session) load();
    });
    const timer = poll ? setInterval(load, USAGE_RELOAD_MS) : undefined;
    return () => {
      alive = false;
      off();
      clearInterval(timer);
    };
  }, [api, env, session, poll, withAccount]);
  return reading;
}

/** A leitura do plano mais recente entre a da sessão e a do ambiente. */
export function latestPlan(a: PlanReading | null | undefined, b: PlanReading | null | undefined): PlanReading | null {
  if (!a) return b ?? null;
  if (!b) return a;
  return b.read_at > a.read_at ? b : a;
}

/** Nome da conta para o rótulo: e-mail, senão organização. */
export function accountName(a: PlanAccount | null): string | null {
  return a?.email ?? a?.org_name ?? null;
}

const pad = (n: number) => String(n).padStart(2, "0");

/** "14:30" no mesmo dia de `now`; senão "03/11 14:30" (horário local). */
export function clock(ts: number, now: number): string {
  const d = new Date(ts);
  const n = new Date(now);
  const hm = `${pad(d.getHours())}:${pad(d.getMinutes())}`;
  const sameDay = d.getFullYear() === n.getFullYear() && d.getMonth() === n.getMonth() && d.getDate() === n.getDate();
  return sameDay ? hm : `${pad(d.getDate())}/${pad(d.getMonth() + 1)} ${hm}`;
}

/** Uma janela do plano já com a regra do reset: "42% · renova 14:30". */
export function windowText(v: WindowView, now: number): string {
  if (v.state === "none") return "—";
  if (v.state === "renewed") return `renovada ${clock(v.resets_at, now)} · aguardando nova leitura`;
  return `${Math.round(v.used_percentage)}% · renova ${clock(v.resets_at, now)}`;
}

/** "US$ 1,23"; estimativa do cliente, não cobrança. */
export function costText(usd: number): string {
  return `US$ ${usd.toLocaleString("pt-BR", { minimumFractionDigits: 2, maximumFractionDigits: 2 })}`;
}

/** Cache de prompt: "acerto 87% · válido até 14:35", "expirou 14:35" ou "frio". */
export function cacheText(c: UsagePromptCache, now: number): string | null {
  const parts: string[] = [];
  if (c.hit_ratio !== null) parts.push(`acerto ${Math.round(c.hit_ratio * 100)}%`);
  if (c.expires_at !== null) {
    parts.push(now < c.expires_at ? `válido até ${clock(c.expires_at, now)}` : `expirou ${clock(c.expires_at, now)}`);
  } else if (c.warm === false) {
    parts.push("frio");
  }
  if (c.ttl) parts.push(`TTL ${c.ttl}`);
  return parts.length ? parts.join(" · ") : null;
}
