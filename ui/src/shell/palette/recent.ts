// Itens usados há pouco na paleta (#312): por máquina, em localStorage,
// como a disposição do shell. Só os ids; o item é refeito na abertura.
const KEY = "ctower.palette.recent";
export const MAX_RECENT = 8;

export function loadRecent(): string[] {
  try {
    const v: unknown = JSON.parse(localStorage.getItem(KEY) ?? "[]");
    return Array.isArray(v) ? v.filter((x): x is string => typeof x === "string").slice(0, MAX_RECENT) : [];
  } catch {
    // Sem armazenamento: sem recentes.
    return [];
  }
}

/** Põe o id no topo e grava; devolve a lista nova. */
export function pushRecent(id: string, list = loadRecent()): string[] {
  const next = [id, ...list.filter((x) => x !== id)].slice(0, MAX_RECENT);
  try {
    localStorage.setItem(KEY, JSON.stringify(next));
  } catch {
    // Sem armazenamento: vale só nesta execução.
  }
  return next;
}
