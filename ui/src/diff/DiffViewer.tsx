// Visualizador de diff (#168): o código que o agente mudou na worktree do
// work item, numa aba, em qualquer ambiente e também na web (pelo hub). O
// daemon do ambiente faz o diff: `git.diff` (lista de arquivos), sob demanda
// `git.diff_file` (o patch de um arquivo) e `git.commits` (a lista para o
// modo commit). Binários e diffs grandes aparecem resumidos.
import "../project/issue-body.css";
import "./diff.css";
import { useEffect, useMemo, useState, type ReactNode } from "react";
import type { BranchCommits, DiffFile, DiffPatch, DiffSummary } from "../protocol/gen";
import { useTower } from "../state/tower";
import { itemTitle } from "../util/item";
import { ago } from "../util/time";
import { useNow } from "../App";
import { isUnknownMethod } from "../files/SharedFiles";
import {
  changedLines,
  diffParams,
  fileLabel,
  languageOf,
  LARGE_LINES,
  MODE_LABEL,
  parsePatch,
  patchKey,
  splitRows,
  STATUS_LETTER,
  STATUS_TEXT,
  type DiffMode,
  type DiffScreen,
  type PatchLine,
} from "./diff";
import { highlightLines } from "./highlight";

export type DiffView = "unified" | "split";

const VIEW_KEY = "ct.diff.view";

export const DIFF_OLD = "Este ambiente ainda não mostra o diff: atualize o ambiente.";

export function loadView(): DiffView {
  try {
    return localStorage.getItem(VIEW_KEY) === "split" ? "split" : "unified";
  } catch {
    return "unified";
  }
}

function saveView(v: DiffView) {
  try {
    localStorage.setItem(VIEW_KEY, v);
  } catch {
    // Sem armazenamento: vale só nesta aba.
  }
}

const message = (e: unknown) => (isUnknownMethod(e) ? DIFF_OLD : e instanceof Error ? e.message : String(e));

type PatchState = { loading: true } | { patch: DiffPatch } | { error: string };

const short = (sha: string | null | undefined) => (sha ? sha.slice(0, 8) : "");

