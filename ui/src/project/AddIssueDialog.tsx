import "./add-issue.css";
import { useEffect, useId, useRef, useState, type FormEvent, type KeyboardEvent } from "react";
import type { TrackerIssueAdded } from "../protocol/gen";
import type { ProviderProject } from "../util/item";
import { ADD_ISSUE_PRUNE_NOTE, checkIssueKey, errorText } from "./add-issue";

const focusables = (root: HTMLElement) =>
  Array.from(
    root.querySelectorAll<HTMLElement>("button:not(:disabled), input:not(:disabled), [tabindex]:not([tabindex='-1'])"),
  );

/**
 * Diálogo do "Adicionar issue pelo ID" (#150): o campo da chave, conferida
 * antes do envio (`checkIssueKey`), o aviso de que sem work item a issue sai
 * do espelho na próxima reconciliação, e o erro do ambiente no próprio
 * diálogo. Desabilitado (`reason`), mostra o motivo e não envia. Deu certo
 * (ou a issue já estava no espelho), `onAdded` abre o item; se um pai não foi
 * lido, o aviso fica no diálogo com o "Abrir" à mão.
 */
export function AddIssueDialog({
  project,
  reason,
  submit,
  onAdded,
  onClose,
}: {
  project: ProviderProject | null;
  /** Motivo de não poder adicionar agora (sem ambiente de sync, desconectado); `null` = pode. */
  reason: string | null;
  /** Chama o ambiente (ou o hub) com a chave conferida. */
  submit: (key: string) => Promise<TrackerIssueAdded>;
  /** Abre o item adicionado (e fecha o diálogo). */
  onAdded: (r: TrackerIssueAdded) => void | Promise<void>;
  onClose: () => void;
}) {
  const [value, setValue] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [warned, setWarned] = useState<TrackerIssueAdded | null>(null);
  const dialog = useRef<HTMLElement>(null);
  const input = useRef<HTMLInputElement>(null);
  const ids = useId();
  const pk = project?.tracker?.kind === "jira" ? project.tracker.project_key : null;
  const off = reason !== null;

  useEffect(() => {
    const before = document.activeElement as HTMLElement | null;
    (input.current && !input.current.disabled ? input.current : dialog.current)?.focus();
    return () => {
      if (before?.isConnected) before.focus();
    };
  }, []);

  // Com erro, o foco volta ao campo para corrigir (já habilitado).
  useEffect(() => {
    if (error !== null) input.current?.focus();
  }, [error]);

  const send = async (e: FormEvent) => {
    e.preventDefault();
    if (off || busy) return;
    const checked = checkIssueKey(value, pk);
    if ("error" in checked) {
      setError(checked.error);
      return;
    }
    setBusy(true);
    setError(null);
    let r: TrackerIssueAdded;
    try {
      r = await submit(checked.key);
    } catch (err) {
      setBusy(false);
      setError(errorText(err));
      return;
    }
    setBusy(false);
    // Um pai não foi lido: a issue entrou, mas o aviso fica à vista.
    if (r.warning) setWarned(r);
    else await onAdded(r);
  };

  const onKeyDown = (e: KeyboardEvent) => {
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      if (!busy) onClose();
      return;
    }
    if (e.key === "Tab" && dialog.current) {
      // Foco preso: do último volta ao primeiro (e vice-versa com Shift).
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

  return (
    <div className="ai-scrim" onMouseDown={(e) => e.target === e.currentTarget && !busy && onClose()}>
      <section
        ref={dialog}
        className="ai-modal"
        role="dialog"
        aria-modal="true"
        aria-labelledby={`${ids}-title`}
        tabIndex={-1}
        onKeyDown={onKeyDown}
      >
        <h2 id={`${ids}-title`}>Adicionar issue pelo ID</h2>
        {warned ? (
          <>
            <p role="status">
              {warned.key} {warned.title} {warned.already ? "já estava no espelho." : "foi adicionada ao espelho."}
            </p>
            <p role="alert" className="ai-error">
              {warned.warning}
            </p>
            {!warned.work_item && <p className="muted ai-note">{ADD_ISSUE_PRUNE_NOTE}</p>}
            <div className="ai-actions">
              <button type="button" className="btn btn-primary" onClick={() => void onAdded(warned)}>
                Abrir {warned.key}
              </button>
              <button type="button" className="btn" onClick={onClose}>
                Fechar
              </button>
            </div>
          </>
        ) : (
          <form className="ai-form" aria-labelledby={`${ids}-title`} noValidate onSubmit={(e) => void send(e)}>
            <div className="field">
              <label htmlFor={`${ids}-key`}>Chave da issue no Jira</label>
              <input
                ref={input}
                id={`${ids}-key`}
                type="text"
                autoComplete="off"
                spellCheck={false}
                placeholder={`${pk ?? "ACME"}-123, só o número ou o link`}
                value={value}
                disabled={off || busy}
                aria-invalid={error !== null || undefined}
                aria-describedby={`${ids}-note`}
                onChange={(e) => {
                  setValue(e.target.value);
                  setError(null);
                }}
              />
            </div>
            <p id={`${ids}-note`} className="muted ai-note">
              Traz ao Control Tower uma issue que está fora do filtro (JQL) do projeto, com o épico dela.{" "}
              {ADD_ISSUE_PRUNE_NOTE}
            </p>
            {off && (
              <p role="status" className="ai-error">
                {reason}
              </p>
            )}
            {error && (
              <p role="alert" className="ai-error">
                {error}
              </p>
            )}
            <div className="ai-actions">
              <button type="submit" className="btn btn-primary" disabled={off || busy}>
                {busy ? "Adicionando…" : "Adicionar"}
              </button>
              <button type="button" className="btn" disabled={busy} onClick={onClose}>
                Cancelar
              </button>
            </div>
          </form>
        )}
      </section>
    </div>
  );
}
