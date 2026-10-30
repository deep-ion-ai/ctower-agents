import "./review.css";
import { GESTOR_LABEL } from "../control-room/attention";
import { changeLabel, changeLabels, changeRef, forgeLabel, isGitlabProject, itemRef, itemShort, PROJECT_ITEM } from "../util/item";
import { useCallback, useEffect, useId, useState } from "react";
import type { Decision, Marked, PrComment, Project, Review } from "../protocol/gen";
import type { HubHistory } from "../protocol/hub-gen";
import { useNav } from "../state/nav";
import { useTower } from "../state/tower";
import { Icon } from "../shell/icons";
import { SideContent, SideGroup, SidePanel, SideRow } from "../shell/SidePanel";
import { prUrl } from "../terminal/TerminalScreen";
import { reviewKey, useReviewUi, useReviews, type ReviewFilter } from "./ui-store";

/** Item com decisões a revisar (ou em revisão), num ambiente. */
interface Entry {
  env: string;
  workItemId: number;
  /** Ausente no gestor (o sentinela fica fora de `projects`). */
  project?: Project;
  ghNumber?: number;
  title: string;
  gestor: boolean;
  /** Pendentes: do `review.get` quando já lido; senão, do dashboard. */
  count: number;
  review?: Review;
}

const openPr = (r?: Review) => r?.prs.find((p) => p.state === "open");

/** Nome do item: "#28 título", "Planejamento · projeto" ou o gestor. */
function entryName(e: Pick<Entry, "gestor" | "ghNumber" | "title" | "project">): string {
  if (e.gestor) return GESTOR_LABEL;
  if (e.ghNumber === PROJECT_ITEM) return `Planejamento · ${e.project?.name ?? ""}`.trim();
  return `${e.ghNumber !== undefined ? itemRef(e.ghNumber, e.project) : ""} ${e.title}`.trim();
}

/**
 * Revisão das decisões autônomas antes do merge (spec §7.6), no modelo do
 * Source Control do VS Code (#318): os itens no painel lateral, agrupados
 * por PR aberto, e o item escolhido (endereço da aba) na área de editor.
 */
