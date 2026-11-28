// Saúde do hub e dos ambientes na Sala de controle (#357): um botão no topo
// ("Hub e N ambientes conectados · N avisos") que abre os detalhes, antes
// espalhados no painel lateral e nos itens de hub da faixa.
import { useId, useState } from "react";
import { envTab } from "../env/sections";
import { useNav } from "../state/nav";
import { useTower } from "../state/tower";
import { ago } from "../util/time";
import { configLine } from "../util/hub";
import type { EnvInfo, HubSyncResult } from "../protocol/gen";
import type { AttentionAction, AttentionItem } from "./attention";

/** Itens de "Precisa de você" que são da infraestrutura: entram também nos detalhes da saúde. */
export const isInfraItem = (i: AttentionItem) => i.kind === "hub" || i.kind === "env_down" || i.kind === "docs";

const plural = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`;

/** Envio ao hub: pendências e erro (spec do ledger centralizado §5). */
function hubLine(info: EnvInfo | undefined, now: number): string | null {
  const s = info?.hub_sync;
  if (!s) return null;
  if (s.last_error && s.pending > 0) return `hub: ${s.pending} pendentes · falhou ${ago(s.last_attempt_at ?? now, now)}`;
  if (s.syncing) return `hub: ${s.pending} pendentes · enviando…`;
  if (s.pending > 0) return `hub: ${s.pending} pendentes`;
  return s.last_ok_at ? `hub: enviado ${ago(s.last_ok_at, now)}` : null;
}

/** Resultado de `hub.sync.now` em uma linha. */
export function syncResultText(r: HubSyncResult): string {
  if (r.error) return `falhou: ${r.error}`;
  if (r.pending > 0) return `${r.sent} enviadas · ${r.pending} ainda pendentes`;
  if (r.sent === 0) return "nada a enviar: o hub já tem tudo";
  return `${r.sent} ${r.sent === 1 ? "mudança enviada" : "mudanças enviadas"} ao hub`;
}

/** Envio ao hub sob demanda (`hub.sync.now`), com progresso e resultado (também no detalhe do ambiente, #23). */
export function HubSyncButton({ env, name }: { env: string; name: string }) {
  const { api, refresh } = useTower();
  const [busy, setBusy] = useState(false);
  const [msg, setMsg] = useState<{ text: string; warn: boolean } | null>(null);

  const run = async () => {
    setBusy(true);
    setMsg(null);
    try {
      const r = await api.call<HubSyncResult>(env, "hub.sync.now");
      setMsg({ text: syncResultText(r), warn: !r.ok });
    } catch (e) {
      setMsg({ text: `falhou: ${e instanceof Error ? e.message : String(e)}`, warn: true });
    } finally {
      setBusy(false);
      // Pendências e último envio vêm do env.info.
      void refresh(env);
    }
  };

  return (
    <>
      <button
        type="button"
        className="btn btn-sm cr-sync"
        onClick={run}
        disabled={busy}
        aria-busy={busy}
        aria-label={`Enviar ao hub o estado de ${name}`}
        title={`Enviar agora ao hub as mudanças pendentes de ${name} (sessões, work items e eventos)`}
      >
        {busy ? "Enviando…" : "Enviar ao hub"}
      </button>
      <span className="cr-sub" role="status" data-warn={msg?.warn ? "true" : undefined}>
        {msg?.text}
      </span>
    </>
  );
}

/** Conexão com o hub (`cthubd`): estado, `config_rev` e avisos de `hub.status`. */
function HubEntry({ now }: { now: number }) {
  const { hub } = useTower();
  const nav = useNav();
  if (!hub.conn && !hub.unconfigured) return null;
  const warnings = hub.health?.warnings.length ?? 0;
  let text: string;
  if (!hub.conn) text = "conexão não configurada";
  else if (!hub.status) text = "conectando…";
  else if (!hub.connected) text = `indisponível ${ago(hub.status.since, now)}`;
  else text = hub.info ? `conectado · configuração rev ${hub.info.config_rev}` : "conectado";
  return (
    <li className="sp-row cr-item">
      <button
        type="button"
        className="cr-item-main"
        onClick={() => nav.go({ name: hub.conn ? "hub" : "settings" })}
        aria-label={`Hub: ${text}`}
      >
        <span className="sp-dot" data-tone={hub.connected ? "done" : "error"} aria-hidden="true" />
        <span className="cr-item-text">
          <span className="cr-item-name">{hub.info?.name ?? "hub"}</span>
          <span className="cr-sub" data-warn={hub.connected ? undefined : "true"}>
            {text}
          </span>
          {warnings > 0 && (
            <span className="cr-sub" data-warn="true">
              {plural(warnings, "aviso", "avisos")}
            </span>
          )}
        </span>
      </button>
    </li>
  );
}

/**
 * Estado de um ambiente: conexão, configuração do hub, envio ao hub e o
 * botão de envio. A linha abre o detalhe do ambiente (#23).
 */
function EnvRow({ id, name, now }: { id: string; name: string; now: number }) {
  const { status, dashboards, infos, web } = useTower();
  const nav = useNav();
  const st = status[id];
  const live = dashboards[id]?.live_sessions.length ?? 0;
  // Web (fase 1): o estado é o do último envio ao hub, não uma conexão.
  const text = !st
    ? "conectando…"
    : web
      ? st.connected
        ? `envio ao hub ${ago(st.since, now)} · ${plural(live, "sessão", "sessões")}`
        : (st.error ?? "sem envio ao hub")
      : st.connected
        ? `conectado · ${plural(live, "sessão", "sessões")}`
        : `desconectado ${ago(st.since, now)}`;
  const sending = hubLine(infos[id], now);
  const config = st?.connected ? configLine(infos[id], now) : null;
  // `docs_root` configurado e inacessível (#441): sessões novas sobem sem a pasta de docs.
  const docs = infos[id]?.docs;
  const docsDown = st?.connected && docs?.warning === "docs_unreachable";
  return (
    <li className="sp-row cr-item cr-env">
      <button
        type="button"
        className="cr-item-main"
        title={`Tudo sobre ${name}: conexão, hub, projetos, atualização e limpeza`}
        onClick={() => nav.openTab(envTab(id, name))}
      >
        <span className="sp-dot" data-tone={!st ? "neutral" : st.connected ? "done" : "error"} aria-hidden="true" />
        <span className="cr-item-text">
          <span className="cr-item-name">
            {name}
            {docsDown && (
              <>
                {" "}
                <span
                  className="badge"
                  data-tone="error"
                  title={`docs_root ${docs?.root ?? ""} inacessível: sessões novas sobem sem a pasta de docs`}
                >
                  docs inacessíveis
                </span>
              </>
            )}
          </span>
          <span className="cr-sub">{text}</span>
          {config && (
            <span className="cr-sub" data-warn={config.warn ? "true" : undefined}>
              {config.text}
            </span>
          )}
          {sending && (
            <span className="cr-sub" data-warn={infos[id]?.hub_sync?.last_error ? "true" : undefined}>
              {sending}
            </span>
          )}
        </span>
      </button>
      {/* Fora do botão da linha (botão não vai dentro de botão). */}
      {infos[id]?.hub_sync && st?.connected && (
        <span className="cr-env-sync">
          <HubSyncButton env={id} name={name} />
        </span>
      )}
    </li>
  );
}

export interface HealthInput {
  /** Hub configurado nesta UI; sem ele, só os ambientes contam. */
  hub: { connected: boolean; known: boolean } | null;
  envs: { connected: boolean | null }[];
  /** Itens de "Precisa de você": os avisos são os de infraestrutura (hub e docs). */
  items: AttentionItem[];
}

export interface HealthSummary {
  text: string;
  /** Avisos do hub (incluindo a atualização) e pastas de docs inacessíveis. */
  warnings: number;
  /** Algo fora do ar ou algum aviso. */
  warn: boolean;
}

/**
 * Texto do botão de saúde: "Hub e 3 ambientes conectados", "Hub
 * indisponível · 2 de 3 ambientes conectados" ou, sem hub, só os ambientes.
 */
export function healthSummary({ hub, envs, items }: HealthInput): HealthSummary {
  const n = envs.length;
  const up = envs.filter((e) => e.connected === true).length;
  const warnings = items.filter((i) => (i.kind === "hub" && i.key !== "hub:down") || i.kind === "docs").length;
  const all = up === n;
  const envText = all
    ? n === 1
      ? "1 ambiente conectado"
      : `${n} ambientes conectados`
    : `${up} de ${n} ${n === 1 ? "ambiente conectado" : "ambientes conectados"}`;
  let text: string;
  if (!hub) text = n === 0 ? "Nenhum ambiente" : envText.charAt(0).toUpperCase() + envText.slice(1);
  else if (!hub.known) text = `Conectando ao hub · ${envText}`;
  else if (!hub.connected) text = `Hub indisponível · ${envText}`;
  else if (all) text = n === 0 ? "Hub conectado" : `Hub e ${envText}`;
  else text = `Hub conectado · ${envText}`;
  return { text, warnings, warn: warnings > 0 || !all || (!!hub && hub.known && !hub.connected) };
}

/** Nome do painel de detalhes (e a descrição do botão que o abre). */
export const HEALTH_LABEL = "Saúde do hub e dos ambientes";

/** Botão de saúde e, aberto, os detalhes do hub e de cada ambiente. */
export function Health({
  now,
  items,
  onAction,
}: {
  now: number;
  items: AttentionItem[];
  onAction(a: AttentionAction): void;
}) {
  const { hub, envs, status } = useTower();
  const [open, setOpen] = useState(false);
  const id = useId();
  const infra = items.filter(isInfraItem);
  const s = healthSummary({
    hub: hub.conn ? { connected: hub.connected, known: !!hub.status } : null,
    envs: envs.map((e) => ({ connected: status[e.id]?.connected ?? null })),
    items,
  });
  return (
    <>
      <button
        type="button"
        className="btn btn-sm cr-health"
        data-warn={s.warn ? "true" : undefined}
        aria-expanded={open}
        aria-controls={id}
        title={HEALTH_LABEL}
        onClick={() => setOpen((o) => !o)}
      >
        <span className="sp-dot" data-tone={s.warn ? "attention" : "done"} aria-hidden="true" />
        {s.text}
        {s.warnings > 0 && <span className="cr-health-warn"> · {plural(s.warnings, "aviso", "avisos")}</span>}
      </button>
      <section id={id} className="cr-health-panel" aria-label={HEALTH_LABEL} hidden={!open}>
        {open && (
          <>
            <ul className="cr-health-list">
              <HubEntry now={now} />
              {envs.map((e) => (
                <EnvRow key={e.id} id={e.id} name={e.name} now={now} />
              ))}
            </ul>
            {infra.length > 0 && (
              <ul className="cr-health-warnings" aria-label="Avisos da infraestrutura">
                {infra.map((it) => (
                  <li key={it.key}>
                    <span className="cr-item-text">
                      <span className="cr-item-name">{it.title}</span>
                      <span className="cr-sub">{it.detail}</span>
                    </span>
                    <button type="button" className="btn btn-sm" onClick={() => onAction(it.action)}>
                      {it.actionLabel}
                    </button>
                  </li>
                ))}
              </ul>
            )}
          </>
        )}
      </section>
    </>
  );
}
