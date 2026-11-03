// "Descartar" uma sessão interrompida sem retomá-la (#138). Ponto único, como
// o `useStartSession`: quem quer descartar chama `useDiscard()` com uma ou
// mais sessões, e a confirmação (`DiscardHost`, montada pelo shell) arquiva
// (`session.archive`) e, só se a pessoa escolher, muda o status do work item
// pelo hub (`hub.workitem.set_status`).
import "./discard.css";
import { createContext, useCallback, useContext, useEffect, useRef, useState, type KeyboardEvent, type ReactNode } from "react";
import { issueRef, itemLabel, lookup } from "../control-room/attention";
import type { Dashboard, Session, WorkItemStatus } from "../protocol/gen";
import { useNav } from "../state/nav";
import { useTower } from "../state/tower";

/** O mesmo rótulo em todos os pontos (Recuperação, Sala de controle, Sessões, painel do item, paleta). */
export const DISCARD_LABEL = "Descartar";
/** Motivo gravado no evento quando a pessoa escolhe mudar o status. */
export const DISCARD_REASON = "sessão interrompida descartada";

/** Sessão a descartar. `item` = issue no hub (sem ela, o status não muda: gestor, planejamento). */
export interface DiscardTarget {
  env: string;
  session: string;
  /** Nome curto da sessão, como a tela a mostra. */
  name: string;
  item?: { owner: string; repo: string; number: number } | null;
}

/** O que fazer com o work item; o padrão é não mexer. */
export type DiscardOutcome = "keep" | Extract<WorkItemStatus, "concluido" | "backlog">;

export const OUTCOME_TEXT: Record<DiscardOutcome, string> = {
  keep: "Não mexer no item",
  concluido: "Marcar o item como concluído",
  backlog: "Voltar o item para o backlog",
};

/** Resultado de cada sessão: descartada (com ou sem o status) ou o erro. */
export interface DiscardResult {
  target: DiscardTarget;
  ok: boolean;
  /** Erro do arquivamento ou, com `ok`, da mudança de status. */
  error?: string;
}

/** Alvo de uma sessão do dashboard: nome e issue tirados do work item. */
export function discardTarget(env: string, d: Dashboard | undefined, s: Session): DiscardTarget {
  const { wi, project, gestor, scope } = lookup(d, s);
  return { env, session: s.id, name: itemLabel(project, wi, gestor, scope), item: issueRef(project, wi, gestor) };
}

/** Abre a confirmação; `onDone` recebe o resultado de cada sessão depois de executar. */
export type Discard = (
  targets: DiscardTarget | DiscardTarget[],
  opts?: { origin?: Element | null; onDone?: (r: DiscardResult[]) => void },
) => void;

export const DiscardCtx = createContext<Discard | null>(null);

/** Descarta pela confirmação do shell; fora dele, cai na Recuperação (que tem o seu Descartar). */
export function useDiscard(): Discard {
  const open = useContext(DiscardCtx);
  const { go } = useNav();
  const fallback = useCallback<Discard>(
    (t) => {
      const one = Array.isArray(t) ? t[0] : t;
      go(one ? { name: "recovery", env: one.env, session: one.session } : { name: "recovery" });
    },
    [go],
  );
  return open ?? fallback;
}

interface Pending {
  targets: DiscardTarget[];
  origin: Element | null;
  onDone?: (r: DiscardResult[]) => void;
}

export function DiscardHost({ children }: { children: ReactNode }) {
  const [pending, setPending] = useState<Pending | null>(null);
  const open = useCallback<Discard>((t, opts) => {
    const targets = Array.isArray(t) ? t : [t];
    if (targets.length === 0) return;
    setPending({
      targets,
      origin: opts?.origin ?? (document.activeElement as Element | null),
      onDone: opts?.onDone,
    });
  }, []);
  const close = () => {
    const origin = pending?.origin;
    setPending(null);
    if (origin instanceof HTMLElement && origin.isConnected) origin.focus();
  };
  return (
    <DiscardCtx.Provider value={open}>
      {children}
      {pending && <DiscardDialog targets={pending.targets} onDone={pending.onDone} onClose={close} />}
    </DiscardCtx.Provider>
  );
}

const focusables = (root: HTMLElement) =>
  Array.from(
    root.querySelectorAll<HTMLElement>("button:not(:disabled), input:not(:disabled), [tabindex]:not([tabindex='-1'])"),
  );

