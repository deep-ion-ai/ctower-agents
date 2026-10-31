import "../hub/hub.css";
import "../project/project.css";
import "../settings/settings.css";
import "../maintenance/maintenance.css";
import "./env.css";
import { useId, type ReactNode } from "react";
import { useNow } from "../App";
import type { EnvView } from "../api/types";
import { HubSyncButton } from "../control-room/Sidebar";
import { hubTab } from "../hub/sections";
import { logsTab, maintenanceTab } from "../maintenance/sections";
import { EnvUpdates } from "../maintenance/Updates";
import { projectAt } from "../project/sections";
import type { HubAssignment, HubEnv } from "../protocol/hub-gen";
import { EnvSettingsItems } from "../settings/SettingsScreen";
import { EnvsSettings } from "../settings/EnvsSettings";
import { ProjectsSettings } from "../settings/ProjectsSettings";
import { settingsTab } from "../settings/sections";
import { useNav } from "../state/nav";
import { useTower, type HubView } from "../state/tower";
import { humanBytes } from "../util/bytes";
import { assignmentText, configLine } from "../util/hub";
import { ago } from "../util/time";
import {
  DESKTOP_ONLY_ENV_SECTIONS,
  ENV_SECTIONS,
  ENV_SECTION_LABEL,
  envSectionOf,
  envTab,
  type EnvAddr,
  type EnvSection,
} from "./sections";

/**
 * Detalhe do ambiente (#23, épico #20): tudo sobre um ambiente numa tela
 * só, no padrão das Configurações do projeto (#17): o índice das seções à
 * esquerda e a seção do endereço à direita, uma por vez. Cada seção compõe
 * o que já existia (Configurações › Conexões, Hub › Ambientes, Manutenção ›
 * Atualizações, o escopo Ambiente das Configurações, Manutenção › Limpeza);
 * esses lugares levam para cá. Sem `env`, o índice dos ambientes.
 */
export function EnvScreen({ screen }: { screen: EnvAddr }) {
  const { web } = useTower();
  if (!screen.env) return <EnvIndex />;
  return <EnvDetail env={screen.env} section={envSectionOf(screen, web)} />;
}

/** Estado da conexão em uma linha (na web, o do último envio ao hub, como na Sala de controle). */
function useStateText(env: string): { text: string; on: boolean } {
  const { status, web } = useTower();
  const now = useNow();
  const st = status[env];
  if (!st) return { text: "conectando…", on: false };
  if (web) {
    return { text: st.connected ? `envio ao hub ${ago(st.since, now)}` : (st.error ?? "sem envio ao hub"), on: st.connected };
  }
  return {
    text: st.connected ? "conectado" : `desconectado · ${st.error ?? "tentando de novo"}`,
    on: st.connected,
  };
}

/** Índice: os ambientes deste app, cada um levando ao detalhe. */
function EnvIndex() {
  const { envs, web } = useTower();
  const nav = useNav();
  return (
    <main className="page">
      <h1 className="section-title">Ambientes</h1>
      {envs.length === 0 ? (
        <p className="muted">Nenhum ambiente neste app.</p>
      ) : (
        <ul className="hub-list env-index" aria-label="Ambientes">
          {envs.map((e) => (
            <EnvIndexRow key={e.id} env={e} onOpen={() => nav.openTab(envTab(e.id, e.name))} />
          ))}
        </ul>
      )}
      {!web && (
        <div className="form-actions">
          <button type="button" className="btn" onClick={() => nav.openTab(settingsTab({ section: "connections" }))}>
            Adicionar ambiente em Configurações › Conexões
          </button>
        </div>
      )}
    </main>
  );
}

function EnvIndexRow({ env, onOpen }: { env: EnvView; onOpen: () => void }) {
  const st = useStateText(env.id);
  return (
    <li>
      <button type="button" className="link-btn strong" onClick={onOpen}>
        {env.name}
      </button>{" "}
      <span className={st.on ? "state-on" : "state-off"}>{st.text}</span>
    </li>
  );
}

/** Registro do ambiente no hub (`hub.envs.list`), pelo `env_id` do daemon (`envByDaemon`). */
export function hubEnvOf(hub: Pick<HubView, "envs" | "envByDaemon">, env: string): HubEnv | undefined {
  return hub.envs.find((h) => hub.envByDaemon[h.env_id] === env);
}

