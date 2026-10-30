// Busca difusa da paleta de comandos (#312). Funções puras.
//
// A busca é dividida em palavras; cada palavra precisa casar com o rótulo
// (com os caracteres marcados) ou, com peso menor, com o detalhe e as
// palavras-chave do item. Trecho contínuo vale mais que subsequência, e
// início de palavra vale mais que o meio dela.
import type { PaletteItem, PaletteKind } from "./items";

/** `#` = só issues e épicos; `>` = só comandos. */
export type PaletteMode = "all" | "issue" | "command";

export function parseQuery(raw: string): { mode: PaletteMode; text: string } {
  const q = raw.trimStart();
  if (q.startsWith("#")) return { mode: "issue", text: q.slice(1).trim() };
  if (q.startsWith(">")) return { mode: "command", text: q.slice(1).trim() };
  return { mode: "all", text: q.trim() };
}

const MODE_KINDS: Record<PaletteMode, PaletteKind[] | null> = {
  all: null,
  issue: ["issue", "epic"],
  command: ["command"],
};

/** Minúscula e sem acento, caractere a caractere (os índices continuam valendo). */
export function fold(s: string): string {
  let out = "";
  for (const c of s) {
    // Fora do BMP o caractere ocupa 2 posições: vira 2 espaços.
    out += c.length === 2 ? "  " : (c.normalize("NFD")[0].toLowerCase()[0] ?? " ");
  }
  return out;
}

const isWordStart = (t: string, i: number) => i === 0 || !/[a-z0-9]/.test(t[i - 1]);

export interface TokenMatch {
  score: number;
  /** Posições casadas no texto. */
  hits: number[];
}

/**
 * Casa uma palavra (já em `fold`) com o texto (já em `fold`). Trecho
 * contínuo: 100+; subsequência: até 60. `null` = não casa.
 */
export function matchToken(text: string, tok: string): TokenMatch | null {
  if (!tok) return { score: 0, hits: [] };
  // Trecho contínuo: o melhor dos lugares em que aparece.
  let best: TokenMatch | null = null;
  for (let p = text.indexOf(tok); p >= 0; p = text.indexOf(tok, p + 1)) {
    const score = 100 + (isWordStart(text, p) ? 30 : 0) + (p === 0 ? 20 : 0) + tok.length * 2 - Math.min(p, 20) * 0.5;
    if (!best || score > best.score) best = { score, hits: Array.from(tok, (_, i) => p + i) };
  }
  if (best) return best;
  // Subsequência: prefere inícios de palavra; se assim não casar, a primeira ocorrência de cada caractere.
  const sub = subsequence(text, tok, true) ?? subsequence(text, tok, false);
  return sub && { score: Math.max(1, (60 * sub.raw) / (tok.length * 11)), hits: sub.hits };
}

function subsequence(text: string, tok: string, words: boolean): { raw: number; hits: number[] } | null {
  const hits: number[] = [];
  let raw = 0;
  let from = 0;
  for (const c of tok) {
    let at = text.indexOf(c, from);
    if (at < 0) return null;
    const prev = hits[hits.length - 1];
    // Sem quebrar uma sequência, pula para o próximo início de palavra com o caractere.
    if (words && !isWordStart(text, at) && prev !== at - 1) {
      for (let j = at + 1; j < text.length; j++) {
        if (text[j] === c && isWordStart(text, j)) {
          at = j;
          break;
        }
      }
    }
    raw += 1 + (prev !== undefined && at === prev + 1 ? 4 : 0) + (isWordStart(text, at) ? 6 : 0);
    if (prev !== undefined) raw -= Math.min(at - prev - 1, 10) * 0.2;
    hits.push(at);
    from = at + 1;
  }
  return { raw, hits };
}

export interface PaletteResult {
  item: PaletteItem;
  score: number;
  /** Posições casadas no rótulo (para destacar). */
  hits: number[];
  recent: boolean;
}

/** Ordem de desempate entre tipos (issues primeiro: "#297" é o item). */
const KIND_ORDER: Record<PaletteKind, number> = {
  issue: 0,
  epic: 0,
  session: 1,
  project: 2,
  place: 3,
  command: 4,
};

export const MAX_RESULTS = 50;

/**
 * Resultados para a busca. Vazia: os recentes (na ordem dada) e depois os
 * itens de navegação (`browse`), sem as listas longas (issues, inícios).
 * Com texto: por pontuação, recentes com um pequeno bônus.
 */
export function search(items: PaletteItem[], raw: string, recent: string[] = [], limit = MAX_RESULTS): PaletteResult[] {
  const { mode, text } = parseQuery(raw);
  const kinds = MODE_KINDS[mode];
  const pool = kinds ? items.filter((i) => kinds.includes(i.kind)) : items;
  const recentRank = new Map(recent.map((id, i) => [id, i]));

  if (!text) {
    const byId = new Map(pool.map((i) => [i.id, i]));
    const head = recent.flatMap((id) => {
      const it = byId.get(id);
      return it ? [{ item: it, score: 0, hits: [], recent: true }] : [];
    });
    // Em modo `#`, a lista são as issues (abertas primeiro); fora dele, só o navegável.
    const rest = pool
      .filter((i) => !recentRank.has(i.id) && (mode === "issue" || i.browse !== false))
      .sort((a, b) => (mode === "issue" ? Number(!!a.closed) - Number(!!b.closed) : 0))
      .map((item) => ({ item, score: 0, hits: [], recent: false }));
    return [...head, ...rest].slice(0, limit);
  }

  const toks = fold(text).split(/\s+/).filter(Boolean);
  const out: PaletteResult[] = [];
  for (const item of pool) {
    const label = fold(item.label);
    const extra = fold([item.detail, item.keywords].filter(Boolean).join(" "));
    let score = 0;
    const hits: number[] = [];
    let ok = true;
    for (const t of toks) {
      const tok = t.replace(/^#/, "");
      const inLabel = matchToken(label, t) ?? (tok !== t ? matchToken(label, tok) : null);
      const inExtra = inLabel ? null : matchToken(extra, tok);
      if (!inLabel && !inExtra) {
        ok = false;
        break;
      }
      if (inLabel) {
        score += inLabel.score;
        hits.push(...inLabel.hits);
      } else score += inExtra!.score * 0.6;
      // Número exato da issue ("297" ou "#297"): no topo.
      if (item.num !== undefined && tok === String(item.num)) score += 200;
    }
    if (!ok) continue;
    if (item.closed) score -= 10;
    const r = recentRank.get(item.id);
    if (r !== undefined) score += 15 - r;
    out.push({ item, score, hits: [...new Set(hits)].sort((a, b) => a - b), recent: r !== undefined });
  }
  out.sort((a, b) => b.score - a.score || KIND_ORDER[a.item.kind] - KIND_ORDER[b.item.kind]);
  return out.slice(0, limit);
}
