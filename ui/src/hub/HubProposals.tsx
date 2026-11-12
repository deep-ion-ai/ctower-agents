import type React from "react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { HubProposal, HubProposals, HubRanking, ProposalDecided } from "../protocol/hub-gen";
import { useNav } from "../state/nav";
import { useTower } from "../state/tower";
import { STATUS } from "../project/status";
import { ago } from "../util/time";
import { itemFullRef, itemShort, type ProviderProject } from "../util/item";
import { hubProjectOf } from "../util/hub";
import { errText } from "./calls";
import {
  CLAUDE_EFFORTS,
  CLAUDE_MODELS,
  DEFAULT_EFFORT_VALUE,
  issueAgentLabel,
} from "../settings/session-options";
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

/** Nome do tipo da proposta, no selo e no histórico. */
export const KIND_TEXT: Record<HubProposal["kind"], string> = {
  priority: "prioridade",
  status: "status",
  model: "modelo",
};

/** `{ model, effort }` da proposta `model` (#182), com os não-texto como nulos. */
export function agentOf(v: unknown): { model: string | null; effort: string | null } {
  const o = (v ?? {}) as Value;
  return {
    model: typeof o.model === "string" ? o.model : null,
    effort: typeof o.effort === "string" ? o.effort : null,
  };
}

