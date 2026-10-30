import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { HubProposal, HubProposals, HubRanking, ProposalDecided } from "../protocol/hub-gen";
import { useNav } from "../state/nav";
import { useTower } from "../state/tower";
import { STATUS } from "../project/status";
import { ago } from "../util/time";
import { itemFullRef, itemShort, type ProviderProject } from "../util/item";
import { hubProjectOf } from "../util/hub";
import { errText } from "./calls";
import { placeProposals, type PlacedProposal } from "./proposalOrder";

/** Quantas decididas ou obsoletas a seção mostra abaixo das pendentes. */
export const PROPOSALS_HISTORY = 50;

/** Teto do `hub.ranking.get` (o do hub) para posicionar as propostas. */
const RANKING_LIMIT = 200;

/** Justificativa maior que isso aparece resumida, com o resto sob demanda. */
export const WHY_SHORT = 120;

const STATE_TEXT = {
  pendente: "pendente",
  aprovada: "aprovada",
  rejeitada: "rejeitada",
  obsoleta: "obsoleta",
} as const;

type Value = Record<string, unknown>;

/** "P1", "não triado", "Pausado"; com posição, "P1 · antes de #12" (ou da chave do Jira). */
export function valueText(kind: HubProposal["kind"], v: unknown, project?: ProviderProject | null): string {
  const o = (v ?? {}) as Value;
  if (kind === "status") {
    const s = o.status as keyof typeof STATUS | undefined;
    return s && STATUS[s] ? STATUS[s].text : String(o.status ?? "?");
  }
  const p = typeof o.priority === "string" ? o.priority : "não triado";
  if (typeof o.before === "number") return `${p} · antes de ${itemShort(project, o.before)}`;
  if (typeof o.after === "number") return `${p} · depois de ${itemShort(project, o.after)}`;
  return p;
}

/** "antes de #12" ou "depois de #12", se a proposta posiciona o item. */
export function anchorText(v: unknown, project?: ProviderProject | null): string | null {
  const o = (v ?? {}) as Value;
  if (typeof o.before === "number") return `antes de ${itemShort(project, o.before)}`;
  if (typeof o.after === "number") return `depois de ${itemShort(project, o.after)}`;
  return null;
}

/** Selo P0–P3 (o mesmo do Board) ou "não triado". */
function Prio({ v, strong }: { v: unknown; strong?: boolean }) {
  const o = (v ?? {}) as Value;
  if (typeof o.priority !== "string") return <span className={strong ? "strong" : "muted"}>não triado</span>;
  return (
    <span className="prio" data-p={o.priority}>
      {o.priority}
    </span>
  );
}

/** "7 → 2 ↑5", "2 → 7 ↓5", "— → 9 novo" ou "3 → 3 =". */
function Position({ x }: { x: PlacedProposal }) {
  const label = {
    up: x.from !== null ? `sobe ${x.from - x.to}` : "",
    down: x.from !== null ? `desce ${x.to - x.from}` : "",
    same: "mesma posição",
    new: "entra no ranking",
  }[x.move];
  const mark = {
    up: `↑${x.from !== null ? x.from - x.to : ""}`,
    down: `↓${x.from !== null ? x.to - x.from : ""}`,
    same: "=",
    new: "novo",
  }[x.move];
  return (
    <span className="proposal-pos">
      {x.from ?? "—"} → <strong>{x.to}</strong>{" "}
      <span className="proposal-move" data-move={x.move} title={label} aria-label={label}>
        {mark}
      </span>
    </span>
  );
}

/** Justificativa curta inteira; a longa resumida, expansível. */
function Why({ text }: { text: string }) {
  if (text.length <= WHY_SHORT) return <>{text}</>;
  return (
    <details>
      <summary>{text.slice(0, WHY_SHORT).trimEnd()}…</summary>
      {text}
    </details>
  );
}

export interface DecideResult {
  text: string;
  error: boolean;
  /** O hub recusou o pedido (nada decidido): a nota e as marcadas ficam. */
  failed?: boolean;
}

/** Propostas do hub: pendentes, últimas decididas e o ranking que as posiciona. */
export interface ProposalsData {
  pending: HubProposal[];
  past: HubProposal[];
  /** Ranking atual; `undefined` enquanto carrega, nulo se o hub falhou. */
  ranking: HubRanking | null | undefined;
  /** Falha ao ler as propostas. */
  error: string | null;
  load(): Promise<void>;
  /** Aprova ou rejeita (uma ou várias) e relê; devolve a mensagem para a tela. */
  decide(ids: number[], approve: boolean, note?: string): Promise<DecideResult>;
}

