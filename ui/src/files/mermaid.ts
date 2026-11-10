// Diagramas Mermaid do visualizador de markdown (#167). O pacote é grande:
// carrega na primeira vez que um documento tem diagrama (import dinâmico,
// chunk à parte no build). Configuração segura: `securityLevel: "strict"`
// (sem clique nem script do diagrama), rótulos em SVG puro (sem HTML), e o
// SVG ainda passa pelo DOMPurify antes de ir para a tela.
import DOMPurify from "dompurify";
import type { Theme } from "../state/theme";

type Mermaid = (typeof import("mermaid"))["default"];

let loading: Promise<Mermaid> | null = null;
/** Mermaid é global (config e ids): um diagrama de cada vez. */
let queue: Promise<unknown> = Promise.resolve();
let seq = 0;

function load(): Promise<Mermaid> {
  loading ??= import("mermaid").then((m) => m.default);
  // Falhou (offline, chunk ausente): a próxima tentativa carrega de novo.
  loading.catch(() => (loading = null));
  return loading;
}

/** Configuração passada ao `initialize` (exportada para o teste). */
export function mermaidConfig(theme: Theme) {
  return {
    startOnLoad: false,
    securityLevel: "strict" as const,
    theme: theme === "dark" ? ("dark" as const) : ("default" as const),
    // Erro de sintaxe não desenha a "bomba" no documento: volta como exceção.
    suppressErrorRendering: true,
    htmlLabels: false,
    flowchart: { htmlLabels: false },
    // Nada de diretiva `%%{init}%%` mudando a segurança ou o tema.
    secure: ["secure", "securityLevel", "startOnLoad", "maxTextSize", "htmlLabels", "theme", "themeCSS", "fontFamily"],
    maxTextSize: 50_000,
  };
}

/** SVG do Mermaid sanitizado: só SVG (sem `foreignObject`, script nem evento). */
export function sanitizeSvg(svg: string): string {
  return DOMPurify.sanitize(svg, {
    USE_PROFILES: { svg: true, svgFilters: true },
    ADD_TAGS: ["style"],
    FORBID_TAGS: ["foreignObject", "script", "a"],
  });
}

export type DiagramResult = { svg: string } | { error: string };

/** Desenha `code` no tema `theme`; erro de sintaxe vira `{ error }`. */
export function renderDiagram(code: string, theme: Theme): Promise<DiagramResult> {
  const run = async (): Promise<DiagramResult> => {
    const id = `ct-mermaid-${++seq}`;
    try {
      const mermaid = await load();
      mermaid.initialize(mermaidConfig(theme));
      await mermaid.parse(code);
      const { svg } = await mermaid.render(id, code);
      return { svg: sanitizeSvg(svg) };
    } catch (e) {
      return { error: e instanceof Error ? e.message : String(e) };
    } finally {
      // O render deixa um contêiner temporário no body quando falha.
      document.getElementById(id)?.remove();
      document.getElementById(`d${id}`)?.remove();
    }
  };
  const next = queue.then(run, run);
  queue = next;
  return next;
}
