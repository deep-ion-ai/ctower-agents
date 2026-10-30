import "./recovery.css";
import { GESTOR_LABEL } from "../control-room/attention";
import { itemRef, PROJECT_ITEM } from "../util/item";
import { useCallback, useEffect, useState } from "react";
import { contextNote } from "../project/templates";
import type {
  ArchivedItem,
  RecoveryItem,
  Session,
  TranscriptIntegrity,
  TranscriptTail,
} from "../protocol/gen";
import type { DaemonApi } from "../api/daemon";
import { useNav } from "../state/nav";
import { Icon } from "../shell/icons";
import { SideContent, SideGroup, SidePanel, SideRow } from "../shell/SidePanel";
import { useStartSession } from "../shell/start";
import { useTower } from "../state/tower";
import { duration } from "../util/time";

/** Mesmo texto do daemon (`ct_sessions::RESUME_PROMPT`), editável. */
export const RESUME_PROMPT =
  "A sessão foi interrompida antes de terminar. Verifique o estado do worktree e dos commits antes de continuar.";

const INTEGRITY: Record<TranscriptIntegrity, { text: string; tone: string }> = {
  original: { text: "íntegro", tone: "done" },
  restauravel_do_backup: { text: "restaurável do backup", tone: "attention" },
  perdido: { text: "perdido (sem original e sem backup)", tone: "error" },
};

/** Último evento do ledger em texto (o código cru fica no title). */
const EVENT_TEXT: Record<string, string> = {
  session_state: "mudança de estado da sessão",
  needs_human: "pediu sua atenção",
  transcript_backup: "backup do transcript",
  transcript_volatile: "transcript em local volátil",
  session_started: "sessão iniciada",
  session_exit_error: "o agente saiu com erro",
  worktree_set: "worktree criado",
  status_changed: "status do work item mudou",
};

function eventText(kind: string, ts: number): string {
  const hora = new Date(ts).toLocaleTimeString("pt-BR", { hour: "2-digit", minute: "2-digit" });
  return `${EVENT_TEXT[kind] ?? kind} · ${hora}`;
}