/** "P1", "não triado", "Pausado", "Opus · esforço high"; com posição, "P1 · antes de #12" (ou da chave do Jira). */
export function valueText(kind: HubProposal["kind"], v: unknown, project?: ProviderProject | null): string {
  const o = (v ?? {}) as Value;
  if (kind === "model") return issueAgentLabel(agentOf(v));
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
export function Why({ text }: { text: string }) {
  if (text.length <= WHY_SHORT) return <>{text}</>;
  return (
    <details>
      <summary>{text.slice(0, WHY_SHORT).trimEnd()}…</summary>
      {text}
    </details>
  );
}

/** Uma proposta do lote que o hub não aplicou, com o motivo (#139). */
export interface RefusedProposal {
  id: number;
  /** Item da proposta (`owner/repo#n` ou a chave do tracker), se conhecido. */
  ref: string | null;
  reason: string;
}

export interface DecideResult {
  text: string;
  error: boolean;
  /** O hub recusou o pedido (nada decidido): a nota e as marcadas ficam. */
  failed?: boolean;
  /** Lote: ids das aplicadas (aprovadas ou rejeitadas). */
  applied?: number[];
  /** Lote: as que não foram aplicadas, cada uma com o motivo. */
  refused?: RefusedProposal[];
}

/** Propostas do hub: pendentes, últimas decididas e o ranking que as posiciona. */
export interface ProposalsData {
  pending: HubProposal[];
  past: HubProposal[];
  /** Ranking atual; `undefined` enquanto carrega, nulo se o hub falhou. */
  ranking: HubRanking | null | undefined;
  /** Falha ao ler as propostas. */
  error: string | null;
  /** Já houve uma leitura com sucesso (antes, "vazio" ainda não quer dizer nada). */
  loaded: boolean;
  load(): Promise<void>;
  /**
   * Aprova ou rejeita (uma ou várias) e relê; devolve a mensagem para a tela.
   * `value`: na proposta `model` (uma só), o valor aprovado no lugar do proposto (#182).
   */
  decide(ids: number[], approve: boolean, note?: string, value?: unknown): Promise<DecideResult>;
}

/** Projeto de `owner/repo`, sem diferença de caixa (como a chave do hub). */
const sameProject = (p: HubProposal, owner: string, repo: string) =>
  p.owner.toLowerCase() === owner.toLowerCase() && p.repo.toLowerCase() === repo.toLowerCase();

/**
 * Lê as propostas com o hub no ar e relê com o push `hub.proposals.changed`.
 * Fica na tela Hub (não na seção) para os contadores do painel lateral, a
 * Visão geral e a seção Propostas usarem os mesmos dados.
 *
 * Com `project`, só as pendentes daquele `owner/repo` (o filtro do
 * `hub.proposals.list`) e sem o histórico: é o cartão da Visão geral do
 * projeto (#100).
 */
export function useHubProposals(project?: { owner: string; repo: string }): ProposalsData {
  const { api, hub } = useTower();
  const [pending, setPending] = useState<HubProposal[]>([]);
  const [past, setPast] = useState<HubProposal[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [loaded, setLoaded] = useState(false);
  const [ranking, setRanking] = useState<HubRanking | null | undefined>(undefined);
  const seq = useRef(0);
  // Para nomear as recusadas do lote pelo item, sem refazer `decide` a cada leitura.
  const known = useRef<HubProposal[]>([]);
  known.current = pending;
  const owner = project?.owner;
  const repo = project?.repo;

  const load = useCallback(async () => {
    const id = ++seq.current;
    try {
      const scope = owner && repo ? { owner, repo } : null;
      const [p, all] = await Promise.all([
        api.hubCall<HubProposals>("hub.proposals.list", { state: "pendente", limit: 200, ...scope }),
        scope ? null : api.hubCall<HubProposals>("hub.proposals.list", { state: "todos", limit: 200 }),
      ]);
      if (id !== seq.current) return;
      // O filtro é do hub; aqui só garante (um hub antigo que o ignore não mistura projetos).
      setPending(scope ? p.proposals.filter((x) => sameProject(x, scope.owner, scope.repo)) : p.proposals);
      setPast(all ? all.proposals.filter((x) => x.state !== "pendente").slice(0, PROPOSALS_HISTORY) : []);
      setError(null);
      setLoaded(true);
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
  }, [api, owner, repo]);

  useEffect(() => {
    if (!hub.connected) return;
    void load();
    return api.onHubPush((e) => {
      if (e.event === "hub.proposals.changed") void load();
    });
  }, [api, load, hub.connected]);

  const decide = useCallback(
    async (ids: number[], approve: boolean, note?: string, value?: unknown): Promise<DecideResult> => {
      const extra = note?.trim() ? { note: note.trim() } : {};
      let out: DecideResult;
      try {
        if (ids.length === 1) {
          const r = await api.hubCall<HubProposal>("hub.proposals.decide", {
            id: ids[0],
            approve,
            ...extra,
            ...(value !== undefined ? { value } : {}),
          });
          out =
            r.state === "obsoleta"
              ? { text: `A proposta #${r.id} ficou obsoleta: ${r.decision_note ?? "o valor atual mudou"}.`, error: true }
              : { text: `Proposta #${r.id} ${STATE_TEXT[r.state]}.`, error: false };
        } else {
          const res = await api.hubCall<ProposalDecided[]>("hub.proposals.decide", { ids, approve, ...extra });
          out = batchResult(res, approve, known.current);
        }
      } catch (e) {
        return { text: errText(e), error: true, failed: true };
      }
      await load();
      return out;
    },
    [api, load],
  );

  return { pending, past, ranking, error, loaded, load, decide };
}

/**
 * Resultado de `hub.proposals.decide` em lote: cada id na sua transação, e
 * uma recusada (obsoleta, já decidida, inexistente) não impede as outras.
 * O texto resume; `applied` e `refused` (com o motivo) detalham.
 */
export function batchResult(res: ProposalDecided[], approve: boolean, known: readonly HubProposal[] = []): DecideResult {
  const applied: number[] = [];
  const refused: RefusedProposal[] = [];
  for (const o of res) {
    const p = o.proposal ?? known.find((x) => x.id === o.id) ?? null;
    const ref = p ? (p.ref ?? `${p.owner}/${p.repo}#${p.number}`) : null;
    if (o.proposal && o.proposal.state !== "obsoleta" && o.proposal.state !== "pendente") applied.push(o.id);
    else if (o.proposal?.state === "obsoleta")
      refused.push({ id: o.id, ref, reason: `obsoleta: ${o.proposal.decision_note ?? "o valor atual mudou"}` });
    else refused.push({ id: o.id, ref, reason: o.error ?? "não decidida" });
  }
  return {
    text: `${applied.length} proposta(s) ${approve ? "aprovada(s)" : "rejeitada(s)"}${
      refused.length > 0 ? `; ${refused.length} não (obsoleta ou já decidida)` : ""
    }.`,
    error: refused.length > 0,
    applied,
    refused,
  };
}

/**
 * Seleção e decisão em lote das propostas pendentes (#139), comum à seção
 * Hub › Propostas e ao cartão da Visão geral do projeto: as marcadas, a nota
 * opcional, ocupado enquanto o hub responde e a mensagem do resultado (ou a
 * falha de leitura). As marcadas que deixam de estar pendentes (decididas
 * aqui ou noutro lugar, relidas pelo push `hub.proposals.changed`) saem da
 * seleção. O lote vai na ordem das pendentes, não na de marcação.
 */
export function useProposalBatch(data: ProposalsData) {
  const { pending } = data;
  const [picked, setPicked] = useState<Set<number>>(new Set());
  const [note, setNote] = useState("");
  const [busy, setBusy] = useState(false);
  const [msg, setMsg] = useState<DecideResult | null>(null);
  const shown = msg ?? (data.error ? { text: data.error, error: true } : null);

  useEffect(() => {
    setPicked((s) => {
      const n = new Set([...s].filter((id) => pending.some((x) => x.id === id)));
      return n.size === s.size ? s : n;
    });
  }, [pending]);

  const decide = async (ids: number[], approve: boolean, value?: unknown) => {
    setBusy(true);
    setMsg(null);
    const r = await data.decide(ids, approve, note, value);
    setMsg(r);
    if (!r.failed) {
      setNote("");
      setPicked((s) => {
        const n = new Set(s);
        for (const id of ids) n.delete(id);
        return n;
      });
    }
    setBusy(false);
  };

  const toggle = (id: number) =>
    setPicked((s) => {
      const n = new Set(s);
      if (!n.delete(id)) n.add(id);
      return n;
    });

  const allPicked = pending.length > 0 && pending.every((p) => picked.has(p.id));
  const toggleAll = () => setPicked(allPicked ? new Set() : new Set(pending.map((p) => p.id)));
  /** As marcadas, na ordem das pendentes. */
  const pickedIds = pending.filter((p) => picked.has(p.id)).map((p) => p.id);

  return { picked, pickedIds, toggle, allPicked, toggleAll, note, setNote, busy, shown, decide };
}

/**
 * Decisão de uma proposta por vez, nas listas dos cartões (Visão geral do
 * Hub e do projeto): ocupado enquanto o hub responde e a mensagem do
 * resultado, ou a falha de leitura se não houver uma.
 */
export function useProposalDecision(data: ProposalsData) {
  const [busy, setBusy] = useState(false);
  const [msg, setMsg] = useState<DecideResult | null>(null);
  const decide = async (id: number, approve: boolean) => {
    setBusy(true);
    setMsg(null);
    setMsg(await data.decide([id], approve));
    setBusy(false);
  };
  const note = msg ?? (data.error ? { text: data.error, error: true } : null);
  return { busy, note, decide };
}

/** Mensagem do cartão: o resultado da decisão ou o erro, como alerta. */
export function ProposalNote({ note, className = "hub-card-msg" }: { note: DecideResult | null; className?: string }) {
  if (!note) return null;
  const cls = `${className} ${note.error ? "start-error" : "state-on"}`.trim();
  const role = note.error ? "alert" : "status";
  if (!note.applied?.length && !note.refused?.length)
    return (
      <p role={role} className={cls}>
        {note.text}
      </p>
    );
  return (
    <div role={role} className={`${cls} proposal-result`}>
      <p>{note.text}</p>
      {note.applied && note.applied.length > 0 && (
        <p className="muted">Aplicadas: {note.applied.map((id) => `#${id}`).join(", ")}</p>
      )}
      {note.refused && note.refused.length > 0 && (
        <ul aria-label="Propostas recusadas">
          {note.refused.map((r) => (
            <li key={r.id}>
              Proposta #{r.id}
              {r.ref && <span className="mono"> ({r.ref})</span>}: {r.reason}
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

/**
 * Uma proposta pendente na lista de um cartão (`hub-card-list`): o tipo, o
 * item (abre o item na tela Projeto), o título, o valor proposto (com o
 * atual antes, em `withCurrent`), a justificativa resumida com o resto sob
 * demanda e os botões de rejeitar e aprovar. `readOnly` (web, #100): os
 * botões desabilitados, ligados ao aviso pelo id dele.
 */
export function ProposalItem({
  p,
  title = p.title,
  withCurrent = false,
  rejectLabel = "Recusar",
  busy,
  onDecide,
  readOnly,
  pick,
}: {
  p: HubProposal;
  title?: string | null;
  withCurrent?: boolean;
  rejectLabel?: string;
  busy: boolean;
  onDecide(id: number, approve: boolean): void;
  readOnly?: { noteId: string };
  /** Caixa de seleção para a decisão em lote (#139). */
  pick?: { checked: boolean; onToggle(): void };
}) {
  const nav = useNav();
  const { hub } = useTower();
  const proj = hubProjectOf(hub.projects, p.owner, p.repo);
  return (
    <li aria-label={`Proposta #${p.id}`}>
      {pick && (
        <input
          type="checkbox"
          aria-label={`Marcar a proposta #${p.id}`}
          checked={pick.checked}
          disabled={busy || !!readOnly}
          aria-describedby={readOnly?.noteId}
          onChange={pick.onToggle}
        />
      )}
      <span className="badge">{KIND_TEXT[p.kind] ?? p.kind}</span>
      <div className="hub-card-text">
        <span className="proposal-line">
          <button
            type="button"
            className="linklike mono"
            title={p.ref ?? `${p.owner}/${p.repo}#${p.number}`}
            onClick={() => nav.go({ name: "project", owner: p.owner, repo: p.repo, item: { ghNumber: p.number } })}
          >
            {itemShort(proj, p.number)}
          </button>
          {title && (
            <>
              {" "}
              <span className="proposal-line-title" title={title}>
                {title}
              </span>
            </>
          )}{" "}
          <span className="proposal-line-change">
            {withCurrent && <>{valueText(p.kind, p.current, proj)} </>}
            <span className="muted">→</span> <strong>{valueText(p.kind, p.proposed, proj)}</strong>
          </span>
        </span>
        <div className="hub-card-sub">
          <Why text={p.justification} />
        </div>
      </div>
      <button
        type="button"
        className="btn btn-sm"
        disabled={busy || !!readOnly}
        aria-describedby={readOnly?.noteId}
        aria-label={`${rejectLabel} a proposta #${p.id}`}
        onClick={() => onDecide(p.id, false)}
      >
        {rejectLabel}
      </button>
      <button
        type="button"
        className="btn btn-sm btn-primary"
        disabled={busy || !!readOnly}
        aria-describedby={readOnly?.noteId}
        aria-label={`Aprovar a proposta #${p.id}`}
        onClick={() => onDecide(p.id, true)}
      >
        Aprovar
      </button>
    </li>
  );
}

/**
 * Linha da proposta `model` (#182): o valor proposto já nos seletores, que a
 * pessoa ajusta antes de aprovar. Aprovar com outro valor manda `value` e o
 * hub grava o escolhido; igual ao proposto, aprova como veio.
 */
export function ModelRow({
  p,
  busy,
  pick,
  project,
  issue,
  origin,
  age,
  onDecide,
}: {
  p: HubProposal;
  busy: boolean;
  pick: React.ReactNode;
  project: React.ReactNode;
  issue: React.ReactNode;
  origin: React.ReactNode;
  age: string;
  onDecide(approve: boolean, value?: unknown): void;
}) {
  const proposed = agentOf(p.proposed);
  const [model, setModel] = useState(proposed.model ?? CLAUDE_MODELS[0].value);
  const [effort, setEffort] = useState(proposed.effort ?? DEFAULT_EFFORT_VALUE);
  const changed = model !== proposed.model || (effort || null) !== proposed.effort;
  return (
    <tr aria-label={`Proposta #${p.id}`}>
      {pick}
      <td>{project}</td>
      <td>{issue}</td>
      <td className="nowrap">{valueText(p.kind, p.current)}</td>
      <td className="nowrap">
        <select aria-label={`Modelo da proposta #${p.id}`} value={model} onChange={(e) => setModel(e.target.value)}>
          {CLAUDE_MODELS.map((m) => (
            <option key={m.value} value={m.value}>
              {m.label}
            </option>
          ))}
        </select>{" "}
        <select aria-label={`Esforço da proposta #${p.id}`} value={effort} onChange={(e) => setEffort(e.target.value)}>
          <option value={DEFAULT_EFFORT_VALUE}>esforço padrão</option>
          {CLAUDE_EFFORTS.map((x) => (
            <option key={x.value} value={x.value}>
              {x.label}
            </option>
          ))}
        </select>
        {changed && <div className="muted small">proposto: {valueText(p.kind, p.proposed)}</div>}
      </td>
      <td className="proposal-why">
        <Why text={p.justification} />
      </td>
      {origin}
      <td className="nowrap">{age}</td>
      <td>
        <span className="env-actions">
          <button
            type="button"
            className="btn"
            disabled={busy}
            aria-label={`Aprovar a proposta #${p.id}`}
            onClick={() => onDecide(true, changed ? { model, effort: effort || null } : undefined)}
          >
            {changed ? "Aprovar alterada" : "Aprovar"}
          </button>
          <button
            type="button"
            className="btn btn-danger"
            disabled={busy}
            aria-label={`Rejeitar a proposta #${p.id}`}
            onClick={() => onDecide(false)}
          >
            Rejeitar
          </button>
        </span>
      </td>
    </tr>
  );
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
  const { picked, pickedIds, toggle, allPicked, toggleAll, note, setNote, busy, shown, decide } = useProposalBatch(data);

  const placed = useMemo(
    () => placeProposals(pending, ranking?.items ?? [], hub.projects),
    [pending, ranking, hub.projects],
  );
  const statusOnes = pending.filter((p) => p.kind === "status");
  const modelOnes = pending.filter((p) => p.kind === "model");
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
          {modelOnes.length > 0 && (
            <>
              <h3 className="section-title">Modelo e esforço</h3>
              <div className="table-wrap proposals-table">
                <table aria-label="Propostas de modelo">
                  <thead>
                    <tr>
                      <th>
                        <span className="sr-only">Marcar</span>
                      </th>
                      <th>Projeto</th>
                      <th>Issue</th>
                      <th>Atual</th>
                      <th title="Ajuste antes de aprovar: o hub grava o valor escolhido">Aprovar com</th>
                      <th>Justificativa</th>
                      <th>Ambiente</th>
                      <th>Idade</th>
                      <th>Ações</th>
                    </tr>
                  </thead>
                  <tbody>
                    {modelOnes.map((p) => (
                      <ModelRow
                        key={p.id}
                        p={p}
                        busy={busy}
                        pick={pick(p)}
                        project={project(p)}
                        issue={issue(p, p.title)}
                        origin={origin(p)}
                        age={ago(p.created_at, now)}
                        onDecide={(approve, value) => decide([p.id], approve, value)}
                      />
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
              onClick={toggleAll}
            >
              {allPicked ? "Desmarcar todas" : "Marcar todas"}
            </button>
            <button
              type="button"
              className="btn btn-primary"
              disabled={busy || pickedIds.length === 0}
              onClick={() => decide(pickedIds, true)}
            >
              Aprovar marcadas ({pickedIds.length})
            </button>
            <button
              type="button"
              className="btn btn-danger"
              disabled={busy || pickedIds.length === 0}
              onClick={() => decide(pickedIds, false)}
            >
              Rejeitar marcadas ({pickedIds.length})
            </button>
          </div>
        </>
      )}
      <ProposalNote note={shown} className="" />
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
                      {KIND_TEXT[p.kind] ?? p.kind}: {valueText(p.kind, p.current, projOf(p))} →{" "}
                      {valueText(p.kind, p.proposed, projOf(p))}
                      {p.chosen != null && (
                        <>
                          {" "}
                          <span className="muted">(aprovada com {valueText(p.kind, p.chosen, projOf(p))})</span>
                        </>
                      )}
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
