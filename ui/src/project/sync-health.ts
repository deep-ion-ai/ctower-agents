// Limiar do aviso de sync falhando (#90). Sem dependências: a faixa
// "Precisa de você" (control-room/attention.ts) e a tela do projeto usam.
import type { SyncState } from "../protocol/gen";

/**
 * Falhas seguidas do sync de um projeto a partir das quais a faixa "Precisa
 * de você", a aba Avisos e o selo da lista de Projetos avisam (#90): uma
 * queda de rede passageira não chega lá. O cartão e o cabeçalho mostram a
 * falha desde a primeira.
 */
export const SYNC_FAIL_ALERT = 3;

/** Falhas seguidas; ambiente ou hub anterior ao campo (`failures` ausente) conta 0. */
export const syncFailures = (s: SyncState | null | undefined): number => s?.failures ?? 0;

/** O sync do projeto falha há `SYNC_FAIL_ALERT` rodadas ou mais. */
export const syncAlert = (s: SyncState | null | undefined): boolean => syncFailures(s) >= SYNC_FAIL_ALERT;
