// Cartões de ação da Visão geral do projeto (#19), que entram pela prop
// `extra` de `ProjectOverview`: saúde do projeto, "Buscar issues agora", o
// gestor com o escopo do projeto, as propostas dele (#100) e as revisões
// (#101). As regras ficam em overview.ts.
import { useEffect, useId, useMemo, useState } from "react";
import type { AutonomyWip, HubProject } from "../protocol/hub-gen";
import { aheadText, ciText } from "../hub/autopilot";
import { useNav } from "../state/nav";
import { useTower } from "../state/tower";
import { GestorLauncher } from "../hub/HubGestor";
import { kindText, SEVERITY_TEXT, SEVERITY_TONE, useHubHealth } from "../hub/HubHealth";
import { hubTab } from "../hub/sections";
import { plural } from "../shell/summary";
import { itemShort } from "../util/item";
import { projectSignals } from "./overview";
import { runProjectSync, useSyncRun, type ProjectSyncInfo } from "./sync-now";
import { SYNC_DESKTOP_NOTE, SyncStatus } from "./SyncNow";
import { DesktopOnly } from "../web/DesktopOnly";
import { DESKTOP_ONLY_NOTE } from "../web/mode";
import { ProposalItem, ProposalNote, useHubProposals, useProposalBatch } from "../hub/HubProposals";
import { CARD_WIDTH_TEXT, CARD_WIDTHS, useCardWidth, type CardWidth } from "./ui-store";
import { pendingInOrder } from "../hub/proposalOrder";
import type { Review } from "../protocol/gen";
import { changeLabel, changeRef, forgeLabel, PROJECT_ITEM } from "../util/item";
import type { Clone } from "./clones";
import { entryName, markPr, markReviewed, openPr, projectEntries, useReviewEntries, type ReviewEntry } from "../review/entries";
import { noteReviewed, reviewKey, useReviews, wasReviewed } from "../review/ui-store";

/**
 * Saúde do projeto: os sinais de aviso e críticos do hub com este
 * `owner/repo` (um item bloqueado há mais que o limiar, 2 dias por padrão,
 * aparece aqui), cada um com o atalho para o item. Só na visão do hub:
 * os sinais são calculados lá. "Ver no Hub" leva ao painel do portfólio,
 * com os informativos.
 */
