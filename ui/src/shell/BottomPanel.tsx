// Painel inferior do shell (⌘J): Eventos, Avisos e Sincronização.
import { useEffect, useRef, useState } from "react";
import type { DaemonApi } from "../api/daemon";
import type { AttentionAction, AttentionItem } from "../control-room/attention";
import { maintenanceTab } from "../maintenance/sections";
import { useNav, type Nav } from "../state/nav";
import { useTower } from "../state/tower";
import { ago } from "../util/time";
import { Icon } from "./icons";
import type { PanelTab } from "./layout";
import type { Dashboard } from "../protocol/gen";
import { pushItemRef } from "./summary";
import { trackerLabel } from "../util/item";

export interface LoggedEvent {
  id: number;
  at: number;
  env: string;
  text: string;
}

/** Eventos guardados (os mais antigos saem). */
export const EVENT_LOG_MAX = 200;

const EVENT_TEXT: Record<string, string> = {
  session_started: "sessão iniciada",
  session_state: "estado da sessão mudou",
  session_exited: "sessão encerrada",
  session_resumed: "sessão retomada",
  session_archived: "sessão arquivada",
  sessions_interrupted: "sessões interrompidas",
  work_item_status: "status do work item mudou",
  needs_human: "sessão aguardando você",
  sync_status: "sync com o GitHub",
  alert: "alerta",
  "update.status": "atualização do ambiente",
};

/**
 * Pushes e mudanças de conexão dos ambientes desde que o app abriu (não
 * ficam no ledger). Fica no shell, montado mesmo com o painel fechado.
 */
export function useEventLog(api: DaemonApi, dashboards: Record<string, Dashboard> = {}): LoggedEvent[] {
  const [log, setLog] = useState<LoggedEvent[]>([]);
  // O projeto do item (chave do Jira, #352) vem do dashboard da hora do push.
  const dash = useRef(dashboards);
  dash.current = dashboards;
  useEffect(() => {
    let seq = 0;
    const add = (env: string, text: string) =>
      setLog((all) => [...all.slice(-(EVENT_LOG_MAX - 1)), { id: ++seq, at: Date.now(), env, text }]);
    const offPush = api.onPush((e) => {
      const ref = pushItemRef(dash.current, e.env, e.data as Record<string, unknown> | undefined);
      const issue = ref ? ` ${ref}` : "";
      // O sync do projeto Jira diz Jira (o `project_id` vem no push).
      const data = e.data as Record<string, unknown> | undefined;
      const syncProject =
        e.event === "sync_status" ? dash.current[e.env]?.projects.find((p) => p.project.id === data?.project_id)?.project : undefined;
      const what = syncProject ? `sync com o ${trackerLabel(syncProject)}` : (EVENT_TEXT[e.event] ?? e.event);
      add(e.env, `${what}${issue}`);
    });
    const offStatus = api.onStatus((s) => add(s.env, s.connected ? "conectado" : `desconectado${s.error ? `: ${s.error}` : ""}`));
    return () => {
      offPush();
      offStatus();
    };
  }, [api]);
  return log;
}

/**
 * Leva à tela do aviso (as mesmas ações da faixa "Precisa de você"); a de
 * documentação abre no navegador por `openUrl`.
 */
export function goToAction(nav: Nav, a: AttentionAction, openUrl?: (url: string) => void) {
  switch (a.to) {
    case "url":
      return openUrl?.(a.url);
    case "terminal":
      return nav.openTerminal({ env: a.env, session: a.session });
    case "project":
      return nav.go({ name: "project", env: a.env, projectId: a.projectId });
    case "maintenance":
      return nav.openTab(maintenanceTab(a.section));
    default:
      return nav.go({ name: a.to });
  }
}

const TABS: { id: PanelTab; label: string; hint?: string }[] = [
  { id: "events", label: "Eventos" },
  { id: "warnings", label: "Avisos" },
  {
    id: "sync",
    label: "Sincronização",
    // Os dois sentidos, com os nomes dos botões: "Buscar issues agora" e "Enviar ao hub".
    hint: "Busca de issues no rastreador, por projeto, e envio ao hub, por ambiente",
  },
];

const time = (ms: number) =>
  new Date(ms).toLocaleTimeString("pt-BR", { hour: "2-digit", minute: "2-digit", second: "2-digit" });

