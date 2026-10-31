// Ícones de traço do shell (os do mockup), na cor do texto (`currentColor`).
const PATHS = {
  control: (
    <>
      <circle cx="12" cy="12" r="9" />
      <circle cx="12" cy="12" r="3.5" />
      <path d="M12 3v5M12 16v5M3 12h5M16 12h5" />
    </>
  ),
  terminal: (
    <>
      <rect x="3" y="4" width="18" height="16" rx="2" />
      <path d="M7 9l3 3-3 3M13 15h4" />
    </>
  ),
  project: <path d="M3 6h6l2 2h10v11H3z" />,
  review: (
    <>
      <circle cx="6" cy="6" r="2" />
      <circle cx="6" cy="18" r="2" />
      <circle cx="18" cy="18" r="2" />
      <path d="M6 8v8M18 16V9a3 3 0 0 0-3-3h-4M13 4l-2 2 2 2" />
    </>
  ),
  recovery: (
    <>
      <circle cx="12" cy="12" r="9" />
      <circle cx="12" cy="12" r="4" />
      <path d="M5.6 5.6l3.6 3.6M14.8 14.8l3.6 3.6M18.4 5.6l-3.6 3.6M9.2 14.8l-3.6 3.6" />
    </>
  ),
  hub: (
    <>
      <rect x="3" y="4" width="18" height="7" rx="1.5" />
      <rect x="3" y="13" width="18" height="7" rx="1.5" />
      <path d="M7 7.5h.01M7 16.5h.01" />
    </>
  ),
  /** Chave inglesa (Manutenção). */
  maintenance: (
    <path d="M14.7 6.3a1 1 0 0 0 0 1.4l1.6 1.6a1 1 0 0 0 1.4 0l3.8-3.8a6 6 0 0 1-7.9 7.9l-6.9 6.9a2.1 2.1 0 0 1-3-3l6.9-6.9a6 6 0 0 1 7.9-7.9z" />
  ),
  settings: (
    <>
      <circle cx="12" cy="12" r="3" />
      <path d="M12 2v3M12 19v3M2 12h3M19 12h3M4.9 4.9l2.1 2.1M17 17l2.1 2.1M4.9 19.1L7 17M17 7l2.1-2.1" />
    </>
  ),
  search: (
    <>
      <circle cx="11" cy="11" r="7" />
      <path d="M20 20l-4-4" />
    </>
  ),
  sync: <path d="M20 11a8 8 0 0 0-14-5l-2 2M4 13a8 8 0 0 0 14 5l2-2M4 4v4h4M20 20v-4h-4" />,
  bell: (
    <>
      <path d="M6 16v-5a6 6 0 0 1 12 0v5l2 2H4z" />
      <path d="M10 20a2 2 0 0 0 4 0" />
    </>
  ),
  close: <path d="M6 6l12 12M18 6L6 18" />,
  /** Alfinete (fixar aba); de pé = fixada, inclinado (CSS) = provisória. */
  pin: <path d="M9 4h6M10 4v6l-3 4h10l-3-4V4M12 14v6" />,
  chevronDown: <path d="M6 9l6 6 6-6" />,
  chevronRight: <path d="M9 6l6 6-6 6" />,
  sidebar: (
    <>
      <rect x="3" y="4" width="18" height="16" rx="2" />
      <path d="M9 4v16" />
    </>
  ),
  warning: (
    <>
      <path d="M12 3l10 18H2z" />
      <path d="M12 10v5M12 18h.01" />
    </>
  ),
  plus: <path d="M12 5v14M5 12h14" />,
  /** Sair (sessão web, #201). */
  logout: <path d="M14 4h5v16h-5M10 8l-4 4 4 4M6 12h10" />,
  collapseAll: <path d="M7 15l5-5 5 5" />,
  expandAll: <path d="M7 9l5 5 5-5" />,
  split: (
    <>
      <rect x="3" y="4" width="18" height="16" rx="2" />
      <path d="M12 4v16" />
    </>
  ),
  /** Prompt (`>_`): shell livre no worktree (#423). */
  prompt: <path d="M4 7l5 5-5 5M12 18h8" />,
  stop: <rect x="6" y="6" width="12" height="12" rx="1.5" />,
  code: <path d="M8 7l-5 5 5 5M16 7l5 5-5 5" />,
  pr: (
    <>
      <circle cx="6" cy="6" r="2" />
      <circle cx="6" cy="18" r="2" />
      <circle cx="18" cy="18" r="2" />
      <path d="M6 8v8M18 16V9a3 3 0 0 0-3-3h-4M13 4l-2 2 2 2" />
    </>
  ),
  /** Provedores do projeto (#352): traço simples, na cor do texto. */
  github: (
    <path d="M9 19c-5 1.5-5-2.5-7-3m14 6v-3.9a3.4 3.4 0 0 0-.9-2.6c3.1-.4 6.4-1.5 6.4-7A5.4 5.4 0 0 0 20 4.8 5 5 0 0 0 19.9 1S18.7.7 16 2.5a13.4 13.4 0 0 0-7 0C6.3.7 5.1 1 5.1 1A5 5 0 0 0 5 4.8a5.4 5.4 0 0 0-1.5 3.7c0 5.5 3.3 6.6 6.4 7a3.4 3.4 0 0 0-.9 2.6V22" />
  ),
  gitlab: (
    <path d="M22.6 14.4L12 22.1 1.4 14.4a.8.8 0 0 1-.3-.9l1.2-3.8 2.4-7.5a.4.4 0 0 1 .8 0l2.4 7.5h8.2l2.4-7.5a.4.4 0 0 1 .8 0l2.4 7.5 1.2 3.8a.8.8 0 0 1-.3.9z" />
  ),
  jira: (
    <>
      <path d="M12 3l9 9-9 9-9-9z" />
      <path d="M12 8.5l3.5 3.5-3.5 3.5-3.5-3.5z" />
    </>
  ),
  /** Gaveta do painel lateral no celular (#202). */
  menu: <path d="M4 7h16M4 12h16M4 17h16" />,
  /** Lista de abas no cabeçalho do celular (#202). */
  chevron: <path d="M6 9l6 6 6-6" />,
  /** Celular (Remote Control do Claude Code, #197). */
  remote: (
    <>
      <rect x="7" y="2.5" width="10" height="19" rx="2" />
      <path d="M11 18.5h2" />
    </>
  ),
  /** Enviar prompt pronto ao agente (#425). */
  send: <path d="M21 3L10 14M21 3l-7 18-4-7-7-4z" />,
  /** Painel de contexto à direita do console (#426). */
  context: (
    <>
      <rect x="3" y="4" width="18" height="16" rx="2" />
      <path d="M15 4v16" />
    </>
  ),
  /** Menu ⋯ das ações secundárias do cabeçalho do console (#424). */
  /** Fechar várias abas (#427): duas janelas, a da frente com um ×. */
  closeAll: <path d="M8 5h11v11M5 8h11v11H5zM8 11l5 5M13 11l-5 5" />,
  more: (
    <>
      <circle cx="6" cy="12" r="1" fill="currentColor" />
      <circle cx="12" cy="12" r="1" fill="currentColor" />
      <circle cx="18" cy="12" r="1" fill="currentColor" />
    </>
  ),
} as const;

export type IconName = keyof typeof PATHS;

export function Icon({ name, size = 16 }: { name: IconName; size?: number }) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.8"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
      data-icon={name}
    >
      {PATHS[name]}
    </svg>
  );
}
