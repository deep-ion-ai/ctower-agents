// Shell livre no worktree (#423; spec da #87, §4): um PTY do daemon
// (`shell.open|list|close`) mostrado pela tela Terminal como uma aba de tipo
// `shell`. Aqui ficam os rótulos, a lista dos shells vivos por ambiente
// (`shell.list`, atualizada pelo push `shell_exited`) e o ponto único de
// "abrir shell" (botão do cabeçalho, paleta, atalho e tela Projeto).
// Só no app desktop: `shell.*` fica fora da allowlist web do hub.
import { useCallback, useSyncExternalStore } from "react";
import type { DaemonApi } from "../api/daemon";
import type { Dashboard, DashboardWorkItem, Project, Shell } from "../protocol/gen";
import { isShell, useNav, type Pane, type ShellTab, type Tab } from "../state/nav";
import { useTower } from "../state/tower";
import { itemShort, PROJECT_ITEM } from "../util/item";
import { lastTermSize } from "./size";

export { shellEndText } from "./adapter";

/** Work item do shell e o projeto dele, pelo id (dashboard do ambiente). */
export function shellItem(
  d: Dashboard | undefined,
  workItemId: number,
): { wi?: DashboardWorkItem; project?: Project } {
  for (const p of d?.projects ?? []) {
    const wi = p.work_items.find((w) => w.id === workItemId);
    if (wi) return { wi, project: p.project };
  }
  return {};
}

/**
 * Work item onde abrir um shell a partir da aba (o da sessão ou o do shell);
 * `null` no gestor (não há pasta de item) ou sem o work item.
 */
export function shellTarget(
  p: Pane | null,
  info: (t: Tab) => { wi?: DashboardWorkItem; gestor?: boolean },
): { env: string; workItemId: number } | null {
  if (!p) return null;
  if (isShell(p)) return { env: p.env, workItemId: p.workItemId };
  const { wi, gestor } = info(p);
  return wi && !gestor ? { env: p.env, workItemId: wi.id } : null;
}

/** "#87", "PROJ-87" ou "sessão do projeto" (#0, clone principal). */
export function shellRef(wi: DashboardWorkItem | undefined, project: Project | undefined): string {
  if (!wi) return "item";
  return wi.gh_number === PROJECT_ITEM ? "sessão do projeto" : itemShort(project, wi.gh_number);
}

/** Título da aba: "Shell · #87" ou "Shell · sessão do projeto". */
export const shellTitle = (wi: DashboardWorkItem | undefined, project: Project | undefined) =>
  `Shell · ${shellRef(wi, project)}`;

/** Nome do programa sem a pasta: `/bin/zsh` → `zsh`. */
export const programName = (program: string | undefined) => program?.split("/").pop() || "shell";

/** Rótulo na árvore: "zsh · #87". */
export const shellLabel = (sh: Pick<Shell, "program"> | undefined, wi: DashboardWorkItem | undefined, project: Project | undefined) =>
  `${programName(sh?.program)} · ${shellRef(wi, project)}`;

/**
 * O que fazer ao fechar a aba, pelo `shell.list` consultado na hora:
 * `close` (sem comando rodando, ou o shell já saiu), `busy` (pedir
 * confirmação) ou `unknown` (a lista falhou: confirmar sem saber).
 */
export function closeCheck(list: Shell[] | null, id: string): "close" | "busy" | "unknown" {
  if (!list) return "unknown";
  return list.find((s) => s.id === id)?.busy ? "busy" : "close";
}

/** Consulta o `shell.list` (o `busy` de agora); `null` se falhar. */
export const listShells = (api: DaemonApi, env: string) =>
  api.call<Shell[]>(env, "shell.list").catch(() => null);

// Estado comum às telas (a tela Projeto abre shells com a tela Terminal
// desmontada): shells vivos por ambiente, os já vistos (o cabeçalho de um
// shell encerrado segue com pasta e programa) e o último erro de abrir.
interface Store {
  live: Record<string, Shell[]>;
  seen: Record<string, Shell>;
  error: string | null;
}

let store: Store = { live: {}, seen: {}, error: null };
const subs = new Set<() => void>();

function update(f: (s: Store) => Store) {
  store = f(store);
  for (const cb of subs) cb();
}

const subscribe = (cb: () => void) => {
  subs.add(cb);
  return () => subs.delete(cb);
};

export function useShellStore(): Store {
  return useSyncExternalStore(subscribe, () => store);
}

const seenWith = (seen: Record<string, Shell>, list: Shell[]) => {
  const out = { ...seen };
  for (const s of list) out[s.id] = s;
  return out;
};

/** A lista do daemon (`shell.list`) de um ambiente. */
export function setShells(env: string, list: Shell[]) {
  update((s) => ({ ...s, live: { ...s.live, [env]: list }, seen: seenWith(s.seen, list) }));
}

export function addShell(env: string, sh: Shell) {
  update((s) => ({
    ...s,
    live: { ...s.live, [env]: [...(s.live[env] ?? []).filter((x) => x.id !== sh.id), sh] },
    seen: seenWith(s.seen, [sh]),
  }));
}

/** O shell saiu (push `shell_exited`): some da árvore. */
export function dropShell(env: string, id: string) {
  update((s) => ({ ...s, live: { ...s.live, [env]: (s.live[env] ?? []).filter((x) => x.id !== id) } }));
}

export function setShellError(error: string | null) {
  if (store.error !== error) update((s) => ({ ...s, error }));
}

/** Só para os testes. */
export function resetShellStore() {
  store = { live: {}, seen: {}, error: null };
}

/**
 * Abre um shell novo no worktree do work item (cada chamada, um shell) e a
 * aba dele. A recusa do daemon (sem worktree, gestor, limite de 8) aparece
 * na tela Terminal, que fica ativa, como os outros erros do shell.
 */
export function useOpenShell(): (env: string, workItemId: number) => Promise<ShellTab | null> {
  const { api } = useTower();
  const { openTerminal, go } = useNav();
  return useCallback(
    async (env, workItemId) => {
      const size = lastTermSize();
      try {
        const sh = await api.call<Shell>(env, "shell.open", { work_item_id: workItemId, ...size });
        setShellError(null);
        addShell(env, sh);
        const tab = { env, shell: sh.id, workItemId };
        openTerminal(tab);
        return tab;
      } catch (e) {
        setShellError(`Não foi possível abrir o shell: ${e instanceof Error ? e.message : String(e)}`);
        go({ name: "terminal" });
        return null;
      }
    },
    [api, openTerminal, go],
  );
}
