// Seletor de status do work item (tela Projeto, #317), também no painel de
// contexto do console (#426): botão "Mudar status ▾" com o motivo opcional e
// as opções; o foco vai para o status atual e Escape devolve ao botão.
import { useEffect, useId, useRef, useState } from "react";
import type { WorkItemStatus } from "../protocol/gen";
import { STATUS } from "./status";

export const STATUS_ORDER: WorkItemStatus[] = [
  "backlog",
  "em_execucao",
  "aguardando_voce",
  "em_revisao",
  "aguardando_epico",
  "concluido",
  "bloqueado",
  "pausado",
];

/**
 * `onSet` muda o status (no hub) e devolve `true` quando deu certo: o menu
 * fecha e o motivo é limpo. O erro fica com quem chama.
 */
export function StatusMenu({
  current,
  enabled,
  busy,
  describedBy,
  placeholder,
  className = "btn",
  subIssue,
  onSet,
}: {
  current: WorkItemStatus;
  /**
   * O item é sub-issue: só então "Aguardando o épico" (#457) aparece entre as
   * opções (ou quando já é o status atual).
   */
  subIssue: boolean;
  /** Sem o hub, o botão fica desabilitado. */
  enabled: boolean;
  busy: boolean;
  /** Id do aviso de por que está desabilitado. */
  describedBy?: string;
  placeholder?: string;
  className?: string;
  onSet: (status: WorkItemStatus, reason: string | undefined) => Promise<boolean>;
}) {
  const [menu, setMenu] = useState(false);
  const [reason, setReason] = useState("");
  const btn = useRef<HTMLButtonElement>(null);
  const currentOpt = useRef<HTMLButtonElement>(null);
  const id = useId();
  const pop = `${id}-pop`;
  const input = `${id}-reason`;

  // Ao abrir, o foco vai para o status atual.
  useEffect(() => {
    if (menu) currentOpt.current?.focus();
  }, [menu]);

  // "Aguardando o épico" só faz sentido para sub-issue (épico #456).
  const options = STATUS_ORDER.filter((k) => k !== "aguardando_epico" || subIssue || current === k);

  const pick = async (k: WorkItemStatus) => {
    if (k === current) {
      setMenu(false);
      return;
    }
    if (await onSet(k, reason.trim() || undefined)) {
      setMenu(false);
      setReason("");
    }
  };

  return (
    <div
      className="status-menu"
      onKeyDown={(e) => {
        if (e.key === "Escape" && menu) {
          setMenu(false);
          btn.current?.focus();
        }
      }}
    >
      <button
        ref={btn}
        type="button"
        className={className}
        aria-controls={pop}
        aria-expanded={menu}
        disabled={!enabled}
        aria-describedby={enabled ? undefined : describedBy}
        onClick={() => setMenu((m) => !m)}
      >
        Mudar status ▾
      </button>
      {menu && enabled && (
        <div id={pop} className="status-pop" role="group" aria-label="Mudar status">
          <label htmlFor={input}>Motivo (opcional)</label>
          <input
            id={input}
            type="text"
            value={reason}
            placeholder={placeholder}
            onChange={(e) => setReason(e.target.value)}
          />
          {options.map((k) => (
            <button
              key={k}
              type="button"
              ref={k === current ? currentOpt : undefined}
              className="opt"
              disabled={busy}
              aria-current={k === current ? "true" : undefined}
              onClick={() => void pick(k)}
            >
              <span>{STATUS[k].text}</span>
              {k === current && <span className="muted">atual</span>}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