export function ReviewScreen({ env, workItemId }: { env?: string; workItemId?: number }) {
  const { envs, status, dashboards, api } = useTower();
  const nav = useNav();
  const [reviews, putReview] = useReviews();

  const connected = envs.filter((e) => status[e.id]?.connected).map((e) => e.id);
  const candidates: Entry[] = connected.flatMap((id) => {
    const d = dashboards[id];
    if (!d) return [];
    const items: Entry[] = d.projects.flatMap((p) =>
      p.work_items
        .filter((w) => w.status === "em_revisao" || w.pending_decisions > 0)
        .map((w) => ({
          env: id,
          workItemId: w.id,
          project: p.project,
          ghNumber: w.gh_number,
          title: w.title ?? "",
          gestor: false,
          count: w.pending_decisions,
        })),
    );
    if (d.gestor_work_item_id !== null)
      items.push({ env: id, workItemId: d.gestor_work_item_id, title: GESTOR_LABEL, gestor: true, count: 0 });
    return items;
  });
  // Relê quando a lista ou os contadores do dashboard mudam.
  const sig = candidates.map((c) => `${c.env}:${c.workItemId}:${c.count}`).join(",");

  useEffect(() => {
    for (const c of sig.split(",").filter(Boolean)) {
      const [e, id] = c.split(":");
      api.call<Review>(e, "review.get", { work_item_id: Number(id) }).then(
        (r) => putReview(reviewKey(e, Number(id)), r),
        () => {},
      );
    }
  }, [api, sig, putReview]);

  const entries = candidates
    .map((c) => {
      const review = reviews.get(reviewKey(c.env, c.workItemId));
      return { ...c, review, count: review?.decisions.length ?? c.count };
    })
    // O gestor não tem contador no dashboard: só entra com decisões pendentes.
    .filter((e) => !e.gestor || e.count > 0);
  const withPr = entries.filter((e) => openPr(e.review));
  const noPr = entries.filter((e) => !openPr(e.review));
  // "PR", "MR" ou "PR/MR", conforme as forjas dos itens listados (#352).
  const groupChange = changeLabels(entries.filter((e) => !e.gestor).map((e) => e.project));

  const addressed = env !== undefined && workItemId !== undefined;
  const first = withPr[0] ?? noPr[0];
  const target = addressed ? { env, workItemId } : first ? { env: first.env, workItemId: first.workItemId } : null;
  const isTarget = (e: Entry) => !!target && e.env === target.env && e.workItemId === target.workItemId;
  const select = (e: Entry) =>
    nav.openTab({ id: nav.current, screen: { name: "review", env: e.env, workItemId: e.workItemId } });
  const multiEnv = connected.length > 1;
  const envName = (id: string) => envs.find((e) => e.id === id)?.name ?? id;

  const row = (e: Entry, sub: string) => (
    <SideRow
      key={`${e.env}:${e.workItemId}`}
      label={entryName(e)}
      sub={multiEnv ? `${sub} · ${envName(e.env)}` : sub}
      tone={e.count > 0 ? "attention" : undefined}
      trailing={e.count}
      description={`${e.count} ${e.count === 1 ? "pendente" : "pendentes"}`}
      selected={isTarget(e)}
      onClick={() => select(e)}
    />
  );

  return (
    <main className="page review">
      <SideContent>
        <SidePanel title="Revisão" label="Itens com decisões a revisar">
          {entries.length === 0 && <p className="rv-side-empty">Nenhum item com decisões a revisar.</p>}
          {withPr.length > 0 && (
            <SideGroup title={`Com ${groupChange} aberto`} count={withPr.length}>
              {withPr.map((e) => {
                const pr = openPr(e.review)!;
                return row(e, `${changeRef(e.project, pr.number)} → ${pr.base}`);
              })}
            </SideGroup>
          )}
          {noPr.length > 0 && (
            <SideGroup title={`Sem ${groupChange}`} count={noPr.length}>
              {noPr.map((e) =>
                row(
                  e,
                  e.gestor
                    ? "decisões autônomas do gestor"
                    : e.ghNumber === PROJECT_ITEM
                      ? "sessões sem issue associada"
                      : `sem ${changeLabel(e.project)} aberto`,
                ),
              )}
            </SideGroup>
          )}
        </SidePanel>
      </SideContent>
      {target ? (
        <ReviewDetail
          key={reviewKey(target.env, target.workItemId)}
          env={target.env}
          workItemId={target.workItemId}
        />
      ) : (
        <>
          <h1>Revisão</h1>
          <p className="empty">Nada para revisar: nenhuma decisão autônoma pendente.</p>
        </>
      )}
    </main>
  );
}

/** Campos comuns à decisão do ambiente (`review.get`) e à do hub (histórico). */
type AnyDecision = Pick<
  Decision,
  "id" | "uid" | "work_item_id" | "origin" | "title" | "context" | "decision" | "consequences" | "reviewed_at" | "reviewed_in_pr" | "created_at"
>;

const FILTERS: { id: ReviewFilter; label: string }[] = [
  { id: "autonomas", label: "Autônomas" },
  { id: "orientadas", label: "Orientadas" },
  { id: "revisadas", label: "Revisadas" },
];

