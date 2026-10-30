// Ponto único de "iniciar sessão" (paleta #312, modal da #317). Quem quer
// iniciar uma sessão chama `useStartSession()` e passa o alvo: a modal de
// início (`StartSessionHost`, montada pelo shell) resolve projeto, ambiente,
// prévia e prompt sozinha, então quem chama não precisa da árvore.
import { createContext, useCallback, useContext } from "react";
import type { GestorScope, SessionMode } from "../protocol/gen";
import { useNav, type Tab } from "../state/nav";

/**
 * Alvo da sessão. Basta uma das chaves do projeto: a do hub ou a local.
 * Sem `ghNumber`, é a sessão de planejamento do projeto (clone principal).
 */
export interface StartTarget {
  ghNumber?: number;
  /** Chave natural do projeto (visão do hub). */
  owner?: string;
  repo?: string;
  /** Projeto local de um ambiente (na visão do hub, o ambiente sugerido). */
  env?: string;
  projectId?: number;
  /** Texto acrescentado ao prompt do template (ex.: recomeçar com nota). */
  note?: string;
  /** Modelo e modo pré-escolhidos (Reiniciar: os da sessão anterior); `model: null` = padrão do Claude Code. */
  model?: string | null;
  mode?: SessionMode;
  /**
   * Reiniciar (#424): a sessão que a nova substitui, na mesma aba. Com
   * `kill`, a modal encerra a antiga (`session.kill`) logo antes de iniciar.
   */
  replace?: Tab;
  kill?: boolean;
  /** Gestor de projetos (sem projeto): `session.start_gestor` no `env`, com este escopo. */
  gestor?: GestorScope;
}

/** Abre a modal no alvo; o foco volta para `origin` (ou o elemento em foco) ao fechar. */
export type StartSession = (t: StartTarget, origin?: Element | null) => void;

export const StartCtx = createContext<StartSession | null>(null);

/**
 * Inicia uma sessão: abre a modal de início. Fora do shell (sem a modal),
 * cai na tela Projeto com o item em foco.
 */
export function useStartSession(): StartSession {
  const open = useContext(StartCtx);
  const { go } = useNav();
  const fallback = useCallback<StartSession>(
    (t) =>
      go({
        name: "project",
        env: t.env,
        projectId: t.projectId,
        owner: t.owner,
        repo: t.repo,
        item: t.ghNumber !== undefined ? { ghNumber: t.ghNumber } : undefined,
      }),
    [go],
  );
  return open ?? fallback;
}
