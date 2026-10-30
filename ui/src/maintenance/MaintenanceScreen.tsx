import "../settings/settings.css";
import "../hub/hub.css";
import "./maintenance.css";
import { settingsTab } from "../settings/sections";
import { useEnvChoice } from "../state/env-choice";
import { useNav } from "../state/nav";
import { useTower } from "../state/tower";
import { Icon } from "../shell/icons";
import { shortcutLabel } from "../shell/layout";
import { useShell } from "../shell/Shell";
import { SideContent, SideGroup, SidePanel, SideRow } from "../shell/SidePanel";
import {
  MAINTENANCE_GROUPS,
  MAINTENANCE_SECTION_LABEL,
  isEnvSection,
  maintenancePath,
  maintenanceTab,
  type MaintenanceScreen as MaintenanceAddress,
  type MaintenanceSection,
} from "./sections";
import { Backup } from "./Backup";
import { Cleanup } from "./Cleanup";
import { Leftovers } from "./Leftovers";
import { MaintenanceOverview } from "./Overview";
import { AppUpdate, EnvUpdates, HubUpdate } from "./Updates";

/**
 * Tela Manutenção (#335): estado e ações de manutenção do app, dos ambientes
 * e do hub, em seções. O painel lateral lista as seções (Visão geral,
 * Atualizações deste app, dos ambientes e do hub, Limpeza, Backup e snapshots); cada
 * uma abre como aba de editor (`maintenanceTab`). As seções de ambiente
 * mostram o ambiente do endereço (sem ele, o mesmo critério das
 * Configurações) e o seletor quando há mais de um; Atualizações › Ambientes
 * mostra todos numa tabela (#336) e a Limpeza, o plano e os restos fora do
 * hub do ambiente escolhido (#337). Backup e snapshots são os do hub (#338),
 * com as mesmas chamadas `hub.*`. A Visão geral (#339) junta o estado de
 * todos, com cada célula levando à seção. Os ajustes continuam nas
 * Configurações.
 */
export function MaintenanceScreen({ screen }: { screen: MaintenanceAddress }) {
  const nav = useNav();
  const { envs } = useTower();
  const section = screen.section ?? "overview";
  const perEnv = isEnvSection(section);
  const env = useEnvChoice(screen.env);
  const envName = envs.find((e) => e.id === env)?.name;
  const path = maintenancePath(section);

  return (
    <main className="maint-screen" aria-labelledby="maint-h1">
      <SideContent>
        <MaintenanceSide section={section} env={env} />
      </SideContent>
      <nav className="hub-crumbs" aria-label="Trilha">
        <ol>
          <li>Manutenção</li>
          {path.map((p, i) => (
            <li key={p} aria-current={i === path.length - 1 && !(perEnv && envName) ? "page" : undefined}>
              {p}
            </li>
          ))}
          {perEnv && envName && <li aria-current="page">{envName}</li>}
        </ol>
      </nav>
      <div className="settings hub-content">
        <div className="settings-main maint-main">
          <div className="maint-head">
            <h1 id="maint-h1">{path.join(" · ")}</h1>
            {perEnv && envs.length > 1 && (
              <select
                aria-label="Ambiente da manutenção"
                className="set-env-select"
                value={env}
                onChange={(e) => nav.openTab({ ...maintenanceTab(section, e.target.value), id: nav.current })}
              >
                {envs.map((e) => (
                  <option key={e.id} value={e.id}>
                    {e.name}
                  </option>
                ))}
              </select>
            )}
          </div>
          {(perEnv || section === "env-updates") && envs.length === 0 ? (
            <NoEnv />
          ) : section === "app-updates" ? (
            <section className="panel settings-section" aria-label="Este app">
              <AppUpdate />
            </section>
          ) : section === "env-updates" ? (
            <EnvUpdates
              onSettings={() => nav.openTab(settingsTab({ section: "daemon-update" }))}
              onHub={() => nav.openTab(maintenanceTab("hub-updates"))}
            />
          ) : section === "hub-updates" ? (
            <HubUpdate onHubSettings={() => nav.openTab(settingsTab({ section: "connections" }))} />
          ) : section === "cleanup" && env ? (
            <>
              <Cleanup key={env} env={env} onSettings={() => nav.openTab(settingsTab({ section: "cleanup", env }))} />
              <Leftovers env={env} />
            </>
          ) : section === "backup" ? (
            <BackupSection />
          ) : (
            <MaintenanceOverview />
          )}
        </div>
      </div>
    </main>
  );
}

/** Sem ambiente configurado: as seções de ambiente explicam e levam às Configurações. */
function NoEnv() {
  const nav = useNav();
  return (
    <section className="panel settings-section" aria-label="Sem ambiente">
      <p className="muted">Nenhum ambiente configurado: a manutenção de cada ambiente aparece aqui ao conectar um.</p>
      <div>
        <button type="button" className="btn" onClick={() => nav.openTab(settingsTab({ section: "connections" }))}>
          Configurar ambientes
        </button>
      </div>
    </section>
  );
}

/**
 * Backup e snapshots do hub (#338): sem hub configurado ou fora do ar,
 * explica (como a tela do Hub) em vez de chamar `hub.backup.*`.
 */
function BackupSection() {
  const { hub } = useTower();
  const nav = useNav();
  if (!hub.conn) {
    return (
      <section className="panel settings-section" aria-label="Hub não configurado">
        <p className="muted">
          A conexão com o hub não está configurada: os snapshots e o backup remoto são do hub (
          <span className="mono">cthubd</span>).
        </p>
        <div>
          <button type="button" className="btn" onClick={() => nav.openTab(settingsTab({ section: "connections" }))}>
            Configurar o hub
          </button>
        </div>
      </section>
    );
  }
  if (!hub.connected) {
    return (
      <section className="panel settings-section" aria-label="Hub indisponível">
        <p role="status" className="muted">
          Hub indisponível: o backup volta quando ele responder.
        </p>
        <p className="mono muted small">indisponível · {hub.status?.error ?? "tentando de novo"}</p>
      </section>
    );
  }
  return <Backup />;
}

/**
 * Painel lateral: as seções soltas e o grupo Atualizações (Este app,
 * Ambientes e Hub). As seções de ambiente abrem no ambiente desta aba.
 */
function MaintenanceSide({ section, env }: { section: MaintenanceSection; env?: string }) {
  const nav = useNav();
  const shell = useShell();
  const row = (s: MaintenanceSection) => (
    <SideRow
      key={s}
      label={MAINTENANCE_SECTION_LABEL[s]}
      selected={s === section}
      onClick={() => nav.openTab(maintenanceTab(s, env))}
    />
  );

  return (
    <SidePanel
      title="Manutenção"
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
      {MAINTENANCE_GROUPS.map((g) =>
        g.label ? (
          <SideGroup key={g.label} title={g.label}>
            {g.sections.map(row)}
          </SideGroup>
        ) : (
          <ul key={g.sections.join()} className="sp-rows">
            {g.sections.map(row)}
          </ul>
        ),
      )}
    </SidePanel>
  );
}
