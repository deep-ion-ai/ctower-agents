// Folha de atalhos e gestos (#27): o que existe de teclado e de gesto em
// cada tela, como dado. Função pura (testada em shortcuts.test.ts); cada
// linha corresponde a um tratador real (`shellShortcut`, `keyAction`,
// `useProjectOrder`…). No celular (web abaixo de 768 px), só os gestos.
import { isMac } from "../terminal/links";
import type { ShortcutKey } from "./layout";

export interface ShortcutRow {
  /** Teclas (cada uma vira um `<kbd>`) ou o gesto, em texto. */
  keys: string[];
  /** É gesto (toque, clique, arrasto), não tecla. */
  gesture?: boolean;
  text: string;
}

export interface ShortcutGroup {
  title: string;
  /** Observação curta sob o título (ex.: quando o atalho não vale). */
  note?: string;
  rows: ShortcutRow[];
}

export interface ShortcutContext {
  /** Modo web (#201): sem Terminal e sem reordenar projetos. */
  web: boolean;
  /** Celular: só os gestos de toque. */
  mobile: boolean;
  mac?: boolean;
}

/**
 * `?` abre a folha (fora de campos de texto e do console). Aceita AltGr
 * (Ctrl+Alt), com que alguns layouts digitam o `?`; Ctrl, Alt ou ⌘ sozinhos, não.
 */
export function isHelpKey(e: ShortcutKey & { target?: EventTarget | null; repeat?: boolean }): boolean {
  if (e.key !== "?" || e.metaKey || e.ctrlKey !== e.altKey || e.repeat) return false;
  return !(e.target instanceof Element && e.target.closest(TYPING));
}

/** Onde o `?` é texto: campos, editáveis e o console (o xterm digita num textarea). */
const TYPING = 'input, textarea, select, [contenteditable]:not([contenteditable="false"]), .xterm';

/** Gestos do celular: o que o toque faz no shell da web (#202). */
const MOBILE: ShortcutGroup[] = [
  {
    title: "Geral",
    rows: [
      { keys: ["Tocar em ☰"], gesture: true, text: "Abre o painel lateral; tocar fora fecha" },
      { keys: ["Tocar no título"], gesture: true, text: "Lista as abas abertas, para trocar ou fechar" },
      { keys: ["Tocar na lupa"], gesture: true, text: "Busca sessões, issues, projetos e comandos" },
      { keys: ["Tocar no sino"], gesture: true, text: "Mostra os avisos" },
      { keys: ["Tocar na tela atual"], gesture: true, text: "Na barra inferior, mostra ou recolhe o painel lateral" },
    ],
  },
  {
    title: "Listas",
    rows: [{ keys: ["Tocar em ⋯"], gesture: true, text: "Mais ações da linha (projeto, sessão)" }],
  },
];

export function shortcutGroups({ web, mobile, mac = isMac() }: ShortcutContext): ShortcutGroup[] {
  if (mobile) return MOBILE;
  const mod = mac ? "⌘" : "Ctrl";
  const alt = mac ? "⌥" : "Alt";
  // "⌘⌥B" no macOS (como `shortcutLabel`), "Ctrl+Alt+B" fora dele.
  const k = (...keys: string[]) => keys.join(mac ? "" : "+");
  const menu: ShortcutRow = { keys: ["Shift+F10"], text: "Menu de contexto do item com o foco (ou a tecla Menu)" };

  const groups: ShortcutGroup[] = [
    {
      title: "Geral",
      note: mac ? undefined : "No console, Ctrl+B, Ctrl+J e Ctrl+K ficam com o terminal.",
      rows: [
        { keys: ["?"], text: "Esta folha de atalhos" },
        { keys: [k(mod, "K")], text: "Paleta de comandos" },
        { keys: [k(mod, "B")], text: "Mostra ou recolhe o painel lateral" },
        { keys: [k(mod, "J")], text: "Mostra ou recolhe o painel inferior" },
        { keys: ["Esc"], text: "Fecha a paleta, o menu ou a janela aberta" },
      ],
    },
    {
      title: "Paleta de comandos",
      rows: [
        { keys: ["↑", "↓"], text: "Navega nos resultados (PgUp/PgDn de 8 em 8)" },
        { keys: ["Enter"], text: "Abre o resultado" },
        { keys: ["#"], text: "No início da busca: só issues" },
        { keys: [">"], text: "No início da busca: só comandos" },
      ],
    },
    {
      title: "Abas e painel lateral",
      rows: [
        { keys: ["←", "→"], text: "Troca de aba (com o foco na faixa de abas)" },
        menu,
        { keys: ["Clique do meio"], gesture: true, text: "Fecha a aba" },
        { keys: ["Duplo clique"], gesture: true, text: "Fixa a aba provisória" },
        { keys: ["Arrastar a divisória"], gesture: true, text: "Muda a largura do painel lateral (ou ←/→ nela, Shift = passo maior)" },
      ],
    },
  ];

  if (!web) {
    groups.push(
      {
        title: "Terminal",
        rows: [
          { keys: ["Ctrl+Shift+`"], text: "Shell novo no worktree da sessão ativa" },
          { keys: [k(mod, alt, "B")], text: "Mostra ou recolhe o painel de contexto da sessão" },
          { keys: [k(mod, "F")], text: "Busca no console (Enter e Shift+Enter: próximo e anterior)" },
          { keys: [mac ? "⌘C" : "Ctrl+C"], text: mac ? "Copia a seleção" : "Copia a seleção (sem seleção, é ^C para o agente)" },
          { keys: [k(mod, "V")], text: "Cola" },
          { keys: ["Shift+Enter"], text: "Nova linha sem enviar" },
          { keys: [k(mod, "+"), k(mod, "-"), k(mod, "0")], text: "Aumenta, diminui e restaura a fonte" },
          { keys: ["Clique direito"], gesture: true, text: "Copia a seleção (ou o link); sem seleção, cola" },
          { keys: [`${mod}+clique`], gesture: true, text: "Abre o link" },
        ],
      },
      {
        title: "Árvore de sessões",
        rows: [
          { keys: ["↑", "↓"], text: "Move entre as linhas (Home e End: primeira e última)" },
          { keys: ["←", "→"], text: "Recolhe e expande o grupo" },
          { keys: ["Enter"], text: "Abre a sessão" },
          menu,
        ],
      },
    );
  }

  groups.push({
    title: "Projetos",
    rows: [
      ...(web
        ? []
        : [
            { keys: [k(alt, "↑"), k(alt, "↓")], text: "Sobe e desce o projeto na prioridade" },
            { keys: ["Arrastar a linha"], gesture: true, text: "Muda a prioridade do projeto" },
          ]),
      menu,
    ],
  });

  groups.push({
    title: "Iniciar sessão",
    rows: [{ keys: [k(mod, "Enter")], text: "Inicia a sessão" }],
  });
  return groups;
}