export function DiffViewer({ screen }: { screen: DiffScreen }) {
  const { api, envs, dashboards } = useTower();
  const now = useNow();
  const env = screen.env;
  const envName = envs.find((e) => e.id === env)?.name ?? env;
  const [mode, setMode] = useState<DiffMode>("branch");
  const [commit, setCommit] = useState<string | null>(null);
  const [commits, setCommits] = useState<BranchCommits | null>(null);
  const [commitsError, setCommitsError] = useState<string | null>(null);
  const [summary, setSummary] = useState<DiffSummary | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [tick, setTick] = useState(0);
  const [selected, setSelected] = useState<string | null>(null);
  const [view, setView] = useState<DiffView>(loadView);
  const [patches, setPatches] = useState<Record<string, PatchState>>({});
  const [forced, setForced] = useState<Record<string, true>>({});

  const d = dashboards[env];
  const owner = d?.projects.find((p) => p.work_items.some((w) => w.id === screen.workItem));
  const wi = owner?.work_items.find((w) => w.id === screen.workItem);
  const title = wi ? itemTitle(wi, owner?.project) : screen.title ? `Item ${screen.title}` : `Work item ${screen.workItem}`;

  // Commits da branch: no modo commit (e a cada Recarregar).
  useEffect(() => {
    if (mode !== "commit") return;
    let live = true;
    api.call<BranchCommits>(env, "git.commits", { work_item_id: screen.workItem }).then(
      (c) => {
        if (!live) return;
        setCommits(c);
        setCommitsError(null);
        setCommit((cur) => (cur && c.commits.some((x) => x.sha === cur) ? cur : (c.commits[0]?.sha ?? null)));
      },
      (e) => live && setCommitsError(message(e)),
    );
    return () => {
      live = false;
    };
  }, [api, env, screen.workItem, mode, tick]);

  // Lista de arquivos.
  useEffect(() => {
    if (mode === "commit" && !commit) {
      setSummary(null);
      setLoading(false);
      return;
    }
    let live = true;
    setLoading(true);
    api.call<DiffSummary>(env, "git.diff", diffParams(screen.workItem, mode, commit)).then(
      (s) => {
        if (!live) return;
        setSummary(s);
        setError(null);
        setLoading(false);
        setSelected((cur) => (cur && s.files.some((f) => f.path === cur) ? cur : (s.files[0]?.path ?? null)));
      },
      (e) => {
        if (!live) return;
        setSummary(null);
        setError(message(e));
        setLoading(false);
      },
    );
    return () => {
      live = false;
    };
  }, [api, env, screen.workItem, mode, commit, tick]);

  const file = summary?.files.find((f) => f.path === selected) ?? null;
  const key = file ? patchKey(mode, commit, file) : null;
  const large = !!file && changedLines(file) > LARGE_LINES && !(key && forced[key]);
  const want = !!file && !file.binary && !large;

  // Patch do arquivo escolhido, sob demanda (e guardado até Recarregar).
  useEffect(() => {
    if (!want || !file || !key || patches[key]) return;
    setPatches((p) => ({ ...p, [key]: { loading: true } }));
    api
      .call<DiffPatch>(env, "git.diff_file", {
        ...diffParams(screen.workItem, mode, commit),
        path: file.path,
        ...(file.old_path ? { old_path: file.old_path } : {}),
      })
      .then(
        (patch) => setPatches((p) => (p[key] ? { ...p, [key]: { patch } } : p)),
        (e) => setPatches((p) => (p[key] ? { ...p, [key]: { error: message(e) } } : p)),
      );
  }, [api, env, screen.workItem, mode, commit, want, file, key, patches]);

  const reload = () => {
    setPatches({});
    setTick((t) => t + 1);
  };

  const chooseMode = (m: DiffMode) => {
    if (m === mode) return;
    setMode(m);
    setSummary(null);
    setError(null);
  };

  const chooseView = (v: DiffView) => {
    setView(v);
    saveView(v);
  };

  const totals = useMemo(() => {
    const fs = summary?.files ?? [];
    return {
      add: fs.reduce((n, f) => n + (f.additions ?? 0), 0),
      del: fs.reduce((n, f) => n + (f.deletions ?? 0), 0),
    };
  }, [summary]);

  const current = commits?.commits.find((c) => c.sha === commit);
  const range =
    mode === "branch"
      ? summary && `${summary.base_ref ?? "base"} (merge-base ${short(summary.from)}) → working tree`
      : mode === "uncommitted"
        ? summary && `HEAD ${short(summary.from)} → working tree`
        : current && `${current.short} · ${current.subject}`;

  return (
    <main className="dv" aria-labelledby="dv-title">
      <header className="dv-head">
        <div className="dv-id">
          <h1 id="dv-title" className="dv-title" title={title}>
            Diff · {title}
          </h1>
          <p className="dv-where muted">
            {envName}
            {wi?.branch && (
              <>
                {" · "}
                <span className="mono">{wi.branch}</span>
              </>
            )}
            {range && ` · ${range}`}
          </p>
        </div>
        <div className="dv-actions">
          <div className="dv-seg" role="group" aria-label="Modo do diff">
            {(Object.keys(MODE_LABEL) as DiffMode[]).map((m) => (
              <button key={m} type="button" className="btn btn-sm" aria-pressed={mode === m} onClick={() => chooseMode(m)}>
                {MODE_LABEL[m]}
              </button>
            ))}
          </div>
          {mode === "commit" && (
            <select
              className="dv-commit"
              aria-label="Commit"
              value={commit ?? ""}
              disabled={!commits || commits.commits.length === 0}
              onChange={(e) => setCommit(e.target.value || null)}
            >
              {!commits && <option value="">Carregando commits…</option>}
              {commits?.commits.length === 0 && <option value="">Nenhum commit na branch</option>}
              {commits?.commits.map((c) => (
                <option key={c.sha} value={c.sha}>
                  {`${c.short} · ${c.subject} · ${c.author} · ${ago(c.date, now)}${c.merge ? " · merge" : ""}`}
                </option>
              ))}
            </select>
          )}
          <div className="dv-seg" role="group" aria-label="Visão do diff">
            <button type="button" className="btn btn-sm" aria-pressed={view === "unified"} onClick={() => chooseView("unified")}>
              Unificado
            </button>
            <button type="button" className="btn btn-sm" aria-pressed={view === "split"} onClick={() => chooseView("split")}>
              Lado a lado
            </button>
          </div>
          <button type="button" className="btn btn-sm" disabled={loading} title="Calcular o diff de novo no ambiente" onClick={reload}>
            {loading && summary ? "Recarregando…" : "Recarregar"}
          </button>
        </div>
      </header>
      {(error ?? commitsError) !== null && (
        <div className="ib-error dv-error" role={error === DIFF_OLD || commitsError === DIFF_OLD ? "status" : "alert"}>
          <p>{error ?? commitsError}</p>
          {error !== DIFF_OLD && commitsError !== DIFF_OLD && (
            <button type="button" className="btn" onClick={reload}>
              Tentar de novo
            </button>
          )}
        </div>
      )}
      {mode === "commit" && commits?.truncated && (
        <p className="muted dv-note">Só os {commits.commits.length} commits mais recentes da branch aparecem na lista.</p>
      )}
      <div className="dv-body">
        <nav className="dv-files" aria-label="Arquivos do diff">
          {summary ? (
            <>
              <p className="dv-count muted">
                {summary.files.length === 1 ? "1 arquivo" : `${summary.files.length} arquivos`}
                {summary.files.length > 0 && (
                  <>
                    {" · "}
                    <span className="dv-add">+{totals.add}</span> <span className="dv-del">−{totals.del}</span>
                  </>
                )}
              </p>
              {summary.truncated && <p className="muted dv-note">Lista cortada: arquivos demais.</p>}
              <ul>
                {summary.files.map((f) => (
                  <li key={f.path}>
                    <button
                      type="button"
                      className="dv-file"
                      aria-current={f.path === selected ? "true" : undefined}
                      title={`${fileLabel(f)} (${STATUS_TEXT[f.status]}${f.untracked ? ", não rastreado" : ""})`}
                      onClick={() => setSelected(f.path)}
                    >
                      <span className="dv-st" data-status={f.status} aria-label={STATUS_TEXT[f.status]}>
                        {STATUS_LETTER[f.status]}
                      </span>
                      <span className="dv-fname mono">{fileLabel(f)}</span>
                      <span className="dv-fstat mono">
                        {f.binary ? (
                          "bin"
                        ) : (
                          <>
                            <span className="dv-add">+{f.additions ?? 0}</span> <span className="dv-del">−{f.deletions ?? 0}</span>
                          </>
                        )}
                      </span>
                    </button>
                  </li>
                ))}
              </ul>
            </>
          ) : (
            error === null && (
              <p className="muted" role="status">
                {mode === "commit" && commits?.commits.length === 0
                  ? "Nenhum commit na branch desde a base."
                  : `Calculando o diff no ambiente ${envName}…`}
              </p>
            )
          )}
        </nav>
        <section className="dv-patch" aria-label={file ? `Diff de ${file.path}` : "Diff"}>
          {summary && summary.files.length === 0 ? (
            <p className="muted dv-empty">
              {mode === "uncommitted" ? "Nada por commitar na worktree." : "Nenhuma mudança."}
            </p>
          ) : !file ? null : (
            <FilePatch
              file={file}
              view={view}
              large={large}
              state={key ? patches[key] : undefined}
              onForce={() => key && setForced((f) => ({ ...f, [key]: true }))}
              onRetry={() =>
                key &&
                setPatches((p) => {
                  const { [key]: _, ...rest } = p;
                  return rest;
                })
              }
            />
          )}
        </section>
      </div>
    </main>
  );
}

