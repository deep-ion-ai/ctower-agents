import { useEffect, useState, type DragEvent, type KeyboardEvent } from "react";
import { useNow } from "../App";
import { useNav } from "../state/nav";
import { useTower } from "../state/tower";
import type { HubHealth, HubProject, HubProjectView, HubSession, ProjectAdded } from "../protocol/hub-gen";
import { projectCounts } from "./HubHealth";
import { hubTab, type ProjectRef } from "./sections";
import {
  gitlabOf,
  hostsOf,
  hubSupportsJira,
  jiraOf,
  kindOf,
  providerParams,
  type ProjectKindForm,
} from "./tracker";
import { ProviderIcon } from "../project/ProviderIcon";
import {
  AssignmentsCard,
  DangerZone,
  EMPTY_PROJECT_FORM as EMPTY,
  GeneralForm,
  ProjectSecretsCard,
  RepositoryCard,
  SyncEnvCard,
  branches,
  keyOf,
  plural,
  projectSecretsOf,
  syncText,
  useSecrets,
  useSettingsRun,
  type ProjectForm,
  type Tone,
} from "../project/settings";
import { AgentRules } from "./AgentRules";

// Fica exportada daqui para quem já importa de HubProjects.
export { syncText };

/** Onde soltar o projeto arrastado: antes ou depois do alvo. */
interface Drop {
  key: string;
  where: "before" | "after";
}

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
  addUrl,
  addKind,
  onAddDone,
  health = null,
}: {
  /** Projeto mostrado no detalhe; sem ele (ou se sumiu), o primeiro. */
  project?: ProjectRef;
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
  const { hub, api, web } = useTower();
  const nav = useNav();
  const now = useNow();
  const [filter, setFilter] = useState("");
  const [add, setAdd] = useState<ProjectForm | null>(null);
  const [editing, setEditing] = useState(false);
  const { busy, run, fail, error, done } = useSettingsRun();
  const settings = { busy, run, fail };
  const [dragging, setDragging] = useState<string | null>(null);
  const [drop, setDrop] = useState<Drop | null>(null);
  const secrets = useSecrets(!web);

  useEffect(() => {
    if (addUrl !== null && (addUrl || addKind === "jira")) setAdd({ ...EMPTY, ...hostsOf(addKind ?? "github"), url: addUrl });
  }, [addUrl, addKind]);

  const projects = hub.projects;
  const jiraOk = hubSupportsJira(hub.info?.protocol ?? null);

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

  const select = (pr: HubProject) => {
    setAdd(null);
    setEditing(false);
    // Na própria aba (a seção pode estar na aba "hub", vinda de `go`): troca só o endereço.
    nav.openTab({ ...hubTab("projects", { project: { owner: pr.gh_owner, repo: pr.gh_repo } }), id: nav.current });
  };


  const save = async () => {
    if (!add) return;
    let added: ProjectRef | undefined;
    const ok = await run(async () => {
      // Jira + GitLab: sem URL do GitHub; o repositório vem da URL do GitLab.
      let source: Record<string, unknown> = { url: add.url.trim() };
      if (kindOf(add.code, add.issues) === "jira") {
        const pv = providerParams(add.jira);
        if ("error" in pv) throw new Error(pv.error);
        source = {
          ...pv,
          ...(add.jira.branch_pattern.trim() ? { branch_pattern: add.jira.branch_pattern.trim() } : {}),
          attribution: add.jira.attribution,
        };
      }
      const r = await api.hubCall<ProjectAdded>("hub.projects.add", {
        ...source,
        ...(add.name.trim() ? { name: add.name.trim() } : {}),
        ...(add.base_branch.trim() ? { base_branch: add.base_branch.trim() } : {}),
        ...(add.protected.trim() ? { protected_branches: branches(add.protected) } : {}),
      });
      added = { owner: r.project.gh_owner, repo: r.project.gh_repo };
      const extra = r.reapplied > 0 ? ` ${r.reapplied} linha(s) da quarentena reaplicada(s).` : "";
      return `${r.project.gh_owner}/${r.project.gh_repo} cadastrado no hub.${extra}`;
    });
    if (ok) {
      setAdd(null);
      onAddDone(added);
    }
  };


  // Ordem entre projetos (desempata o ranking do portfólio): `pr` vai para
  // antes ou depois de `other`.
  const move = (pr: HubProject, to: "before" | "after", other: HubProject) =>
    void run(async () => {
      await api.hubCall("hub.projects.move", {
        owner: pr.gh_owner,
        repo: pr.gh_repo,
        [to]: { owner: other.gh_owner, repo: other.gh_repo },
      });
      const i = projects.findIndex((p) => p.project.id === pr.id);
      const j = projects.findIndex((p) => p.project.id === other.id);
      return `${pr.name} ${j < i ? "subiu" : "desceu"} na ordem dos projetos.`;
    });

  const neighbors = (pr: HubProject) => {
    const i = projects.findIndex((p) => p.project.id === pr.id);
    return { prev: projects[i - 1]?.project, next: projects[i + 1]?.project };
  };

  /** Sobe ou desce um lugar (botões do detalhe e Alt+↑/↓ na lista). */
  const step = (pr: HubProject, up: boolean) => {
    const { prev, next } = neighbors(pr);
    if (up && prev) move(pr, "before", prev);
    if (!up && next) move(pr, "after", next);
  };

  const onKey = (e: KeyboardEvent, pr: HubProject) => {
    if (web || !e.altKey || (e.key !== "ArrowUp" && e.key !== "ArrowDown") || busy) return;
    e.preventDefault();
    step(pr, e.key === "ArrowUp");
  };

  // Arrasto (HTML5): metade de cima do alvo = antes; de baixo = depois.
  const dropWhere = (e: DragEvent<HTMLElement>): Drop["where"] => {
    const r = e.currentTarget.getBoundingClientRect();
    return e.clientY < r.top + r.height / 2 ? "before" : "after";
  };
  const onDrop = (target: HubProject, where: Drop["where"]) => {
    const from = projects.find((p) => keyOf(p.project.gh_owner, p.project.gh_repo) === dragging)?.project;
    setDragging(null);
    setDrop(null);
    if (!from || from.id === target.id || busy) return;
    const { prev, next } = neighbors(target);
    // Soltar ao lado de onde já está não muda a ordem.
    if ((where === "before" && prev?.id === from.id) || (where === "after" && next?.id === from.id)) return;
    move(from, where, target);
  };

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

        <AssignmentsCard project={p} settings={settings} live={sessions} readOnly={web} />

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
                  setAdd({ ...EMPTY });
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
                <li
                  key={pr.id}
                  draggable={!busy && !web}
                  data-drop={drop?.key === key ? drop.where : undefined}
                  data-dragging={dragging === key ? "true" : undefined}
                  onDragStart={(e) => {
                    setDragging(key);
                    try {
                      e.dataTransfer.effectAllowed = "move";
                      e.dataTransfer.setData("text/plain", key);
                    } catch {
                      // Sem dataTransfer (testes): o estado basta.
                    }
                  }}
                  onDragOver={(e) => {
                    if (!dragging || dragging === key) return;
                    e.preventDefault();
                    const where = dropWhere(e);
                    if (drop?.key !== key || drop.where !== where) setDrop({ key, where });
                  }}
                  onDragLeave={() => drop?.key === key && setDrop(null)}
                  onDrop={(e) => {
                    e.preventDefault();
                    onDrop(pr, dropWhere(e));
                  }}
                  onDragEnd={() => {
                    setDragging(null);
                    setDrop(null);
                  }}
                >
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
            <form
              className="env-form"
              aria-label="Novo projeto"
              onSubmit={(e) => {
                e.preventDefault();
                void save();
              }}
            >
              <h2 className="hub-ptitle hub-wide">Cadastrar projeto</h2>
              <GeneralForm form={add} onChange={setAdd} prefix="np" withUrl />
              <div className="form-actions">
                <button
                  type="button"
                  className="btn"
                  onClick={() => {
                    setAdd(null);
                    onAddDone();
                  }}
                >
                  Cancelar
                </button>
                <button
                  type="submit"
                  className="btn btn-primary"
                  disabled={
                    busy ||
                    (() => {
                      const kind = kindOf(add.code, add.issues);
                      if (kind === "github") return !add.url.trim();
                      if (kind === "jira")
                        return !jiraOk || !add.jira.jira_url.trim() || !add.jira.jira_key.trim() || !add.jira.gitlab_url.trim();
                      return true;
                    })()
                  }
                >
                  {busy ? "Validando…" : "Cadastrar"}
                </button>
              </div>
            </form>
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