/**
 * Lê as propostas com o hub no ar e relê com o push `hub.proposals.changed`.
 * Fica na tela Hub (não na seção) para os contadores do painel lateral, a
 * Visão geral e a seção Propostas usarem os mesmos dados.
 */
export function useHubProposals(): ProposalsData {
  const { api, hub } = useTower();
  const [pending, setPending] = useState<HubProposal[]>([]);
  const [past, setPast] = useState<HubProposal[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [ranking, setRanking] = useState<HubRanking | null | undefined>(undefined);
  const seq = useRef(0);

  const load = useCallback(async () => {
    const id = ++seq.current;
    try {
      const [p, all] = await Promise.all([
        api.hubCall<HubProposals>("hub.proposals.list", { state: "pendente", limit: 200 }),
        api.hubCall<HubProposals>("hub.proposals.list", { state: "todos", limit: 200 }),
      ]);
      if (id !== seq.current) return;
      setPending(p.proposals);
      setPast(all.proposals.filter((x) => x.state !== "pendente").slice(0, PROPOSALS_HISTORY));
      setError(null);
      if (!p.proposals.some((x) => x.kind === "priority")) return;
      // O ranking só posiciona; se falhar, a tabela sai sem as posições.
      try {
        const r = await api.hubCall<HubRanking>("hub.ranking.get", { limit: RANKING_LIMIT });
        if (id === seq.current) setRanking(r);
      } catch {
        if (id === seq.current) setRanking(null);
      }
    } catch (e) {
      if (id === seq.current) setError(errText(e));
    }
  }, [api]);

  useEffect(() => {
    if (!hub.connected) return;
    void load();
    return api.onHubPush((e) => {
      if (e.event === "hub.proposals.changed") void load();
    });
  }, [api, load, hub.connected]);

  const decide = useCallback(
    async (ids: number[], approve: boolean, note?: string): Promise<DecideResult> => {
      const extra = note?.trim() ? { note: note.trim() } : {};
      let out: DecideResult;
      try {
        if (ids.length === 1) {
          const r = await api.hubCall<HubProposal>("hub.proposals.decide", { id: ids[0], approve, ...extra });
          out =
            r.state === "obsoleta"
              ? { text: `A proposta #${r.id} ficou obsoleta: ${r.decision_note ?? "o valor atual mudou"}.`, error: true }
              : { text: `Proposta #${r.id} ${STATE_TEXT[r.state]}.`, error: false };
        } else {
          const res = await api.hubCall<ProposalDecided[]>("hub.proposals.decide", { ids, approve, ...extra });
          const ok = res.filter((o) => o.proposal && o.proposal.state !== "obsoleta").length;
          const failed = res.length - ok;
          out = {
            text: `${ok} proposta(s) ${approve ? "aprovada(s)" : "rejeitada(s)"}${
              failed > 0 ? `; ${failed} não (obsoleta ou já decidida)` : ""
            }.`,
            error: failed > 0,
          };
        }
      } catch (e) {
        return { text: errText(e), error: true, failed: true };
      }
      await load();
      return out;
    },
    [api, load],
  );

  return { pending, past, ranking, error, load, decide };
}

/**
 * Propostas do gestor (spec do gestor §8 e §11), em tabelas. As de
 * prioridade vêm na ordem que resulta de aprovar todas (classe proposta,
 * depois a posição, §4.3), com a posição atual no ranking do portfólio
 * (`hub.ranking.get`) → a resultante e se sobe ou desce. As de status vêm
 * numa tabela à parte. Aprova ou rejeita uma a uma ou as marcadas, com nota
 * opcional. Abaixo, as últimas decididas ou obsoletas, com o motivo. Quem
 * aplica a aprovação é o hub. Os dados vêm de `useHubProposals`.
 */
export function ProposalsPanel({ now, data }: { now: number; data: ProposalsData }) {
  const { hub } = useTower();
  const nav = useNav();
  const { pending, past, ranking } = data;
  const [picked, setPicked] = useState<Set<number>>(new Set());
  const [note, setNote] = useState("");
  const [busy, setBusy] = useState(false);
  const [msg, setMsg] = useState<DecideResult | null>(null);
  const shown = msg ?? (data.error ? { text: data.error, error: true } : null);

  // Marcadas que deixaram de estar pendentes saem da seleção.
  useEffect(() => {
    setPicked((s) => {
      const n = new Set([...s].filter((id) => pending.some((x) => x.id === id)));
      return n.size === s.size ? s : n;
    });
  }, [pending]);

  const decide = async (ids: number[], approve: boolean) => {
    setBusy(true);
    setMsg(null);
    const r = await data.decide(ids, approve, note);
    setMsg(r);
    if (!r.failed) {
      setNote("");
      setPicked(new Set());
    }
    setBusy(false);
  };

  const toggle = (id: number) =>
    setPicked((s) => {
      const n = new Set(s);
      if (!n.delete(id)) n.add(id);
      return n;
    });

  const placed = useMemo(
    () => placeProposals(pending, ranking?.items ?? [], hub.projects),
    [pending, ranking, hub.projects],
  );
  const statusOnes = pending.filter((p) => p.kind === "status");
  const rankingNote =
    ranking === undefined
      ? null
      : ranking === null
        ? "Ranking atual indisponível: a ordem considera só as propostas, sem as posições."
        : ranking.truncated
          ? `Posições entre os ${RANKING_LIMIT} primeiros do ranking; abaixo disso, aproximadas.`
          : null;

  // Provedor do projeto da proposta: chave do Jira no lugar de #n (#352).
  const projOf = (p: HubProposal) => hubProjectOf(hub.projects, p.owner, p.repo);
  const project = (p: HubProposal) => (
    <span className="mono">
      {p.owner}/{p.repo}
    </span>
  );
  const issue = (p: HubProposal, title: string | null) => (
    <>
      <button
        type="button"
        className="linklike mono"
        title={p.ref ?? itemFullRef(projOf(p) ?? { gh_owner: p.owner, gh_repo: p.repo }, p.number)}
        onClick={() => nav.go({ name: "project", owner: p.owner, repo: p.repo })}
      >
        {itemShort(projOf(p), p.number)}
      </button>
      {title && <span className="proposal-title"> {title}</span>}
    </>
  );
  const pick = (p: HubProposal) => (
    <td>
      <input
        type="checkbox"
        aria-label={`Marcar a proposta #${p.id}`}
        checked={picked.has(p.id)}
        onChange={() => toggle(p.id)}
      />
    </td>
  );
  const origin = (p: HubProposal) => (
    <td>
      {p.env_name}
      {p.session_id && <span className="muted small"> · sessão {p.session_id.slice(0, 8)}</span>}
    </td>
  );
  const actions = (p: HubProposal) => (
    <td>
      <span className="env-actions">
        <button
          type="button"
          className="btn"
          disabled={busy}
          aria-label={`Aprovar a proposta #${p.id}`}
          onClick={() => decide([p.id], true)}
        >
          Aprovar
        </button>
        <button
          type="button"
          className="btn btn-danger"
          disabled={busy}
          aria-label={`Rejeitar a proposta #${p.id}`}
          onClick={() => decide([p.id], false)}
        >
          Rejeitar
        </button>
      </span>
    </td>
  );

  return (
    <section aria-labelledby="hub-propostas" className="panel settings-section">
      <h2 id="hub-propostas">
        Propostas do gestor{" "}
        {pending.length > 0 && (
          <span className="badge" data-tone="attention" aria-label={`${pending.length} pendente(s)`}>
            {pending.length}
          </span>
        )}
      </h2>
      {pending.length === 0 ? (
        <p className="muted">Nenhuma proposta pendente.</p>
      ) : (
        <>
          {placed.length > 0 && (
            <>
              <h3 className="section-title">Prioridade · ordem resultante</h3>
              <div className="table-wrap proposals-table">
                <table aria-label="Propostas de prioridade">
                  <thead>
                    <tr>
                      <th>
                        <span className="sr-only">Marcar</span>
                      </th>
                      <th title="Ordem na tabela, de cima para baixo, depois de aprovar todas">Ordem</th>
                      <th>Projeto</th>
                      <th>Issue</th>
                      <th>Prioridade</th>
                      <th title="Posição no ranking do portfólio (hub.ranking.get), atual → depois de aprovar todas">
                        Posição
                      </th>
                      <th>Justificativa</th>
                      <th>Ambiente</th>
                      <th>Idade</th>
                      <th>Ações</th>
                    </tr>
                  </thead>
                  <tbody>
                    {placed.map((x, i) => {
                      const p = x.proposal;
                      return (
                        <tr key={p.id} aria-label={`Proposta #${p.id}`}>
                          {pick(p)}
                          <td className="strong">{i + 1}</td>
                          <td>{project(p)}</td>
                          <td>{issue(p, x.title)}</td>
                          <td className="nowrap">
                            <Prio v={p.current} /> → <Prio v={p.proposed} strong />
                            {anchorText(p.proposed, projOf(p)) && (
                              <div className="muted small">{anchorText(p.proposed, projOf(p))}</div>
                            )}
                          </td>
                          <td className="nowrap">{ranking ? <Position x={x} /> : <span className="muted">—</span>}</td>
                          <td className="proposal-why">
                            <Why text={p.justification} />
                          </td>
                          {origin(p)}
                          <td className="nowrap">{ago(p.created_at, now)}</td>
                          {actions(p)}
                        </tr>
                      );
                    })}
                  </tbody>
                </table>
              </div>
              {rankingNote && <p className="muted hint">{rankingNote}</p>}
            </>
          )}
          {statusOnes.length > 0 && (
            <>
              <h3 className="section-title">Status</h3>
              <div className="table-wrap proposals-table">
                <table aria-label="Propostas de status">
                  <thead>
                    <tr>
                      <th>
                        <span className="sr-only">Marcar</span>
                      </th>
                      <th>Projeto</th>
                      <th>Issue</th>
                      <th>Status</th>
                      <th>Justificativa</th>
                      <th>Ambiente</th>
                      <th>Idade</th>
                      <th>Ações</th>
                    </tr>
                  </thead>
                  <tbody>
                    {statusOnes.map((p) => (
                      <tr key={p.id} aria-label={`Proposta #${p.id}`}>
                        {pick(p)}
                        <td>{project(p)}</td>
                        <td>{issue(p, p.title)}</td>
                        <td className="nowrap">
                          {valueText(p.kind, p.current, projOf(p))} → <strong>{valueText(p.kind, p.proposed, projOf(p))}</strong>
                        </td>
                        <td className="proposal-why">
                          <Why text={p.justification} />
                        </td>
                        {origin(p)}
                        <td className="nowrap">{ago(p.created_at, now)}</td>
                        {actions(p)}
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            </>
          )}
          <div className="field">
            <label htmlFor="proposal-note">Nota (opcional, vai junto com a decisão)</label>
            <input id="proposal-note" type="text" value={note} onChange={(e) => setNote(e.target.value)} />
          </div>
          <div className="form-actions">
            <button
              type="button"
              className="btn"
              onClick={() => setPicked(picked.size === pending.length ? new Set() : new Set(pending.map((p) => p.id)))}
            >
              {picked.size === pending.length ? "Desmarcar todas" : "Marcar todas"}
            </button>
            <button
              type="button"
              className="btn btn-primary"
              disabled={busy || picked.size === 0}
              onClick={() => decide([...picked], true)}
            >
              Aprovar marcadas ({picked.size})
            </button>
            <button
              type="button"
              className="btn btn-danger"
              disabled={busy || picked.size === 0}
              onClick={() => decide([...picked], false)}
            >
              Rejeitar marcadas ({picked.size})
            </button>
          </div>
        </>
      )}
      {shown && (
        <p role={shown.error ? "alert" : "status"} className={shown.error ? "start-error" : "state-on"}>
          {shown.text}
        </p>
      )}
      {past.length > 0 && (
        <details className="proposals-past">
          <summary>Decididas e obsoletas · {past.length}</summary>
          <div className="table-wrap proposals-table compact">
            <table aria-label="Propostas decididas e obsoletas">
              <thead>
                <tr>
                  <th>Estado</th>
                  <th>Alvo</th>
                  <th>Mudança</th>
                  <th>Nota</th>
                  <th>Quando</th>
                </tr>
              </thead>
              <tbody>
                {past.map((p) => (
                  <tr key={p.id} data-state={p.state}>
                    <td>
                      <span className="hub-state" data-state={p.state}>
                        {STATE_TEXT[p.state]}
                      </span>
                    </td>
                    <td>
                      <button
                        type="button"
                        className="linklike mono"
                        onClick={() =>
                          nav.go({
                            name: "project",
                            owner: p.owner,
                            repo: p.repo,
                          })
                        }
                      >
                        {p.ref ?? itemFullRef(projOf(p) ?? { gh_owner: p.owner, gh_repo: p.repo }, p.number)}
                      </button>
                    </td>
                    <td>
                      {p.kind === "priority" ? "prioridade" : "status"}: {valueText(p.kind, p.current, projOf(p))} →{" "}
                      {valueText(p.kind, p.proposed, projOf(p))}
                    </td>
                    <td className="muted">{p.decision_note ?? ""}</td>
                    <td className="muted nowrap">{p.decided_at !== null ? ago(p.decided_at, now) : ""}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </details>
      )}
      <p className="muted hint">
        O gestor só propõe. Aprovar aplica a mudança no hub na hora; se o valor atual mudou desde a proposta, ela vira
        obsoleta e nada é aplicado.
      </p>
    </section>
  );
}