function FileHead({ file }: { file: DiffFile }) {
  return (
    <div className="dv-fhead">
      <span className="dv-st" data-status={file.status}>
        {STATUS_LETTER[file.status]}
      </span>
      <span className="mono dv-fhead-path">{fileLabel(file)}</span>
      <span className="muted">
        {STATUS_TEXT[file.status]}
        {file.untracked && " · não rastreado"}
      </span>
    </div>
  );
}

function FilePatch({
  file,
  view,
  large,
  state,
  onForce,
  onRetry,
}: {
  file: DiffFile;
  view: DiffView;
  large: boolean;
  state: PatchState | undefined;
  onForce: () => void;
  onRetry: () => void;
}) {
  let body: ReactNode;
  if (file.binary) {
    body = <p className="dv-summary">Arquivo binário: o conteúdo não é mostrado.</p>;
  } else if (large) {
    body = (
      <div className="dv-summary">
        <p>
          Diff grande: {changedLines(file)} linhas mudadas (+{file.additions ?? 0} −{file.deletions ?? 0}).
        </p>
        <button type="button" className="btn btn-sm" onClick={onForce}>
          Mostrar mesmo assim
        </button>
      </div>
    );
  } else if (!state || "loading" in state) {
    body = (
      <p className="muted dv-summary" role="status">
        Lendo o diff do arquivo…
      </p>
    );
  } else if ("error" in state) {
    body = (
      <div className="ib-error" role="alert">
        <p>Não foi possível ler o diff: {state.error}</p>
        <button type="button" className="btn btn-sm" onClick={onRetry}>
          Tentar de novo
        </button>
      </div>
    );
  } else if (state.patch.too_large || state.patch.patch === null) {
    body = <p className="dv-summary">Diff grande demais para mostrar (mais de 1 MiB).</p>;
  } else {
    body = <PatchView patch={state.patch.patch} path={file.path} view={view} />;
  }
  return (
    <>
      <FileHead file={file} />
      {body}
    </>
  );
}

