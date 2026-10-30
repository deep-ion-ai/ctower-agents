import { useEffect, useState } from "react";
import { useNow } from "../App";
import { useNav } from "../state/nav";
import { useTower } from "../state/tower";
import type { HubHealth, HubProject, HubProjectView, HubSession } from "../protocol/hub-gen";
import { projectCounts } from "./HubHealth";
import { hubTab, type ProjectRef } from "./sections";
import { gitlabOf, jiraOf, type ProjectKindForm } from "./tracker";
import { ProviderIcon } from "../project/ProviderIcon";
import {
  AssignmentsCard,
  DangerZone,
  ProjectSecretsCard,
  RepositoryCard,
  SyncEnvCard,
  keyOf,
  plural,
  projectSecretsOf,
  syncText,
  useSecrets,
  useSettingsRun,
  type Tone,
} from "../project/settings";
import { NewProjectDialog, type RegisterIntent } from "../project/NewProjectDialog";
import { useProjectOrder } from "../project/order";
import { AgentRules } from "./AgentRules";

// Fica exportada daqui para quem já importa de HubProjects.
export { syncText };

/**
 * Estado do projeto na lista: atribuição com erro ou pendente pede atenção
 * antes da sincronização; sem nada disso, ambientes e sincronização.
 */
export function projectState(p: HubProjectView, now: number): { text: string; tone: Tone } {
  const errors = p.assignments.filter((a) => a.state === "error").length;
  if (errors > 0) return { text: plural(errors, "atribuição com erro", "atribuições com erro"), tone: "error" };
  const pending = p.assignments.filter((a) => a.state === "pending").length;
  if (pending > 0) return { text: plural(pending, "atribuição pendente", "atribuições pendentes"), tone: "attention" };
  const sync = syncText(p.sync, now);
  const envs = p.assignments.length === 0 ? "sem ambiente" : plural(p.assignments.length, "ambiente", "ambientes");
  return { text: `${envs} · ${sync.text}`, tone: p.assignments.length === 0 && sync.tone === "ok" ? "neutral" : sync.tone };
}

/**
 * Projetos no hub (spec §3.1–§3.3) em lista e detalhe (#314). À esquerda,
 * os projetos na ordem entre eles (`rank`, reordenável por arrasto, por
 * Alt+↑/↓ ou pelos botões do detalhe), com filtro e "Cadastrar". À direita,
 * o projeto escolhido (`project`, senão o primeiro): repositório e edição,
 * atribuições por ambiente (estado, clone, sessões vivas, remover,
 * atribuir), secrets e variáveis do projeto com a entrega por ambiente, e a
 * exclusão, que mostra o impacto (`DeleteImpact`) e pede a confirmação que
 * o hub pedir.
 */