export function ProjectHealthCard({ owner, repo, project }: { owner: string; repo: string; project: HubProject | null }) {
  const nav = useNav();
  const { health, error } = useHubHealth();
  const signals = projectSignals(health, owner, repo);
  return (
    <section className="hub-card" aria-labelledby="pj-ov-health">
      <div className="hub-card-head">
        <h2 id="pj-ov-health">Saúde · {plural(signals.length, "sinal", "sinais")}</h2>
        <button type="button" className="btn btn-sm" onClick={() => nav.openTab(hubTab("health"))}>
          Ver no Hub
        </button>
      </div>
      {error && (
        <p role="alert" className="hub-card-msg start-error">
          {error}
        </p>
      )}
      {signals.length === 0 ? (
        <p className="muted hub-card-empty">
          {health ? "Nenhum sinal de aviso ou crítico: nada parado, bloqueado ou falhando neste projeto." : "Lendo os sinais…"}
        </p>
      ) : (
        <ul className="hub-card-list" aria-label="Sinais de saúde do projeto">
          {signals.map((s, i) => (
            <li key={`${s.kind}:${s.number}:${s.env}:${i}`} data-severity={s.severity}>
              <span className="badge" data-tone={SEVERITY_TONE[s.severity]} title={SEVERITY_TEXT[s.severity]}>
                {kindText(s, project)}
              </span>
              <span className="hub-card-text">
                {s.number !== null ? (
                  <button
                    type="button"
                    className="linklike mono"
                    onClick={() => nav.go({ name: "project", owner, repo, item: { ghNumber: s.number! } })}
                  >
                    {itemShort(project, s.number)}
                  </button>
                ) : null}
                {s.title && <> {s.title}</>}
                <span className="hub-card-sub">
                  {s.value}
                  {s.threshold && ` · limiar ${s.threshold}`}
                  {s.detail && ` · ${s.detail}`}
                </span>
              </span>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}

/**
 * "Buscar issues agora" só deste projeto: `sync.now` com o `project_id` em
 * cada ambiente do plano (`syncPlan`), com o resultado ou o erro de cada
 * um, e o estado da última rodada ("Último sync" ou "Sync falhando desde",
 * #90). O andamento é o mesmo do cabeçalho, da lista de Projetos e da
 * paleta (`runProjectSync`). Na web, o botão fica desabilitado com o motivo
 * (#26): o canal reverso recusa `sync.now` (spec do épico #205 §5.2).
 *
 * Na visão do hub de um projeto GitHub, o que muda na hora é o espelho do
 * ambiente (issues, PRs e os gatilhos de status, que chegam ao hub no envio
 * seguinte, em segundos); o espelho do próprio hub segue o ciclo periódico
 * dele. Num projeto Jira, o ambiente de sync é a fonte do espelho do hub.
 */
export function ProjectSyncCard({ info }: { info: ProjectSyncInfo }) {
  const { api, web } = useTower();
  const run = useSyncRun(info.key);
  const { plan } = info;
  const failing = (info.state?.failures ?? 0) > 0;
  if (web) {
    return (
      <section className="hub-card" aria-labelledby="pj-ov-sync">
        <div className="hub-card-head">
          <h2 id="pj-ov-sync">Buscar issues</h2>
          <DesktopOnly
            className="btn btn-sm btn-primary"
            labels={
              <>
                Buscar issues agora<span className="sr-only"> este projeto</span>
              </>
            }
            note={SYNC_DESKTOP_NOTE}
          />
        </div>
        <SyncStatus info={info} className="hub-card-empty" />
      </section>
    );
  }

  return (
    <section className="hub-card" aria-labelledby="pj-ov-sync">
      <div className="hub-card-head">
        <h2 id="pj-ov-sync">Buscar issues</h2>
        <button
          type="button"
          className="btn btn-sm btn-primary"
          disabled={run.busy || plan.targets.length === 0}
          title="Buscar agora no rastreador as issues só deste projeto"
          onClick={() => void runProjectSync(api, info)}
        >
          {/* O nome acessível difere do "Buscar issues agora" da barra de título (todos os ambientes). */}
          {run.busy ? "Buscando issues…" : failing ? "Tentar de novo" : "Buscar issues agora"}
          <span className="sr-only"> este projeto</span>
        </button>
      </div>
      <SyncStatus info={info} className="hub-card-empty" />
      <p className="muted hub-card-empty">
        {plan.reason ??
          `Só este projeto, pelo ${plan.targets.length === 1 ? "ambiente" : "ambientes"} ${plan.targets
            .map((t) => t.name)
            .join(", ")}.`}
      </p>
      {run.msg && (
        <p role={run.msg.error ? "alert" : "status"} className={`hub-card-msg ${run.msg.error ? "start-error" : "state-on"}`}>
          {run.msg.text}
        </p>
      )}
    </section>
  );
}

/**
 * Gestor deste projeto: o `GestorLauncher` do Hub com o escopo "Um
 * projeto" já neste `owner/repo`. Na web só aparecem as sessões vivas
 * (iniciar o gestor é do desktop, como na Visão geral do Hub).
 */
export function ProjectGestorCard({ owner, repo }: { owner: string; repo: string }) {
  const { web } = useTower();
  return (
    <section className="hub-card" aria-labelledby="pj-ov-gestor">
      <div className="hub-card-head">
        <h2 id="pj-ov-gestor">Gestor deste projeto</h2>
      </div>
      <p className="muted hub-card-empty">
        {web
          ? "As sessões vivas do gestor com o foco neste projeto aparecem aqui."
          : `Uma sessão do Claude Code com o foco em ${owner}/${repo}, que propõe prioridades e status; você aprova no Hub.`}
      </p>
      <div className="hub-card-msg">
        <GestorLauncher project={{ owner, repo }} />
      </div>
    </section>
  );
}

/**
 * Modo autônomo (#185): a branch de integração (`wip/autonoma`) com o
 * número de issues à frente da base (as mescladas nela que ainda estão em
 * revisão), o link do PR rascunho para a base e o aviso de conflito. Lê
 * `hub.autopilot.wip` e relê com o push `hub.autopilot.changed`; num hub
 * anterior à 1.20 fica só com o aviso de conflito (`warning`).
 */
export function ProjectAutonomyCard({
  owner,
  repo,
  warning,
  onSettings,
}: {
  owner: string;
  repo: string;
  warning: string | null;
  onSettings(): void;
}) {
  const { api, hub } = useTower();
  const [wip, setWip] = useState<AutonomyWip | null>(null);

  useEffect(() => {
    if (!hub.connected) return;
    let live = true;
    const load = () =>
      api
        .hubCall<AutonomyWip[]>("hub.autopilot.wip", { owner, repo })
        .then((w) => live && setWip(w[0] ?? null))
        .catch(() => live && setWip(null));
    void load();
    const off = api.onHubPush((e) => {
      if (e.event === "hub.autopilot.changed" || e.event === "hub_sync_status") void load();
    });
    return () => {
      live = false;
      off();
    };
  }, [api, hub.connected, owner, repo]);

  if (!wip && !warning) return null;
  return (
    <section className="hub-card" aria-labelledby="pj-ov-auto">
      <div className="hub-card-head">
        <h2 id="pj-ov-auto">Modo autônomo</h2>
        <button type="button" className="btn btn-sm" onClick={onSettings}>
          Ver modo autônomo
        </button>
      </div>
      {wip && (
        <p className="hub-card-msg" aria-label="Branch de integração">
          <span className="mono">{wip.branch}</span>: {aheadText(wip)}
          {wip.ahead.length > 0 && (
            <span className="hub-card-sub muted">{wip.ahead.map((i) => `#${i.number}`).join(", ")} em revisão</span>
          )}
          {wip.pr_url && (
            <span className="hub-card-sub">
              <a href={wip.pr_url} target="_blank" rel="noreferrer">
                PR {wip.branch} → {wip.base}
              </a>{" "}
              <span className="muted">({ciText(wip.ci)})</span>
            </span>
          )}
        </p>
      )}
      {warning && (
        <p role="alert" className="start-error">
          {warning}: nenhuma issue autônoma começa até alguém resolver.
        </p>
      )}
    </section>
  );
}

/**
 * Controle de largura de um cartão da Visão geral (#139): normal (uma
 * coluna), larga (duas) ou a linha inteira, lembrado por cartão nesta
 * máquina (`useCardWidth`). Some quando o grid tem uma coluna só (CSS).
 */
export function CardWidthControl({
  label,
  value,
  onChange,
}: {
  label: string;
  value: CardWidth;
  onChange(w: CardWidth): void;
}) {
  return (
    <span className="card-width" role="group" aria-label={`Largura do cartão ${label}`}>
      {CARD_WIDTHS.map((w) => (
        <button
          key={w}
          type="button"
          className="card-width-opt"
          data-w={w}
          aria-pressed={value === w}
          aria-label={CARD_WIDTH_TEXT[w]}
          title={CARD_WIDTH_TEXT[w]}
          onClick={() => onChange(w)}
        >
          <span aria-hidden="true" />
        </button>
      ))}
    </span>
  );
}

/**
 * Propostas do gestor deste projeto (#100): só as pendentes deste
 * `owner/repo`, na ordem do ranking (`pendingInOrder`), cada uma com o item,
 * o valor atual → o proposto e a justificativa resumida. Aprovar e Rejeitar
 * na própria linha (`hub.proposals.decide`): a decidida sai do cartão na
 * releitura, e a decidida em outro lugar sai pelo push
 * `hub.proposals.changed`.
 *
 * Decisão em lote (#139), com a lógica da seção do Hub (`useProposalBatch`):
 * caixa por proposta, "Marcar todas" no cabeçalho, nota opcional e
 * "Aprovar/Rejeitar marcadas (N)", com confirmação quando o lote tem mais de
 * uma. O resultado lista as aplicadas e as recusadas com o motivo. Largura
 * ajustável, "larga" por padrão. Na web, os botões e as caixas ficam
 * desabilitados com o aviso padrão das ações só do desktop.
 */
export function ProjectProposalsCard({ owner, repo }: { owner: string; repo: string }) {
  const { hub, web } = useTower();
  const nav = useNav();
  const data = useHubProposals({ owner, repo });
  const batch = useProposalBatch(data);
  const { busy, pickedIds } = batch;
  const [confirm, setConfirm] = useState<{ approve: boolean; ids: number[] } | null>(null);
  const [width, setWidth] = useCardWidth("proposals", "wide");
  const noteId = useId();
  const noteFieldId = useId();
  const list = useMemo(
    () => pendingInOrder(data.pending, data.ranking?.items ?? [], hub.projects),
    [data.pending, data.ranking, hub.projects],
  );
  const readOnly = web ? { noteId } : undefined;
  // A lista mudou (push, releitura): o lote a confirmar já não vale.
  useEffect(() => setConfirm(null), [data.pending]);

  const ask = (approve: boolean) => {
    if (pickedIds.length > 1) setConfirm({ approve, ids: pickedIds });
    else void batch.decide(pickedIds, approve);
  };
  const run = async () => {
    if (!confirm) return;
    setConfirm(null);
    await batch.decide(confirm.ids, confirm.approve);
  };
  const off = busy || !!readOnly;

  return (
    <section className="hub-card pj-proposals" data-width={width} aria-labelledby="pj-ov-propostas">
      <div className="hub-card-head">
        <h2 id="pj-ov-propostas">Propostas do gestor · {list.length}</h2>
        <CardWidthControl label="Propostas do gestor" value={width} onChange={setWidth} />
        {list.length > 0 && (
          <button
            type="button"
            className="btn btn-sm"
            disabled={off}
            aria-describedby={readOnly?.noteId}
            onClick={batch.toggleAll}
          >
            {batch.allPicked ? "Desmarcar todas" : "Marcar todas"}
          </button>
        )}
        <button type="button" className="btn btn-sm" onClick={() => nav.openTab(hubTab("proposals"))}>
          Ver no Hub
        </button>
      </div>
      {list.length === 0 ? (
        // Sem leitura e com erro, só o erro (abaixo).
        (data.loaded || !data.error) && (
          <p className="muted hub-card-empty">
            {data.loaded ? "Nenhuma proposta pendente neste projeto." : "Lendo as propostas…"}
          </p>
        )
      ) : (
        <ul className="hub-card-list" aria-label="Propostas pendentes do projeto">
          {list.map((x) => (
            <ProposalItem
              key={x.proposal.id}
              p={x.proposal}
              title={x.title}
              withCurrent
              rejectLabel="Rejeitar"
              busy={busy}
              onDecide={(id, approve) => void batch.decide([id], approve)}
              readOnly={readOnly}
              pick={{ checked: batch.picked.has(x.proposal.id), onToggle: () => batch.toggle(x.proposal.id) }}
            />
          ))}
        </ul>
      )}
      {list.length > 0 && (
        <div className="pj-proposals-batch">
          {!web && (
            <div className="field">
              <label htmlFor={noteFieldId}>Nota (opcional, vai junto com a decisão)</label>
              <input
                id={noteFieldId}
                type="text"
                value={batch.note}
                onChange={(e) => batch.setNote(e.target.value)}
              />
            </div>
          )}
          <div className="form-actions">
            <button
              type="button"
              className="btn btn-sm btn-primary"
              disabled={off || pickedIds.length === 0}
              aria-describedby={readOnly?.noteId}
              onClick={() => ask(true)}
            >
              Aprovar marcadas ({pickedIds.length})
            </button>
            <button
              type="button"
              className="btn btn-sm btn-danger"
              disabled={off || pickedIds.length === 0}
              aria-describedby={readOnly?.noteId}
              onClick={() => ask(false)}
            >
              Rejeitar marcadas ({pickedIds.length})
            </button>
          </div>
          {confirm && (
            <div
              role="alertdialog"
              aria-label={confirm.approve ? "Aprovar as marcadas" : "Rejeitar as marcadas"}
              className="pj-proposals-confirm"
            >
              <p>
                {confirm.approve ? "Aprovar" : "Rejeitar"} {plural(confirm.ids.length, "proposta", "propostas")}
                {batch.note.trim() && <> com a nota “{batch.note.trim()}”</>}?
                {confirm.approve && " Se o valor atual mudou desde a proposta, ela vira obsoleta e as outras seguem."}
              </p>
              <button
                type="button"
                className={`btn btn-sm ${confirm.approve ? "btn-primary" : "btn-danger"}`}
                onClick={() => void run()}
              >
                {confirm.approve ? "Aprovar" : "Rejeitar"} {confirm.ids.length}
              </button>
              <button type="button" className="btn btn-sm" onClick={() => setConfirm(null)}>
                Cancelar
              </button>
            </div>
          )}
        </div>
      )}
      {web && list.length > 0 && (
        <p id={noteId} className="muted hub-card-msg desktop-only-note">
          {DESKTOP_ONLY_NOTE}
        </p>
      )}
      <ProposalNote note={batch.shown} />
    </section>
  );
}

/**
 * Revisões deste projeto (#101): os itens em revisão (`em_revisao`) ou com
 * decisões autônomas pendentes, do dashboard de cada ambiente conectado com
 * o projeto (`clones`), cada um com o título, as pendentes (do `review.get`,
 * o mesmo da tela de Revisão) e o link para a tela completa. "Marcar como
 * revisada" na própria linha grava as pendentes no PR do item
 * (`review.mark_reviewed`, como o botão da tela de Revisão), sem sair da
 * Visão geral; o item marcado sai do cartão e só volta com decisão nova. Não
 * há reprovar: o usuário só confirma que revisou. Na web, os botões ficam
 * desabilitados com o aviso padrão das ações só do desktop.
 */
export function ProjectReviewsCard({ clones }: { clones: readonly Clone[] }) {
  const { api, web } = useTower();
  const nav = useNav();
  const [, putReview] = useReviews();
  const [busy, setBusy] = useState(false);
  const [note, setNote] = useState<{ text: string; error: boolean } | null>(null);
  const noteId = useId();
  const candidates = clones.flatMap((c) => projectEntries(c.env, c.project, c.items));
  // Na web, os work items vêm do dashboard do hub (ids do hub): sem `review.get`.
  const read = useReviewEntries(api, candidates, { read: !web });
  // Marcado aqui ou na tela de Revisão: sai até chegar decisão nova.
  const list = read.entries.filter((e) => !(wasReviewed(reviewKey(e.env, e.workItemId)) && e.count === 0));
  const multiEnv = clones.length > 1;
  const envName = (id: string) => clones.find((c) => c.env === id)?.name ?? id;
  const [width, setWidth] = useCardWidth("reviews", "normal");

  const mark = async (e: ReviewEntry) => {
    const name = entryName(e);
    setBusy(true);
    setNote(null);
    try {
      // As pendentes e o PR de agora (o dashboard pode estar atrasado).
      const r = await api.call<Review>(e.env, "review.get", { work_item_id: e.workItemId });
      const pr = markPr(r);
      let marked = 0;
      if (r.decisions.length > 0) {
        if (!pr)
          throw new Error(
            `${name}: sem ${changeLabel(e.project)} no espelho do ${forgeLabel(e.project)} (sincronize depois de abrir o ${changeLabel(e.project)}).`,
          );
        marked = (await markReviewed(api, e.env, e.workItemId, { pr: pr.number })).marked;
      } else {
        // Nada pendente (só em revisão): a confirmação de que revisou, sem gravar nada.
        noteReviewed(reviewKey(e.env, e.workItemId));
      }
      putReview(reviewKey(e.env, e.workItemId), await api.call<Review>(e.env, "review.get", { work_item_id: e.workItemId }));
      setNote({
        text: `${name} revisado${marked > 0 ? `: ${plural(marked, "decisão marcada", "decisões marcadas")} como revisada${marked === 1 ? "" : "s"}` : ""}.`,
        error: false,
      });
    } catch (err) {
      setNote({ text: err instanceof Error ? err.message : String(err), error: true });
    } finally {
      setBusy(false);
    }
  };

  const sub = (e: ReviewEntry) => {
    const pr = openPr(e.review);
    return [
      plural(e.count, "decisão pendente", "decisões pendentes"),
      e.ghNumber === PROJECT_ITEM
        ? "sessões sem issue associada"
        : pr
          ? `${changeRef(e.project, pr.number)} aberto`
          : e.review
            ? `sem ${changeLabel(e.project)} aberto`
            : null,
      multiEnv ? envName(e.env) : null,
    ]
      .filter(Boolean)
      .join(" · ");
  };

  return (
    <section className="hub-card" data-width={width} aria-labelledby="pj-ov-revisoes">
      <div className="hub-card-head">
        <h2 id="pj-ov-revisoes">Revisões · {list.length}</h2>
        <CardWidthControl label="Revisões" value={width} onChange={setWidth} />
        <button type="button" className="btn btn-sm" onClick={() => nav.go({ name: "review" })}>
          Ver na Revisão
        </button>
      </div>
      {read.error && (
        <p role="alert" className="hub-card-msg start-error">
          {read.error}
        </p>
      )}
      {list.length === 0 ? (
        <p className="muted hub-card-empty">
          {clones.length === 0
            ? "Nenhum ambiente conectado tem este projeto: as revisões aparecem quando um deles conectar."
            : "Nada em revisão neste projeto."}
        </p>
      ) : (
        <ul className="hub-card-list" aria-label="Itens em revisão do projeto">
          {list.map((e) => {
            const name = entryName(e);
            return (
              <li key={reviewKey(e.env, e.workItemId)} aria-label={`Revisão de ${name}`}>
                <span className="badge" data-tone={e.count > 0 ? "attention" : undefined}>
                  {e.count}
                </span>
                <span className="hub-card-text">
                  <button
                    type="button"
                    className="linklike"
                    title="Abrir na tela de Revisão"
                    onClick={() => nav.go({ name: "review", env: e.env, workItemId: e.workItemId })}
                  >
                    {name}
                  </button>
                  <span className="hub-card-sub">{sub(e)}</span>
                </span>
                <button
                  type="button"
                  className="btn btn-sm btn-primary"
                  disabled={busy || web}
                  aria-describedby={web ? noteId : undefined}
                  aria-label={`Marcar ${name} como revisada`}
                  onClick={() => void mark(e)}
                >
                  Marcar como revisada
                </button>
              </li>
            );
          })}
        </ul>
      )}
      {web && list.length > 0 && (
        <p id={noteId} className="muted hub-card-msg desktop-only-note">
          {DESKTOP_ONLY_NOTE}
        </p>
      )}
      {note && (
        <p role={note.error ? "alert" : "status"} className={`hub-card-msg ${note.error ? "start-error" : "state-on"}`}>
          {note.text}
        </p>
      )}
    </section>
  );
}
