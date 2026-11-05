// "Só minhas" (#149): mesma regra de `same_identity` e `is_assignee` em
// crates/ct-core/src/tracker.rs. Os testes repetem os casos do Rust.
//
// O "eu" do projeto é `SyncState.me` (`ProjectTree.sync`, `HubTree.sync`,
// `HubProjectView.sync`): no GitHub, o login do token do sync; no Jira, a
// saída do `jira me` (username no Server/DC, e-mail no Cloud), que também
// vem em `TrackerReadiness.jira.me`. Os responsáveis da issue são
// `IssueRow.assignees`, com todos os identificadores conhecidos de cada um.

/** Dois identificadores são a mesma pessoa: iguais sem caixa, sem os espaços das pontas. Vazio nunca bate. */
export function sameIdentity(a: string, b: string): boolean {
  const x = a.trim();
  return x !== "" && x.toLowerCase() === b.trim().toLowerCase();
}

/** `me` é um dos responsáveis? Basta um identificador bater; sem `me` ou sem responsáveis, não. */
export function isAssignee(assignees: readonly string[] | null | undefined, me: string | null | undefined): boolean {
  if (!me || !assignees) return false;
  return assignees.some((a) => sameIdentity(a, me));
}
