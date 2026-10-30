import type { ProjectTree, TreeNode, WorkItemSummary } from "../protocol/gen";

export const EMPTY_SUMMARY: WorkItemSummary = {
  pr: null,
  decisions: { total: 0, pending_autonomous: 0 },
  last_activity_at: null,
  live_envs: [],
};

/**
 * Daemons anteriores à #46 não mandam `summary` no `tree.get` local. A UI trata o campo como
 * opcional na borda: nó sem resumo vira o resumo vazio, e a tela mostra só o que o daemon sabe.
 */
export function withSummaries(tree: ProjectTree): ProjectTree {
  const fill = (n: TreeNode): TreeNode => ({
    ...n,
    summary: n.summary ?? EMPTY_SUMMARY,
    children: (n.children ?? []).map(fill),
  });
  return { ...tree, roots: tree.roots.map(fill) };
}
