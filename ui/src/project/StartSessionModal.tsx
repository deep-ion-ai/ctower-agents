import "./start-modal.css";
import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
  type ReactNode,
} from "react";
import type {
  EnvInfo,
  GestorScope,
  IssueRow,
  ProjectTree,
  Session,
  SessionMode,
  WorkItemKind,
  WorkItemStatus,
} from "../protocol/gen";
import type { HubTree } from "../protocol/hub-gen";
import { CLAUDE_MODELS, DEFAULT_MODEL_VALUE, SESSION_MODES } from "../settings/session-options";
import { hubTab } from "../hub/sections";
import { templateTab } from "../settings/sections";
import { shortcutLabel } from "../shell/layout";
import { sessionReadiness } from "../hub/tracker";
import { StartCtx, type StartSession, type StartTarget } from "../shell/start";
import { useNav, type OpenTab, type Tab } from "../state/nav";
import { useTower, type Tower } from "../state/tower";
import { lastTermSize } from "../terminal/size";
import { changeLabel, itemKey, itemShort, PROJECT_ITEM } from "../util/item";
import { branchFor, branchFromPattern, patternOf, worktreeDirName } from "./branch";
import { STATE_SUFFIX, hubClones, liveSessions, localClone, type Clone } from "./clones";
import { itemTab, projectKey } from "./tabs";
import { renderGestorTemplate, renderTemplate, scopeText, templateKeyFor } from "./templates";
import { cachedTree } from "./ui-store";
import { rcRequested } from "../web/envActions";

const same = (a: string, b: string) => a.toLowerCase() === b.toLowerCase();

const STATE_TEXT: Record<string, string> = {
  starting: "iniciando",
  running: "rodando",
  idle: "ociosa",
  exited: "encerrada",
  interrupted: "interrompida",
  archived: "arquivada",
};

/** Nó da árvore (local ou do hub) no que a modal precisa. */
interface StartNode {
  issue: IssueRow;
  work_item: { status: WorkItemStatus; branch: string | null } | null;
  children: StartNode[];
}

/** De onde vêm árvore e ambientes: a visão do hub (por `owner/repo`) ou um projeto local. */
type Where = { view: "hub"; owner: string; repo: string } | { view: "local"; env: string; projectId: number };

/** Mesma regra do `ProjectRoute`: com o hub no ar, a visão do hub; senão, o projeto local. */
export function resolveWhere(t: StartTarget, tower: Pick<Tower, "hub" | "envs" | "dashboards">): Where | null {
  const { hub, envs, dashboards } = tower;
  let key = t.owner && t.repo ? { owner: t.owner, repo: t.repo } : undefined;
  if (!key && t.env && t.projectId !== undefined) {
    const p = dashboards[t.env]?.projects.find((x) => x.project.id === t.projectId)?.project;
    if (p) key = { owner: p.gh_owner, repo: p.gh_repo };
  }
  if (hub.up && hub.dashboard && key) return { view: "hub", ...key };
  if (t.env && t.projectId !== undefined) return { view: "local", env: t.env, projectId: t.projectId };
  if (!key) return null;
  for (const e of envs) {
    const p = dashboards[e.id]?.projects.find((x) => same(x.project.gh_owner, key!.owner) && same(x.project.gh_repo, key!.repo));
    if (p) return { view: "local", env: e.id, projectId: p.project.id };
  }
  return null;
}

/** Árvore do projeto para a modal: a última da tela Projeto na hora, relida em seguida. */
function useStartTree(where: Where | null): { roots: StartNode[] | null; error: string | null } {
  const { api } = useTower();
  const key = where ? projectKey(where) : "";
  const [roots, setRoots] = useState<StartNode[] | null>(() => cachedTree<{ roots: StartNode[] }>(key)?.roots ?? null);
  const [error, setError] = useState<string | null>(null);
  const hubView = where?.view === "hub";
  const a = where?.view === "hub" ? where.owner : where?.env;
  const b = where?.view === "hub" ? where.repo : where?.projectId;
  useEffect(() => {
    if (a === undefined || b === undefined) return;
    let alive = true;
    const p: Promise<{ roots: StartNode[] }> = hubView
      ? api.hubCall<HubTree>("hub.tree.get", { owner: a, repo: b })
      : api.call<ProjectTree>(a as string, "tree.get", { project_id: b });
    p.then(
      (t) => alive && setRoots(t.roots),
      (e) => alive && setError(String(e)),
    );
    return () => {
      alive = false;
    };
  }, [api, hubView, a, b]);
  return { roots, error };
}

