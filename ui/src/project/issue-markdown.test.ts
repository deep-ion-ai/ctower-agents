import { expect, test } from "vitest";
import type { ProviderProject } from "../util/item";
import { linkedIssue, renderIssueMarkdown } from "./issue-markdown";

const GH: ProviderProject = { gh_owner: "deep-ion-ai", gh_repo: "ctower-agents", tracker: { kind: "github" } };
const JIRA = {
  gh_owner: "pje",
  gh_repo: "app",
  tracker: { kind: "jira", url: "https://jira.x", project_key: "PROJ" },
} as unknown as ProviderProject;

const dom = (html: string) => {
  const d = document.createElement("div");
  d.innerHTML = html;
  return d;
};

test("GFM: títulos, listas, checklist, código, tabela, links e imagens", () => {
  const d = dom(
    renderIssueMarkdown(
      [
        "# T",
        "- a",
        "- [ ] b",
        "1. c",
        "```rust",
        "fn a() {}",
        "```",
        "| h |",
        "| - |",
        "| v |",
        "[l](https://x.io) ![i](https://x.io/a.png)",
      ].join("\n"),
      GH,
    ),
  );
  expect(d.querySelector("h1")?.textContent).toBe("T");
  expect(d.querySelectorAll("li")).toHaveLength(3);
  expect(d.querySelector("input[type=checkbox]")?.hasAttribute("disabled")).toBe(true);
  expect(d.querySelector("pre code")?.textContent).toBe("fn a() {}\n");
  expect(d.querySelector("td")?.textContent).toBe("v");
  expect(d.querySelector("a")?.getAttribute("href")).toBe("https://x.io");
  expect(d.querySelector("img")?.getAttribute("src")).toBe("https://x.io/a.png");
});

test("sanitização: script, eventos, javascript:, iframe, estilo e formulário somem", () => {
  const html = renderIssueMarkdown(
    [
      "<script>alert(1)</script>",
      '<img src="x" onerror="alert(2)">',
      "[x](javascript:alert(3))",
      '<a href="javascript:alert(4)">y</a>',
      '<iframe src="https://evil"></iframe>',
      '<p style="position:fixed">z</p>',
      '<form><button>b</button></form>',
      '<svg><script>alert(5)</script></svg>',
    ].join("\n\n"),
    GH,
  );
  expect(html).not.toMatch(/<script|onerror|javascript:|<iframe|style=|<form|<button/i);
});

test("#n do GitHub vira link para a aba; dentro de código, entidade ou palavra, não", () => {
  const d = dom(renderIssueMarkdown("Ver #12, (#3) e `#4`; a#5, &#38; x/#6 #7a", GH));
  const refs = [...d.querySelectorAll("a[data-issue]")].map((a) => a.textContent);
  expect(refs).toEqual(["#12", "#3"]);
  const a = d.querySelector("a[data-issue]") as HTMLAnchorElement;
  expect(a.getAttribute("href")).toBe("https://github.com/deep-ion-ai/ctower-agents/issues/12");
  expect(linkedIssue(a, GH)).toBe(12);
});

test("chave do Jira do projeto vira link; de outro projeto, não", () => {
  const d = dom(renderIssueMarkdown("PROJ-5450 e OUTRO-1 e XPROJ-2", JIRA));
  const refs = [...d.querySelectorAll("a[data-issue]")].map((a) => a.textContent);
  expect(refs).toEqual(["PROJ-5450"]);
  expect(d.querySelector("a")?.getAttribute("href")).toBe("https://jira.x/browse/PROJ-5450");
});

test("linkedIssue: URL da issue do mesmo repositório abre no app; outra, não", () => {
  const a = (href: string) => {
    const el = document.createElement("a");
    el.setAttribute("href", href);
    return el;
  };
  expect(linkedIssue(a("https://github.com/deep-ion-ai/ctower-agents/issues/40#issuecomment-1"), GH)).toBe(40);
  expect(linkedIssue(a("https://github.com/deep-ion-ai/ctower-agents/issues/40x"), GH)).toBeNull();
  expect(linkedIssue(a("https://github.com/outro/repo/issues/40"), GH)).toBeNull();
  expect(linkedIssue(a("https://jira.x/browse/PROJ-9"), JIRA)).toBe(9);
});
