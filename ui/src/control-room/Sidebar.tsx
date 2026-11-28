// Painel lateral da Sala de controle (#316, #357): projetos e contas do plano.
import { useState } from "react";
import { projectsPlace } from "../project/sections";
import { projectMenuItems, useProjectRows, type ProjectRow } from "../project/rows";
import { syncAction, useSyncRun } from "../project/sync-now";
import { ContextMenu, type MenuItem } from "../shell/ContextMenu";
import { Icon } from "../shell/icons";
import { shortcutLabel } from "../shell/layout";
import { useShell } from "../shell/Shell";
import { SideContent, SideGroup, SidePanel, SideRow } from "../shell/SidePanel";
import { useStartSession } from "../shell/start";
import { useNav } from "../state/nav";
import { useTower } from "../state/tower";
import { DesktopOnlyIcon } from "../web/DesktopOnly";
import { PlanAccounts } from "../plan/PlanAccounts";

/**
 * Painel lateral da Sala de controle: projetos com contadores (pelo hub, se
 * no ar) e as contas do plano. As visões viraram os chips do topo, e o hub e
 * os ambientes, o botão de saúde e o rodapé Infraestrutura (#357).
 */
export function Sidebar({ now }: { now: number }) {
  const tower = useTower();
  const { hub, web, api } = tower;
  const shell = useShell();
  const nav = useNav();
  const startSession = useStartSession();
  const projects = useProjectRows();
  const [menu, setMenu] = useState<{ x: number; y: number; p: ProjectRow; origin: HTMLElement } | null>(null);
  /** Último "Sincronizar este projeto" pedido por aqui, para o resultado sob a lista (#90). */
  const [synced, setSynced] = useState<{ name: string; key: string } | null>(null);
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
      sync: (() => {
        const s = syncAction(tower, p.syncAddr);
        return {
          ...s,
          run: () => {
            setSynced({ name: p.name, key: s.info.key });
            s.run();
          },
        };
      })(),
    });

  return (
    <SideContent>
      <SidePanel
        title="Sala de controle"
        label="Projetos e contas"
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
        <PlanAccounts now={now} />
        <SideGroup
          title="Projetos"
          count={projects.length}
          actions={
            // Web: o "+" fica desabilitado com o motivo (#26).
            web && hub.connected ? (
              <DesktopOnlyIcon label="Novo projeto" icon="plus" />
            ) : (
              editable && (
                <button
                  type="button"
                  className="ibtn"
                  aria-label="Novo projeto"
                  title="Novo projeto"
                  // O cadastro abre no índice de Projetos (#18).
                  onClick={() => nav.go(projectsPlace({ register: { url: "" } }))}
                >
                  <Icon name="plus" size={15} />
                </button>
              )
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
          {synced && <SyncNote name={synced.name} syncKey={synced.key} />}
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

/** Andamento e resultado do "Sincronizar este projeto" pedido pelo menu de um projeto (#90). */
function SyncNote({ name, syncKey }: { name: string; syncKey: string }) {
  const run = useSyncRun(syncKey);
  if (run.busy) return <p className="cr-sub cr-sync-note" role="status">{name}: buscando issues…</p>;
  if (!run.msg) return null;
  return (
    <p className={`cr-sub cr-sync-note ${run.msg.error ? "start-error" : "state-on"}`} role={run.msg.error ? "alert" : "status"}>
      {name}: {run.msg.text}
    </p>
  );
}