/** Elementos que recebem foco por Tab dentro de `root`. */
function focusables(root: HTMLElement): HTMLElement[] {
  return [
    ...root.querySelectorAll<HTMLElement>(
      'button:not([disabled]), select:not([disabled]), textarea:not([disabled]), input:not([disabled]), a[href], [tabindex]:not([tabindex="-1"])',
    ),
  ].filter((el) => !el.hidden && !el.closest("[hidden]"));
}

/**
 * Reiniciar (#424): encerra a sessão substituída, se pedido, logo antes de
 * iniciar a nova. Se ela já tinha saído nesse meio-tempo, segue.
 */
async function killReplaced(tower: Pick<Tower, "api" | "dashboards">, t: StartTarget): Promise<void> {
  const r = t.replace;
  if (!t.kill || !r) return;
  try {
    await tower.api.call(r.env, "session.kill", { session_id: r.session });
  } catch (e) {
    if (tower.dashboards[r.env]?.live_sessions.some((x) => x.id === r.session)) throw e;
  }
}

/** Esc fecha, Ctrl/⌘+Enter inicia e o Tab fica preso dentro da modal. */
function dialogKeys(ref: { current: HTMLElement | null }, onClose: () => void, start: () => void) {
  return (e: ReactKeyboardEvent) => {
    const dialog = ref.current;
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      onClose();
      return;
    }
    if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
      e.preventDefault();
      start();
      return;
    }
    if (e.key === "Tab" && dialog) {
      // Foco preso: do último volta ao primeiro (e vice-versa com Shift).
      const list = focusables(dialog);
      if (list.length === 0) return;
      const first = list[0];
      const last = list[list.length - 1];
      const at = document.activeElement;
      if (e.shiftKey && (at === first || at === dialog)) {
        e.preventDefault();
        last.focus();
      } else if (!e.shiftKey && at === last) {
        e.preventDefault();
        first.focus();
      }
    }
  };
}

type StartedFn = (s: Session, env: string, envName: string, label: string, openTerminal: boolean) => void;

/**
 * Modal de início: a do item (ou do planejamento) ou, no Reiniciar do gestor
 * (`target.gestor`, #424), a do gestor de projetos.
 */
export function StartSessionModal(props: {
  target: StartTarget;
  onClose: () => void;
  onStarted: StartedFn;
  onOpenExisting: (t: Tab) => void;
  /** Atalho para outra aba (template, regras): fecha a modal e abre a aba. */
  onOpenTab: (t: OpenTab) => void;
}) {
  return props.target.gestor ? (
    <GestorStartModal target={props.target} scope={props.target.gestor} onClose={props.onClose} onStarted={props.onStarted} />
  ) : (
    <ItemStartModal {...props} />
  );
}

/**
 * Modal de início de sessão (#317): substitui o painel `StartSessionPanel`.
 * Tipo e título; aviso de sessão já existente; no épico, o escopo e atalhos
 * para as sub-issues a fazer (que trocam o alvo); ambiente (o da atribuição,
 * na visão do hub), ferramenta, modelo e modo (padrão das Configurações);
 * prévia de branch e worktree e o prompt do template, editável. As chamadas
 * são as de antes: `session.start` com o número (na visão do hub, `owner/repo`
 * mais título e labels) ou `session.start_project` no planejamento.
 * Atalhos (#11) para o template usado e as regras do projeto e do épico.
 */
