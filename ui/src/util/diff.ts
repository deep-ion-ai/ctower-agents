/** Linha da diferença: igual, removida (só no antigo) ou incluída (só no novo). */
export interface DiffLine {
  op: "same" | "del" | "add";
  text: string;
}

/** Acima disso (linhas × linhas), a diferença vira "tudo saiu, tudo entrou" (sem a tabela da LCS). */
const MAX_CELLS = 4_000_000;

const lines = (s: string) => (s === "" ? [] : s.replace(/\r\n/g, "\n").replace(/\n$/, "").split("\n"));

/**
 * Diferença por linhas (maior subsequência comum), para mostrar a proposta de
 * regras contra o texto vigente. As regras têm até 16 KiB, então a tabela
 * cabe; as linhas iguais nas pontas ficam fora dela.
 */
export function lineDiff(before: string, after: string): DiffLine[] {
  const a = lines(before);
  const b = lines(after);
  let start = 0;
  while (start < a.length && start < b.length && a[start] === b[start]) start++;
  let endA = a.length;
  let endB = b.length;
  while (endA > start && endB > start && a[endA - 1] === b[endB - 1]) {
    endA--;
    endB--;
  }
  const head: DiffLine[] = a.slice(0, start).map((text) => ({ op: "same", text }));
  const tail: DiffLine[] = a.slice(endA).map((text) => ({ op: "same", text }));
  const x = a.slice(start, endA);
  const y = b.slice(start, endB);
  const n = x.length;
  const m = y.length;
  if (n * m > MAX_CELLS) {
    return [
      ...head,
      ...x.map((text): DiffLine => ({ op: "del", text })),
      ...y.map((text): DiffLine => ({ op: "add", text })),
      ...tail,
    ];
  }
  // lcs[i][j] = tamanho da maior subsequência comum de x[i..] e y[j..].
  const w = m + 1;
  const lcs = new Uint32Array((n + 1) * w);
  for (let i = n - 1; i >= 0; i--) {
    for (let j = m - 1; j >= 0; j--) {
      lcs[i * w + j] = x[i] === y[j] ? lcs[(i + 1) * w + j + 1] + 1 : Math.max(lcs[(i + 1) * w + j], lcs[i * w + j + 1]);
    }
  }
  const mid: DiffLine[] = [];
  let i = 0;
  let j = 0;
  while (i < n && j < m) {
    if (x[i] === y[j]) {
      mid.push({ op: "same", text: x[i] });
      i++;
      j++;
    } else if (lcs[(i + 1) * w + j] >= lcs[i * w + j + 1]) {
      mid.push({ op: "del", text: x[i++] });
    } else {
      mid.push({ op: "add", text: y[j++] });
    }
  }
  while (i < n) mid.push({ op: "del", text: x[i++] });
  while (j < m) mid.push({ op: "add", text: y[j++] });
  return [...head, ...mid, ...tail];
}

/** Quantas linhas entram e saem. */
export function diffStats(d: DiffLine[]): { added: number; removed: number } {
  return {
    added: d.filter((l) => l.op === "add").length,
    removed: d.filter((l) => l.op === "del").length,
  };
}