export function BottomPanel({
  tab,
  onTab,
  onClose,
  events,
  warnings,
  now,
}: {
  tab: PanelTab;
  onTab(t: PanelTab): void;
  onClose(): void;
  events: LoggedEvent[];
  warnings: AttentionItem[];
  now: number;
}) {
  const { envs } = useTower();
  const envName = (id: string) => (id === "hub" ? "hub" : (envs.find((e) => e.id === id)?.name ?? id));

  return (
    <section className="bpanel" aria-label="Painel inferior">
      <div className="bpanel-head">
        <div role="tablist" aria-label="Abas do painel inferior" className="bpanel-tabs">
          {TABS.map((t) => (
            <button
              key={t.id}
              type="button"
              role="tab"
              id={`bpanel-tab-${t.id}`}
              aria-selected={tab === t.id}
              aria-controls="bpanel-body"
              title={t.hint}
              onClick={() => onTab(t.id)}
            >
              {t.label}
              {t.id === "warnings" && warnings.length > 0 && ` · ${warnings.length}`}
            </button>
          ))}
        </div>
        <button type="button" className="ibtn" aria-label="Fechar painel inferior" onClick={onClose}>
          <Icon name="close" size={14} />
        </button>
      </div>
      <div className="bpanel-body" id="bpanel-body" role="tabpanel" aria-labelledby={`bpanel-tab-${tab}`}>
        {tab === "events" && <Events events={events} envName={envName} />}
        {tab === "warnings" && <Warnings items={warnings} envName={envName} now={now} />}
        {tab === "sync" && <SyncList now={now} />}
      </div>
    </section>
  );
}

function Events({ events, envName }: { events: LoggedEvent[]; envName(id: string): string }) {
  if (events.length === 0) return <p className="muted bpanel-empty">Nenhum evento desde que o app abriu.</p>;
  return (
    <ol className="bpanel-log mono">
      {[...events].reverse().map((e) => (
        <li key={e.id}>
          <span className="muted">{time(e.at)}</span>
          <span className="bpanel-env">{envName(e.env)}</span>
          <span>{e.text}</span>
        </li>
      ))}
    </ol>
  );
}

export function Warnings({
  items,
  envName,
  now,
  onGo,
}: {
  items: AttentionItem[];
  envName(id: string): string;
  now: number;
  /** Depois de levar à tela do aviso (a folha do celular se fecha). */
  onGo?(): void;
}) {
  const nav = useNav();
  const { api } = useTower();
  if (items.length === 0) return <p className="muted bpanel-empty">Nenhum aviso.</p>;
  return (
    <ul className="bpanel-list">
      {items.map((a) => (
        <li key={a.key}>
          <Icon name="warning" size={14} />
          <span className="bpanel-env">{envName(a.env)}</span>
          <span className="bpanel-title">{a.title}</span>
          <span className="muted bpanel-detail">
            {a.detail} · {ago(a.since, now)}
          </span>
          <button
            type="button"
            className="btn btn-sm"
            onClick={() => {
              goToAction(nav, a.action, (url) => void api.openUrl(url).catch(() => {}));
              onGo?.();
            }}
          >
            {a.actionLabel}
          </button>
        </li>
      ))}
    </ul>
  );
}

/** Sync com o rastreador (GitHub ou Jira) por projeto e envio ao hub, por ambiente. */
function SyncList({ now }: { now: number }) {
  const { envs, dashboards, infos } = useTower();
  const rows = envs.flatMap((e) => {
    const d = dashboards[e.id];
    const out = (d?.sync ?? []).map((s) => {
      const project = d?.projects.find((p) => p.project.id === s.project_id)?.project;
      const name = project?.name ?? `projeto ${s.project_id}`;
      const text = s.last_error
        ? `falhou ${ago(s.last_attempt_at, now)}: ${s.last_error}`
        : s.last_ok_at
          ? `sincronizado ${ago(s.last_ok_at, now)}`
          : "ainda não sincronizado";
      return { key: `${e.id}:${s.project_id}`, env: e.name, what: `busca de issues · ${trackerLabel(project)} · ${name}`, text, warn: !!s.last_error };
    });
    const h = infos[e.id]?.hub_sync;
    if (h) {
      const text = h.last_error
        ? `${h.pending} pendentes · falhou: ${h.last_error}`
        : h.pending > 0
          ? `${h.pending} pendentes`
          : h.last_ok_at
            ? `enviado ${ago(h.last_ok_at, now)}`
            : "nada enviado ainda";
      out.push({ key: `${e.id}:hub`, env: e.name, what: "envio ao hub", text, warn: !!h.last_error });
    }
    return out;
  });
  if (rows.length === 0) return <p className="muted bpanel-empty">Nenhum projeto sincronizando.</p>;
  return (
    <ul className="bpanel-list">
      {rows.map((r) => (
        <li key={r.key}>
          <span className="bpanel-env">{r.env}</span>
          <span className="bpanel-title">{r.what}</span>
          <span className="bpanel-detail" data-warn={r.warn ? "true" : undefined}>
            {r.text}
          </span>
        </li>
      ))}
    </ul>
  );
}