function ReviewDetail({ env, workItemId }: { env: string; workItemId: number }) {
  const { api, dashboards, hub } = useTower();
  const [review, setReview] = useState<Review | null>(null);
  const [history, setHistory] = useState<HubHistory | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [ui, setUi] = useReviewUi(reviewKey(env, workItemId));
  const [, putReview] = useReviews();

  const dproject = dashboards[env]?.projects.find((p) => p.work_items.some((w) => w.id === workItemId));
  const project = dproject?.project;
  const dwi = dproject?.work_items.find((w) => w.id === workItemId);
  const gestor = dashboards[env]?.gestor_work_item_id === workItemId;
  // Orientadas e revisadas só existem no histórico do hub (`review.get` traz as pendentes).
  const withHistory = hub.up && !gestor && !!project && !!dwi && dwi.gh_number !== PROJECT_ITEM;

  const load = useCallback(async () => {
    try {
      const r = await api.call<Review>(env, "review.get", { work_item_id: workItemId });
      setReview(r);
      // O painel lateral usa o mesmo contador.
      putReview(reviewKey(env, workItemId), r);
      setError(null);
    } catch (e) {
      setError(String(e));
    }
  }, [api, env, workItemId, putReview]);

  const owner = project?.gh_owner;
  const repo = project?.gh_repo;
  const number = dwi?.gh_number;
  const loadHistory = useCallback(async () => {
    if (!withHistory) return;
    try {
      // No épico, as decisões das sub-issues entram junto (rotuladas por `sub_work_items`).
      setHistory(
        await api.hubCall<HubHistory>("hub.workitem.history", { owner, repo, number, include_sub_issues: true }),
      );
    } catch {
      setHistory(null);
    }
  }, [api, withHistory, owner, repo, number]);

  useEffect(() => {
    void load();
  }, [load]);
  useEffect(() => {
    void loadHistory();
  }, [loadHistory]);

  const pr = openPr(review ?? undefined) ?? review?.prs[review.prs.length - 1];
  const pending = review?.decisions ?? [];
  const n = pending.length;
  const guided = history?.decisions.filter((d) => d.origin === "orientada") ?? [];
  const reviewed = history?.decisions.filter((d) => d.origin === "autonoma" && d.reviewed_at !== null) ?? [];
  const lists: Record<ReviewFilter, AnyDecision[]> = { autonomas: pending, orientadas: guided, revisadas: reviewed };
  const filter = !history && ui.filter !== "autonomas" ? "autonomas" : ui.filter;
  const shown = lists[filter];

  const name = entryName({ gestor, ghNumber: dwi?.gh_number, title: dwi?.title ?? "", project });
  const url = review && project ? prUrl(review, project, project.base_branch) : null;
  // Nomes no provedor do projeto: MR !7 e GitLab no Jira + GitLab (#352).
  const change = changeLabel(project);
  const forge = forgeLabel(project);
  const gitlab = isGitlabProject(project);

  // O gestor não abre PR: a revisão dele não leva número (#239).
  // `ids`: só essas decisões ("Revisada" no cartão); sem, todas as pendentes.
  const mark = async (ids?: number[]) => {
    if (!pr && !gestor) return;
    setBusy(true);
    try {
      await api.call<Marked>(env, "review.mark_reviewed", {
        work_item_id: workItemId,
        ...(pr && !gestor ? { pr: pr.number } : {}),
        ...(ids ? { decision_ids: ids } : {}),
      });
      await load();
      await loadHistory();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const open = async (u: string | null) => {
    if (u) await api.openUrl(u).catch((e) => setError(String(e)));
  };

  // Comentário na conversa do PR com o token do GitHub do ambiente (#328).
  // No GitLab o daemon não comenta no MR: o texto vai para a área de
  // transferência e o MR abre no navegador, para colar (#352).
  const comment = async (body: string) => {
    if (gitlab) {
      await api.writeClipboard(body);
      await api.openUrl(pr!.url);
      return pr!.url;
    }
    const c = await api.call<PrComment>(env, "github.pr.comment", { project_id: project!.id, pr: pr!.number, body });
    return c.url;
  };

  // Aberta: a escolhida pelo usuário; sem escolha, só a primeira.
  const isOpen = (d: AnyDecision, i: number) => ui.open[d.uid] ?? i === 0;
  const toggle = (uid: string, v: boolean) => setUi((u) => ({ ...u, open: { ...u.open, [uid]: v } }));

  // Ids do ambiente nas pendentes (`review.get`); do hub nas do histórico.
  const subOf = (d: AnyDecision) => {
    if (filter === "autonomas") {
      if (d.work_item_id === workItemId) return null;
      const sub = dashboards[env]?.projects.flatMap((p) => p.work_items).find((w) => w.id === d.work_item_id);
      return sub ? `sub-issue ${itemShort(project, sub.gh_number)}` : "sub-issue";
    }
    if (!history || d.work_item_id === history.work_item.id) return null;
    const sub = (history.sub_work_items ?? []).find((w) => w.id === d.work_item_id);
    return sub ? `sub-issue ${itemShort(project, sub.gh_number)}` : "sub-issue";
  };

  return (
    <section className="review-detail" aria-label={name}>
      <div className="review-head">
        <h1>
          {!gestor && dwi && dwi.gh_number !== PROJECT_ITEM ? (
            <>
              <span className="mono review-num">{itemRef(dwi.gh_number, project)}</span> {dwi.title ?? ""}
            </>
          ) : (
            name || "Revisão"
          )}
        </h1>
        <span className="review-spacer" />
        {!gestor && (
          <button type="button" className="btn" onClick={() => void open(url)} disabled={!url}>
            {pr ? `Abrir ${changeRef(project, pr.number)}` : `Abrir ${change} no ${forge}`}
          </button>
        )}
        <button type="button" className="btn btn-primary" onClick={() => void mark()} disabled={(!pr && !gestor) || n === 0 || busy}>
          {n === 0 ? "Marcar como revisadas" : n === 1 ? "Marcar 1 como revisada" : `Marcar as ${n} como revisadas`}
        </button>
      </div>
      <p className="muted review-sub">
        {gestor ? "Decisões do gestor de projetos · " : project ? `${project.name} · ` : ""}
        {review?.work_item.kind === "epic" ? "épico · " : ""}
        {gestor ? (
          "O gestor não abre PR: marcar como revisada grava só a data da revisão."
        ) : pr ? (
          <>
            {changeRef(project, pr.number)} {pr.state === "open" ? "aberto" : pr.state === "merged" ? "mesclado" : "fechado"} ·{" "}
            <span className="mono">
              {pr.head} → {pr.base}
            </span>
          </>
        ) : (
          `Sem ${change} no espelho do ${forge} (sincronize depois de abrir o ${change}).`
        )}
      </p>
      <div className="review-filters" role="group" aria-label="Filtrar decisões">
        {FILTERS.map((f) => {
          const count = f.id === "autonomas" ? n : history ? lists[f.id].length : null;
          return (
            <button
              key={f.id}
              type="button"
              className="chip-btn"
              aria-pressed={filter === f.id}
              disabled={count === null}
              title={count === null ? "Vem do histórico do hub (fora do ar ou sem issue)" : undefined}
              onClick={() => setUi((u) => ({ ...u, filter: f.id }))}
            >
              {f.label}
              {count !== null && ` · ${count}`}
            </button>
          );
        })}
      </div>
      {filter === "autonomas" && (
        <p className="muted review-help">
          Decisões que o agente tomou sozinho (<strong>autônomas</strong>) e ainda não foram revisadas. As que você
          orientou na conversa já nascem revisadas. <strong>Revisada</strong> grava no ledger só aquela decisão;
          “Marcar como revisadas” grava todas de uma vez.
        </p>
      )}
      {error && (
        <p role="alert" className="start-error">
          {error}
        </p>
      )}
      {review && shown.length === 0 && (
        <p className="empty">
          {filter === "autonomas"
            ? "Nenhuma decisão pendente."
            : filter === "orientadas"
              ? "Nenhuma decisão orientada."
              : "Nenhuma decisão revisada."}
        </p>
      )}
      {shown.map((d, i) => (
        <DecisionCard
          key={d.uid}
          d={d}
          kind={filter}
          sub={subOf(d)}
          open={isOpen(d, i)}
          onToggle={(v) => toggle(d.uid, v)}
          onReviewed={
            filter === "autonomas" ? () => void mark([d.id]) : undefined
          }
          canReview={(!!pr || gestor) && !busy}
          onComment={pr && !gestor && project ? comment : undefined}
          change={change}
          forge={forge}
          copyOnly={gitlab}
          changeRefOf={(x) => changeRef(project, x)}
          onOpen={(u) => void open(u)}
        />
      ))}
    </section>
  );
}

const KIND: Record<ReviewFilter, { text: string; tone?: string }> = {
  autonomas: { text: "autônoma", tone: "attention" },
  orientadas: { text: "orientada" },
  revisadas: { text: "revisada", tone: "done" },
};

/** Texto do comentário no PR (ou MR): a decisão inteira, editável antes de enviar. */
function commentBody(d: AnyDecision): string {
  return [
    `**Decisão ${d.origin === "autonoma" ? "autônoma" : "orientada"}: ${d.title}**`,
    `**Contexto:** ${d.context}`,
    `**Decisão:** ${d.decision}`,
    ...(d.consequences ? [`**Consequências:** ${d.consequences}`] : []),
  ].join("\n\n");
}

/** Decisão recolhível: cabeçalho com as ações e, aberta, Contexto/Decisão/Consequências. */
function DecisionCard({
  d,
  kind,
  sub,
  open,
  onToggle,
  onReviewed,
  canReview,
  onComment,
  onOpen,
  change = "PR",
  forge = "GitHub",
  copyOnly = false,
  changeRefOf = (x: number) => `PR #${x}`,
}: {
  d: AnyDecision;
  kind: ReviewFilter;
  sub: string | null;
  open: boolean;
  onToggle(v: boolean): void;
  /** Só nas pendentes: grava esta decisão como revisada no ledger. */
  onReviewed?: () => void;
  canReview: boolean;
  /** Publica o comentário no PR; devolve a URL dele. */
  onComment?: (body: string) => Promise<string>;
  onOpen(url: string): void;
  /** `PR` ou `MR` (#352). */
  change?: string;
  /** `GitHub` ou `GitLab`. */
  forge?: string;
  /** Sem comentário pelo daemon (GitLab): copia o texto e abre o MR. */
  copyOnly?: boolean;
  /** `PR #n` ou `MR !n`. */
  changeRefOf?: (n: number) => string;
}) {
  const id = useId();
  const k = KIND[kind];
  const when = new Date(d.created_at).toLocaleString("pt-BR");
  // `null` = campo fechado.
  const [draft, setDraft] = useState<string | null>(null);
  const [sending, setSending] = useState(false);
  const [sent, setSent] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const send = async () => {
    if (!onComment || draft === null) return;
    setSending(true);
    try {
      setSent(await onComment(draft));
      setDraft(null);
      setError(null);
    } catch (e) {
      setError(String(e));
    } finally {
      setSending(false);
    }
  };
  return (
    <article className="rv-dec" aria-label={d.title}>
      <div className="rv-dec-head">
        <span className="badge" data-tone={k.tone}>
          {k.text}
        </span>
        <h2>
          <button
            type="button"
            className="rv-dec-toggle"
            aria-expanded={open}
            aria-controls={`${id}-b`}
            onClick={() => onToggle(!open)}
          >
            <Icon name={open ? "chevronDown" : "chevronRight"} size={14} />
            {d.title}
          </button>
        </h2>
        <span className="muted rv-dec-meta">
          {sub ? `${sub} · ` : ""}
          {kind === "revisadas" && d.reviewed_in_pr !== null ? `no ${changeRefOf(d.reviewed_in_pr)} · ` : ""}
          {when}
        </span>
        {onComment && (
          <button
            type="button"
            className="btn btn-sm"
            aria-expanded={draft !== null}
            aria-controls={`${id}-c`}
            onClick={() => setDraft((v) => (v === null ? commentBody(d) : null))}
          >
            Comentar no {change}
          </button>
        )}
        {onReviewed && (
          <button
            type="button"
            className="btn btn-sm btn-primary"
            onClick={onReviewed}
            disabled={!canReview}
            title={canReview ? "Grava só esta decisão como revisada" : undefined}
          >
            Revisada
          </button>
        )}
      </div>
      {draft !== null && (
        <div className="rv-dec-comment" id={`${id}-c`}>
          <label htmlFor={`${id}-t`}>Comentário no {change}</label>
          <textarea id={`${id}-t`} rows={6} value={draft} onChange={(e) => setDraft(e.target.value)} />
          <div className="rv-dec-comment-actions">
            <button type="button" className="btn btn-sm" onClick={() => setDraft(null)} disabled={sending}>
              Cancelar
            </button>
            <button
              type="button"
              className="btn btn-sm btn-primary"
              onClick={() => void send()}
              disabled={sending || draft.trim() === ""}
            >
              {sending ? "Enviando…" : copyOnly ? `Copiar e abrir o ${change}` : "Enviar comentário"}
            </button>
          </div>
        </div>
      )}
      {error && (
        <p role="alert" className="start-error rv-dec-note">
          {error}
        </p>
      )}
      {sent && (
        <p className="muted rv-dec-note" role="status">
          {copyOnly ? `Comentário copiado: cole na conversa do ${change}.` : `Comentário publicado no ${change}.`}{" "}
          <button type="button" className="rv-link" onClick={() => onOpen(sent)}>
            {copyOnly ? `Abrir o ${change} de novo` : `Ver no ${forge}`}
          </button>
        </p>
      )}
      <div className="rv-dec-body" id={`${id}-b`} hidden={!open}>
        <div>
          <h3>Contexto</h3>
          <p>{d.context}</p>
        </div>
        <div>
          <h3>Decisão</h3>
          <p>{d.decision}</p>
        </div>
        <div>
          <h3>Consequências</h3>
          <p className={d.consequences ? undefined : "muted"}>{d.consequences ?? "—"}</p>
        </div>
      </div>
    </article>
  );
}