function EnvDetail({ env, section }: { env: string; section: EnvSection }) {
  const { envs, infos, status, web } = useTower();
  const nav = useNav();
  const e = envs.find((x) => x.id === env);
  const st = useStateText(env);
  const noteId = useId();
  if (!e) {
    return (
      <main className="page">
        <h1 className="section-title">Ambiente</h1>
        <p className="empty">Este ambiente não está (mais) neste app.</p>
        <div className="form-actions">
          <button type="button" className="btn" onClick={() => nav.go({ name: "env" })}>
            Ver os ambientes
          </button>
        </div>
      </main>
    );
  }
  const info = status[env]?.connected ? infos[env] : undefined;
  /** Na web, as seções do desktop ficam no índice, desabilitadas com o motivo (#26). */
  const desktopOnly = (s: EnvSection) => web && DESKTOP_ONLY_ENV_SECTIONS.has(s);

  let body: ReactNode;
  switch (section) {
    case "connection":
      body = <ConnectionSection env={e} />;
      break;
    case "hub":
      body = <HubSection env={e} />;
      break;
    case "projects":
      body = <ProjectsSection env={e} />;
      break;
    case "update":
      body = (
        <>
          <EnvUpdates only={env} onHub={() => nav.openTab(maintenanceTab("hub-updates"))} />
          {/* Canal, intervalo e Reverter: os mesmos itens das Configurações › Ambiente. */}
          <EnvSettingsItems env={env} sections={["daemon-update"]} skip={["update-maintenance"]} />
        </>
      );
      break;
    case "config":
      body = (
        <>
          <EnvSettingsItems
            env={env}
            sections={["github", "local-projects", "env-notifications"]}
            // Os clones vão em Projetos atribuídos e a limpeza na seção Limpeza.
            skip={["projects", "cleanup-maintenance"]}
          />
          <div>
            <button
              type="button"
              className="link-btn"
              onClick={() => nav.openTab(settingsTab({ scope: "env", env }))}
            >
              Ver nas Configurações › Ambiente (com a busca) ›
            </button>
          </div>
        </>
      );
      break;
    case "cleanup":
      body = <CleanupSection env={e} />;
      break;
  }

  return (
    <main className="page env-detail">
      <div className="hub-page-head">
        <h1 className="section-title">{e.name}</h1>
        <span className={st.on ? "state-on" : "state-off"} role="status">
          {st.text}
        </span>
        {/* Tela Logs (#65) já na fonte deste ambiente. */}
        <button type="button" className="btn btn-sm" onClick={() => nav.openTab(logsTab({ kind: "env", env }))}>
          Logs do ambiente
        </button>
      </div>
      {info && (
        <p className="muted hint">
          ctowerd {info.version} · protocolo {info.protocol} · {info.hostname}
        </p>
      )}
      <div className="pj-settings">
        <nav className="pj-settings-nav" aria-label="Seções do ambiente">
          <ul>
            {ENV_SECTIONS.map((s) => (
              <li key={s}>
                <button
                  type="button"
                  aria-current={s === section ? "page" : undefined}
                  disabled={desktopOnly(s)}
                  aria-describedby={desktopOnly(s) ? noteId : undefined}
                  onClick={() => nav.openTab(envTab(env, e.name, s))}
                >
                  {ENV_SECTION_LABEL[s]}
                </button>
              </li>
            ))}
          </ul>
          {web && (
            <p id={noteId} className="muted desktop-only-note pj-settings-note">
              Atualização, configuração do daemon e limpeza no app desktop
            </p>
          )}
        </nav>
        <div key={`${env}:${section}`} className="pj-settings-body" role="group" aria-label={ENV_SECTION_LABEL[section]}>
          <div className="hub-page-head">
            <h2>{ENV_SECTION_LABEL[section]}</h2>
          </div>
          {body}
        </div>
      </div>
    </main>
  );
}

/** Conexão: a mesma linha de Configurações › Conexões (editar, remover) e o daemon do outro lado. */
function ConnectionSection({ env }: { env: EnvView }) {
  const { infos, status, web } = useTower();
  const info = infos[env.id];
  const on = !!status[env.id]?.connected;
  return (
    <>
      {web ? (
        <p className="muted">A conexão de cada ambiente (URL e token de UI) se edita no app desktop.</p>
      ) : (
        <EnvsSettings only={env.id} />
      )}
      <section className="hub-card" aria-labelledby={`env-daemon-${env.id}`}>
        <div className="hub-card-head">
          <h3 id={`env-daemon-${env.id}`}>Daemon</h3>
        </div>
        {info ? (
          <>
            <dl className="hub-kv">
              <div>
                <dt>Versão</dt>
                <dd className="mono">
                  {info.version}
                  {info.commit ? ` · ${info.commit.slice(0, 7)}` : ""} · protocolo {info.protocol}
                </dd>
              </div>
              <div>
                <dt>Máquina</dt>
                <dd className="mono">{info.hostname}</dd>
              </div>
              <div>
                <dt>Identidade</dt>
                <dd className="mono">
                  {info.env_name} · {info.env_id}
                </dd>
              </div>
              <div>
                <dt>Worktrees</dt>
                <dd className="mono">{info.worktrees_root}</dd>
              </div>
            </dl>
            {!on && <p className="muted hint">Último estado recebido: o ambiente está sem conexão.</p>}
          </>
        ) : (
          <p className="muted">{on ? "Carregando…" : "Sem conexão: o daemon ainda não respondeu."}</p>
        )}
      </section>
    </>
  );
}

