// Recuperação e Revisão no modo web (épico #205, spec §2): a lista vem da
// visão do hub; as ações vão ao ambiente pelo canal reverso (fase 2, #343) e
// só aparecem com ele online: retomar (com o Remote Control) e marcar as
// decisões autônomas como revisadas.
import "./web.css";
import { useState } from "react";
import { useNow } from "../App";
import { itemLabel, lookup } from "../control-room/attention";
import { STATUS } from "../project/status";
import { itemTab } from "../project/tabs";
import { useNav } from "../state/nav";
import { useTower } from "../state/tower";
import { PROJECT_ITEM } from "../util/item";
import { ago } from "../util/time";
import type { Session } from "../protocol/gen";
import { RESUME_PROMPT } from "../recovery/RecoveryScreen";
import { markReviewedOnEnvs, rcRequested } from "./envActions";

/** Recuperação na web: as sessões interrompidas; retomar com o ambiente online. */
export function WebRecovery() {
  const { envs, dashboards, status, api } = useTower();
  const nav = useNav();
  const now = useNow();
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const resume = async (env: string, s: Session) => {
    setBusy(s.id);
    setError(null);
    try {
      // Pelo celular, a conversa é pelo Remote Control: a retomada já o liga.
      const n = await api.call<Session>(env, "session.resume", {
        session_id: s.id,
        prompt: RESUME_PROMPT,
        remote_control: true,
      });
      rcRequested(n.id);
      nav.openTerminal({ env, session: n.id });
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(null);
    }
  };
  const rows = envs.flatMap((e) =>
    (dashboards[e.id]?.interrupted_sessions ?? []).map((s) => ({
      env: e,
      s,
      ...lookup(dashboards[e.id], s),
    })),
  );
  return (
    <main className="web-session" aria-labelledby="web-rec-h">
      <h1 id="web-rec-h" className="web-session-title">
        Recuperação · {rows.length}
      </h1>
      <p className="muted">
        Sessões que pararam no meio do trabalho. Retomar liga o Remote Control, para continuar no app Claude; descartar
        fica no app desktop.
      </p>
      {error && (
        <p role="alert" className="web-error">
          {error}
        </p>
      )}
      {rows.length === 0 ? (
        <p className="empty">Nenhuma sessão interrompida.</p>
      ) : (
        <ul className="web-list">
          {rows.map((r) => (
            <li key={`${r.env.id}:${r.s.id}`} className="web-row">
              <span className="web-row-main">
                <strong>{itemLabel(r.project, r.wi, r.gestor, r.scope)}</strong>
                <span className="muted small">
                  {r.env.name} · parou {ago(r.s.last_event_at, now)}
                  {r.s.exit_code !== null ? ` · código ${r.s.exit_code}` : ""}
                  {!status[r.env.id]?.connected && " · ambiente offline"}
                </span>
              </span>
              {status[r.env.id]?.connected && (
                <button
                  type="button"
                  className="btn btn-sm btn-primary"
                  disabled={busy !== null}
                  aria-label={`Retomar ${itemLabel(r.project, r.wi, r.gestor, r.scope)}`}
                  onClick={() => void resume(r.env.id, r.s)}
                >
                  {busy === r.s.id ? "Retomando…" : "Retomar"}
                </button>
              )}
              <button
                type="button"
                className="btn btn-sm"
                onClick={() => nav.openTerminal({ env: r.env.id, session: r.s.id })}
              >
                Ver sessão
              </button>
            </li>
          ))}
        </ul>
      )}
    </main>
  );
}

/**
 * Revisão na web: itens em revisão ou com decisões autônomas pendentes,
 * abertos no projeto (histórico do hub); com os ambientes do projeto online,
 * marcar as pendentes como revisadas (`review.mark_reviewed` em cada um).
 */