function ItemStartModal({
  target,
  onClose,
  onStarted,
  onOpenExisting,
  onOpenTab,
}: {
  target: StartTarget;
  onClose: () => void;
  onStarted: StartedFn;
  onOpenExisting: (t: Tab) => void;
  onOpenTab: (t: OpenTab) => void;
}) {
  const tower = useTower();
  const { api, config, web } = tower;
  const where = resolveWhere(target, tower);
  const { roots, error: treeError } = useStartTree(where);
  const byKey = where?.view === "hub";

  const clones: Clone[] = !where
    ? []
    : where.view === "hub"
      ? hubClones(tower, where.owner, where.repo)
      : [localClone(tower, where.env, where.projectId)].filter((c): c is Clone => c !== null);

  // Alvo dentro da modal: começa no pedido; os atalhos do épico trocam.
  const [gh, setGh] = useState<number | undefined>(target.ghNumber);
  const [envChoice, setEnvChoice] = useState<string | null>(null);
  // Reiniciar (#424): o modelo e o modo da sessão anterior.
  const [model, setModel] = useState(
    (target.model !== undefined ? target.model : config.session.model) ?? DEFAULT_MODEL_VALUE,
  );
  const [mode, setMode] = useState<SessionMode>(target.mode ?? config.session.mode);
  // Web (fase 2): sempre marcado, porque pelo celular não há outro jeito de
  // conversar com a sessão (spec do épico #205 §6.2).
  const [remote, setRemote] = useState(web || config.session.remote_control);
  // Web: o `env.info` do ambiente escolhido, pelo canal (o desktop já o tem).
  const [webInfos, setWebInfos] = useState<Record<string, EnvInfo>>({});
  const infos = web ? webInfos : tower.infos;
  const [edits, setEdits] = useState<Record<string, string>>({});
  const [openTerm, setOpenTerm] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const dialog = useRef<HTMLElement>(null);

  const all = useMemo(() => (roots ?? []).flatMap((r) => [r, ...r.children]), [roots]);
  const node = gh !== undefined ? all.find((x) => x.issue.gh_number === gh) : undefined;
  // Fora da árvore (ainda carregando, ou fora do espelho): o work item do ambiente basta.
  const known = gh !== undefined && !node ? clones.flatMap((c) => c.items).find((w) => w.gh_number === gh) : undefined;
  const issue: IssueRow | null =
    node?.issue ??
    (known
      ? {
          gh_number: known.gh_number,
          title: known.title ?? "",
          state: known.closed_at !== null ? "closed" : "open",
          labels: [],
          parent_number: null,
          has_sub_issues: known.kind === "epic",
          url: "",
          updated_at: 0,
        }
      : null);
  const planning = gh === undefined;
  const clone =
    clones.find((c) => c.env === envChoice) ?? clones.find((c) => c.env === target.env) ?? clones[0] ?? null;
  const project = clone?.project ?? null;
  // Chave do Jira e MR do GitLab no lugar de #n e PR (#352).
  const ref = (x: number) => itemShort(project, x);
  const kind: WorkItemKind = issue?.has_sub_issues ? "epic" : "issue";

  const wi = issue
    ? (clone?.items.find((w) => w.gh_number === issue.gh_number) ??
      (node?.work_item ? { branch: node.work_item.branch, worktree_path: null } : null))
    : null;
  const branch = project
    ? issue
      ? (wi?.branch ??
        // Padrão do projeto (#352; `PROJ-5450` no exemplo); o GitHub sem padrão segue o de sempre.
        (project.tracker?.kind === "jira" || project.branch_pattern
          ? branchFromPattern(
              patternOf(project),
              kind,
              issue.gh_number,
              itemKey(project, issue.gh_number),
              issue.title,
              issue.labels,
            )
          : null) ??
        branchFor(kind, issue.gh_number, issue.title, issue.labels))
      : project.base_branch
    : "";
  const cloneEnv = clone?.env;
  useEffect(() => {
    if (!web || !cloneEnv || webInfos[cloneEnv]) return;
    let alive = true;
    api.call<EnvInfo>(cloneEnv, "env.info").then(
      (i) => alive && setWebInfos((all) => ({ ...all, [cloneEnv]: i })),
      () => {},
    );
    return () => {
      alive = false;
    };
  }, [web, api, cloneEnv, webInfos]);
  const root = clone ? (infos[clone.env]?.worktrees_root ?? null) : null;
  // Remote Control (#197): só com daemon 2.3 (`env.info.remote_control`).
  const rcStatus = clone ? infos[clone.env]?.remote_control : undefined;
  // Web sem o `env.info` ainda: pede e deixa o ambiente recusar se não puder.
  const withRemote = remote && (rcStatus ? rcStatus.available : web);
  const worktree = project
    ? (wi?.worktree_path ?? `${root ?? "<raiz dos worktrees>"}/${project.name}/${worktreeDirName(branch)}`)
    : "";

  const editKey = String(gh ?? PROJECT_ITEM);
  // Template pelo rastreador do projeto (#351): os `jira_*` no Jira.
  const tracker = project?.tracker?.kind ?? "github";
  const tplKey = templateKeyFor(issue ? kind : "project", tracker);
  const base =
    project && (planning || issue)
      ? renderTemplate(config.templates[tplKey], {
          kind,
          number: issue?.gh_number ?? 0,
          owner: project.gh_owner,
          repo: project.gh_repo,
          branch,
          protected: project.protected_branches,
          title: issue?.title ?? "",
          base: project.base_branch,
          tracker: project.tracker,
          forge: project.forge,
        }) + (gh === target.ghNumber ? (target.note ?? "") : "")
      : "";
  const prompt = edits[editKey] ?? base;
  const edited = edits[editKey] !== undefined && edits[editKey] !== base;

  // Regras (#11): as do projeto ficam em Hub › Projetos; as do épico (o
  // próprio item ou o pai da sub-issue), no detalhe do épico na tela Projeto.
  const epicN = issue ? (kind === "epic" ? issue.gh_number : issue.parent_number) : null;
  const epicTab =
    where && epicN !== null
      ? itemTab(
          where.view === "hub"
            ? { name: "project", owner: where.owner, repo: where.repo }
            : { name: "project", env: where.env, projectId: where.projectId },
          epicN,
          all.find((x) => x.issue.gh_number === epicN)?.issue.title ?? "",
          project,
        )
      : null;

  // jira-cli e glab no ambiente escolhido (spec do Jira e GitLab §9.3): só avisa.
  const readiness = project && clone ? sessionReadiness(project, infos[clone.env]?.trackers) : null;

  // A sessão que o Reiniciar substitui não conta como "já existe".
  const replaced = target.replace;
  const live = liveSessions(clones, gh ?? PROJECT_ITEM).filter(
    (l) => !replaced || l.env !== replaced.env || l.session.id !== replaced.session,
  );
  const subsTodo =
    issue?.has_sub_issues && node
      ? node.children.filter((c) => c.issue.state === "open" && (c.work_item?.status ?? "backlog") === "backlog")
      : [];
  const repoText = project ? `${project.gh_owner}/${project.gh_repo}` : where?.view === "hub" ? `${where.owner}/${where.repo}` : "";
  const name = project?.name ?? (where?.view === "hub" ? where.repo : "projeto");
  const heading = planning ? `Sessão de planejamento · ${name}` : issue ? `${ref(issue.gh_number)} ${issue.title}` : ref(gh ?? 0);
  const kindLabel = planning ? "planejamento" : kind === "epic" ? "épico" : "issue";

  let blocked: string | null = null;
  if (!where) blocked = "Projeto não encontrado em nenhum ambiente conectado.";
  else if (!clone)
    blocked =
      where.view === "hub"
        ? web
          ? "Nenhum ambiente online tem este projeto atribuído: as sessões começam quando um deles reconectar ao hub."
          : "Nenhum ambiente conectado tem este projeto atribuído. Atribua o projeto a um ambiente na tela Hub."
        : "O ambiente deste projeto não está conectado.";
  else if (!planning && !issue)
    blocked = treeError
      ? `Não foi possível carregar a árvore do projeto: ${treeError}`
      : roots
        ? `${ref(gh ?? 0)} não está na árvore do projeto.`
        : null;
  const loading = !blocked && !planning && !issue;

  // Ao abrir, o foco vai para o primeiro controle da modal.
  useEffect(() => {
    const el = dialog.current;
    if (!el) return;
    (focusables(el)[0] ?? el).focus();
  }, []);

  const start = async () => {
    if (!clone || !project || busy || blocked || loading || !prompt.trim()) return;
    setBusy(true);
    setError(null);
    const tgt = byKey ? { owner: project.gh_owner, repo: project.gh_repo } : { project_id: project.id };
    // O PTY já nasce no tamanho da área do terminal (sem ele, 120×40).
    const size = lastTermSize() ?? {};
    // Sem `model`, o daemon não passa `--model` (padrão do Claude Code).
    const agent = { ...(model ? { model } : {}), mode, ...(withRemote ? { remote_control: true } : {}) };
    try {
      await killReplaced(tower, target);
      const s = issue
        ? await api.call<Session>(clone.env, "session.start", {
            ...tgt,
            gh_number: issue.gh_number,
            // Labels só da árvore: o work item não as tem.
            ...(byKey ? { title: issue.title, ...(node ? { labels: issue.labels } : {}) } : {}),
            prompt,
            ...agent,
            ...size,
          })
        : await api.call<Session>(clone.env, "session.start_project", { ...tgt, prompt, ...agent, ...size });
      // Web: o painel da sessão espera o id do Remote Control e abre o link.
      if (web && withRemote) rcRequested(s.id);
      onStarted(s, clone.env, clone.name, planning ? "Sessão de planejamento" : `Sessão de ${ref(gh ?? 0)}`, openTerm);
    } catch (e) {
      setError(String(e));
      setBusy(false);
    }
  };

  const onKeyDown = (e: ReactKeyboardEvent) => dialogKeys(dialog, onClose, () => void start())(e);

  const field = (id: string, label: string, control: ReactNode) => (
    <div className="sm-field">
      <label htmlFor={id}>{label}</label>
      {control}
    </div>
  );

  return (
    <div className="sm-scrim">
      <section
        ref={dialog}
        className="sm-modal"
        role="dialog"
        aria-modal="true"
        aria-labelledby="sm-title"
        tabIndex={-1}
        onKeyDown={onKeyDown}
      >
        <div className="sm-head">
          <div className="sm-kind">
            <span className="sm-chip" data-kind={planning ? "project" : kind}>
              {kindLabel}
            </span>
            {repoText && <span className="muted mono">{repoText}</span>}
          </div>
          <h2 id="sm-title">{heading}</h2>
        </div>

        <div className="sm-body">
          {blocked && (
            <p className="sm-blocked" role="alert">
              {blocked}
            </p>
          )}
          {loading && (
            <p className="muted" role="status">
              Carregando o item…
            </p>
          )}

          {target.kill && replaced && <RestartNote />}

          {live.length > 0 && (
            <div className="sm-note" role="status">
              <span className="sp-dot" data-tone="attention" aria-hidden="true" />
              <span className="sm-note-text">
                Já existe uma sessão para este item:{" "}
                <strong>{live.map((l) => `${l.name} · ${STATE_TEXT[l.session.state] ?? l.session.state}`).join(", ")}</strong>.
                Iniciar outra cria uma sessão nova{planning ? " no clone principal" : " no mesmo worktree"}.
              </span>
              <button
                type="button"
                className="btn btn-sm"
                onClick={() => onOpenExisting({ env: live[0].env, session: live[0].session.id })}
              >
                Abrir a existente
              </button>
            </div>
          )}

          {readiness && clone && (
            <div className="sm-note" role="status" aria-label="Prontidão do Jira e do GitLab">
              <span className="sp-dot" data-tone="attention" aria-hidden="true" />
              <span className="sm-note-text">
                {readiness.unknown ? (
                  <>
                    <strong>{clone.name}</strong> ainda não conferiu o jira-cli e o glab deste projeto: a prontidão chega no
                    primeiro ciclo de sync do ambiente. A sessão inicia mesmo assim.
                  </>
                ) : (
                  <>
                    O Jira ou o GitLab não estão prontos em <strong>{clone.name}</strong>; a sessão inicia, mas os comandos
                    do jira-cli e do glab podem falhar.
                    <ul className="sm-readiness">
                      {readiness.missing.map((m) => (
                        <li key={m.label}>
                          {m.label}: falta{m.fix ? ` (${m.fix})` : ""}
                        </li>
                      ))}
                      {readiness.errors.map((e) => (
                        <li key={e} className="mono">
                          {e}
                        </li>
                      ))}
                    </ul>
                  </>
                )}
              </span>
            </div>
          )}

          {issue?.has_sub_issues && project && (
            <div className="sm-field">
              <span className="sm-label">Escopo</span>
              <p className="sm-scope">
                A sessão do épico trabalha uma sub-issue por vez, cada uma em {changeLabel(project)} para{" "}
                <span className="mono">{branch}</span>.
                {subsTodo.length > 0 ? " Para uma sub-issue só:" : " Nenhuma sub-issue a fazer."}
              </p>
              {subsTodo.length > 0 && (
                <div className="sm-subs" role="group" aria-label="Sub-issues a fazer">
                  {subsTodo.map((c) => (
                    <button
                      key={c.issue.gh_number}
                      type="button"
                      className="sm-sub"
                      aria-label={`Iniciar só a sub-issue ${ref(c.issue.gh_number)} ${c.issue.title}`}
                      onClick={() => {
                        setGh(c.issue.gh_number);
                        setError(null);
                      }}
                    >
                      <span className="mono muted">{ref(c.issue.gh_number)}</span>
                      {c.issue.title}
                    </button>
                  ))}
                </div>
              )}
            </div>
          )}
          {target.ghNumber !== undefined && gh !== target.ghNumber && (
            <button type="button" className="linklike sm-back" onClick={() => setGh(target.ghNumber)}>
              ← Voltar para {ref(target.ghNumber)}
            </button>
          )}

          <div className="sm-grid">
            {field(
              "sm-env",
              "Ambiente",
              <select
                id="sm-env"
                value={clone?.env ?? ""}
                disabled={clones.length === 0}
                onChange={(e) => setEnvChoice(e.target.value)}
              >
                {clones.length === 0 && <option value="">nenhum ambiente</option>}
                {clones.map((c) => (
                  <option key={c.env} value={c.env}>
                    {c.name}
                    {c.state ? STATE_SUFFIX[c.state] : ""}
                  </option>
                ))}
              </select>,
            )}
            {field(
              "sm-tool",
              "Ferramenta",
              <select id="sm-tool" defaultValue="claude">
                <option value="claude">Claude Code</option>
                <option value="codex" disabled>
                  Codex (fase 2)
                </option>
              </select>,
            )}
            {field(
              "sm-model",
              "Modelo",
              <select id="sm-model" value={model} onChange={(e) => setModel(e.target.value)}>
                <option value={DEFAULT_MODEL_VALUE}>Padrão do Claude Code</option>
                {CLAUDE_MODELS.map((m) => (
                  <option key={m.value} value={m.value}>
                    {m.label}
                  </option>
                ))}
              </select>,
            )}
            {field(
              "sm-mode",
              "Modo",
              <select id="sm-mode" value={mode} onChange={(e) => setMode(e.target.value as SessionMode)}>
                {SESSION_MODES.map((m) => (
                  <option key={m.value} value={m.value}>
                    {m.label} · {m.hint}
                  </option>
                ))}
              </select>,
            )}
          </div>

          {(rcStatus || (web && clone)) && (
            <div className="sm-rc">
              <label className="sm-check">
                <input
                  type="checkbox"
                  checked={withRemote}
                  disabled={rcStatus ? !rcStatus.available : false}
                  onChange={(e) => setRemote(e.target.checked)}
                />
                Remote Control
              </label>
              <span className="muted small">
                {!rcStatus || rcStatus.available
                  ? web
                    ? "Pelo navegador, a conversa com a sessão é pelo app Claude ou em claude.ai/code: o link abre ao conectar."
                    : "Continue a sessão pelo celular, no app Claude ou em claude.ai/code."
                  : rcStatus.reason}
              </span>
            </div>
          )}

          {project && (planning || issue) && (
            <div className="sm-field">
              <span className="sm-label">{planning ? "Onde roda" : `Branch e worktree em ${clone?.name}`}</span>
              <div className="sm-where">
                <span className="sm-where-top">
                  <span className="mono">{planning ? `${branch} (clone principal)` : branch}</span>
                  <span className="sm-chip" data-tone={planning || wi?.branch ? "neutral" : "done"}>
                    {planning ? "clone principal, sem branch nova" : wi?.branch ? "já existentes" : "criados ao iniciar"}
                  </span>
                </span>
                <span className="mono muted sm-path">{planning ? project.repo_path : worktree}</span>
                {planning && (
                  <span className="muted small">Commits em branches protegidas são bloqueados.</span>
                )}
              </div>
            </div>
          )}

          {project && (planning || issue) && (
            <div className="sm-field">
              <span className="sm-prompt-head">
                <label htmlFor="sm-prompt">
                  Prompt inicial · template{" "}
                  {planning ? "da sessão do projeto" : kind === "epic" ? "de épico" : "de issue"}
                  {tracker === "jira" ? " do Jira" : ""}
                </label>
                <span className="sm-spacer" />
                <button type="button" className="linklike" onClick={() => onOpenTab(templateTab(tplKey))}>
                  Editar template ›
                </button>
                {edited && (
                  <button
                    type="button"
                    className="btn btn-sm"
                    onClick={() =>
                      setEdits((all) => {
                        const { [editKey]: _omit, ...rest } = all;
                        return rest;
                      })
                    }
                  >
                    Restaurar template
                  </button>
                )}
              </span>
              <textarea
                id="sm-prompt"
                rows={8}
                value={prompt}
                onChange={(e) => setEdits((all) => ({ ...all, [editKey]: e.target.value }))}
              />
              {tower.hub.expected && (
                <span className="sm-rules">
                  <button
                    type="button"
                    className="linklike"
                    onClick={() =>
                      onOpenTab(hubTab("projects", { project: { owner: project.gh_owner, repo: project.gh_repo } }))
                    }
                  >
                    Regras do projeto ›
                  </button>
                  {epicTab && epicN !== null && (
                    <button type="button" className="linklike" onClick={() => onOpenTab(epicTab)}>
                      Regras do épico {ref(epicN)} ›
                    </button>
                  )}
                </span>
              )}
            </div>
          )}

          {error && (
            <p role="alert" className="start-error">
              {error}
            </p>
          )}
        </div>

        <div className="sm-foot">
          {/* No Reiniciar a sessão nova sempre ocupa a aba da antiga. */}
          {!replaced && (
            <label className="sm-check">
              <input type="checkbox" checked={openTerm} onChange={(e) => setOpenTerm(e.target.checked)} />
              {web ? "Abrir a sessão ao iniciar" : "Abrir o terminal ao iniciar"}
            </label>
          )}
          <span className="sm-spacer" />
          <button type="button" className="btn" onClick={onClose}>
            Cancelar
          </button>
          <button
            type="button"
            className="btn btn-primary"
            aria-keyshortcuts="Meta+Enter Control+Enter"
            disabled={busy || !!blocked || loading || !prompt.trim()}
            onClick={() => void start()}
          >
            {busy ? "Iniciando…" : "Iniciar sessão"}
            <kbd className="kbd sm-kbd" aria-hidden="true">
              {shortcutLabel("↵")}
            </kbd>
          </button>
        </div>
      </section>
    </div>
  );
}

