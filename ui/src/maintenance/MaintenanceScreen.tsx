import "../settings/settings.css";
import "../hub/hub.css";
import "./maintenance.css";
import { envTab } from "../env/sections";
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
  projectFilterKey,
  type MaintenanceScreen as MaintenanceAddress,
  type MaintenanceSection,
} from "./sections";
import { Backup } from "./Backup";
import { Cleanup } from "./Cleanup";
import { CleanupRules } from "./CleanupRules";
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
 * hub do ambiente escolhido (#337), com as regras (`[cleanup]`, antes
 * nas Configurações) e o filtro por projeto (#22). Backup e snapshots são os do hub (#338),
 * com as mesmas chamadas `hub.*`. A Visão geral (#339) junta o estado de
 * todos, com cada célula levando à seção. Os demais ajustes continuam
 * nas Configurações.
 */
export function MaintenanceScreen({ screen }: { screen: MaintenanceAddress }) {
  const nav = useNav();
  const { envs, dashboards } = useTower();
  const section = screen.section ?? "overview";
  const perEnv = isEnvSection(section);
  const env = useEnvChoice(screen.env);
  const envName = envs.find((e) => e.id === env)?.name;
  const path = maintenancePath(section);
  // Filtro por projeto (#22): só na Limpeza. As opções são os projetos do
  // ambiente, mais o do endereço se ele não estiver lá (outro ambiente).
  const project = section === "cleanup" ? screen.project : undefined;
  const filterKey = project ? projectFilterKey(project) : "";
  const options = (env ? (dashboards[env]?.projects ?? []) : []).map((p) => ({
    key: projectFilterKey({ owner: p.project.gh_owner, repo: p.project.gh_repo }),
    label: p.project.name,
    ref: { owner: p.project.gh_owner, repo: p.project.gh_repo },
  }));
  if (project && !options.some((o) => o.key === filterKey))
    options.unshift({ key: filterKey, label: `${project.owner}/${project.repo}`, ref: project });
  const filterLabel = project && options.find((o) => o.key === filterKey)?.label;

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
          {perEnv && envName && <li aria-current={project ? undefined : "page"}>{envName}</li>}
          {project && <li aria-current="page">{filterLabel}</li>}
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
                onChange={(e) => nav.openTab({ ...maintenanceTab(section, e.target.value, project), id: nav.current })}
              >
                {envs.map((e) => (
                  <option key={e.id} value={e.id}>
                    {e.name}
                  </option>
                ))}
              </select>
            )}
            {section === "cleanup" && env && (
              <select
                aria-label="Projeto da limpeza"
                className="set-env-select"
                value={filterKey}
                onChange={(e) =>
                  nav.openTab({
                    ...maintenanceTab(section, env, options.find((o) => o.key === e.target.value)?.ref),
                    id: nav.current,
                  })
                }
              >
                <option value="">Todos os projetos</option>
                {options.map((o) => (
                  <option key={o.key} value={o.key}>
                    {o.label}
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
              // O nome leva ao detalhe do ambiente (#23), na seção Atualização.
              onEnv={(e) => nav.openTab(envTab(e.id, e.name, "update"))}
            />
          ) : section === "hub-updates" ? (
            <HubUpdate onHubSettings={() => nav.openTab(settingsTab({ section: "connections" }))} />
          ) : section === "cleanup" && env ? (
            <>
              <Cleanup key={env} env={env} project={project} />
              <CleanupRules env={env} />
              <Leftovers env={env} project={project} />
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