export function WebReview() {
  const { envs, dashboards, status, api, refreshHub } = useTower();
  const nav = useNav();
  const [busy, setBusy] = useState<string | null>(null);
  const [msg, setMsg] = useState<{
    key: string;
    text: string;
    error: boolean;
  } | null>(null);
  const seen = new Set<string>();
  const same = (a: string, b: string) => a.toLowerCase() === b.toLowerCase();
  /** Ambientes online que têm o projeto (as decisões ficam onde a sessão rodou). */
  const onlineFor = (owner: string, repo: string) =>
    envs.filter(
      (e) =>
        status[e.id]?.connected &&
        dashboards[e.id]?.projects.some((p) => same(p.project.gh_owner, owner) && same(p.project.gh_repo, repo)),
    );
  const mark = async (key: string, owner: string, repo: string, gh: number) => {
    setBusy(key);
    setMsg(null);
    try {
      const r = await markReviewedOnEnvs(api, onlineFor(owner, repo), owner, repo, gh);
      const text =
        r.noPr.length > 0
          ? `Sem PR em ${r.noPr.join(", ")}: marque pelo app desktop.`
          : r.marked === 1
            ? "1 decisão marcada como revisada."
            : `${r.marked} decisões marcadas como revisadas.`;
      setMsg({ key, text, error: r.noPr.length > 0 });
      await refreshHub();
    } catch (e) {
      setMsg({
        key,
        text: e instanceof Error ? e.message : String(e),
        error: true,
      });
    } finally {
      setBusy(null);
    }
  };
  const rows = envs.flatMap((e) =>
    (dashboards[e.id]?.projects ?? []).flatMap((p) =>
      p.work_items
        .filter((w) => w.gh_number !== PROJECT_ITEM && (w.status === "em_revisao" || w.pending_decisions > 0))
        .filter((w) => {
          const k = `${p.project.gh_owner}/${p.project.gh_repo}#${w.gh_number}`.toLowerCase();
          if (seen.has(k)) return false;
          seen.add(k);
          return true;
        })
        .map((w) => ({ project: p.project, wi: w })),
    ),
  );
  return (
    <main className="web-session" aria-labelledby="web-rev-h">
      <h1 id="web-rev-h" className="web-session-title">
        Revisão · {rows.length}
      </h1>
      <p className="muted">
        As decisões aparecem no histórico de cada item. Marcar como revisadas precisa do ambiente online; comentar no PR
        fica no app desktop.
      </p>
      {rows.length === 0 ? (
        <p className="empty">Nada para revisar.</p>
      ) : (
        <ul className="web-list">
          {rows.map(({ project, wi }) => {
            const key = `${project.id}:${wi.id}`;
            const canMark = wi.pending_decisions > 0 && onlineFor(project.gh_owner, project.gh_repo).length > 0;
            return (
              <li key={key} className="web-row">
                <span className="web-row-main">
                  <strong>{itemLabel(project, wi)}</strong>
                  <span className="muted small">
                    {STATUS[wi.status]?.text ?? wi.status}
                    {wi.pending_decisions > 0 &&
                      ` · ${wi.pending_decisions} ${wi.pending_decisions === 1 ? "decisão pendente" : "decisões pendentes"}`}
                  </span>
                </span>
                <button
                  type="button"
                  className="btn btn-sm"
                  onClick={() =>
                    nav.openTab(
                      itemTab(
                        {
                          name: "project",
                          owner: project.gh_owner,
                          repo: project.gh_repo,
                        },
                        wi.gh_number,
                        wi.title ?? "",
                      ),
                    )
                  }
                >
                  Abrir item
                </button>
                {canMark && (
                  <button
                    type="button"
                    className="btn btn-sm btn-primary"
                    disabled={busy !== null}
                    aria-label={`Marcar decisões de ${itemLabel(project, wi)} como revisadas`}
                    onClick={() => void mark(key, project.gh_owner, project.gh_repo, wi.gh_number)}
                  >
                    {busy === key ? "Marcando…" : "Marcar como revisadas"}
                  </button>
                )}
                {msg?.key === key && (
                  <span role={msg.error ? "alert" : "status"} className={msg.error ? "web-error small" : "muted small"}>
                    {msg.text}
                  </span>
                )}
              </li>
            );
          })}
        </ul>
      )}
    </main>
  );
}
