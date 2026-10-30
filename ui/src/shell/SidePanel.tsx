// Painel lateral do shell: componentes de layout que cada tela usa para
// preencher o painel (cabeçalho, grupos recolhíveis, linhas com ação inline)
// e o encaixe (`SideContent`) pelo qual a tela manda o conteúdo para lá.
import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useId,
  useMemo,
  useState,
  type ReactNode,
} from "react";
import { createPortal } from "react-dom";
import { Icon } from "./icons";

interface SideSlot {
  node: HTMLElement | null;
  /** Uma tela passou a fornecer o conteúdo; devolve a liberação. */
  claim(): () => void;
}

const SlotCtx = createContext<SideSlot | null>(null);

/** Encaixe do painel lateral; `filled` = alguma tela fornece o conteúdo. */
export function useSideSlot(): { slot: SideSlot; filled: boolean; ref: (n: HTMLElement | null) => void } {
  const [node, setNode] = useState<HTMLElement | null>(null);
  const [claims, setClaims] = useState(0);
  const claim = useCallback(() => {
    setClaims((n) => n + 1);
    return () => setClaims((n) => n - 1);
  }, []);
  const slot = useMemo(() => ({ node, claim }), [node, claim]);
  return { slot, filled: claims > 0, ref: setNode };
}

export function SideSlotProvider({ slot, children }: { slot: SideSlot; children: ReactNode }) {
  return <SlotCtx.Provider value={slot}>{children}</SlotCtx.Provider>;
}

/**
 * Conteúdo do painel lateral fornecido pela tela: renderizado no painel,
 * mas no componente da tela (o estado continua com ela). Enquanto montado,
 * substitui o conteúdo padrão do shell. Fora do shell (testes de uma tela
 * sozinha), não mostra nada.
 */
export function SideContent({ children }: { children: ReactNode }) {
  const slot = useContext(SlotCtx);
  const claim = slot?.claim;
  useEffect(() => claim?.(), [claim]);
  if (!slot?.node) return null;
  return createPortal(children, slot.node);
}

/** Painel com cabeçalho (título e ações), corpo rolável e rodapé opcional (preso embaixo). */
export function SidePanel({
  title,
  label,
  actions,
  footer,
  children,
}: {
  title: string;
  /** Nome do painel como região complementar (`<aside>`), ex.: "Sessões por projeto". */
  label?: string;
  actions?: ReactNode;
  footer?: ReactNode;
  children: ReactNode;
}) {
  const Root = label ? "aside" : "div";
  return (
    <Root className="sp" aria-label={label}>
      <div className="sp-head">
        <span className="sp-title">{title}</span>
        {actions && <div className="sp-actions">{actions}</div>}
      </div>
      <div className="sp-body">{children}</div>
      {footer && <div className="sp-foot">{footer}</div>}
    </Root>
  );
}

/** Grupo recolhível do painel lateral. */
export function SideGroup({
  title,
  count,
  actions,
  defaultOpen = true,
  children,
}: {
  title: string;
  count?: number;
  actions?: ReactNode;
  defaultOpen?: boolean;
  children: ReactNode;
}) {
  const [open, setOpen] = useState(defaultOpen);
  const id = useId();
  return (
    <section className="sp-group" aria-labelledby={`${id}-t`}>
      <div className="sp-group-head">
        <button
          type="button"
          id={`${id}-t`}
          className="sp-group-toggle"
          aria-expanded={open}
          aria-controls={`${id}-b`}
          onClick={() => setOpen(!open)}
        >
          <Icon name={open ? "chevronDown" : "chevronRight"} size={14} />
          {title}
          {count !== undefined && <span className="sp-group-count">{count}</span>}
        </button>
        {actions && <div className="sp-actions">{actions}</div>}
      </div>
      <ul className="sp-rows" id={`${id}-b`} hidden={!open}>
        {children}
      </ul>
    </section>
  );
}

/**
 * Linha do painel lateral: um botão (seleciona) e, opcionalmente, ações
 * inline à direita (aparecem ao passar o mouse ou focar a linha).
 */
export function SideRow({
  label,
  sub,
  ariaLabel,
  description,
  icon,
  tone,
  trailing,
  selected,
  depth = 0,
  onClick,
  onMenu,
  actions,
}: {
  label: ReactNode;
  /** Segunda linha, menor (ex.: "PR #36 → develop"); a linha fica com duas alturas. */
  sub?: ReactNode;
  ariaLabel?: string;
  /** Descrição acessível (ex.: o contador "2 pendentes"), fora do nome. */
  description?: string;
  icon?: ReactNode;
  /** Ponto de estado à esquerda (attention, running, done, error, neutral). */
  tone?: string;
  trailing?: ReactNode;
  selected?: boolean;
  depth?: number;
  onClick?: () => void;
  /**
   * Menu de contexto da linha: botão direito na linha ou Shift+F10 / tecla de
   * menu no botão. Recebe a posição e o botão (para devolver o foco).
   */
  onMenu?: (x: number, y: number, origin: HTMLElement) => void;
  actions?: ReactNode;
}) {
  const id = useId();
  return (
    <li
      className={sub !== undefined ? "sp-row sp-row-2" : "sp-row"}
      data-selected={selected ? "true" : undefined}
      style={{ paddingLeft: 18 + depth * 16 }}
      onContextMenu={
        onMenu &&
        ((e) => {
          e.preventDefault();
          const el = e.currentTarget.querySelector<HTMLElement>(".sp-row-main") ?? e.currentTarget;
          onMenu(e.clientX, e.clientY, el);
        })
      }
    >
      <button
        type="button"
        className="sp-row-main"
        aria-label={ariaLabel}
        aria-current={selected ? "true" : undefined}
        aria-describedby={description ? `${id}-d` : undefined}
        onClick={onClick}
        onKeyDown={
          onMenu &&
          ((e) => {
            if (e.key !== "ContextMenu" && !(e.key === "F10" && e.shiftKey)) return;
            e.preventDefault();
            const r = e.currentTarget.getBoundingClientRect();
            onMenu(r.left + 16, r.bottom, e.currentTarget);
          })
        }
      >
        {tone && <span className="sp-dot" data-tone={tone} aria-hidden="true" />}
        {icon}
        {sub !== undefined ? (
          <span className="sp-row-text">
            <span className="sp-row-label">{label}</span>
            <span className="sp-row-sub">{sub}</span>
          </span>
        ) : (
          <span className="sp-row-label">{label}</span>
        )}
        {trailing !== undefined && (
          <span className="sp-row-trailing" aria-hidden={description ? true : undefined}>
            {trailing}
          </span>
        )}
      </button>
      {description && (
        <span id={`${id}-d`} hidden>
          {description}
        </span>
      )}
      {actions && <span className="sp-row-actions">{actions}</span>}
    </li>
  );
}