/** Aviso do Reiniciar com a sessão viva: o processo atual sai ao iniciar a nova. */
function RestartNote() {
  return (
    <div className="sm-note" role="status">
      <span className="sp-dot" data-tone="attention" aria-hidden="true" />
      <span className="sm-note-text">
        Ao iniciar, a sessão atual é encerrada (<span className="mono">session.kill</span>) e a nova ocupa a aba dela.
      </span>
    </div>
  );
}

/**
 * Reiniciar do gestor de projetos (#424): o mesmo ambiente e escopo da
 * sessão anterior, modelo e modo dela e o prompt do template `gestor`,
 * editável. Chama `session.start_gestor`, como o `GestorLauncher` da tela Hub.
 */
function GestorStartModal({
  target,
  scope,
  onClose,
  onStarted,
}: {
  target: StartTarget;
  scope: GestorScope;
  onClose: () => void;
  onStarted: StartedFn;
}) {
  const tower = useTower();
  const { api, config, envs } = tower;
  const env = target.env ?? "";
  const envName = envs.find((e) => e.id === env)?.name ?? env;
  const [model, setModel] = useState(
    (target.model !== undefined ? target.model : config.session.model) ?? DEFAULT_MODEL_VALUE,
  );
  const [mode, setMode] = useState<SessionMode>(target.mode ?? config.session.mode);
  const [prompt, setPrompt] = useState(() => renderGestorTemplate(config.templates.gestor, envName, scope));
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const dialog = useRef<HTMLElement>(null);

  useEffect(() => {
    const el = dialog.current;
    if (!el) return;
    (focusables(el)[0] ?? el).focus();
  }, []);

  const start = async () => {
    if (!env || busy || !prompt.trim()) return;
    setBusy(true);
    setError(null);
    try {
      await killReplaced(tower, target);
      const s = await api.call<Session>(env, "session.start_gestor", {
        scope,
        prompt,
        ...(model ? { model } : {}),
        mode,
        ...(lastTermSize() ?? {}),
      });
      onStarted(s, env, envName, "Sessão do gestor", true);
    } catch (e) {
      setError(String(e));
      setBusy(false);
    }
  };

  return (
    <div className="sm-scrim">
      <section
        ref={dialog}
        className="sm-modal"
        role="dialog"
        aria-modal="true"
        aria-labelledby="sm-title"
        tabIndex={-1}
        onKeyDown={dialogKeys(dialog, onClose, () => void start())}
      >
        <div className="sm-head">
          <div className="sm-kind">
            <span className="sm-chip" data-kind="project">
              gestor
            </span>
            <span className="muted">{`${envName} · ${scopeText(scope)}`}</span>
          </div>
          <h2 id="sm-title">Gestor de projetos</h2>
        </div>

        <div className="sm-body">
          {target.kill && target.replace && <RestartNote />}
          <div className="sm-grid">
            <div className="sm-field">
              <label htmlFor="sm-model">Modelo</label>
              <select id="sm-model" value={model} onChange={(e) => setModel(e.target.value)}>
                <option value={DEFAULT_MODEL_VALUE}>Padrão do Claude Code</option>
                {CLAUDE_MODELS.map((m) => (
                  <option key={m.value} value={m.value}>
                    {m.label}
                  </option>
                ))}
              </select>
            </div>
            <div className="sm-field">
              <label htmlFor="sm-mode">Modo</label>
              <select id="sm-mode" value={mode} onChange={(e) => setMode(e.target.value as SessionMode)}>
                {SESSION_MODES.map((m) => (
                  <option key={m.value} value={m.value}>
                    {m.label} · {m.hint}
                  </option>
                ))}
              </select>
            </div>
          </div>
          <div className="sm-field">
            <label htmlFor="sm-prompt">Prompt inicial · template do gestor</label>
            <textarea id="sm-prompt" rows={8} value={prompt} onChange={(e) => setPrompt(e.target.value)} />
          </div>
          {error && (
            <p role="alert" className="start-error">
              {error}
            </p>
          )}
        </div>

        <div className="sm-foot">
          <span className="sm-spacer" />
          <button type="button" className="btn" onClick={onClose}>
            Cancelar
          </button>
          <button
            type="button"
            className="btn btn-primary"
            aria-keyshortcuts="Meta+Enter Control+Enter"
            disabled={busy || !env || !prompt.trim()}
            onClick={() => void start()}
          >
            {busy ? "Iniciando…" : "Iniciar sessão"}
            <kbd className="kbd sm-kbd" aria-hidden="true">
              {shortcutLabel("↵")}
            </kbd>
          </button>
        </div>
      </section>
    </div>
  );
}

