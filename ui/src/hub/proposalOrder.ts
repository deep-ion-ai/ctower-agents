import type { HubProjectView, HubProposal, HubRankingItem, Priority } from "../protocol/hub-gen";

/** Classes na ordem da §4.3 da spec do gestor; `null` (não triado) no fim. */
const CLASSES: (Priority | null)[] = ["P0", "P1", "P2", "P3", null];

type Value = Record<string, unknown>;

/** Para onde a proposta leva o item no ranking, comparado ao atual. */
export type Move = "up" | "down" | "same" | "new";

/** Uma proposta de prioridade posicionada na ordem resultante. */
export type PlacedProposal = {
  proposal: HubProposal;
  /** Título da proposta ou, na falta, o do ranking. */
  title: string | null;
  /** Posição no ranking atual (`hub.ranking.get`); nula se o item não está nele. */
  from: number | null;
  /** Posição no ranking depois de aplicar todas as pendentes de prioridade. */
  to: number;
  move: Move;
};

const lc = (owner: string, repo: string) => `${owner}/${repo}`.toLowerCase();

function priorityOf(v: unknown): Priority | null {
  const p = (v ?? {}) as Value;
  return typeof p.priority === "string" ? (p.priority as Priority) : null;
}

function anchor(v: unknown, k: "before" | "after"): number | null {
  const n = ((v ?? {}) as Value)[k];
  return typeof n === "number" ? n : null;
}

/**
 * Simula a aprovação de todas as propostas pendentes de prioridade sobre o
 * ranking atual e devolve cada uma com a posição atual e a resultante, na
 * ordem resultante (spec do gestor §4.3: classe, ordem do projeto, ordem do
 * item na classe). Aplica da mais velha para a mais nova, como o hub faria
 * aprovando nessa ordem: tira o item da classe atual e o põe na proposta,
 * antes ou depois da âncora se ela está na mesma classe do mesmo projeto,
 * senão no fim da classe (§8.4). Não triados ficam por número.
 *
 * O ranking pode vir cortado (`limit`); aí as posições valem para a parte
 * conhecida. Os projetos seguem `project.rank`, nome e chave, como no hub.
 */
export function placeProposals(
  proposals: HubProposal[],
  ranking: HubRankingItem[],
  projects: HubProjectView[],
): PlacedProposal[] {
  const pri = proposals.filter((p) => p.kind === "priority");
  // Classe (prioridade + projeto) → números na ordem da classe.
  const groups = new Map<string, number[]>();
  const gk = (p: Priority | null, proj: string) => `${p ?? "-"}|${proj}`;
  const titles = new Map<string, string | null>();
  const fromPos = new Map<string, number>();
  const projOrder = new Map<string, [number, string, string]>();
  for (const v of projects) {
    const p = v.project;
    projOrder.set(lc(p.gh_owner, p.gh_repo), [p.rank ?? Number.MAX_SAFE_INTEGER, p.name, lc(p.gh_owner, p.gh_repo)]);
  }
  // Projeto ausente da lista do hub: depois dos conhecidos, na ordem em que aparece.
  let unknown = 0;
  const seenProj = (k: string) => {
    if (!projOrder.has(k)) projOrder.set(k, [Infinity, String(unknown++).padStart(6, "0"), k]);
  };
  for (const it of ranking) {
    const pk = lc(it.owner, it.repo);
    seenProj(pk);
    const g = gk(it.priority, pk);
    groups.set(g, [...(groups.get(g) ?? []), it.number]);
    titles.set(`${pk}#${it.number}`, it.title);
    fromPos.set(`${pk}#${it.number}`, it.position);
  }
  const where = new Map<string, Priority | null>(
    ranking.map((it) => [`${lc(it.owner, it.repo)}#${it.number}`, it.priority]),
  );

  const ordered = [...pri].sort((a, b) => a.created_at - b.created_at || a.id - b.id);
  for (const p of ordered) {
    const pk = lc(p.owner, p.repo);
    seenProj(pk);
    const key = `${pk}#${p.number}`;
    const cur = where.has(key) ? where.get(key)! : priorityOf(p.current);
    const old = groups.get(gk(cur, pk));
    if (old) {
      const i = old.indexOf(p.number);
      if (i >= 0) old.splice(i, 1);
    }
    const to = priorityOf(p.proposed);
    const g = groups.get(gk(to, pk)) ?? [];
    groups.set(gk(to, pk), g);
    const before = anchor(p.proposed, "before");
    const after = anchor(p.proposed, "after");
    const bi = before !== null ? g.indexOf(before) : -1;
    const ai = after !== null ? g.indexOf(after) : -1;
    if (to === null) {
      const i = g.findIndex((n) => n > p.number);
      g.splice(i < 0 ? g.length : i, 0, p.number);
    } else if (bi >= 0) g.splice(bi, 0, p.number);
    else if (ai >= 0) g.splice(ai + 1, 0, p.number);
    else g.push(p.number);
    where.set(key, to);
  }

  const projKeys = [...projOrder.keys()].sort((a, b) => {
    const [ra, na, ka] = projOrder.get(a)!;
    const [rb, nb, kb] = projOrder.get(b)!;
    return ra - rb || na.localeCompare(nb) || ka.localeCompare(kb);
  });
  const toPos = new Map<string, number>();
  let pos = 0;
  for (const c of CLASSES) {
    for (const pk of projKeys) {
      for (const n of groups.get(gk(c, pk)) ?? []) toPos.set(`${pk}#${n}`, ++pos);
    }
  }

  return pri
    .map((p): PlacedProposal => {
      const key = `${lc(p.owner, p.repo)}#${p.number}`;
      const from = fromPos.get(key) ?? null;
      const to = toPos.get(key)!;
      const move: Move = from === null ? "new" : to < from ? "up" : to > from ? "down" : "same";
      return {
        proposal: p,
        title: p.title ?? titles.get(key) ?? null,
        from,
        to,
        move,
      };
    })
    .sort((a, b) => a.to - b.to);
}