export function HubProjects({
  project,
  assign: assignReq,
  addUrl,
  addKind,
  onAddDone,
  health = null,
}: {
  /** Projeto mostrado no detalhe; sem ele (ou se sumiu), o primeiro. */
  project?: ProjectRef;
  /** Abrir o formulário de atribuição do projeto mostrado (`Screen` do hub, #7). */
  assign?: true;
  addUrl: string | null;
  /** Tipo do formulário pedido (quarentena de projeto GitLab, #352); ausente = GitHub. */
  addKind?: ProjectKindForm;
  /** Cadastro fechado; com `added`, o projeto recém-cadastrado. */
  onAddDone: (added?: ProjectRef) => void;
  /** Sinais de saúde do hub, para a contagem em cada projeto. */
  health?: HubHealth | null;
}) {
  // Web (épico #205): só leitura; cadastro, ordem, atribuições, secrets e
  // exclusão ficam no desktop (spec §4.1 e §10.2).
  const { hub, web } = useTower();
  const nav = useNav();
  const now = useNow();
  const [filter, setFilter] = useState("");
  /** Cadastro aberto; `id` novo a cada pedido, para o formulário recomeçar. */
  const [add, setAdd] = useState<{ id: number; intent: RegisterIntent } | null>(null);
  const [editing, setEditing] = useState(false);
  const { busy, run, fail, error, done } = useSettingsRun();
  const settings = { busy, run, fail };
  const secrets = useSecrets(!web);

  useEffect(() => {
    if (addUrl !== null) setAdd((a) => ({ id: (a?.id ?? 0) + 1, intent: { url: addUrl, kind: addKind } }));
  }, [addUrl, addKind]);

  const projects = hub.projects;

  const wanted = project ? keyOf(project.owner, project.repo).toLowerCase() : null;
  const sel =
    (wanted && projects.find((p) => keyOf(p.project.gh_owner, p.project.gh_repo).toLowerCase() === wanted)) ||
    projects[0] ||
    null;
  const q = filter.trim().toLowerCase();
  const shown = q
    ? projects.filter(
        (p) => p.project.name.toLowerCase().includes(q) || keyOf(p.project.gh_owner, p.project.gh_repo).toLowerCase().includes(q),
      )
    : projects;

  // Pedido de atribuição vindo de fora: vale para o projeto mostrado, e sai da aba
  // quando o cartão o atende (como o cadastro).
  const shownWanted = !!sel && (!wanted || keyOf(sel.project.gh_owner, sel.project.gh_repo).toLowerCase() === wanted);
  const assignFor = assignReq && shownWanted ? sel : null;

  const select = (pr: HubProject) => {
    setAdd(null);
    setEditing(false);
    // Na própria aba (a seção pode estar na aba "hub", vinda de `go`): troca só o endereço.
    nav.openTab({ ...hubTab("projects", { project: { owner: pr.gh_owner, repo: pr.gh_repo } }), id: nav.current });
  };


  // Ordem entre projetos (arrasto, Alt+↑/↓ e os botões do detalhe): o mesmo
  // gancho do índice de Projetos (#16).
  const order = useProjectOrder(projects.map((p) => p.project), settings, !web);
  const { neighbors, step, onKey } = order;

  /** Sessões vivas relatadas no hub para o projeto (pelos work items dele). */
  const liveOf = (pr: HubProject): HubSession[] => {
    const d = hub.dashboard;
    if (!d) return [];
    const ids = new Set(d.projects.find((x) => x.project.id === pr.id)?.work_items.map((w) => w.id) ?? []);
    return d.live_sessions.filter((s) => ids.has(s.work_item_id));
  };

  const detail = (p: HubProjectView) => {
    const pr = p.project;
    const key = keyOf(pr.gh_owner, pr.gh_repo);
    const counts = projectCounts(health, pr.gh_owner, pr.gh_repo);
    const { prev, next } = neighbors(pr);
    const sessions = liveOf(pr);
    const jira = jiraOf(pr);
    const gitlab = gitlabOf(pr);
    const { own } = projectSecretsOf(secrets, pr.gh_owner, pr.gh_repo);

    return (
      // `key`: cada seção guarda o próprio rascunho; trocar de projeto o descarta.
      <div key={pr.id} className="hub-pdetail-body" role="group" aria-label={`Projeto ${pr.name}`}>

        <div className="hub-page-head">
          <h2 className="hub-ptitle">{pr.name}</h2>
          <ProviderIcon project={pr} size={16} />
          {jira ? (
            <span className="small">
              <a className="mono" href={`${jira.url}/projects/${jira.project_key}`} target="_blank" rel="noreferrer">
                Jira {jira.project_key}
              </a>
              {gitlab && (
                <>
                  {" · "}
                  <a className="mono" href={`https://${gitlab.host}/${gitlab.path}`} target="_blank" rel="noreferrer">
                    {gitlab.host}/{gitlab.path}
                  </a>
                </>
              )}
            </span>
          ) : (
            <a className="mono small" href={`https://github.com/${key}`} target="_blank" rel="noreferrer">
              github.com/{key}
            </a>
          )}
          {health && (
            <span className="health-counts" aria-label={`${counts.critico} crítico(s), ${counts.aviso} aviso(s)`}>
              {counts.critico + counts.aviso === 0 ? (
                <span className="state-on small">sem riscos</span>
              ) : (
                <>
                  {counts.critico > 0 && (
                    <span className="badge" data-tone="error">
                      {counts.critico} crítico{counts.critico > 1 ? "s" : ""}
                    </span>
                  )}
                  {counts.aviso > 0 && (
                    <span className="badge" data-tone="attention">
                      {counts.aviso} aviso{counts.aviso > 1 ? "s" : ""}
                    </span>
                  )}
                </>
              )}
            </span>
          )}
          <div className="hub-page-actions">
            <button
              type="button"
              className="btn"
              onClick={() => nav.go({ name: "project", owner: pr.gh_owner, repo: pr.gh_repo })}
            >
              Abrir árvore
            </button>
            {!web && (
              <button
                type="button"
                className="btn"
                onClick={() => setEditing(true)}
              >
                Editar <span className="sr-only">{pr.name}</span>
              </button>
            )}
          </div>
        </div>

        <RepositoryCard
          project={p}
          editing={editing}
          onEditingChange={setEditing}
          settings={settings}
          now={now}
          readOnly={web}
          order={{
            pos: projects.findIndex((x) => x.project.id === pr.id) + 1,
            total: projects.length,
            canUp: !!prev,
            canDown: !!next,
            onStep: (up) => step(pr, up),
          }}
        />

        {jira && <SyncEnvCard project={p} settings={settings} now={now} readOnly={web} />}

        <AssignmentsCard
          project={p}
          settings={settings}
          live={sessions}
          readOnly={web}
          requested={assignFor?.project.id === pr.id}
          onRequestHandled={() =>
            nav.openTab({ ...hubTab("projects", { project: { owner: pr.gh_owner, repo: pr.gh_repo } }), id: nav.current })
          }
        />

        <AgentRules owner={pr.gh_owner} repo={pr.gh_repo} epic={0} label={pr.name} now={now} className="hub-card" />

        {!web && (
          <>
            <ProjectSecretsCard project={pr} secrets={secrets} />
            <DangerZone project={p} settings={settings} live={sessions} secretCount={own.length} />
          </>
        )}
      </div>
    );
  };

  return (
    <section aria-labelledby="hub-projetos" className="hub-projects">
      <h2 id="hub-projetos" className="sr-only">
        Projetos · {projects.length}
      </h2>
      <div className="hub-split">
        <div className="hub-plist" role="region" aria-label="Projetos cadastrados">
          <div className="hub-plist-tool">
            <label htmlFor="hub-pfilter" className="sr-only">
              Filtrar projetos
            </label>
            <input
              id="hub-pfilter"
              type="search"
              placeholder="Filtrar projetos"
              value={filter}
              onChange={(e) => setFilter(e.target.value)}
            />
            {!web && (
              <button
                type="button"
                className="btn btn-primary btn-sm"
                aria-label="Cadastrar projeto"
                onClick={() => {
                  setEditing(false);
                  setAdd((a) => ({ id: (a?.id ?? 0) + 1, intent: { url: "" } }));
                }}
              >
                + Cadastrar
              </button>
            )}
          </div>
          {projects.length > 0 && shown.length === 0 && (
            <p className="muted hub-card-msg">Nenhum projeto com “{filter.trim()}”.</p>
          )}
          <ul className="hub-pitems">
            {shown.map((p) => {
              const pr = p.project;
              const key = keyOf(pr.gh_owner, pr.gh_repo);
              const st = projectState(p, now);
              const pos = projects.indexOf(p) + 1;
              const current = !add && sel?.project.id === pr.id;
              return (
                <li key={pr.id} {...order.itemProps(pr)}>
                  <button
                    type="button"
                    className="hub-pitem"
                    aria-label={pr.name}
                    aria-describedby={`pi-${pr.id}`}
                    aria-current={current ? "true" : undefined}
                    onClick={() => select(pr)}
                    onKeyDown={(e) => onKey(e, pr)}
                  >
                    <span className="hub-pitem-top">
                      <strong>{pr.name}</strong>
                      <span className="badge">#{pos}</span>
                    </span>
                    <span id={`pi-${pr.id}`} className="hub-pitem-meta">
                      <span className="mono">{key}</span>
                      <span>
                        <span className="hub-dot" data-tone={st.tone} aria-hidden="true" /> {st.text}
                      </span>
                    </span>
                  </button>
                </li>
              );
            })}
          </ul>
          {!web && projects.length > 1 && (
            <p className="muted hint hub-plist-foot">
              A ordem da lista é a prioridade entre projetos. Arraste para reordenar, ou use Alt+↑/↓ no projeto.
            </p>
          )}
        </div>

        <div className="hub-pdetail">
          {add ? (
            <NewProjectDialog
              key={add.id}
              intent={add.intent}
              settings={settings}
              onDone={(added) => {
                setAdd(null);
                onAddDone(added);
              }}
            />
          ) : sel ? (
            detail(sel)
          ) : (
            <p className="muted">
              Nenhum projeto no hub. Cadastre pela URL do GitHub (o hub valida o repositório e a base) ou como Jira +
              GitLab (o ambiente de sync valida).
            </p>
          )}
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
          <p className="muted hint">
            Projetos nascem e são editados só no hub. Cada ambiente puxa a configuração dos projetos atribuídos a ele,
            clona ou valida o clone e relata o resultado: <strong>pendente</strong> até o relato chegar,{" "}
            <strong>aplicada</strong> ou <strong>com erro</strong>. Remover uma atribuição não apaga o clone nem encerra
            sessões.
          </p>
        </div>
      </div>
    </section>
  );
}