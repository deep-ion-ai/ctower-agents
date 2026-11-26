// Execução de épico (#304, épico #295): funções puras do painel. Ondas do
// plano pelo grafo de dependências, sobreposição de áreas (o mesmo critério
// do motor, `ct_core::area_overlaps`), textos de estado e das opções de
// decisão. Testadas em epicRun.test.ts.
import type {
  EpicDecisionOrigin,
  EpicNode,
  EpicNodeState,
  EpicPlan,
  EpicRun,
  EpicRunStatus,
} from "../protocol/hub-gen";

/** Protocolo do hub com a execução de épico (`hub.epic_run.*`). */
export const EPIC_RUN_PROTOCOL = "1.26";

export const RUN_STATUS_TEXT: Record<EpicRunStatus, string> = {
  planejando: "Planejando",
  aguardando_aprovacao: "Plano aguardando sua aprovação",
  executando: "Executando",
  pausado: "Pausado por você",
  estacionado: "Estacionado · janela da conta esgotada",
  replanejando: "Replanejando",
  integrando: "Integrando",
  aguardando_merge: "Aguardando o seu merge",
  concluido: "Concluído",
  cancelado: "Cancelado",
};

/** Âmbar só para o que pede o desenvolvedor. */
export type Tone = "attention" | "running" | "done" | "idle" | "error";

export function runTone(s: EpicRunStatus): Tone {
  switch (s) {
    case "aguardando_aprovacao":
    case "aguardando_merge":
      return "attention";
    case "concluido":
      return "done";
    case "cancelado":
      return "error";
    case "pausado":
    case "estacionado":
      return "idle";
    default:
      return "running";
  }
}

export const NODE_STATE_TEXT: Record<EpicNodeState, string> = {
  pendente: "Pendente",
  pronta: "Pronta",
  em_execucao: "Rodando",
  em_pr: "PR aberto",
  mesclada: "Mesclada na wip",
  assumida: "Assumida por você",
  estacionada: "Estacionada",
  bloqueada: "Pede decisão",
};

export function nodeTone(s: EpicNodeState): Tone | "assumed" {
  switch (s) {
    case "mesclada":
      return "done";
    case "em_execucao":
    case "em_pr":
      return "running";
    case "assumida":
      return "assumed";
    case "bloqueada":
      return "attention";
    default:
      return "idle";
  }
}

/** O motor está na fase de execução das sub-issues. */
export const isExecuting = (s: EpicRunStatus) =>
  s === "executando" || s === "pausado" || s === "estacionado" || s === "replanejando";
/** A fase da integração. */
export const isIntegrating = (s: EpicRunStatus) =>
  s === "integrando" || s === "aguardando_merge" || s === "concluido";
/** Pode pausar, interromper e cancelar. */
export const isActive = (s: EpicRunStatus) => s !== "concluido" && s !== "cancelado";

/** "sonnet · medium" / "sonnet" / "sem modelo". */
export function agentText(model: string | null | undefined, effort: string | null | undefined): string {
  if (!model) return "sem modelo";
  return effort ? `${model} · ${effort}` : model;
}

/** Os bloqueantes de cada nó no plano. */
export function blockersOf(plan: EpicPlan): Map<number, number[]> {
  const m = new Map<number, number[]>();
  for (const e of plan.edges) {
    const l = m.get(e.number) ?? [];
    l.push(e.blocker);
    m.set(e.number, l);
  }
  for (const l of m.values()) l.sort((a, b) => a - b);
  return m;
}

/**
 * Ondas do plano: onda 1 = sem bloqueante dentro do plano; cada nó fica uma
 * onda depois do bloqueante mais tardio. Bloqueante fora do plano não conta
 * (o motor espera o merge dele na base). Ciclo: os nós dele ficam numa onda
 * final, para a tela não sumir com eles. Dentro da onda, a ordem do plano.
 */
export function waves(plan: EpicPlan): number[][] {
  const order = plan.nodes.map((n) => n.number);
  const inPlan = new Set(order);
  const blockers = blockersOf(plan);
  const level = new Map<number, number>();
  const visiting = new Set<number>();
  const CYCLE = -1;
  const depth = (n: number): number => {
    const known = level.get(n);
    if (known !== undefined) return known;
    if (visiting.has(n)) return CYCLE;
    visiting.add(n);
    let d = 0;
    for (const b of blockers.get(n) ?? []) {
      if (!inPlan.has(b)) continue;
      const bd = depth(b);
      if (bd === CYCLE) {
        d = CYCLE;
        break;
      }
      d = Math.max(d, bd + 1);
    }
    visiting.delete(n);
    level.set(n, d);
    return d;
  };
  for (const n of order) depth(n);
  const max = Math.max(0, ...[...level.values()]);
  const out: number[][] = Array.from({ length: max + 1 }, () => []);
  const cyc: number[] = [];
  for (const n of order) {
    const d = level.get(n) ?? 0;
    if (d === CYCLE) cyc.push(n);
    else out[d].push(n);
  }
  const res = out.filter((w) => w.length > 0);
  if (cyc.length) res.push(cyc);
  return res;
}

