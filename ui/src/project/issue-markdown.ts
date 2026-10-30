// Descrição da issue em Markdown → HTML seguro, para o "Ver issue" (#434).
// O corpo vem de fora (GitHub ou Jira, convertido no hub): o HTML do marked
// passa pelo DOMPurify antes de ir para a tela. Referências do mesmo projeto
// (`#12` no GitHub, `PROJ-5450` no Jira) viram links com `data-issue`, que
// o painel abre na aba do item.
import DOMPurify from "dompurify";
import { Marked, type TokenizerAndRendererExtension, type Tokens } from "marked";
import { itemShort, itemUrl, type ProviderProject } from "../util/item";

const escapeRe = (s: string) => s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
const escapeAttr = (s: string) => s.replace(/&/g, "&amp;").replace(/"/g, "&quot;").replace(/</g, "&lt;");

/** Prefixo da referência curta: `#` no GitHub, `PROJ-` no Jira. */
function refPrefix(project: ProviderProject): string {
  return project.tracker?.kind === "jira" ? `${project.tracker.project_key}-` : "#";
}

function issueRefs(project: ProviderProject): TokenizerAndRendererExtension {
  const prefix = escapeRe(refPrefix(project));
  // Sem letra, dígito, `&` (entidade), `/` (caminho) ou `-` colado antes.
  const start = new RegExp(`(?<![\\w&/-])${prefix}\\d`);
  const whole = new RegExp(`^${prefix}(\\d+)(?![\\w-])`);
  return {
    name: "issueRef",
    level: "inline",
    start: (src: string) => src.match(start)?.index,
    tokenizer(src: string) {
      const m = whole.exec(src);
      if (!m) return undefined;
      return { type: "issueRef", raw: m[0], number: Number(m[1]) };
    },
    renderer(token) {
      const n = (token as Tokens.Generic).number as number;
      return `<a href="${escapeAttr(itemUrl(project, n))}" data-issue="${n}">${escapeAttr(itemShort(project, n))}</a>`;
    },
  };
}

/** Markdown (GFM) → HTML sanitizado: sem script, estilo, formulário nem evento. */
export function renderIssueMarkdown(md: string, project: ProviderProject): string {
  const marked = new Marked({ gfm: true, async: false });
  marked.use({ extensions: [issueRefs(project)] });
  const html = marked.parse(md) as string;
  return DOMPurify.sanitize(html, {
    FORBID_TAGS: ["style", "form", "button", "textarea", "select", "iframe", "object", "embed"],
    FORBID_ATTR: ["style"],
  });
}

/**
 * Número da issue do mesmo projeto apontada por um link: `data-issue` das
 * referências ou a URL da issue no rastreador (`/issues/12` do repositório
 * no GitHub, `/browse/PROJ-12` no Jira). `null` = link externo.
 */
export function linkedIssue(a: HTMLAnchorElement, project: ProviderProject): number | null {
  const data = a.dataset.issue;
  if (data && /^\d+$/.test(data)) return Number(data);
  const href = a.getAttribute("href") ?? "";
  const base = itemUrl(project, 1).replace(/1$/, "");
  if (!href.toLowerCase().startsWith(base.toLowerCase())) return null;
  const rest = href.slice(base.length).match(/^(\d+)(?:[/?#].*)?$/);
  return rest ? Number(rest[1]) : null;
}
