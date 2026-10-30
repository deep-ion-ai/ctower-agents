import { useNav } from "../state/nav";
import { itemRef, itemShort } from "../util/item";
import { useTower } from "../state/tower";

/** Meia-noite local do dia de `now`. */
export function startOfDay(now: number): number {
  const d = new Date(now);
  d.setHours(0, 0, 0, 0);
  return d.getTime();
}

type Dashboards = ReturnType<typeof useTower>["dashboards"];

/** Work items em revisão e concluídos hoje, de todos os ambientes. */
export function dayItems(envs: { id: string }[], dashboards: Dashboards, now: number) {
  const all = envs.flatMap((e) =>
    (dashboards[e.id]?.projects ?? []).flatMap((p) =>
      p.work_items.map((wi) => ({ env: e.id, project: p.project, wi })),
    ),
  );
  const today = startOfDay(now);
  return {
    review: all.filter((x) => x.wi.status === "em_revisao"),
    done: all.filter((x) => x.wi.status === "concluido" && x.wi.closed_at !== null && x.wi.closed_at >= today),
  };
}

/**
 * Em revisão (com decisões pendentes) e concluídas hoje (spec §10): o fim do
 * dia, embaixo da grade e na visão "Concluídas hoje" do painel lateral.
 */
export function Footer({ now }: { now: number }) {
  const { envs, dashboards } = useTower();
  const nav = useNav();
  const { review, done } = dayItems(envs, dashboards, now);

  return (
    <footer className="control-footer">
      <section className="footer-col" aria-labelledby="em-revisao">
        <h2 id="em-revisao" className="section-title">
          Em revisão · {review.length}
        </h2>
        {review.length === 0 && <p className="muted">Nada em revisão.</p>}
        <ul className="footer-list">
          {review.map(({ env, project, wi }) => (
            <li key={`${env}:${wi.id}`}>
              <button
                type="button"
                className="review-row"
                onClick={() => nav.go({ name: "review", env, workItemId: wi.id })}
              >
                <span className="mono muted">
                  {project.name} {itemRef(wi.gh_number, project)}
                </span>
                <span className="review-title">{wi.title ?? ""}</span>
                <span className={wi.pending_decisions > 0 ? "pending" : "muted"}>
                  {wi.pending_decisions === 0
                    ? "sem decisões pendentes"
                    : `${wi.pending_decisions} ${wi.pending_decisions === 1 ? "decisão pendente" : "decisões pendentes"}`}
                </span>
              </button>
            </li>
          ))}
        </ul>
      </section>
      <section className="footer-col" aria-labelledby="concluidas">
        <h2 id="concluidas" className="section-title">
          Concluídas hoje · {done.length}
        </h2>
        <ul className="done-list">
          {done.map(({ env, project, wi }) => (
            <li key={`${env}:${wi.id}`} className="done-chip">
              <span className="mono">{itemShort(project, wi.gh_number)}</span> {wi.title ?? ""}
            </li>
          ))}
        </ul>
      </section>
    </footer>
  );
}
