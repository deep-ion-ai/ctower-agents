// Cartões de ação da Visão geral do projeto (#19), que entram pela prop
// `extra` de `ProjectOverview`: saúde do projeto, "Buscar issues agora", o
// gestor com o escopo do projeto e as propostas dele (#100). As regras ficam
// em overview.ts.
import { useId, useMemo } from "react";
import type { HubProject } from "../protocol/hub-gen";
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
import { ProposalItem, ProposalNote, useHubProposals, useProposalDecision } from "../hub/HubProposals";
import { pendingInOrder } from "../hub/proposalOrder";

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
 * Propostas do gestor deste projeto (#100): só as pendentes deste
 * `owner/repo`, na ordem do ranking (`pendingInOrder`), cada uma com o item,
 * o valor atual → o proposto e a justificativa resumida. Aprovar e Rejeitar
 * na própria linha (`hub.proposals.decide`): a decidida sai do cartão na
 * releitura, e a decidida em outro lugar sai pelo push
 * `hub.proposals.changed`. Na web, os botões ficam desabilitados com o aviso
 * padrão das ações só do desktop. A lista completa, com nota e decisão em
 * lote, segue no Hub.
 */
export function ProjectProposalsCard({ owner, repo }: { owner: string; repo: string }) {
  const { hub, web } = useTower();
  const nav = useNav();
  const data = useHubProposals({ owner, repo });
  const { busy, note, decide } = useProposalDecision(data);
  const noteId = useId();
  const list = useMemo(
    () => pendingInOrder(data.pending, data.ranking?.items ?? [], hub.projects),
    [data.pending, data.ranking, hub.projects],
  );
  return (
    <section className="hub-card" aria-labelledby="pj-ov-propostas">
      <div className="hub-card-head">
        <h2 id="pj-ov-propostas">Propostas do gestor · {list.length}</h2>
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
              onDecide={decide}
              readOnly={web ? { noteId } : undefined}
            />
          ))}
        </ul>
      )}
      {web && list.length > 0 && (
        <p id={noteId} className="muted hub-card-msg desktop-only-note">
          {DESKTOP_ONLY_NOTE}
        </p>
      )}
      <ProposalNote note={note} />
    </section>
  );
}