/**
 * Hub: a ligação (`hub_config`), o envio (`hub_sync`, com "Enviar ao hub",
 * #21) e o que o hub sabe do ambiente (`hub.envs.list`). Ligar e o token do
 * gestor seguem pela CLI: o protocolo não tem métodos para gerar tokens no
 * hub nem para gravá-los no ambiente.
 */
function HubSection({ env }: { env: EnvView }) {
  const { infos, status, hub } = useTower();
  const nav = useNav();
  const now = useNow();
  const info = infos[env.id];
  const on = !!status[env.id]?.connected;
  const config = configLine(info, now);
  const sync = info?.hub_sync ?? null;
  const h = hubEnvOf(hub, env.id);
  const linked = info?.hub_config.linked;

  return (
    <>
      <section className="hub-card" aria-labelledby={`env-hub-link-${env.id}`}>
        <div className="hub-card-head">
          <h3 id={`env-hub-link-${env.id}`}>Ligação</h3>
        </div>
        <dl className="hub-kv">
          <div>
            <dt>Configuração do hub</dt>
            <dd>
              {!info ? (
                <span className="muted">{on ? "carregando…" : "sem conexão"}</span>
              ) : (
                <span className={config?.warn ? "state-off" : undefined}>{config?.text ?? "—"}</span>
              )}
              {info?.hub_config.last_error && <span className="hub-card-sub">{info.hub_config.last_error}</span>}
            </dd>
          </div>
          <div>
            <dt>Envio ao hub</dt>
            <dd>
              {!sync ? (
                <span className="muted">{info ? "este ambiente não envia ao hub" : "—"}</span>
              ) : (
                <>
                  <span className={sync.last_error ? "state-off" : undefined}>
                    {sync.pending} pendente(s)
                    {sync.last_ok_at ? ` · último envio ${ago(sync.last_ok_at, now)}` : " · nenhum envio ainda"}
                  </span>
                  {sync.last_error && <span className="hub-card-sub">{sync.last_error}</span>}
                </>
              )}
              {sync && on && (
                <span className="env-hub-sync">
                  <HubSyncButton env={env.id} name={env.name} />
                </span>
              )}
            </dd>
          </div>
          <div>
            <dt>Gestor de projetos</dt>
            <dd>
              {!info ? (
                "—"
              ) : info.hub_config.agent_token ? (
                "token de agente gravado: o gestor pode rodar neste ambiente"
              ) : (
                <span className="muted">sem token de agente: o gestor não roda neste ambiente</span>
              )}
            </dd>
          </div>
        </dl>
      </section>
      <section className="hub-card" aria-labelledby={`env-hub-view-${env.id}`}>
        <div className="hub-card-head">
          <h3 id={`env-hub-view-${env.id}`}>No hub</h3>
          {hub.connected && (
            <button type="button" className="btn" onClick={() => nav.openTab(hubTab("envs"))}>
              Hub › Ambientes
            </button>
          )}
        </div>
        {!hub.connected ? (
          <p className="muted">Hub fora do ar ou sem conexão configurada neste app.</p>
        ) : !h ? (
          <p className="muted">O hub não conhece este ambiente (ou ainda não recebeu nada dele).</p>
        ) : (
          <dl className="hub-kv">
            <div>
              <dt>Nome no hub</dt>
              <dd>
                {h.name} <span className="mono muted small">{h.env_id}</span>
              </dd>
            </div>
            <div>
              <dt>Último envio recebido</dt>
              <dd>
                {h.last_seen_at ? ago(h.last_seen_at, now) : "nunca"} · {h.pending} pendente(s) no último envio
              </dd>
            </div>
            <div>
              <dt>Canal reverso</dt>
              <dd>{h.online ? "aberto" : <span className="muted">fechado</span>}</dd>
            </div>
          </dl>
        )}
      </section>
      <section className="hub-card" aria-labelledby={`env-hub-cli-${env.id}`}>
        <div className="hub-card-head">
          <h3 id={`env-hub-cli-${env.id}`}>{linked === false ? "Ligar ao hub" : "Tokens (pela CLI)"}</h3>
        </div>
        <p className="muted">
          Ligar o ambiente ao hub e dar o token do gestor é pela CLI. Na máquina do hub,{" "}
          <span className="mono">cthubd add-env {info?.env_name ?? "<nome>"}</span> gera o token de sync; no ambiente,{" "}
          <span className="mono">ctower hub set-token</span> o grava (lido do stdin; ex.:{" "}
          <span className="mono">cthubd add-env {info?.env_name ?? "<nome>"} | ctower hub set-token</span>) e o{" "}
          <span className="mono">hub_url</span> vai no config.toml. Para o gestor,{" "}
          <span className="mono">cthubd agent-token {info?.env_name ?? "<nome>"}</span> e{" "}
          <span className="mono">ctower hub set-agent-token</span>.
        </p>
      </section>
    </>
  );
}

