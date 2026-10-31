// Ações só do desktop na web (#26): em vez de sumir, o botão aparece
// desabilitado e o motivo fica visível ao lado, ligado ao botão por
// `aria-describedby` (o leitor de tela lê junto). Só renderiza na web: quem
// usa decide com `useTower().web`.
import "./web.css";
import { useId, type ReactNode } from "react";
import { Icon, type IconName } from "../shell/icons";
import { DESKTOP_ONLY_NOTE } from "./mode";

/**
 * Um ou mais botões desabilitados com um aviso só. `labels` são os nomes
 * dos botões, na ordem; `className`, a classe de cada um (`btn` por padrão).
 */
export function DesktopOnly({
  labels,
  note = DESKTOP_ONLY_NOTE,
  className = "btn",
}: {
  labels: ReactNode | ReactNode[];
  note?: string;
  className?: string;
}) {
  const id = useId();
  const list = Array.isArray(labels) ? labels : [labels];
  return (
    <span className="desktop-only">
      {list.map((label, i) => (
        <button key={i} type="button" className={className} disabled aria-describedby={id}>
          {label}
        </button>
      ))}
      <span id={id} className="muted desktop-only-note">
        {note}
      </span>
    </span>
  );
}

/**
 * Botão de ícone (`ibtn`) dos cabeçalhos do painel lateral: sem lugar para
 * o aviso visível, o motivo vai no `title` e na descrição acessível.
 */
export function DesktopOnlyIcon({ label, icon, note = DESKTOP_ONLY_NOTE }: { label: string; icon: IconName; note?: string }) {
  const id = useId();
  return (
    <>
      <button
        type="button"
        className="ibtn desktop-only-icon"
        aria-label={label}
        aria-describedby={id}
        title={`${label}: ${note.charAt(0).toLowerCase()}${note.slice(1)}`}
        disabled
      >
        <Icon name={icon} size={15} />
      </button>
      <span id={id} className="sr-only">
        {note}
      </span>
    </>
  );
}