/** Texto de cada linha, realçado quando a linguagem é conhecida. */
function useHighlight(lines: PatchLine[], path: string): Map<string, string> | null {
  const [html, setHtml] = useState<Map<string, string> | null>(null);
  useEffect(() => {
    setHtml(null);
    const lang = languageOf(path);
    if (!lang) return;
    let live = true;
    const texts = [...new Set(lines.filter((l) => l.kind !== "meta").map((l) => l.text))];
    void highlightLines(texts, lang).then((out) => {
      if (live && out) setHtml(new Map(texts.map((t, i) => [t, out[i]])));
    });
    return () => {
      live = false;
    };
  }, [lines, path]);
  return html;
}

function Code({ line, html }: { line: PatchLine; html: Map<string, string> | null }) {
  if (line.kind === "meta") return <code className="dv-meta">{line.text}</code>;
  const h = html?.get(line.text);
  // HTML do highlight.js, já sanitizado (só <span class>).
  return h !== undefined ? <code dangerouslySetInnerHTML={{ __html: h }} /> : <code>{line.text}</code>;
}

const SIGN: Record<PatchLine["kind"], string> = { add: "+", del: "-", ctx: " ", meta: "" };

export function PatchView({ patch, path, view }: { patch: string; path: string; view: DiffView }) {
  const parsed = useMemo(() => parsePatch(patch), [patch]);
  const all = useMemo(() => parsed.hunks.flatMap((h) => h.lines), [parsed]);
  const html = useHighlight(all, path);
  if (parsed.hunks.length === 0) {
    return (
      <div className="dv-summary">
        {parsed.notes.length > 0 ? parsed.notes.map((n) => <p key={n} className="mono">{n}</p>) : <p>Sem mudança de conteúdo.</p>}
      </div>
    );
  }
  return (
    <div className="dv-scroll">
      {parsed.notes.length > 0 && (
        <div className="dv-notes muted mono">
          {parsed.notes.map((n) => (
            <p key={n}>{n}</p>
          ))}
        </div>
      )}
      <table className={`dv-table mono dv-${view}`} aria-label={`Diff de ${path}`}>
        {parsed.hunks.map((h, hi) => (
          <tbody key={hi}>
            <tr className="dv-hunk">
              <td colSpan={view === "split" ? 4 : 3}>{h.header}</td>
            </tr>
            {view === "unified"
              ? h.lines.map((l, i) => (
                  <tr key={i} className={`dv-${l.kind}`}>
                    <td className="dv-no">{l.oldNo ?? ""}</td>
                    <td className="dv-no">{l.newNo ?? ""}</td>
                    <td className="dv-code">
                      <span className="dv-sign" aria-hidden="true">
                        {SIGN[l.kind]}
                      </span>
                      <Code line={l} html={html} />
                    </td>
                  </tr>
                ))
              : splitRows(h.lines).map((r, i) => (
                  <tr key={i}>
                    <td className={`dv-no dv-${r.left?.kind ?? "empty"}`}>{r.left?.oldNo ?? ""}</td>
                    <td className={`dv-code dv-${r.left?.kind ?? "empty"}`}>{r.left && <Code line={r.left} html={html} />}</td>
                    <td className={`dv-no dv-${r.right?.kind ?? "empty"}`}>{r.right?.newNo ?? ""}</td>
                    <td className={`dv-code dv-${r.right?.kind ?? "empty"}`}>{r.right && <Code line={r.right} html={html} />}</td>
                  </tr>
                ))}
          </tbody>
        ))}
      </table>
    </div>
  );
}