/** Projetos atribuídos: as atribuições no hub (cada uma leva ao projeto) e os clones no ambiente. */
function ProjectsSection({ env }: { env: EnvView }) {
  const { hub, infos } = useTower();
  const nav = useNav();
  // Pelo `env_id` do daemon: o do `env.info` ou o do registro no hub.
  const daemonId = infos[env.id]?.env_id ?? hubEnvOf(hub, env.id)?.env_id;
  const list: HubAssignment[] = daemonId
    ? hub.projects.flatMap((p) => p.assignments.filter((a) => a.env_id === daemonId))
    : [];
  return (
    <>
      <section className="hub-card" aria-labelledby={`env-assign-${env.id}`}>
        <div className="hub-card-head">
          <h3 id={`env-assign-${env.id}`}>Atribuições no hub</h3>
        </div>
        {!hub.connected ? (
          <p className="muted">Hub fora do ar ou sem conexão configurada: as atribuições ficam no hub.</p>
        ) : list.length === 0 ? (
          <p className="muted">Nenhum projeto atribuído a este ambiente.</p>
        ) : (
          <ul className="hub-list" aria-label={`Projetos atribuídos a ${env.name}`}>
            {list.map((a) => (
              <li key={a.project_id}>
                <button
                  type="button"
                  className="link-btn mono"
                  title="Ambientes e atribuições do projeto"
                  onClick={() => nav.go(projectAt({ name: "project", owner: a.gh_owner, repo: a.gh_repo }, "settings", "envs"))}
                >
                  {a.gh_owner}/{a.gh_repo}
                </button>
                <span className="hub-state" data-state={a.state}>
                  {assignmentText(a)}
                </span>
              </li>
            ))}
          </ul>
        )}
        <p className="muted hint">Atribuir e remover ficam nas Configurações de cada projeto (Ambientes e atribuições).</p>
      </section>
      <section className="hub-card" aria-labelledby={`env-clones-${env.id}`}>
        <div className="hub-card-head">
          <h3 id={`env-clones-${env.id}`}>No ambiente</h3>
        </div>
        <ProjectsSettings env={env.id} />
      </section>
    </>
  );
}

/**
 * Limpeza: o resumo que o ambiente informou ao hub e o atalho para
 * Manutenção › Limpeza deste ambiente (#22: as regras, o plano e o limpar
 * ficam só lá).
 */
function CleanupSection({ env }: { env: EnvView }) {
  const { hub } = useTower();
  const nav = useNav();
  const now = useNow();
  const c = hubEnvOf(hub, env.id)?.cleanup;
  return (
    <section className="hub-card" aria-labelledby={`env-cleanup-${env.id}`}>
      <div className="hub-card-head">
        <h3 id={`env-cleanup-${env.id}`}>Limpeza</h3>
      </div>
      <p className={c && c.items > 0 ? "strong" : "muted"}>
        {!c
          ? "Pendências não informadas ao hub."
          : c.items > 0
            ? `${c.items} pendente(s) · ${humanBytes(c.bytes)} (verificado ${ago(c.checked_at, now)})`
            : `Nenhuma pendente (verificado ${ago(c.checked_at, now)})`}
      </p>
      <p className="muted">
        Worktrees, <span className="mono">target/</span> e branches dos itens concluídos: o plano, as regras e o limpar
        ficam em Manutenção › Limpeza, com filtro por projeto.
      </p>
      <div>
        <button type="button" className="btn" onClick={() => nav.openTab(maintenanceTab("cleanup", env.id))}>
          Abrir a limpeza de {env.name} ›
        </button>
      </div>
    </section>
  );
}