const plural = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`;

/** "2 descartadas · 1 erro". */
export function discardSummary(results: DiscardResult[]): string {
  const ok = results.filter((r) => r.ok).length;
  const failed = results.length - ok;
  const parts = [plural(ok, "descartada", "descartadas")];
  if (failed) parts.push(plural(failed, "erro", "erros"));
  return parts.join(" · ");
}

/**
 * A confirmação: o aviso (a conversa não é retomada por aqui; o transcript
 * fica em disco e a sessão vai para as descartadas recentemente), a escolha
 * do work item e, num lote ou com erro, o resultado de cada sessão.
 */
function DiscardDialog({
  targets,
  onDone,
  onClose,
}: {
  targets: DiscardTarget[];
  onDone?: (r: DiscardResult[]) => void;
  onClose: () => void;
}) {
  const { api, hub } = useTower();
  const [outcome, setOutcome] = useState<DiscardOutcome>("keep");
  const [busy, setBusy] = useState(false);
  const [results, setResults] = useState<DiscardResult[] | null>(null);
  const dialog = useRef<HTMLElement>(null);
  const many = targets.length > 1;

  // O status muda pelo hub (como no painel do item); se ele não conhece o item, o erro sai no resultado.
  const canStatus = (t: DiscardTarget) => hub.up && !!t.item;
  const withStatus = targets.filter(canStatus).length;
  const statusBlocked =
    withStatus > 0
      ? null
      : !hub.up
        ? "O status do item muda pelo hub, que está fora do ar."
        : many
          ? "Estas sessões não têm issue: não há status para mudar."
          : "Esta sessão não tem issue: não há status para mudar.";

  useEffect(() => {
    dialog.current?.focus();
  }, []);

  const run = async () => {
    setBusy(true);
    const out: DiscardResult[] = [];
    // Uma por vez em cada ambiente; ambientes em paralelo.
    const byEnv = new Map<string, DiscardTarget[]>();
    for (const t of targets) byEnv.set(t.env, [...(byEnv.get(t.env) ?? []), t]);
    await Promise.all(
      [...byEnv.values()].map(async (group) => {
        for (const t of group) {
          try {
            await api.call(t.env, "session.archive", { session_id: t.session });
          } catch (e) {
            out.push({ target: t, ok: false, error: String(e) });
            continue;
          }
          if (outcome !== "keep" && canStatus(t)) {
            try {
              await api.hubCall("hub.workitem.set_status", { ...t.item!, status: outcome, reason: DISCARD_REASON });
            } catch (e) {
              out.push({ target: t, ok: true, error: `o status do item não mudou: ${String(e)}` });
              continue;
            }
          }
          out.push({ target: t, ok: true });
        }
      }),
    );
    // Na ordem em que vieram.
    out.sort((a, b) => targets.indexOf(a.target) - targets.indexOf(b.target));
    setBusy(false);
    onDone?.(out);
    // Uma só, sem erro: fecha. Lote ou erro: mostra o resultado de cada uma.
    if (!many && out.every((r) => r.ok && !r.error)) onClose();
    else setResults(out);
  };

  const onKeyDown = (e: KeyboardEvent) => {
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      if (!busy) onClose();
      return;
    }
    if (e.key === "Tab" && dialog.current) {
      const list = focusables(dialog.current);
      if (list.length === 0) return;
      const first = list[0];
      const last = list[list.length - 1];
      const at = document.activeElement;
      if (e.shiftKey && (at === first || at === dialog.current)) {
        e.preventDefault();
        last.focus();
      } else if (!e.shiftKey && at === last) {
        e.preventDefault();
        first.focus();
      }
    }
  };

  const title = many ? `${DISCARD_LABEL} ${plural(targets.length, "sessão", "sessões")}?` : `${DISCARD_LABEL} a sessão?`;

  return (
    <div className="dc-scrim" onMouseDown={(e) => e.target === e.currentTarget && !busy && onClose()}>
      <section
        ref={dialog}
        className="dc-modal"
        role="alertdialog"
        aria-modal="true"
        aria-labelledby="dc-title"
        aria-describedby="dc-desc"
        tabIndex={-1}
        onKeyDown={onKeyDown}
      >
        <h2 id="dc-title">{results ? `${DISCARD_LABEL}: resultado` : title}</h2>
        {results ? (
          <>
            <p id="dc-desc" role="status">
              {discardSummary(results)}
            </p>
            <ul className="dc-list" aria-label="Resultado de cada sessão">
              {results.map((r) => (
                <li key={`${r.target.env}:${r.target.session}`} data-tone={!r.ok ? "error" : r.error ? "attention" : "done"}>
                  <span className="dc-name">{r.target.name}</span>
                  <span className="dc-state">{!r.ok ? `erro: ${r.error}` : r.error ? `descartada; ${r.error}` : "descartada"}</span>
                </li>
              ))}
            </ul>
            <div className="dc-actions">
              <button type="button" className="btn btn-primary" onClick={onClose}>
                Fechar
              </button>
            </div>
          </>
        ) : (
          <>
            {many ? (
              <ul className="dc-list" aria-label="Sessões a descartar">
                {targets.map((t) => (
                  <li key={`${t.env}:${t.session}`}>
                    <span className="dc-name">{t.name}</span>
                  </li>
                ))}
              </ul>
            ) : (
              <p className="dc-name">{targets[0].name}</p>
            )}
            <p id="dc-desc" className="muted">
              A conversa não será retomada por aqui. O transcript continua em disco e{" "}
              {many ? "as sessões vão" : "a sessão vai"} para as descartadas recentemente da Recuperação.
            </p>
            <fieldset className="dc-outcome" disabled={busy || statusBlocked !== null}>
              <legend>Work item</legend>
              {(Object.keys(OUTCOME_TEXT) as DiscardOutcome[]).map((o) => (
                <label key={o}>
                  <input
                    type="radio"
                    name="dc-outcome"
                    value={o}
                    checked={outcome === o}
                    onChange={() => setOutcome(o)}
                  />
                  {OUTCOME_TEXT[o]}
                </label>
              ))}
              {statusBlocked && <p className="muted dc-note">{statusBlocked}</p>}
              {!statusBlocked && many && withStatus < targets.length && (
                <p className="muted dc-note">
                  O status só muda em {plural(withStatus, "sessão", "sessões")} de {targets.length} (as outras não têm
                  issue).
                </p>
              )}
            </fieldset>
            <div className="dc-actions">
              <button type="button" className="btn btn-danger" disabled={busy} onClick={() => void run()}>
                {busy ? "Descartando…" : many ? `${DISCARD_LABEL} ${targets.length}` : DISCARD_LABEL}
              </button>
              <button type="button" className="btn" disabled={busy} onClick={onClose}>
                Cancelar
              </button>
            </div>
          </>
        )}
      </section>
    </div>
  );
}
