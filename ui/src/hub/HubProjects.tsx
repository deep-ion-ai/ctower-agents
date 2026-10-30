import { useCallback, useEffect, useRef, useState, type DragEvent, type KeyboardEvent } from "react";
import { useNow } from "../App";
import { useNav } from "../state/nav";
import { useTower } from "../state/tower";
import { assignmentText, repoPathText, splitProjectKey } from "../util/hub";
import { ago } from "../util/time";
import type {
  AssignmentAdded,
  AssignmentRemoved,
  DeleteImpact,
  HubAssignment,
  HubHealth,
  HubProject,
  HubProjectView,
  HubSession,
  ProjectAdded,
  ProjectDeleted,
  SecretsList,
  SyncState,
} from "../protocol/hub-gen";
import { projectCounts } from "./HubHealth";
import { DELIVERY_TEXT } from "./HubSecrets";
import { errText, needsConfirm } from "./calls";
import { hubTab, type ProjectRef } from "./sections";
import { TrackerReadiness } from "./TrackerReadiness";
import { AgentRules } from "./AgentRules";
import {
  EMPTY_JIRA,
  GITHUB_BRANCH_PATTERN,
  JIRA_BRANCH_PATTERN,
  JIRA_PROTOCOL,
  branchPreview,
  gitlabOf,
  hostsOf,
  hubSupportsJira,
  isJira,
  jiraFormOf,
  jiraOf,
  kindOf,
  providerParams,
  DEFAULT_MAX_ISSUES,
  MAX_ISSUES_CEILING,
  type CodeHostForm,
  type IssueHostForm,
  type JiraForm,
  type ProjectKindForm,
} from "./tracker";
import { ProviderIcon } from "../project/ProviderIcon";
import { docsDirName, docsFormOf, docsPatch, docsText, type DocsForm } from "./docs";

interface ProjectForm {
  /**
   * Repositório do código-fonte e de demandas (#348); o tipo do projeto sai
   * dos dois (`kindOf`): GitHub + GitHub ou GitLab + Jira.
   */
  code: CodeHostForm;
  issues: IssueHostForm;
  url: string;
  name: string;
  base_branch: string;
  /** Separadas por vírgula. */
  protected: string;
  jira: JiraForm;
  /**
   * Docs do projeto (#444), só na edição; `null` = cadastro novo ou hub
   * anterior à #440 (sem `docs`).
   */
  docs: DocsForm | null;
}

const EMPTY: ProjectForm = {
  ...hostsOf("github"),
  url: "",
  name: "",
  base_branch: "",
  protected: "",
  jira: EMPTY_JIRA,
  docs: null,
};

const branches = (s: string) =>
  s
    .split(",")
    .map((b) => b.trim())
    .filter(Boolean);

const keyOf = (o: string, r: string) => `${o}/${r}`;

/**
 * Exclusão ou remoção de atribuição esperando confirmação; `hub` = o hub pediu
 * `confirm`. Na exclusão, `impact` é o de `hub.projects.impact` (`null` =
 * consultando) e `local` diz que a consulta falhou e o impacto é o que a UI sabe.
 */
type Pending =
  | { kind: "delete"; owner: string; repo: string; hub: string | null; impact: DeleteImpact | null; local: boolean }
  | { kind: "unassign"; a: HubAssignment; hub: string };

/** Onde soltar o projeto arrastado: antes ou depois do alvo. */
interface Drop {
  key: string;
  where: "before" | "after";
}

type Tone = "ok" | "attention" | "error" | "neutral";