interface Started {
  text: string;
  tab: Tab;
}

/**
 * Dono da modal de início (montado pelo shell): `useStartSession()` abre a
 * modal no alvo; fechar devolve o foco a quem abriu. Depois de iniciar, um
 * aviso com "Ver terminal" (e, se marcado, o terminal já aberto).
 */
export function StartSessionHost({ children }: { children: ReactNode }) {
  const nav = useNav();
  const { web } = useTower();
  const [req, setReq] = useState<{ target: StartTarget; origin: Element | null; seq: number } | null>(null);
  const [started, setStarted] = useState<Started | null>(null);
  const seq = useRef(0);

  const open = useCallback<StartSession>((target, origin) => {
    setStarted(null);
    setReq({ target, origin: origin ?? document.activeElement, seq: ++seq.current });
  }, []);

  const close = (restore: boolean) => {
    const el = req?.origin;
    setReq(null);
    if (restore && el instanceof HTMLElement && el.isConnected) el.focus();
  };

  // O aviso some sozinho depois de um tempo.
  useEffect(() => {
    if (!started) return;
    const id = setTimeout(() => setStarted(null), 12_000);
    return () => clearTimeout(id);
  }, [started]);

  return (
    <StartCtx.Provider value={open}>
      {children}
      {req && (
        <StartSessionModal
          key={req.seq}
          target={req.target}
          onClose={() => close(true)}
          onOpenExisting={(t) => {
            close(false);
            nav.openTerminal(t);
          }}
          onOpenTab={(t) => {
            close(false);
            nav.openTab(t);
          }}
          onStarted={(s, env, envName, label, openTerminal) => {
            const tab = { env, session: s.id };
            const old = req.target.replace;
            // Reiniciar (#424): a sessão nova ocupa a aba (e a coluna) da antiga.
            if (old) {
              close(false);
              nav.replaceTerminal(old, tab);
            } else {
              close(!openTerminal);
              if (openTerminal) nav.openTerminal(tab);
            }
            setStarted({ text: `${label} iniciada em ${envName}`, tab });
          }}
        />
      )}
      {started && (
        <div className="sm-toast" role="status">
          <span className="sp-dot" data-tone="done" aria-hidden="true" />
          <span>{started.text}</span>
          <button
            type="button"
            className="btn btn-sm"
            onClick={() => {
              nav.openTerminal(started.tab);
              setStarted(null);
            }}
          >
            {web ? "Ver sessão" : "Ver terminal"}
          </button>
          <button type="button" className="ibtn" aria-label="Fechar aviso" onClick={() => setStarted(null)}>
            ×
          </button>
        </div>
      )}
    </StartCtx.Provider>
  );
}