const GLOB = /[*?[{]/;

function literalPrefix(a: string): [string, boolean] {
  const i = a.search(GLOB);
  return i < 0 ? [a, false] : [a.slice(0, i), true];
}

function pathContains(a: string, b: string): boolean {
  const x = a.replace(/\/+$/, "");
  const y = b.replace(/\/+$/, "");
  return x === "" || x === y || (y.startsWith(x) && y[x.length] === "/");
}

/** Duas áreas podem tocar o mesmo arquivo (o critério do motor, conservador com glob). */
export function areaOverlaps(a: string, b: string): boolean {
  const [pa, ga] = literalPrefix(a);
  const [pb, gb] = literalPrefix(b);
  if (!ga && !gb) return pathContains(a, b) || pathContains(b, a);
  if (ga && !gb) return b.startsWith(pa) || pathContains(b, pa);
  if (!ga && gb) return a.startsWith(pb) || pathContains(a, pb);
  return pa.startsWith(pb) || pb.startsWith(pa);
}

/** As áreas de cada nó que se sobrepõem às de outro, e com quem. */
export function overlaps(plan: EpicPlan): Map<number, { with: number[]; areas: Set<string> }> {
  const out = new Map<number, { with: number[]; areas: Set<string> }>();
  const ns = plan.nodes;
  for (let i = 0; i < ns.length; i++) {
    for (let j = i + 1; j < ns.length; j++) {
      const a = ns[i];
      const b = ns[j];
      for (const x of a.areas) {
        for (const y of b.areas) {
          if (!areaOverlaps(x, y)) continue;
          for (const [n, o, area] of [
            [a.number, b.number, x],
            [b.number, a.number, y],
          ] as const) {
            const cur = out.get(n) ?? { with: [], areas: new Set<string>() };
            if (!cur.with.includes(o)) cur.with.push(o);
            cur.areas.add(area);
            out.set(n, cur);
          }
        }
      }
    }
  }
  for (const v of out.values()) v.with.sort((a, b) => a - b);
  return out;
}

/** Pares de sub-issues que o motor serializa por área sobreposta. */
export function overlapPairs(plan: EpicPlan): number {
  let n = 0;
  for (const v of overlaps(plan).values()) n += v.with.length;
  return n / 2;
}

export const ORIGIN_TEXT: Record<EpicDecisionOrigin, string> = {
  plano_errado: "a sessão informou que o plano está errado",
  ci_esgotado: "o CI falhou depois das tentativas de correção",
  sem_progresso: "a sessão parou sem progresso",
  conflito_rebase: "o rebase na wip não andou",
  issue_alterada: "a sub-issue mudou depois do plano",
};

/** Texto e explicação de cada resposta. */
export const OPTION_TEXT: Record<string, { label: string; hint: string }> = {
  replanejar: {
    label: "Replanejar",
    hint: "Nova sessão de plano; o plano volta para sua aprovação. As sub-issues vivas fora da mudança continuam.",
  },
  aguardar: {
    label: "Aguardar",
    hint: "A sub-issue espera a issue que você indicar (bloqueante manual); sem indicar, fica parada.",
  },
  assumir: { label: "Assumir", hint: "Você entra na sessão; o motor não manda nada até você devolver." },
  mais_tentativas: { label: "Mais tentativas", hint: "Zera as tentativas de correção do CI e volta a acompanhar o PR." },
  reiniciar: { label: "Reiniciar a sessão", hint: "Encerra a sessão atual; a sub-issue começa de novo." },
  manter: { label: "Manter o plano", hint: "Aceita a mudança da sub-issue e segue com o plano aprovado." },
};

/** Sub-issues mescladas / total no plano. */
export function mergedCount(nodes: EpicNode[]): [number, number] {
  return [nodes.filter((n) => n.state === "mesclada").length, nodes.length];
}

/** As linhas que o PR do épico leva no corpo (uma por sub-issue mesclada e a do épico). */
export function closesLines(run: Pick<EpicRun, "epic">, nodes: EpicNode[]): string[] {
  return [
    ...nodes
      .filter((n) => n.state === "mesclada")
      .map((n) => n.number)
      .sort((a, b) => a - b)
      .map((n) => `Closes #${n}`),
    `Closes #${run.epic}`,
  ];
}

/** "14:05" no fuso local. */
export const clock = (ms: number) =>
  new Date(ms).toLocaleTimeString("pt-BR", { hour: "2-digit", minute: "2-digit" });
