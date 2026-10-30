import "../hub/hub.css";
import { useState, type ReactNode } from "react";
import { useNow } from "../App";
import { useNav } from "../state/nav";
import { useTower } from "../state/tower";
import type { HubSession } from "../protocol/hub-gen";
import { AgentRules } from "../hub/AgentRules";
import { isJira } from "../hub/tracker";
import {
  AssignmentsCard,
  CleanupCard,
  DangerZone,
  DocsCard,
  ProjectSecretsCard,
  RepositoryCard,
  SyncEnvCard,
  keyOf,
  projectSecretsOf,
  syncText,
  useSecrets,
  useSettingsRun,
} from "./settings";
import {
  DESKTOP_ONLY_SECTIONS,
  PROJECT_SECTIONS,
  PROJECT_SECTION_LABEL,
  type ProjectSettingsSection,
} from "./sections";

/**
 * Aba Configurações do projeto pela visão do hub (#17): o índice das seções
 * à esquerda e a seção pedida (`section` do endereço) à direita, uma por
 * vez, cada uma composta pelos cartões de `project/settings` (épico #12),
 * os que o Hub › Projetos usava antes da #18. Uma por vez para a seção ser o endereço
 * (paleta, deep link) sem depender de rolagem. Na web (só leitura), sem
 * Secrets e Zona de perigo e sem as ações de mudar (`readOnly`).
 */
export function ProjectSettings({
  owner,
  repo,
  section,
  onSection,
  assign = false,
  onAssignHandled,
}: {
  owner: string;
  repo: string;
  /** Já resolvida por `sectionOf` (na web, nunca uma do desktop). */
  section: ProjectSettingsSection;
  onSection: (s: ProjectSettingsSection) => void;
  /**
   * Pedido de atribuição do endereço (#18, "Atribuir a ambiente" do menu ⋯,
   * da paleta e do aviso da tela): na seção Ambientes, o cartão abre o
   * formulário e chama `onAssignHandled` para tirar o pedido do endereço.
   */
  assign?: boolean;
  onAssignHandled?: () => void;
}) {
  const { hub, web } = useTower();
  const nav = useNav();
  const now = useNow();
  const { busy, run, fail, error, done } = useSettingsRun();
  const settings = { busy, run, fail };
  const [editing, setEditing] = useState(false);
  // Secrets: só onde aparecem (a seção e a contagem da exclusão); a web não lê.
  const secrets = useSecrets(!web && (section === "secrets" || section === "danger"));

  const want = keyOf(owner, repo).toLowerCase();
  const p = hub.projects.find((x) => keyOf(x.project.gh_owner, x.project.gh_repo).toLowerCase() === want);
  if (!hub.connected || !p) {
    return (
      <p className="muted" role="status">
        {hub.connected
          ? "Carregando as configurações do projeto…"
          : "As configurações do projeto ficam no hub, que está fora do ar ou sem conexão configurada."}
      </p>
    );
  }

  const pr = p.project;
  const sections = PROJECT_SECTIONS.filter((s) => !web || !DESKTOP_ONLY_SECTIONS.has(s));
  /** Sessões vivas relatadas no hub para o projeto (o impacto de remover e de excluir). */
  const live: HubSession[] = (() => {
    const d = hub.dashboard;
    if (!d) return [];
    const ids = new Set(d.projects.find((x) => x.project.id === pr.id)?.work_items.map((w) => w.id) ?? []);
    return d.live_sessions.filter((s) => ids.has(s.work_item_id));
  })();
  const pos = hub.projects.findIndex((x) => x.project.id === pr.id) + 1;

  let body: ReactNode;
  switch (section) {
    case "general":
      body = (
        <RepositoryCard
          project={p}
          editing={editing}
          onEditingChange={setEditing}
          settings={settings}
          now={now}
          // A ordem entre projetos se muda no índice da atividade Projetos (#16);
          // aqui, só a posição.
          order={{ pos, total: hub.projects.length }}
        />
      );
      break;
    case "tracker": {
      const sync = syncText(p.sync, now);
      body = isJira(pr) ? (
        <SyncEnvCard project={p} settings={settings} now={now} readOnly={web} />
      ) : (
        <section className="hub-card" aria-labelledby={`pc-gh-${pr.id}`}>
          <div className="hub-card-head">
            <h3 id={`pc-gh-${pr.id}`}>GitHub</h3>
          </div>
          <dl className="hub-kv">
            <div>
              <dt>Repositório e issues</dt>
              <dd>
                <a className="mono" href={`https://github.com/${keyOf(pr.gh_owner, pr.gh_repo)}`} target="_blank" rel="noreferrer">
                  github.com/{keyOf(pr.gh_owner, pr.gh_repo)}
                </a>
                <span className="hub-card-sub">O hub sincroniza o espelho das issues direto do GitHub, com o token dele.</span>
              </dd>
            </div>
            <div>
              <dt>Sincronização</dt>
              <dd>
                <span className="hub-dot" data-tone={sync.tone} aria-hidden="true" /> {sync.text}
                {p.sync?.last_error && <span className="hub-card-sub">{p.sync.last_error}</span>}
              </dd>
            </div>
          </dl>
        </section>
      );
      break;
    }
    case "envs":
      body = (
        <AssignmentsCard
          project={p}
          settings={settings}
          live={live}
          readOnly={web}
          requested={assign}
          onRequestHandled={onAssignHandled}
        />
      );
      break;
    case "rules":
      body = <AgentRules owner={pr.gh_owner} repo={pr.gh_repo} epic={0} label={pr.name} now={now} className="hub-card" />;
      break;
    case "secrets":
      body = <ProjectSecretsCard project={pr} secrets={secrets} />;
      break;
    case "docs":
      body = <DocsCard project={p} settings={settings} readOnly={web} />;
      break;
    case "cleanup":
      body = <CleanupCard project={p} />;
      break;
    case "danger":
      body = (
        <DangerZone
          project={p}
          settings={settings}
          live={live}
          secretCount={projectSecretsOf(secrets, pr.gh_owner, pr.gh_repo).own.length}
          // O projeto não existe mais: volta à atividade Projetos (sem chave),
          // que mostra a mensagem com o snapshot anterior (esta tela desmonta).
          onDeleted={(notice) => nav.go({ name: "project", notice })}
        />
      );
      break;
  }

  return (
    <div className="pj-settings">
      <nav className="pj-settings-nav" aria-label="Seções das configurações do projeto">
        <ul>
          {sections.map((s) => (
            <li key={s}>
              <button
                type="button"
                aria-current={s === section ? "page" : undefined}
                onClick={() => {
                  setEditing(false);
                  onSection(s);
                }}
              >
                {PROJECT_SECTION_LABEL[s]}
              </button>
            </li>
          ))}
        </ul>
      </nav>
      {/* `key`: cada projeto e seção guarda o próprio rascunho; trocar descarta. */}
      <div key={`${pr.id}:${section}`} className="pj-settings-body" role="group" aria-label={PROJECT_SECTION_LABEL[section]}>
        <div className="hub-page-head">
          <h2>{PROJECT_SECTION_LABEL[section]}</h2>
          {section === "general" && !web && !editing && (
            <div className="hub-page-actions">
              <button type="button" className="btn" onClick={() => setEditing(true)}>
                Editar <span className="sr-only">{pr.name}</span>
              </button>
            </div>
          )}
        </div>
        {body}
        {error && (
          <p role="alert" className="start-error">
            {error}
          </p>
        )}
        {done && (
          <p role="status" className="state-on">
            {done}
          </p>
        )}
      </div>
    </div>
  );
}
