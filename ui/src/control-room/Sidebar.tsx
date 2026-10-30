// Painel lateral da Sala de controle (#316): visões, hub, ambientes e projetos.
import { useState } from "react";
import { hubTab } from "../hub/sections";
import { projectMenuItems, useProjectRows, type ProjectRow } from "../project/rows";
import { ContextMenu, type MenuItem } from "../shell/ContextMenu";
import { Icon } from "../shell/icons";
import { shortcutLabel } from "../shell/layout";
import { useShell } from "../shell/Shell";
import { SideContent, SideGroup, SidePanel, SideRow } from "../shell/SidePanel";
import { useStartSession } from "../shell/start";
import { useNav } from "../state/nav";
import { useTower } from "../state/tower";
import { ago } from "../util/time";
import { configLine } from "../util/hub";
import type { ControlView } from "./grid";
import type { EnvInfo, HubSyncResult } from "../protocol/gen";

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

/** Envio ao hub sob demanda (`hub.sync.now`), com progresso e resultado. */
function HubSyncButton({ env, name }: { env: string; name: string }) {
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
        aria-label={`Sincronizar ${name} com o hub`}
      >
        {busy ? "Sincronizando…" : "Sincronizar com o hub"}
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
    <SideGroup title="Hub">
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
                {warnings} {warnings === 1 ? "aviso" : "avisos"}
              </span>
            )}
          </span>
        </button>
      </li>
    </SideGroup>
  );
}

const plural = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`;

/** Estado de um ambiente: conexão, configuração do hub, envio ao hub e o botão de envio. */
function EnvRow({ id, name, now }: { id: string; name: string; now: number }) {
  const { status, dashboards, infos, web } = useTower();
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
    <li className="sp-row cr-item">
      <span className="cr-item-main">
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
          {infos[id]?.hub_sync && st?.connected && <HubSyncButton env={id} name={name} />}
        </span>
      </span>
    </li>
  );
}

const VIEWS: {
  id: ControlView;
  label: string;
  tone: string;
  desc: (n: number) => string;
}[] = [
  {
    id: "attention",
    label: "Precisa de você",
    tone: "attention",
    desc: (n) => plural(n, "item", "itens"),
  },
  {
    id: "running",
    label: "Rodando",
    tone: "running",
    desc: (n) => plural(n, "sessão", "sessões"),
  },
  {
    id: "done",
    label: "Concluídas hoje",
    tone: "done",
    desc: (n) => plural(n, "concluída", "concluídas"),
  },
];

/**
 * Painel lateral da Sala de controle (spec §10): visões (filtram a área de
 * editor; clicar de novo volta a tudo), hub, ambientes com o estado e o
 * número de sessões, e projetos com contadores (pelo hub, se no ar).
 */
export function Sidebar({
  now,
  view,
  onView,
  counts,
}: {
  now: number;
  view: ControlView | null;
  onView(v: ControlView | null): void;
  counts: Record<ControlView, number>;
}) {
  const { envs, hub, web, api } = useTower();
  const shell = useShell();
  const nav = useNav();
  const startSession = useStartSession();
  const projects = useProjectRows();
  const [menu, setMenu] = useState<{ x: number; y: number; p: ProjectRow; origin: HTMLElement } | null>(null);
  // Cadastro e atribuição são no hub e só no desktop (na web, só leitura).
  const editable = hub.connected && !web;

  const menuItems = (p: ProjectRow, origin: HTMLElement): MenuItem[] =>
    projectMenuItems(p, {
      nav,
      hubConnected: hub.connected,
      editable,
      startSession,
      openUrl: (url) => void api.openUrl(url).catch(() => {}),
      origin,
    });

  return (
    <SideContent>
      <SidePanel
        title="Sala de controle"
        label="Ambientes e projetos"
        actions={
          <button
            type="button"
            className="ibtn"
            aria-label={`Recolher painel lateral (${shortcutLabel("B")})`}
            onClick={shell.toggleSide}
          >
            <Icon name="sidebar" size={15} />
          </button>
        }
      >
        <SideGroup title="Visões">
          {VIEWS.map((v) => (
            <SideRow
              key={v.id}
              label={v.label}
              tone={v.tone}
              trailing={counts[v.id]}
              description={v.desc(counts[v.id])}
              selected={view === v.id}
              onClick={() => onView(view === v.id ? null : v.id)}
            />
          ))}
        </SideGroup>
        <HubEntry now={now} />
        <SideGroup title="Ambientes" count={envs.length}>
          {envs.map((e) => (
            <EnvRow key={e.id} id={e.id} name={e.name} now={now} />
          ))}
        </SideGroup>
        <SideGroup
          title="Projetos"
          count={projects.length}
          actions={
            editable && (
              <button
                type="button"
                className="ibtn"
                aria-label="Novo projeto"
                title="Novo projeto"
                onClick={() => nav.openTab(hubTab("projects", { register: { url: "" } }))}
              >
                <Icon name="plus" size={15} />
              </button>
            )
          }
        >
          {projects.map((p) => (
            <SideRow
              key={p.key}
              label={p.name}
              tone={p.waiting > 0 ? "attention" : p.running > 0 ? "running" : undefined}
              trailing={p.running + p.waiting}
              description={`${p.running} em execução, ${p.waiting} aguardando`}
              onClick={() => nav.go(p.screen)}
              onMenu={(x, y, origin) => setMenu({ x, y, p, origin })}
            />
          ))}
        </SideGroup>
      </SidePanel>
      {menu && (
        <ContextMenu
          label={`Ações do projeto ${menu.p.name}`}
          x={menu.x}
          y={menu.y}
          items={menuItems(menu.p, menu.origin)}
          onClose={(restore) => {
            const o = menu.origin;
            setMenu(null);
            if (restore) o.focus();
          }}
        />
      )}
    </SideContent>
  );
}
