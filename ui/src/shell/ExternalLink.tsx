import type { ReactNode } from "react";
import { useTower } from "../state/tower";

/**
 * Link para fora do app (PR, repositório). O clique abre pelo `openUrl`: o
 * navegador do sistema no app desktop, onde o `target="_blank"` da webview
 * não abre nada, e uma aba nova na web. O `href` fica para copiar o link
 * (botão direito).
 */
export function ExternalLink({ href, className, children }: { href: string; className?: string; children: ReactNode }) {
  const { api } = useTower();
  return (
    <a
      href={href}
      className={className}
      target="_blank"
      rel="noreferrer"
      onClick={(e) => {
        e.preventDefault();
        void api.openUrl(href).catch(() => {});
      }}
    >
      {children}
    </a>
  );
}