const plural = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`;

/**
 * Sincronização em uma linha: a do GitHub pelo hub ou, no projeto Jira, a
 * que o ambiente de sync relatou (o mesmo `sync_state`).
 */
export function syncText(s: SyncState | null, now: number): { text: string; tone: Tone } {
  if (!s) return { text: "ainda não sincronizado", tone: "neutral" };
  if (s.last_error) return { text: `sincronização com erro ${ago(s.last_attempt_at, now)}`, tone: "error" };
  if (s.last_ok_at === null) return { text: "ainda não sincronizado", tone: "neutral" };
  return { text: `sincronizado ${ago(s.last_ok_at, now)}`, tone: "ok" };
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
  const { hub, api, refreshHub, web } = useTower();
  const nav = useNav();
  const now = useNow();
  const [filter, setFilter] = useState("");
  const [add, setAdd] = useState<ProjectForm | null>(null);
  const [edit, setEdit] = useState<{ key: string; form: ProjectForm } | null>(null);
  const [assign, setAssign] = useState<{ key: string; env: string; repo_path: string } | null>(null);
  const [syncEnv, setSyncEnv] = useState<{ key: string; env: string } | null>(null);
  const [pending, setPending] = useState<Pending | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [done, setDone] = useState<string | null>(null);
  const [dragging, setDragging] = useState<string | null>(null);
  const [drop, setDrop] = useState<Drop | null>(null);
  const secrets = useSecrets(!web);

  useEffect(() => {
    if (addUrl !== null && (addUrl || addKind === "jira")) setAdd({ ...EMPTY, ...hostsOf(addKind ?? "github"), url: addUrl });
  }, [addUrl, addKind]);

  const projects = hub.projects;
  // Hub anterior ao protocolo 1.5 (épico #353) não cadastra Jira + GitLab (ignora `tracker` e `forge`).
  const protocol = hub.info?.protocol ?? null;
  const jiraOk = hubSupportsJira(protocol);
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
    setEdit(null);
    setAssign(null);
    setSyncEnv(null);
    setPending(null);
    // Na própria aba (a seção pode estar na aba "hub", vinda de `go`): troca só o endereço.
    nav.openTab({ ...hubTab("projects", { project: { owner: pr.gh_owner, repo: pr.gh_repo } }), id: nav.current });
  };

  /** Roda uma mudança no hub e relê tudo; erro fica na seção. */
  const run = async (f: () => Promise<string | null>) => {
    setBusy(true);
    setError(null);
    setDone(null);
    try {
      setDone(await f());
      await refreshHub();
      return true;
    } catch (e) {
      setError(errText(e));
      return false;
    } finally {
      setBusy(false);
    }
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

  const update = async (p: HubProjectView) => {
    if (!edit) return;
    const cur = p.project;
    const f = edit.form;
    const patch: Record<string, unknown> = {
      ...(f.name.trim() && f.name.trim() !== cur.name ? { name: f.name.trim() } : {}),
      ...(f.base_branch.trim() && f.base_branch.trim() !== cur.base_branch ? { base_branch: f.base_branch.trim() } : {}),
      ...(branches(f.protected).join(",") !== cur.protected_branches.join(",")
        ? { protected_branches: branches(f.protected) }
        : {}),
    };
    if (isJira(cur)) {
      // Rastreador e forja vão inteiros quando algo deles mudou; o tipo e o
      // repositório não mudam (o hub recusa).
      const was = jiraFormOf(cur);
      const j = f.jira;
      const same = (keys: (keyof JiraForm)[]) => keys.every((k) => String(j[k]).trim() === String(was[k]).trim());
      if (!same(["jira_url", "jira_key", "active_sprint_only", "jql", "max_issues", "jira_cmd", "type_map", "epic_link_field", "epic_name_field", "sprint_field", "gitlab_url", "gitlab_cmd"])) {
        const pv = providerParams(j);
        if ("error" in pv) {
          setError(pv.error);
          return;
        }
        Object.assign(patch, pv);
      }
      if (j.branch_pattern.trim() !== was.branch_pattern.trim()) patch.branch_pattern = j.branch_pattern.trim();
      if (j.attribution !== was.attribution) patch.attribution = j.attribution;
    }
    // Docs (#444): o objeto inteiro, se mudou; hub sem `docs` não recebe.
    const docs = docsPatch(f.docs, cur);
    if (docs) patch.docs = docs;
    if (Object.keys(patch).length === 0) {
      setEdit(null);
      return;
    }
    const ok = await run(async () => {
      await api.hubCall("hub.projects.update", { owner: cur.gh_owner, repo: cur.gh_repo, ...patch });
      return `${cur.gh_owner}/${cur.gh_repo} atualizado; os ambientes recebem no próximo pull.`;
    });
    if (ok) setEdit(null);
  };

  const remove = async (owner: string, repo: string, confirm: boolean) => {
    setBusy(true);
    setError(null);
    setDone(null);
    try {
      const r = await api.hubCall<ProjectDeleted>("hub.projects.delete", {
        owner,
        repo,
        ...(confirm ? { confirm: true } : {}),
      });
      setPending(null);
      setDone(`${owner}/${repo} excluído do hub. Snapshot anterior: ${r.snapshot}`);
      await refreshHub();
    } catch (e) {
      const m = errText(e);
      if (!confirm && needsConfirm(m))
        setPending((p) => (p?.kind === "delete" ? { ...p, hub: m } : { kind: "delete", owner, repo, hub: m, impact: null, local: true }));
      else setError(m);
    } finally {
      setBusy(false);
    }
  };

  /**
   * Abre a confirmação e consulta o impacto exato no hub. Sem a consulta
   * (hub antigo ou erro), fica o impacto que a UI calcula.
   */
  const askDelete = async (owner: string, repo: string) => {
    const same = (p: Pending | null) => p?.kind === "delete" && keyOf(p.owner, p.repo) === keyOf(owner, repo);
    setPending({ kind: "delete", owner, repo, hub: null, impact: null, local: false });
    try {
      const impact = await api.hubCall<DeleteImpact>("hub.projects.impact", { owner, repo });
      setPending((p) => (same(p) ? { ...(p as Extract<Pending, { kind: "delete" }>), impact } : p));
    } catch {
      setPending((p) => (same(p) ? { ...(p as Extract<Pending, { kind: "delete" }>), local: true } : p));
    }
  };

  const addAssignment = async () => {
    if (!assign) return;
    // Na última barra: o `owner` do GitLab tem barras (#352).
    const { owner, repo } = splitProjectKey(assign.key);
    const ok = await run(async () => {
      const r = await api.hubCall<AssignmentAdded>("hub.assignments.add", {
        owner,
        repo,
        env: assign.env,
        ...(assign.repo_path.trim() ? { repo_path: assign.repo_path.trim() } : {}),
      });
      const sync = r.sync_env ? ` ${r.assignment.env_name} é o ambiente de sync do projeto.` : "";
      return r.changed
        ? `${assign.key} atribuído a ${r.assignment.env_name}; o ambiente aplica no próximo pull.${sync}`
        : `${assign.key} já estava atribuído a ${r.assignment.env_name} assim.`;
    });
    if (ok) setAssign(null);
  };

  /** Troca o ambiente de sync do projeto Jira (spec §5.1), entre os atribuídos. */
  const changeSyncEnv = async (pr: HubProject, env: string, name: string) => {
    const ok = await run(async () => {
      await api.hubCall("hub.projects.update", { owner: pr.gh_owner, repo: pr.gh_repo, sync_env: env });
      return `${name} é o ambiente de sync de ${pr.name}; os ambientes recebem no próximo pull.`;
    });
    if (ok) setSyncEnv(null);
  };

  const unassign = async (a: HubAssignment, confirm: boolean) => {
    setBusy(true);
    setError(null);
    setDone(null);
    try {
      const r = await api.hubCall<AssignmentRemoved>("hub.assignments.remove", {
        owner: a.gh_owner,
        repo: a.gh_repo,
        env: a.env_id,
        ...(confirm ? { confirm: true } : {}),
      });
      setPending(null);
      const live = r.live_sessions.length;
      setDone(
        `Atribuição de ${a.gh_owner}/${a.gh_repo} a ${a.env_name} removida. O clone e os worktrees ficam no ambiente` +
          (live > 0 ? `; ${live} sessão(ões) viva(s) seguem até terminarem.` : "."),
      );
      await refreshHub();
    } catch (e) {
      const m = errText(e);
      if (!confirm && needsConfirm(m)) setPending({ kind: "unassign", a, hub: m });
      else setError(m);
    } finally {
      setBusy(false);
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

  /**
   * Os dois repositórios do projeto, cada um com os campos dele. Na edição o
   * tipo não muda (o hub recusa): os seletores só mostram o atual.
   */
  const formFields = (f: ProjectForm, set: (f: ProjectForm) => void, prefix: string, withUrl: boolean) => {
    const kind = kindOf(f.code, f.issues);
    return (
      <>
        <div className="field">
          <label htmlFor={`${prefix}-code`}>Repositório do código-fonte</label>
          <select
            id={`${prefix}-code`}
            value={f.code}
            disabled={!withUrl}
            onChange={(e) => set({ ...f, code: e.target.value as CodeHostForm })}
          >
            <option value="github">GitHub</option>
            <option value="gitlab" disabled={withUrl && !jiraOk}>
              GitLab
            </option>
          </select>
        </div>
        <div className="field">
          <label htmlFor={`${prefix}-issues`}>Repositório de demandas (issues)</label>
          <select
            id={`${prefix}-issues`}
            value={f.issues}
            disabled={!withUrl}
            onChange={(e) => set({ ...f, issues: e.target.value as IssueHostForm })}
          >
            <option value="github">GitHub</option>
            <option value="jira" disabled={withUrl && !jiraOk}>
              Jira
            </option>
          </select>
        </div>
        {withUrl && !jiraOk && (
          <p className="start-error hub-wide" role="note">
            O hub (protocolo {protocol}) não suporta Jira/GitLab; atualize o hub para {JIRA_PROTOCOL}+.
          </p>
        )}
        {withUrl && !kind && (
          <p className="start-error hub-wide" role="note">
            Combinação ainda não suportada: use GitHub + GitHub ou GitLab + Jira.
          </p>
        )}
        {withUrl && f.code === "github" && (
          <div className="field hub-wide">
            <label htmlFor={`${prefix}-url`}>URL do GitHub</label>
            <input
              id={`${prefix}-url`}
              type="text"
              placeholder="https://github.com/owner/repo"
              value={f.url}
              onChange={(e) => set({ ...f, url: e.target.value })}
            />
          </div>
        )}
        {f.code === "gitlab" && gitlabFields(f, set, prefix)}
        {f.issues === "jira" && jiraFields(f, set, prefix)}
        <div className="field">
          <label htmlFor={`${prefix}-name`}>Nome{withUrl ? " (opcional)" : ""}</label>
          <input id={`${prefix}-name`} type="text" value={f.name} onChange={(e) => set({ ...f, name: e.target.value })} />
        </div>
        <div className="field">
          <label htmlFor={`${prefix}-base`}>Branch base{withUrl ? " (padrão develop)" : ""}</label>
          <input
            id={`${prefix}-base`}
            type="text"
            value={f.base_branch}
            onChange={(e) => set({ ...f, base_branch: e.target.value })}
          />
        </div>
        <div className="field">
          <label htmlFor={`${prefix}-prot`}>Protegidas{withUrl ? " (padrão develop, main, master)" : ""}</label>
          <input
            id={`${prefix}-prot`}
            type="text"
            placeholder="develop, main"
            value={f.protected}
            onChange={(e) => set({ ...f, protected: e.target.value })}
          />
        </div>
      </>
    );
  };

  /**
   * Docs do projeto (#444): liga e desliga e os modelos de pastas, uma por
   * linha. Hub anterior à #440 não manda `docs`: só o aviso.
   */
  const docsFields = (f: ProjectForm, set: (f: ProjectForm) => void, prefix: string) => {
    const d = f.docs;
    if (!d) {
      return (
        <p className="muted hint hub-wide" role="note">
          Este hub não tem docs dos projetos (anterior ao protocolo 1.8): atualize o hub para configurar a pasta de docs.
        </p>
      );
    }
    const setD = (patch: Partial<DocsForm>) => set({ ...f, docs: { ...d, ...patch } });
    const area = (k: "project_dirs" | "epic_dirs", label: string, hint: string, placeholder: string) => (
      <div className="field hub-wide">
        <label htmlFor={`${prefix}-${k}`}>{label}</label>
        <textarea
          id={`${prefix}-${k}`}
          rows={3}
          placeholder={placeholder}
          value={d[k]}
          disabled={!d.enabled}
          aria-describedby={`${prefix}-${k}-hint`}
          onChange={(e) => setD({ [k]: e.target.value })}
        />
        <span id={`${prefix}-${k}-hint`} className="hint muted">
          {hint}
        </span>
      </div>
    );
    return (
      <fieldset className="hub-wide hub-docs">
        <legend>Docs do projeto (fora do repositório)</legend>
        <div className="field hub-wide">
          <label className="ctl" htmlFor={`${prefix}-docs`}>
            <input
              id={`${prefix}-docs`}
              type="checkbox"
              checked={d.enabled}
              onChange={(e) => setD({ enabled: e.target.checked })}
            />
            Pasta de docs do projeto
          </label>
          <span className="hint muted">
            {hub.info?.docs_dir
              ? <>Em <span className="mono">{hub.info.docs_dir}</span>, compartilhada com os ambientes pelo docs_root.</>
              : "O hub está sem docs_dir: as pastas só nascem nos ambientes com docs_root configurado."}{" "}
            Desligar não apaga nada.
          </span>
        </div>
        {area("project_dirs", "Pastas do projeto (em _projeto/)", "Uma por linha, caminho relativo (ex.: analises, specs/telas).", "analises\nspecs/telas")}
        {area("epic_dirs", "Pastas de cada épico (em epicos/<n>-<slug>/)", "Uma por linha; o README.md do épico é gerado à parte.", "prompts\nevidencias")}
      </fieldset>
    );
  };

  /** Campo de texto do Jira + GitLab, guardado em `f.jira`. */
  const jiraText = (f: ProjectForm, set: (f: ProjectForm) => void, prefix: string) => {
    const setJ = (patch: Partial<JiraForm>) => set({ ...f, jira: { ...f.jira, ...patch } });
    return (k: keyof JiraForm, label: string, placeholder = "", wide = false, hint = "") => (
      <div className={`field${wide ? " hub-wide" : ""}`}>
        <label htmlFor={`${prefix}-${k}`}>{label}</label>
        <input
          id={`${prefix}-${k}`}
          type="text"
          placeholder={placeholder}
          value={String(f.jira[k])}
          aria-describedby={hint ? `${prefix}-${k}-hint` : undefined}
          onChange={(e) => setJ({ [k]: e.target.value })}
        />
        {hint && (
          <span id={`${prefix}-${k}-hint`} className="hint muted">
            {hint}
          </span>
        )}
      </div>
    );
  };

  /** Campos do GitLab (#348): a URL de clone e o comando de credencial. */
  const gitlabFields = (f: ProjectForm, set: (f: ProjectForm) => void, prefix: string) => {
    const text = jiraText(f, set, prefix);
    return (
      <>
        {text("gitlab_url", "URL do repositório no GitLab (HTTPS ou SSH)", "https://gitlab.exemplo.com/grupo/projeto.git", true)}
        {text("gitlab_cmd", "Comando de credencial do GitLab", "pass show gitlab/token | head -1", true)}
      </>
    );
  };

  /**
   * Campos do Jira (#348), com o padrão de branch e a atribuição do projeto
   * Jira + GitLab. Credenciais: só o comando, que roda no ambiente; a senha
   * e o token nunca passam pelo hub.
   */
  const jiraFields = (f: ProjectForm, set: (f: ProjectForm) => void, prefix: string) => {
    const j = f.jira;
    const setJ = (patch: Partial<JiraForm>) => set({ ...f, jira: { ...j, ...patch } });
    const text = jiraText(f, set, prefix);
    const pattern = j.branch_pattern.trim() || JIRA_BRANCH_PATTERN;
    const preview = branchPreview(pattern, j.jira_key.trim().toUpperCase() || "PROJ");
    return (
      <>
        {text("jira_url", "URL do Jira", "https://jira.exemplo.com")}
        {text("jira_key", "Chave do projeto no Jira", "PROJ")}
        <div className="field hub-wide">
          <label className="ctl" htmlFor={`${prefix}-active_sprint_only`}>
            <input
              id={`${prefix}-active_sprint_only`}
              type="checkbox"
              checked={j.active_sprint_only}
              aria-describedby={`${prefix}-active_sprint_only-hint`}
              onChange={(e) => setJ({ active_sprint_only: e.target.checked })}
            />
            Sincronizar somente a sprint ativa
          </label>
          <span id={`${prefix}-active_sprint_only-hint`} className="hint muted">
            {j.active_sprint_only
              ? "O sync traz só as issues das sprints abertas do projeto (sprint in openSprints()), sem limite de quantidade."
              : "Desmarcado, informe o JQL do que sincronizar e o limite de issues."}
          </span>
        </div>
        {!j.active_sprint_only && (
          <>
            {text(
              "jql",
              "JQL do sync",
              "ex.: labels = minha-equipe AND updated >= -90d",
              true,
              "Obrigatório: o que é sincronizado do projeto (o primeiro sync lê issue por issue). Mudar o filtro remove do Control Tower as issues que saem dele, exceto as com work item (e os épicos delas).",
            )}
            <div className="field">
              <label htmlFor={`${prefix}-max_issues`}>Limite de issues sincronizadas</label>
              <input
                id={`${prefix}-max_issues`}
                type="number"
                min={1}
                max={MAX_ISSUES_CEILING}
                step={1}
                value={j.max_issues}
                aria-describedby={`${prefix}-max_issues-hint`}
                onChange={(e) => setJ({ max_issues: e.target.value })}
              />
              <span id={`${prefix}-max_issues-hint`} className="hint muted">
                Só as {j.max_issues.trim() || DEFAULT_MAX_ISSUES} issues mais recentes do JQL (por atualização) ficam no
                Control Tower, além das que têm work item. Padrão {DEFAULT_MAX_ISSUES}, máximo {MAX_ISSUES_CEILING}.
              </span>
            </div>
          </>
        )}
        {text("jira_cmd", "Comando de credencial do Jira", "pass show jira/token | head -1", true)}
        <div className="field">
          <label htmlFor={`${prefix}-branch_pattern`}>Padrão de branch</label>
          <input
            id={`${prefix}-branch_pattern`}
            type="text"
            placeholder={JIRA_BRANCH_PATTERN}
            value={j.branch_pattern}
            aria-describedby={`${prefix}-bp-prev`}
            onChange={(e) => setJ({ branch_pattern: e.target.value })}
          />
          <span id={`${prefix}-bp-prev`} className="hint muted">
            {preview ? (
              <>
                Prévia: <span className="mono">{preview}</span>
              </>
            ) : (
              "Padrão inválido: use {prefix}, {n}, {key} e {slug}, com {n} ou {key}."
            )}
          </span>
        </div>
        <div className="field">
          <label className="ctl" htmlFor={`${prefix}-attr`}>
            <input
              id={`${prefix}-attr`}
              type="checkbox"
              checked={j.attribution}
              onChange={(e) => setJ({ attribution: e.target.checked })}
            />
            Linhas de atribuição nos commits e MRs
          </label>
        </div>
        {text("type_map", "Mapa de tipos do Jira (Tipo=papel)", "Epic=epic, Bug=bug", true)}
        {text("epic_link_field", "Campo do Epic Link (opcional)", "customfield_10601")}
        {text("epic_name_field", "Campo do Epic Name (opcional)", "customfield_10604")}
        {text(
          "sprint_field",
          "Campo da sprint (opcional)",
          "customfield_10600 (vazio = detectar)",
          false,
          "Permite filtrar a árvore e o quadro por sprint. Vazio = o ambiente procura o campo de sprint em cada issue.",
        )}
        <p className="muted hint hub-wide">
          O hub não alcança o Jira nem o GitLab: guarda só os comandos de credencial, que rodam no ambiente de sync.
          Ele confere tudo no primeiro ciclo de sync e relata aqui.
        </p>
      </>
    );
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
    const assigned = new Set(p.assignments.map((a) => a.env_id));
    const free = hub.envs.filter((e) => !assigned.has(e.env_id));
    const counts = projectCounts(health, pr.gh_owner, pr.gh_repo);
    const { prev, next } = neighbors(pr);
    const live = liveOf(pr);
    const sync = syncText(p.sync, now);
    const pos = projects.findIndex((x) => x.project.id === pr.id) + 1;
    const scope = key.toLowerCase();
    const own = (secrets.list?.items ?? []).filter((i) => typeof i.scope === "object" && i.scope.project === scope);
    const delivery = (secrets.list?.delivery ?? []).filter((d) => d.projects.includes(scope));
    const deleting = pending?.kind === "delete" && keyOf(pending.owner, pending.repo) === key ? pending : null;
    const impact: DeleteImpact | null =
      deleting?.impact ?? (deleting?.local ? { project: pr, assignments: p.assignments, live_sessions: live } : null);
    // Com o impacto do hub, atribuições ou sessões vivas já pedem `confirm`: um clique só.
    // Se o projeto mudar entre a consulta e a exclusão, a recusa do hub ainda vale.
    const needs = !!deleting?.impact && (deleting.impact.assignments.length > 0 || deleting.impact.live_sessions.length > 0);
    const jira = jiraOf(pr);
    const gitlab = gitlabOf(pr);
    const syncName = pr.sync_env_id ? (p.assignments.find((a) => a.env_id === pr.sync_env_id)?.env_name ?? pr.sync_env_id) : null;

    return (
      <div className="hub-pdetail-body" role="group" aria-label={`Projeto ${pr.name}`}>
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
                onClick={() =>
                  setEdit({
                    key,
                    form: {
                      ...hostsOf(jira ? "jira" : "github"),
                      url: "",
                      name: pr.name,
                      base_branch: pr.base_branch,
                      protected: pr.protected_branches.join(", "),
                      jira: jiraFormOf(pr),
                      docs: docsFormOf(pr),
                    },
                  })
                }
              >
                Editar <span className="sr-only">{pr.name}</span>
              </button>
            )}
          </div>
        </div>

        <section className="hub-card" aria-labelledby={`pc-repo-${pr.id}`}>
          <div className="hub-card-head">
            <h3 id={`pc-repo-${pr.id}`}>Repositório</h3>
          </div>
          {edit?.key === key ? (
            <form
              className="env-form hub-card-form"
              aria-label={`Editar ${pr.name}`}
              onSubmit={(e) => {
                e.preventDefault();
                void update(p);
              }}
            >
              {formFields(edit.form, (form) => setEdit({ key, form }), `ep-${pr.id}`, false)}
              {docsFields(edit.form, (form) => setEdit({ key, form }), `ep-${pr.id}`)}
              <div className="form-actions">
                <button type="button" className="btn" onClick={() => setEdit(null)}>
                  Cancelar
                </button>
                <button type="submit" className="btn btn-primary" disabled={busy}>
                  {busy ? "Salvando…" : "Salvar projeto"}
                </button>
              </div>
            </form>
          ) : (
            <dl className="hub-kv">
              {jira && (
                <>
                  <div>
                    <dt>Jira</dt>
                    <dd>
                      <span className="mono">{jira.url}</span> · projeto <span className="mono">{jira.project_key}</span>
                      {jira.active_sprint_only ? (
                        <span className="hub-card-sub">Sync: somente a sprint ativa</span>
                      ) : (
                        <span className="hub-card-sub">
                          {jira.jql ? <span className="mono">JQL: {jira.jql}</span> : "Sem JQL (cadastro anterior)"}
                          {` · até ${jira.max_issues ?? DEFAULT_MAX_ISSUES} issues`}
                        </span>
                      )}
                    </dd>
                  </div>
                  <div>
                    <dt>GitLab</dt>
                    <dd className="mono">{gitlab ? gitlab.clone_url || `${gitlab.host}/${gitlab.path}` : "—"}</dd>
                  </div>
                  <div>
                    <dt>Credenciais (no ambiente)</dt>
                    <dd>
                      <span className="mono">{jira.credential_cmd}</span>
                      {gitlab && <span className="hub-card-sub mono">{gitlab.credential_cmd}</span>}
                    </dd>
                  </div>
                  <div>
                    <dt>Padrão de branch</dt>
                    <dd>
                      <span className="mono">{pr.branch_pattern ?? JIRA_BRANCH_PATTERN}</span>
                      <span className="hub-card-sub">
                        ex.: <span className="mono">{branchPreview(pr.branch_pattern ?? JIRA_BRANCH_PATTERN, jira.project_key)}</span>
                        {pr.attribution ? "" : " · sem linhas de atribuição"}
                      </span>
                    </dd>
                  </div>
                </>
              )}
              {!jira && pr.branch_pattern && pr.branch_pattern !== GITHUB_BRANCH_PATTERN && (
                <div>
                  <dt>Padrão de branch</dt>
                  <dd className="mono">{pr.branch_pattern}</dd>
                </div>
              )}
              <div>
                <dt>Branch base</dt>
                <dd className="mono">{pr.base_branch}</dd>
              </div>
              <div>
                <dt>Protegidas</dt>
                <dd className="mono">{pr.protected_branches.join(", ")}</dd>
              </div>
              {pr.docs && (
                <div>
                  <dt>Docs</dt>
                  <dd>
                    {docsText(pr.docs)}
                    {pr.docs.enabled && hub.info?.docs_dir && (
                      <span className="hub-card-sub mono">
                        {hub.info.docs_dir}/{docsDirName(pr.gh_owner, pr.gh_repo)}
                      </span>
                    )}
                  </dd>
                </div>
              )}
              <div>
                <dt>Sincronização</dt>
                <dd>
                  <span className="hub-dot" data-tone={sync.tone} aria-hidden="true" /> {sync.text}
                  {jira && <span className="muted">{syncName ? ` · pelo ambiente de sync ${syncName}` : " · sem ambiente de sync"}</span>}
                  {p.sync?.last_error && <span className="hub-card-sub">{p.sync.last_error}</span>}
                </dd>
              </div>
              <div>
                <dt>Ordem entre projetos</dt>
                <dd className="hub-order">
                  <span className="mono">
                    #{pos} de {projects.length}
                  </span>
                  {!web && (
                    <>
                      <button
                        type="button"
                        className="btn btn-sm"
                        disabled={busy || !prev}
                        aria-label={`Subir ${pr.name} na ordem dos projetos`}
                        onClick={() => step(pr, true)}
                      >
                        ↑
                      </button>
                      <button
                        type="button"
                        className="btn btn-sm"
                        disabled={busy || !next}
                        aria-label={`Descer ${pr.name} na ordem dos projetos`}
                        onClick={() => step(pr, false)}
                      >
                        ↓
                      </button>
                    </>
                  )}
                </dd>
              </div>
            </dl>
          )}
        </section>

        {jira && (
          <section className="hub-card" aria-labelledby={`pc-sync-${pr.id}`}>
            <div className="hub-card-head">
              <h3 id={`pc-sync-${pr.id}`}>Ambiente de sync</h3>
              {!web && p.assignments.length > 0 && syncEnv?.key !== key && (
                <button
                  type="button"
                  className="btn btn-sm"
                  onClick={() =>
                    setSyncEnv({
                      key,
                      env: p.assignments.find((a) => a.env_id !== pr.sync_env_id)?.env_id ?? p.assignments[0].env_id,
                    })
                  }
                >
                  {pr.sync_env_id ? "Trocar ambiente de sync" : "Escolher ambiente de sync"}
                </button>
              )}
            </div>
            {syncEnv?.key === key ? (
              <form
                className="env-form hub-card-form"
                aria-label={`Ambiente de sync de ${pr.name}`}
                onSubmit={(e) => {
                  e.preventDefault();
                  const a = p.assignments.find((x) => x.env_id === syncEnv.env);
                  if (a) void changeSyncEnv(pr, a.env_id, a.env_name);
                }}
              >
                <div className="field">
                  <label htmlFor={`se-env-${pr.id}`}>Ambiente (entre os atribuídos)</label>
                  <select
                    id={`se-env-${pr.id}`}
                    value={syncEnv.env}
                    onChange={(e) => setSyncEnv({ key, env: e.target.value })}
                  >
                    {p.assignments.map((a) => (
                      <option key={a.env_id} value={a.env_id}>
                        {a.env_name}
                        {a.env_id === pr.sync_env_id ? " (atual)" : ""}
                      </option>
                    ))}
                  </select>
                </div>
                <div className="form-actions">
                  <button type="button" className="btn" onClick={() => setSyncEnv(null)}>
                    Cancelar
                  </button>
                  <button
                    type="submit"
                    className="btn btn-primary"
                    disabled={busy || syncEnv.env === pr.sync_env_id}
                  >
                    Usar como ambiente de sync
                  </button>
                </div>
              </form>
            ) : (
              <p className="hub-card-msg">
                {syncName ? (
                  <>
                    <strong>{syncName}</strong> busca o Jira e o GitLab e envia o espelho ao hub. Os outros ambientes
                    atribuídos sincronizam só para uso local.
                  </>
                ) : (
                  <span className="muted">
                    Sem ambiente de sync: o hub não recebe o espelho do Jira.{" "}
                    {p.assignments.length === 0
                      ? "Atribua o projeto a um ambiente que alcance o Jira e o GitLab; a primeira atribuição vira o ambiente de sync."
                      : "Escolha um dos ambientes atribuídos."}
                  </span>
                )}
              </p>
            )}
            {pr.sync_env_id && <TrackerReadiness project={pr} status={p.tracker_status} now={now} />}
          </section>
        )}

        <section className="hub-card" aria-labelledby={`pc-atr-${pr.id}`}>
          <div className="hub-card-head">
            <h3 id={`pc-atr-${pr.id}`}>Atribuições · {p.assignments.length}</h3>
            {!web && free.length > 0 && assign?.key !== key && (
              <button
                type="button"
                className="btn btn-sm"
                onClick={() => setAssign({ key, env: free[0].env_id, repo_path: "" })}
              >
                Atribuir a ambiente <span className="sr-only">({pr.name})</span>
              </button>
            )}
          </div>
          {assign?.key === key && (
            <form
              className="env-form hub-card-form"
              aria-label={`Atribuir ${pr.name}`}
              onSubmit={(e) => {
                e.preventDefault();
                void addAssignment();
              }}
            >
              <div className="field">
                <label htmlFor={`as-env-${pr.id}`}>Ambiente</label>
                <select
                  id={`as-env-${pr.id}`}
                  value={assign.env}
                  onChange={(e) => setAssign({ ...assign, env: e.target.value })}
                >
                  {free.map((e) => (
                    <option key={e.env_id} value={e.env_id}>
                      {e.name}
                    </option>
                  ))}
                </select>
              </div>
              <div className="field">
                <label htmlFor={`as-path-${pr.id}`}>Caminho do clone (opcional)</label>
                <input
                  id={`as-path-${pr.id}`}
                  type="text"
                  placeholder={`padrão: <repos_root>/${pr.gh_repo}`}
                  value={assign.repo_path}
                  onChange={(e) => setAssign({ ...assign, repo_path: e.target.value })}
                />
              </div>
              <div className="form-actions">
                <button type="button" className="btn" onClick={() => setAssign(null)}>
                  Cancelar
                </button>
                <button type="submit" className="btn btn-primary" disabled={busy || !assign.env}>
                  Atribuir
                </button>
              </div>
            </form>
          )}
          {p.assignments.length === 0 ? (
            <p className="muted hub-card-empty">Nenhum ambiente executa este projeto.</p>
          ) : (
            <div className="table-wrap">
              <table className="hub-card-table" aria-label={`Atribuições de ${pr.name}`}>
                <thead>
                  <tr>
                    <th scope="col">Ambiente</th>
                    <th scope="col">Estado</th>
                    <th scope="col">Clone</th>
                    <th scope="col">Sessões</th>
                    <th scope="col">
                      <span className="sr-only">Ações</span>
                    </th>
                  </tr>
                </thead>
                <tbody>
                  {p.assignments.flatMap((a) => {
                    const n = live.filter((s) => s.env_id === a.env_id).length;
                    const confirming =
                      pending?.kind === "unassign" &&
                      pending.a.env_id === a.env_id &&
                      keyOf(pending.a.gh_owner, pending.a.gh_repo) === key;
                    const row = (
                      <tr key={a.env_id}>
                        <td className="strong">
                          {a.env_name}
                          {jira && a.env_id === pr.sync_env_id && (
                            <>
                              {" "}
                              <span className="badge" data-tone="done">
                                sync
                              </span>
                            </>
                          )}
                        </td>
                        <td>
                          <span className="hub-state" data-state={a.state}>
                            {assignmentText(a)}
                          </span>
                        </td>
                        <td className="mono small">{a.reported_path ?? repoPathText(a)}</td>
                        <td className="nowrap">{n === 0 ? "nenhuma" : plural(n, "ativa", "ativas")}</td>
                        <td className="hub-card-action">
                          {!web && (
                            <button
                              type="button"
                              className="btn btn-sm"
                              disabled={busy}
                              onClick={() => unassign(a, false)}
                            >
                              Remover <span className="sr-only">atribuição a {a.env_name}</span>
                            </button>
                          )}
                        </td>
                      </tr>
                    );
                    if (!confirming) return [row];
                    return [
                      row,
                      <tr key={`${a.env_id}-confirm`}>
                        <td colSpan={5}>
                          <div role="group" aria-label={`Confirmar remoção de ${a.env_name}`} className="confirm-box">
                            <p>{pending.hub}</p>
                            <div className="form-actions">
                              <button type="button" className="btn" onClick={() => setPending(null)}>
                                Cancelar
                              </button>
                              <button
                                type="button"
                                className="btn btn-danger"
                                disabled={busy}
                                onClick={() => unassign(a, true)}
                              >
                                Remover mesmo assim
                              </button>
                            </div>
                          </div>
                        </td>
                      </tr>,
                    ];
                  })}
                </tbody>
              </table>
            </div>
          )}
        </section>

        <AgentRules owner={pr.gh_owner} repo={pr.gh_repo} epic={0} label={pr.name} now={now} className="hub-card" />

        {!web && (
          <>
            <section className="hub-card" aria-labelledby={`pc-sec-${pr.id}`}>
              <div className="hub-card-head">
                <h3 id={`pc-sec-${pr.id}`}>Secrets e variáveis do projeto · {own.length}</h3>
                <button type="button" className="btn btn-sm" onClick={() => nav.openTab(hubTab("secrets"))}>
                  Abrir Secrets e variáveis
                </button>
              </div>
              {secrets.error ? (
                <p className="muted hub-card-msg">Secrets indisponíveis: {secrets.error}</p>
              ) : !secrets.list ? (
                <p className="muted hub-card-msg">Carregando…</p>
              ) : (
                <>
                  {own.length === 0 ? (
                    <p className="muted hub-card-empty">
                      Nenhum secret ou variável próprio; o projeto herda os globais.
                    </p>
                  ) : (
                    <div className="table-wrap">
                      <table className="hub-card-table" aria-label={`Secrets e variáveis de ${pr.name}`}>
                        <thead>
                          <tr>
                            <th scope="col">Nome</th>
                            <th scope="col">Tipo</th>
                            <th scope="col">Entrega</th>
                          </tr>
                        </thead>
                        <tbody>
                          {own.map((i) => {
                            // Chegou ao ambiente que confirmou uma revisão igual ou posterior à da entrada.
                            const got = delivery.filter(
                              (d) => d.state !== "no_key" && d.applied_rev !== null && d.applied_rev >= i.rev,
                            ).length;
                            return (
                              <tr key={i.name}>
                                <td className="mono">{i.name}</td>
                                <td>{i.kind === "secret" ? "secret" : "variável"}</td>
                                <td className="nowrap">
                                  {got}/{delivery.length} ambientes · rev {i.rev}
                                </td>
                              </tr>
                            );
                          })}
                        </tbody>
                      </table>
                    </div>
                  )}
                  {delivery.length > 0 && (
                    <ul className="hub-envs" aria-label={`Entrega dos secrets de ${pr.name} por ambiente`}>
                      {delivery.map((d) => (
                        <li key={d.env_id}>
                          <strong>{d.env_name}</strong>
                          <span className="hub-state" data-state={d.state}>
                            {DELIVERY_TEXT[d.state]}
                          </span>
                        </li>
                      ))}
                    </ul>
                  )}
                </>
              )}
            </section>

            <section className="hub-card hub-danger" aria-labelledby={`pc-dng-${pr.id}`}>
              <div className="hub-card-head">
                <h3 id={`pc-dng-${pr.id}`}>Zona de perigo</h3>
              </div>
              {deleting ? (
                <div role="group" aria-label={`Confirmar exclusão de ${pr.name}`} className="confirm-box hub-card-msg">
                  {impact ? (
                    <DeleteImpactText impact={impact} secrets={own.length} local={deleting.local && !deleting.impact} />
                  ) : (
                    <p className="hub-card-sub" role="status">
                      Consultando o impacto no hub…
                    </p>
                  )}
                  {deleting.hub ? (
                    <p>{deleting.hub}</p>
                  ) : (
                    <p>
                      Excluir {key} do hub? Isso apaga o histórico do projeto no hub (work items, sessões, decisões,
                      eventos), que é a única cópia. O hub tira um snapshot antes; para desfazer, restaure-o com{" "}
                      <span className="mono">cthubd restore</span>.
                    </p>
                  )}
                  <div className="form-actions">
                    <button type="button" className="btn" onClick={() => setPending(null)}>
                      Cancelar
                    </button>
                    <button
                      type="button"
                      className="btn btn-danger"
                      disabled={busy || !impact}
                      onClick={() => remove(pr.gh_owner, pr.gh_repo, deleting.hub !== null || needs)}
                    >
                      {deleting.hub ? "Excluir mesmo assim" : "Excluir projeto"}
                    </button>
                  </div>
                </div>
              ) : (
                <div className="hub-danger-row">
                  <div className="hub-card-text">
                    Excluir o projeto do hub
                    <span className="hub-card-sub">
                      Mostra o impacto (atribuições, sessões e secrets) antes de confirmar.
                    </span>
                  </div>
                  <button
                    type="button"
                    className="btn btn-danger"
                    onClick={() => void askDelete(pr.gh_owner, pr.gh_repo)}
                  >
                    Excluir projeto… <span className="sr-only">{pr.name}</span>
                  </button>
                </div>
              )}
            </section>
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
                  setEdit(null);
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
              {formFields(add, setAdd, "np", true)}
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

/**
 * O que a exclusão atinge: o impacto do hub (`hub.projects.impact`) ou, com
 * `local`, o que a UI já sabe; os secrets do projeto vêm do `hub.secrets.list`.
 */
function DeleteImpactText({ impact, secrets, local }: { impact: DeleteImpact; secrets: number; local: boolean }) {
  const envs = impact.assignments.map((a) => a.env_name);
  return (
    <ul className="hub-impact" aria-label="Impacto da exclusão">
      {local && <li>Impacto estimado pela UI (o hub não respondeu à consulta); o hub confere de novo ao excluir.</li>}
      <li>
        {envs.length === 0
          ? "Nenhuma atribuição ativa."
          : `${plural(envs.length, "atribuição ativa", "atribuições ativas")}: ${envs.join(", ")}.`}
      </li>
      <li>
        {impact.live_sessions.length === 0
          ? "Nenhuma sessão viva relatada."
          : `${plural(impact.live_sessions.length, "sessão viva", "sessões vivas")}, que os ambientes não encerram.`}
      </li>
      <li>
        {secrets === 0
          ? "Nenhum secret ou variável do projeto."
          : `${plural(secrets, "secret ou variável do projeto", "secrets e variáveis do projeto")}.`}
      </li>
    </ul>
  );
}

/** `hub.secrets.list`, relido junto com a visão do hub (a entrega muda com os pulls). */
function useSecrets(enabled = true): { list: SecretsList | null; error: string | null } {
  const { api, hub } = useTower();
  const [list, setList] = useState<SecretsList | null>(null);
  const [error, setError] = useState<string | null>(null);
  const seq = useRef(0);
  const load = useCallback(async () => {
    // Web: `hub.secrets.list` fica fora do /web/ws (devolve valores de variáveis).
    if (!enabled) return;
    const n = ++seq.current;
    try {
      const l = await api.hubCall<SecretsList>("hub.secrets.list");
      if (n !== seq.current) return;
      setList(l);
      setError(null);
    } catch (e) {
      if (n === seq.current) setError(errText(e));
    }
  }, [api, enabled]);
  useEffect(() => {
    void load();
  }, [load, hub.envs]);
  return { list, error };
}