const plural = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`;

/**
 * Motivo da interrupção pelo que a sessão guarda: com código de saída, o
 * agente saiu com erro; sem, o processo se perdeu (daemon ou máquina caiu).
 */
export function reason(s: Session): string {
  return s.exit_code === null ? "processo perdido" : `saiu com código ${s.exit_code}`;
}

/**
 * Resumo do `git status --short --branch` (resumido pelo daemon): commits
 * sem push, arquivos alterados, falta de branch remota.
 */
export function worktreeSummary(status: string | null): string {
  if (status === null) return "não encontrado";
  const lines = status.split("\n").filter((l) => l.trim());
  const head = lines[0]?.startsWith("## ") ? lines.shift()! : null;
  // O daemon corta em 20 linhas e acrescenta "… +N linhas".
  let files = 0;
  for (const l of lines) {
    const more = /^… \+(\d+) linhas$/.exec(l);
    files += more ? Number(more[1]) : 1;
  }
  const parts: string[] = [];
  if (head && !head.includes("...")) parts.push("sem branch remota");
  const ahead = head ? /\[[^\]]*ahead (\d+)/.exec(head)?.[1] : undefined;
  if (ahead) parts.push(plural(Number(ahead), "commit sem push", "commits sem push"));
  if (files) parts.push(plural(files, "arquivo alterado", "arquivos alterados"));
  return parts.join(" · ") || "limpo e em dia com a remota";
}

interface Row {
  env: string;
  item: RecoveryItem;
}

/** Sessão descartada (arquivada sem ter sido retomada). */
interface Archived {
  env: string;
  item: ArchivedItem;
}

/** Quantas falas do fim do transcript a tela mostra. */
export const TAIL_ENTRIES = 30;

const ROLE_TEXT: Record<string, string> = { user: "você", assistant: "agente", tool: "ferramenta" };

/** Sessão retomada pelo lote: a linha com o link para o terminal. */
interface Resumed {
  env: string;
  session: string;
  name: string;
}

interface Batch {
  done: Resumed[];
  failed: number;
  lost: number;
}

/** "5 retomadas · 1 erro · 1 sem transcript". */
export function batchSummary(b: Batch): string {
  const parts = [plural(b.done.length, "retomada", "retomadas")];
  if (b.failed) parts.push(plural(b.failed, "erro", "erros"));
  if (b.lost) parts.push(`${b.lost} sem transcript`);
  return parts.join(" · ");
}

const rowKey = (r: Row | Archived) => `${r.env}:${r.item.session.id}`;

/**
 * Sessões interrompidas (spec §7.4), no modelo do VS Code (#318): a lista no
 * painel lateral e a sessão escolhida (endereço da aba) na área de editor.
 */
export function RecoveryScreen({ env, session }: { env?: string; session?: string }) {
  const { envs, status, dashboards, api } = useTower();
  const nav = useNav();
  const startSession = useStartSession();
  const [rows, setRows] = useState<Row[] | null>(null);
  const [archived, setArchived] = useState<Archived[]>([]);
  const [prompts, setPrompts] = useState<Record<string, string>>({});
  const [errors, setErrors] = useState<Record<string, string>>({});
  const [busy, setBusy] = useState(false);
  const [confirming, setConfirming] = useState(false);
  const [batch, setBatch] = useState<Batch | null>(null);

  const connected = envs.filter((e) => status[e.id]?.connected).map((e) => e.id);
  const sig = connected.join(",");
  const interrupted = connected.reduce((n, e) => n + (dashboards[e]?.interrupted_sessions.length ?? 0), 0);

  // Interrompidas e descartadas juntas: descartar (ou recomeçar) move a
  // sessão de uma lista para a outra.
  const load = useCallback(async () => {
    const per = <T,>(method: string) =>
      Promise.all(
        sig
          .split(",")
          .filter(Boolean)
          .map(async (env) => {
            try {
              const items = await api.call<T[]>(env, method);
              return items.map((item) => ({ env, item }));
            } catch {
              return [];
            }
          }),
      ).then((all) => all.flat());
    const [list, gone] = await Promise.all([per<RecoveryItem>("recovery.list"), per<ArchivedItem>("recovery.archived")]);
    setRows(list);
    // Mais recentes primeiro, misturando os ambientes.
    setArchived(gone.sort((a, b) => b.item.archived_at - a.item.archived_at));
  }, [api, sig]);

  useEffect(() => {
    void load();
  }, [load, interrupted]);

  const setError = (r: Row, msg: string) => setErrors((all) => ({ ...all, [r.item.session.id]: msg }));

  const act = async (r: Row, f: () => Promise<void>) => {
    setError(r, "");
    try {
      await f();
      await load();
    } catch (e) {
      setError(r, String(e));
    }
  };

  const envName = (id: string) => envs.find((e) => e.id === id)?.name ?? id;
  const gestorOf = (r: Row | Archived) => dashboards[r.env]?.gestor_work_item_id === r.item.work_item.id;
  const titleOf = (r: Row | Archived) =>
    dashboards[r.env]?.projects
      .flatMap((p) => p.work_items)
      .find((w) => w.id === r.item.work_item.id)?.title ?? "";
  /** Nome curto da linha: "#29 título", "Planejamento · projeto" ou o gestor. */
  const nameOf = (r: Row | Archived) => {
    if (gestorOf(r)) return GESTOR_LABEL;
    const wi = r.item.work_item;
    if (wi.gh_number === PROJECT_ITEM) return `Planejamento · ${r.item.project_name}`;
    // Chave do Jira no projeto Jira (#352): o projeto vem do dashboard.
    const project = dashboards[r.env]?.projects.find((p) => p.project.id === wi.project_id)?.project;
    return `${itemRef(wi.gh_number, project)} ${titleOf(r)}`.trim();
  };

  const resume = (r: Row) =>
    api.call<Session>(r.env, "session.resume", {
      session_id: r.item.session.id,
      prompt: prompts[r.item.session.id] ?? RESUME_PROMPT,
    });

  // "Retomar todas": só as que têm transcript, cada uma com o seu prompt. Uma
  // por vez em cada ambiente, com os ambientes em paralelo; não navega.
  const resumable = (rows ?? []).filter((r) => r.item.transcript !== "perdido");
  const lostCount = (rows?.length ?? 0) - resumable.length;
  const resumableEnvs = [...new Set(resumable.map((r) => envName(r.env)))];
  const resumeAll = async () => {
    setConfirming(false);
    setBusy(true);
    setBatch(null);
    const byEnv = new Map<string, Row[]>();
    for (const r of resumable) byEnv.set(r.env, [...(byEnv.get(r.env) ?? []), r]);
    const done: Resumed[] = [];
    let failed = 0;
    await Promise.all(
      [...byEnv.values()].map(async (group) => {
        for (const r of group) {
          setError(r, "");
          try {
            const novo = await resume(r);
            done.push({ env: r.env, session: novo.id, name: nameOf(r) });
          } catch (e) {
            failed++;
            setError(r, String(e));
          }
        }
      }),
    );
    setBatch({ done, failed, lost: lostCount });
    setBusy(false);
    await load();
  };

  // Sem sessão no endereço (ou ela já saiu da lista), a primeira interrompida.
  const at = <R extends Row | Archived>(list: R[] | null | undefined) =>
    list?.find((r) => r.env === env && r.item.session.id === session);
  const gone = at(archived);
  const selected = gone ? undefined : (at(rows) ?? rows?.[0]);
  const select = (r: Row | Archived) =>
    nav.openTab({ id: nav.current, screen: { name: "recovery", env: r.env, session: r.item.session.id } });

  return (
    <main className="page recovery">
      <SideContent>
        <SidePanel
          title="Recuperação"
          label="Sessões interrompidas"
          actions={
            <button
              type="button"
              className="ibtn"
              aria-label="Retomar todas"
              aria-describedby="rc-all-d"
              data-tip={`Retomar todas (${resumable.length})`}
              disabled={busy || resumable.length === 0}
              onClick={() => setConfirming(true)}
            >
              <Icon name="recovery" />
              <span id="rc-all-d" hidden>
                {plural(resumable.length, "sessão com transcript", "sessões com transcript")}
                {lostCount > 0 && `; ${plural(lostCount, "fica de fora", "ficam de fora")}`}
              </span>
            </button>
          }
        >
          {confirming && (
            <div role="alertdialog" aria-label="Retomar todas" className="rc-confirm">
              <p>
                Retomar {plural(resumable.length, "sessão", "sessões")} em {resumableEnvs.join(", ")}?
                {lostCount > 0 &&
                  ` ${plural(lostCount, "sessão perdida fica", "sessões perdidas ficam")} de fora (use Recomeçar com nota).`}
              </p>
              <button type="button" className="btn btn-primary" onClick={() => void resumeAll()}>
                Retomar {resumable.length}
              </button>
              <button type="button" className="btn" onClick={() => setConfirming(false)}>
                Cancelar
              </button>
            </div>
          )}
          {batch && (
            <SideGroup title="Última retomada em lote" count={batch.done.length}>
              <li role="status" className="rc-summary">
                {batchSummary(batch)}
              </li>
              {batch.done.map((d) => (
                <SideRow
                  key={d.session}
                  label={d.name}
                  sub="retomada · abrir terminal"
                  tone="done"
                  onClick={() => nav.openTerminal({ env: d.env, session: d.session })}
                />
              ))}
            </SideGroup>
          )}
          <SideGroup title="Interrompidas" count={rows?.length}>
            {rows?.map((r) => (
              <SideRow
                key={rowKey(r)}
                label={nameOf(r)}
                sub={`${envName(r.env)} · ${reason(r.item.session)} há ${duration(r.item.stopped_for_ms)}`}
                tone={r.item.transcript === "perdido" ? "error" : "attention"}
                selected={selected !== undefined && rowKey(selected) === rowKey(r)}
                onClick={() => select(r)}
              />
            ))}
          </SideGroup>
          {archived.length > 0 && (
            <SideGroup title="Descartadas recentemente" count={archived.length}>
              {archived.map((a) => (
                <SideRow
                  key={rowKey(a)}
                  label={nameOf(a)}
                  sub={`${envName(a.env)} · descartada há ${duration(Date.now() - a.item.archived_at)}`}
                  selected={gone !== undefined && rowKey(gone) === rowKey(a)}
                  onClick={() => select(a)}
                />
              ))}
            </SideGroup>
          )}
        </SidePanel>
      </SideContent>
      {rows === null && <p className="muted">Carregando…</p>}
      {rows?.length === 0 && !gone && (
        <div className="recovery-empty">
          <h1>Recuperação</h1>
          <p className="empty">Nenhuma sessão interrompida.</p>
        </div>
      )}
      {selected && (
        <Detail
          key={rowKey(selected)}
          r={selected}
          name={nameOf(selected)}
          envName={envName(selected.env)}
          canOpen={envs.find((e) => e.id === selected.env)?.kind === "wsl"}
          prompt={prompts[selected.item.session.id] ?? RESUME_PROMPT}
          onPrompt={(v) => setPrompts((all) => ({ ...all, [selected.item.session.id]: v }))}
          error={errors[selected.item.session.id]}
          onResume={() =>
            act(selected, async () => {
              const novo = await resume(selected);
              nav.openTerminal({ env: selected.env, session: novo.id });
            })
          }
          onRestart={(origin) => {
            const { session: s, work_item: wi, transcript } = selected.item;
            void act(selected, async () => {
              await api.call(selected.env, "session.archive", { session_id: s.id });
              // A mesma modal de início, com a nota de contexto já no prompt.
              startSession(
                {
                  env: selected.env,
                  projectId: wi.project_id,
                  ghNumber: wi.gh_number,
                  note: contextNote(s.id, transcript === "perdido"),
                },
                origin,
              );
            });
          }}
          onOpenWorktree={() =>
            act(selected, () => api.openVsCode(selected.env, selected.item.work_item.worktree_path!))
          }
          onDiscard={() =>
            act(selected, async () => {
              await api.call(selected.env, "session.archive", { session_id: selected.item.session.id });
              // Senão o endereço mostraria a própria sessão, agora entre as descartadas.
              nav.openTab({ id: nav.current, screen: { name: "recovery" } });
            })
          }
        />
      )}
      {gone && (
        <ArchivedDetail
          key={rowKey(gone)}
          a={gone}
          name={nameOf(gone)}
          envName={envName(gone.env)}
          onRestart={(origin) => {
            const { session: s, work_item: wi, transcript } = gone.item;
            // Já arquivada: só abre a modal de início com a nota.
            startSession(
              {
                env: gone.env,
                projectId: wi.project_id,
                ghNumber: wi.gh_number,
                note: contextNote(s.id, transcript === "perdido"),
              },
              origin,
            );
          }}
        />
      )}
    </main>
  );
}

/** A sessão escolhida: estado, ações, prompt de retomada e fatos. */
function Detail({
  r,
  name,
  envName,
  canOpen,
  prompt,
  onPrompt,
  error,
  onResume,
  onRestart,
  onOpenWorktree,
  onDiscard,
}: {
  r: Row;
  name: string;
  envName: string;
  canOpen: boolean;
  prompt: string;
  onPrompt(v: string): void;
  error?: string;
  onResume(): void;
  onRestart(origin: Element): void;
  onOpenWorktree(): void;
  onDiscard(): void;
}) {
  const { session: s, work_item: wi, transcript } = r.item;
  const integ = INTEGRITY[transcript];
  const lost = transcript === "perdido";
  const worktree = r.item.worktree_status !== null && wi.worktree_path !== null;
  const why =
    s.exit_code === null
      ? "O processo da sessão terminou sem código de saída (o daemon ou a máquina parou)."
      : `O agente saiu com código ${s.exit_code} antes de terminar.`;
  const state = lost
    ? "Sem transcript não há como retomar a conversa: recomece com uma nota de contexto."
    : transcript === "restauravel_do_backup"
      ? "O transcript original sumiu, mas há cópia no backup: ao retomar, ele é restaurado."
      : "O transcript está íntegro: dá para retomar de onde parou.";

  return (
    <section className="recovery-detail" aria-label={name}>
      <div className="recovery-title">
        <span className="badge" data-tone={lost ? "error" : "attention"}>
          interrompida
        </span>
        <h1>{name}</h1>
        <span className="stopped">parada há {duration(r.item.stopped_for_ms)}</span>
      </div>
      <p className="muted recovery-why">
        {why} {state}
      </p>
      <div className="recovery-actions">
        {!lost && (
          <button type="button" className="btn btn-primary" onClick={onResume}>
            Retomar a conversa
          </button>
        )}
        <button
          type="button"
          className={lost ? "btn btn-primary" : "btn"}
          aria-describedby="rc-restart-d"
          onClick={(e) => onRestart(e.currentTarget)}
        >
          Recomeçar com nota
        </button>
        <span id="rc-restart-d" hidden>
          Descarta esta sessão e abre a modal de início com uma nota de contexto no prompt.
        </span>
        <button
          type="button"
          className="btn"
          onClick={onOpenWorktree}
          disabled={!worktree || !canOpen}
          title={!worktree ? "Worktree não encontrado" : !canOpen ? "Só em ambientes WSL desta máquina" : undefined}
        >
          Abrir worktree
        </button>
        <span className="recovery-spacer" />
        <button type="button" className="btn btn-danger" onClick={onDiscard}>
          Descartar
        </button>
      </div>
      {error && (
        <p role="alert" className="start-error">
          {error}
        </p>
      )}
      {!lost && (
        <div className="field">
          <label htmlFor={`retomar-${s.id}`}>Prompt de retomada</label>
          <textarea id={`retomar-${s.id}`} rows={2} value={prompt} onChange={(e) => onPrompt(e.target.value)} />
        </div>
      )}
      <section className="rc-card" aria-labelledby="rc-state">
        <div className="rc-card-head">
          <h2 id="rc-state">Estado</h2>
        </div>
        <dl className="recovery-facts">
          <div>
            <dt>Ambiente</dt>
            <dd>{envName}</dd>
          </div>
          <div>
            <dt>Branch</dt>
            <dd className="mono">{wi.branch ?? "—"}</dd>
          </div>
          <div>
            <dt>Transcript</dt>
            <dd className="integrity" data-tone={integ.tone}>
              {integ.text}
            </dd>
          </div>
          <div>
            <dt>Worktree</dt>
            <dd>{worktreeSummary(r.item.worktree_status)}</dd>
          </div>
          <div>
            <dt>Último evento</dt>
            <dd title={r.item.last_event?.kind}>
              {r.item.last_event ? eventText(r.item.last_event.kind, r.item.last_event.ts) : "—"}
            </dd>
          </div>
          <div>
            <dt>Sessão do Claude</dt>
            <dd className="mono">{s.tool_session_id ?? "—"}</dd>
          </div>
          <div className="wide">
            <dt>Caminho do worktree</dt>
            <dd className="mono">{wi.worktree_path ?? "—"}</dd>
          </div>
        </dl>
      </section>
      <Tail env={r.env} session={s.id} lost={lost} />
      {r.item.worktree_status !== null && (
        <section className="rc-card" aria-labelledby="rc-git">
          <div className="rc-card-head">
            <h2 id="rc-git">git status</h2>
          </div>
          <pre className="rc-term">{r.item.worktree_status}</pre>
        </section>
      )}
    </section>
  );
}

/**
 * Sessão descartada: só leitura (o daemon não retoma arquivadas). Dá para
 * recomeçar com nota e ver o fim da conversa.
 */
function ArchivedDetail({
  a,
  name,
  envName,
  onRestart,
}: {
  a: Archived;
  name: string;
  envName: string;
  onRestart(origin: Element): void;
}) {
  const { session: s, work_item: wi, transcript } = a.item;
  const integ = INTEGRITY[transcript];
  const lost = transcript === "perdido";
  return (
    <section className="recovery-detail" aria-label={name}>
      <div className="recovery-title">
        <span className="badge">descartada</span>
        <h1>{name}</h1>
        <span className="stopped">descartada há {duration(Date.now() - a.item.archived_at)}</span>
      </div>
      <p className="muted recovery-why">
        A sessão foi descartada sem ser retomada e não volta mais. Para continuar o trabalho, recomece com uma nota de
        contexto.
      </p>
      <div className="recovery-actions">
        <button type="button" className="btn btn-primary" onClick={(e) => onRestart(e.currentTarget)}>
          Recomeçar com nota
        </button>
      </div>
      <section className="rc-card" aria-labelledby="rc-state">
        <div className="rc-card-head">
          <h2 id="rc-state">Estado</h2>
        </div>
        <dl className="recovery-facts">
          <div>
            <dt>Ambiente</dt>
            <dd>{envName}</dd>
          </div>
          <div>
            <dt>Branch</dt>
            <dd className="mono">{wi.branch ?? "—"}</dd>
          </div>
          <div>
            <dt>Transcript</dt>
            <dd className="integrity" data-tone={integ.tone}>
              {integ.text}
            </dd>
          </div>
          <div>
            <dt>Sessão do Claude</dt>
            <dd className="mono">{s.tool_session_id ?? "—"}</dd>
          </div>
        </dl>
      </section>
      <Tail env={a.env} session={s.id} lost={lost} />
    </section>
  );
}

/** "Últimas linhas": o fim da conversa pelo transcript (`transcript.tail`), mesmo com a sessão parada. */
function Tail({ env, session, lost }: { env: string; session: string; lost: boolean }) {
  const { api } = useTower();
  const tail = useTail(api, env, session, lost);
  return (
    <section className="rc-card" aria-labelledby="rc-tail">
      <div className="rc-card-head">
        <h2 id="rc-tail">Últimas linhas</h2>
        {tail.data?.source === "backup" && <span className="muted rc-source">do backup</span>}
      </div>
      {lost ? (
        <p className="muted rc-tail-note">Sem transcript nem backup: não há conversa para mostrar.</p>
      ) : tail.error ? (
        <p className="muted rc-tail-note">Não foi possível ler o transcript: {tail.error}</p>
      ) : !tail.data ? (
        <p className="muted rc-tail-note">Carregando…</p>
      ) : tail.data.entries.length === 0 ? (
        <p className="muted rc-tail-note">O transcript ainda não tem falas.</p>
      ) : (
        <ol className="rc-term rc-tail" aria-label="Últimas linhas do transcript">
          {tail.data.entries.map((e, i) => (
            <li key={i} data-role={e.role} title={e.ts ?? undefined}>
              <span className="rc-role">{ROLE_TEXT[e.role] ?? e.role}</span>
              {e.text}
            </li>
          ))}
        </ol>
      )}
    </section>
  );
}

function useTail(api: DaemonApi, env: string, session: string, lost: boolean) {
  const [state, setState] = useState<{ data?: TranscriptTail; error?: string }>({});
  useEffect(() => {
    if (lost) return;
    let live = true;
    setState({});
    api
      .call<TranscriptTail>(env, "transcript.tail", { session_id: session, entries: TAIL_ENTRIES })
      .then((data) => live && setState({ data }))
      .catch((e) => live && setState({ error: String(e) }));
    return () => {
      live = false;
    };
  }, [api, env, session, lost]);
  return state;
}
