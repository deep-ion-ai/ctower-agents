// Realce de sintaxe do visualizador de diff (#168): highlight.js (núcleo e
// só as linguagens de `languageOf`), carregado por import dinâmico na
// primeira vez que um patch de linguagem conhecida aparece (chunk à parte
// no build). Linha a linha: um comentário de várias linhas perde a cor nas
// do meio, mas as linhas soltas da visão lado a lado continuam alinhadas.
// O highlight.js escapa o texto e só emite `<span class="hljs-…">`; mesmo
// assim o HTML passa pelo DOMPurify, só com <span class>.
import DOMPurify from "dompurify";

type Hljs = (typeof import("highlight.js/lib/core"))["default"];

let loading: Promise<Hljs> | null = null;

async function build(): Promise<Hljs> {
  const [core, ...langs] = await Promise.all([
    import("highlight.js/lib/core"),
    import("highlight.js/lib/languages/rust"),
    import("highlight.js/lib/languages/typescript"),
    import("highlight.js/lib/languages/javascript"),
    import("highlight.js/lib/languages/json"),
    import("highlight.js/lib/languages/css"),
    import("highlight.js/lib/languages/scss"),
    import("highlight.js/lib/languages/xml"),
    import("highlight.js/lib/languages/markdown"),
    import("highlight.js/lib/languages/python"),
    import("highlight.js/lib/languages/go"),
    import("highlight.js/lib/languages/java"),
    import("highlight.js/lib/languages/kotlin"),
    import("highlight.js/lib/languages/bash"),
    import("highlight.js/lib/languages/yaml"),
    import("highlight.js/lib/languages/ini"),
    import("highlight.js/lib/languages/sql"),
    import("highlight.js/lib/languages/c"),
    import("highlight.js/lib/languages/cpp"),
    import("highlight.js/lib/languages/csharp"),
    import("highlight.js/lib/languages/ruby"),
    import("highlight.js/lib/languages/php"),
    import("highlight.js/lib/languages/swift"),
    import("highlight.js/lib/languages/powershell"),
    import("highlight.js/lib/languages/dockerfile"),
    import("highlight.js/lib/languages/makefile"),
  ]);
  const names = [
    "rust",
    "typescript",
    "javascript",
    "json",
    "css",
    "scss",
    "xml",
    "markdown",
    "python",
    "go",
    "java",
    "kotlin",
    "bash",
    "yaml",
    "ini",
    "sql",
    "c",
    "cpp",
    "csharp",
    "ruby",
    "php",
    "swift",
    "powershell",
    "dockerfile",
    "makefile",
  ];
  const hljs = core.default.newInstance();
  langs.forEach((m, i) => hljs.registerLanguage(names[i], m.default));
  return hljs;
}

function load(): Promise<Hljs> {
  loading ??= build();
  // Falhou (offline, chunk ausente): a próxima tentativa carrega de novo.
  loading.catch(() => (loading = null));
  return loading;
}

/** Só `<span class>`: o que o highlight.js emite. */
export const sanitizeHighlight = (html: string) =>
  DOMPurify.sanitize(html, { ALLOWED_TAGS: ["span"], ALLOWED_ATTR: ["class"] });

/**
 * HTML realçado de cada linha (na mesma ordem), ou `null` se a linguagem
 * não está registrada ou o carregamento falhou (o texto aparece sem cor).
 */
export async function highlightLines(lines: string[], language: string): Promise<string[] | null> {
  let hljs: Hljs;
  try {
    hljs = await load();
  } catch {
    return null;
  }
  if (!hljs.getLanguage(language)) return null;
  return lines.map((l) => sanitizeHighlight(hljs.highlight(l, { language, ignoreIllegals: true }).value));
}
