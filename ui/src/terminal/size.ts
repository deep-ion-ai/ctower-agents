// Último tamanho medido da área do terminal, para a sessão nova já nascer
// com ele (`session.start` com `cols`/`rows`). Fica por máquina.
import type { TermSize } from "./adapter";

const KEY = "ctower.terminal.size";

let last: TermSize | null = null;

export function rememberTermSize(size: TermSize) {
  if (size.cols < 1 || size.rows < 1) return;
  last = size;
  try {
    localStorage.setItem(KEY, JSON.stringify(size));
  } catch {
    // Sem armazenamento: vale só nesta execução.
  }
}

/** Tamanho conhecido, ou `null` (o daemon usa 120×40). */
export function lastTermSize(): TermSize | null {
  if (last) return last;
  try {
    const v = JSON.parse(localStorage.getItem(KEY) ?? "null");
    if (Number.isInteger(v?.cols) && Number.isInteger(v?.rows) && v.cols > 0 && v.rows > 0) {
      last = { cols: v.cols, rows: v.rows };
    }
  } catch {
    // Valor inválido ou armazenamento indisponível.
  }
  return last;
}

/** Só para os testes. */
export function forgetTermSize() {
  last = null;
  try {
    localStorage.removeItem(KEY);
  } catch {
    // idem
  }
}
