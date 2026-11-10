import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, expect, test, vi } from "vitest";
import { App } from "../App";
import { FakeDaemonApi } from "../api/fake";
import type { MdFile, SharedFile } from "../protocol/gen";
import { dashboard, session, workItem } from "../test/fixtures";
import { renderDiagram } from "./mermaid";

// Visualizador de markdown (#167). O Mermaid de verdade não roda no jsdom
// (precisa medir texto): o desenho é trocado por um fake que falha nos
// blocos com "inválido".
vi.mock("./mermaid", () => ({
  renderDiagram: vi.fn(async (code: string) =>
    code.includes("inválido") ? { error: "Parse error on line 1" } : { svg: `<svg data-fake="1"><text>${code.length}</text></svg>` },
  ),
}));

const WI = workItem({ gh_number: 32, title: "visualizador" });
const S = session({ id: "s-32", work_item_id: 32, cwd: "/w/acme-web/feat__32" });
const PATH = "/w/acme-web/feat__32/docs/plano.md";

const CONTENT = [
  "# Plano",
  "",
  "Texto com [link externo](https://example.com) e [outro doc](./b.md).",
  "",
  "<script>alert(1)</script>",
  "",
  "```mermaid",
  "graph TD; A-->B",
  "```",
  "",
  "```mermaid",
  "graph inválido <b>",
  "```",
  "",
  "## Depois dos diagramas",
].join("\n");

function file(p: Partial<MdFile> = {}): MdFile {
  return { path: PATH, root: "/w/acme-web/feat__32", content: CONTENT, size: CONTENT.length, modified_at: Date.now(), ...p };
}

function base(content = CONTENT) {
  return new FakeDaemonApi()
    .on("dashboard.get", dashboard({ live_sessions: [S] }, [WI]))
    .on("session.peek", { data: "" })
    .on("shell.list", [])
    .on("recovery.list", [])
    .on("file.read", (p: { path: string }) => file({ path: p.path, content }));
}

const at = (path = PATH) => ({ name: "file" as const, env: "wsl", path, session: S.id });

beforeEach(() => {
  vi.mocked(renderDiagram).mockClear();
  localStorage.clear();
});

test("formatado: markdown sanitizado, diagrama desenhado e o inválido com erro e código, sem quebrar o resto", async () => {
  const api = base();
  render(<App api={api} initial={at()} />);
  const main = await screen.findByRole("main", { name: "plano.md" });
  expect(await within(main).findByRole("heading", { name: "Plano" })).toBeInTheDocument();
  expect(api.callsTo("file.read")[0].params).toEqual({ session_id: S.id, path: PATH });
  // Caminho e ambiente visíveis.
  expect(within(main).getByText(PATH)).toBeInTheDocument();
  expect(within(main).getByText(/ctower-agents \(WSL\)/)).toBeInTheDocument();
  // Sanitizado: o <script> não vai para a tela.
  expect(main.querySelector("script")).toBeNull();
  await waitFor(() => expect(main.querySelector("svg[data-fake]")).not.toBeNull());
  const erro = await within(main).findByRole("alert");
  expect(erro).toHaveTextContent("Diagrama Mermaid inválido: Parse error on line 1");
  // O código do bloco inválido aparece como texto (o <b> não vira HTML).
  expect(main.querySelector('.md-mermaid[data-state="error"] code')?.textContent).toBe("graph inválido <b>");
  expect(main.querySelector('.md-mermaid[data-state="error"] b')).toBeNull();
  expect(within(main).getByRole("heading", { name: "Depois dos diagramas" })).toBeInTheDocument();
  expect(renderDiagram).toHaveBeenCalledTimes(2);
});

test("alterna para raw (lembrado) e recarrega lendo de novo no ambiente", async () => {
  const api = base("# Título\n\ntexto");
  const user = userEvent.setup();
  render(<App api={api} initial={at()} />);
  await screen.findByRole("heading", { name: "Título" });
  await user.click(screen.getByRole("button", { name: "Raw" }));
  expect(screen.getByRole("button", { name: "Raw" })).toHaveAttribute("aria-pressed", "true");
  expect(screen.getByLabelText("Texto do arquivo").textContent).toBe("# Título\n\ntexto");
  expect(screen.queryByRole("heading", { name: "Título" })).toBeNull();
  expect(localStorage.getItem("ct.files.mode")).toBe("raw");
  await user.click(screen.getByRole("button", { name: "Recarregar" }));
  await waitFor(() => expect(api.callsTo("file.read")).toHaveLength(2));
  await user.click(screen.getByRole("button", { name: "Formatado" }));
  expect(await screen.findByRole("heading", { name: "Título" })).toBeInTheDocument();
});

test("erro do daemon (fora das pastas da sessão) aparece com Tentar de novo", async () => {
  const api = base().on("file.read", () => {
    throw new Error("/etc/x.md está fora das pastas da sessão (worktree e pasta de docs)");
  });
  render(<App api={api} initial={at("/etc/x.md")} />);
  expect(await screen.findByRole("alert")).toHaveTextContent("fora das pastas da sessão");
  expect(screen.getByRole("button", { name: "Tentar de novo" })).toBeInTheDocument();
});

test("link para outro .md abre na aba dele; link externo vai ao navegador", async () => {
  const api = base();
  const user = userEvent.setup();
  render(<App api={api} initial={at()} />);
  await user.click(await screen.findByRole("link", { name: "outro doc" }));
  await waitFor(() =>
    expect(api.callsTo("file.read").map((c) => c.params.path)).toContain("/w/acme-web/feat__32/docs/b.md"),
  );
  expect(await screen.findByRole("main", { name: "b.md" })).toBeInTheDocument();
  await user.click(await screen.findByRole("link", { name: "link externo" }));
  expect(api.opened).toEqual(["https://example.com"]);
});

test("Arquivos do painel da sessão: lista, atualiza pelo push e abre no visualizador", async () => {
  const shared: SharedFile[] = [];
  const api = base().on("files.shared", () => shared);
  const user = userEvent.setup();
  const { SharedFiles } = await import("./SharedFiles");
  const { TowerProvider } = await import("../state/tower");
  const { NavProvider, useNav } = await import("../state/nav");
  let current = "";
  function Probe() {
    current = useNav().screen.name;
    return null;
  }
  render(
    <TowerProvider api={api}>
      <NavProvider>
        <SharedFiles env="wsl" session={S.id} now={Date.now()} />
        <Probe />
      </NavProvider>
    </TowerProvider>,
  );
  expect(await screen.findByText(/Nenhum arquivo compartilhado/)).toBeInTheDocument();
  shared.push({ session_id: S.id, path: PATH, title: "Plano da issue", shared_at: Date.now() });
  act(() => api.push("wsl", "session_files", { session_id: "outra" }));
  act(() => api.push("wsl", "session_files", { session_id: S.id }));
  await user.click(await screen.findByRole("button", { name: /Plano da issue/ }));
  expect(api.callsTo("files.shared")).toHaveLength(2);
  expect(current).toBe("file");
});
