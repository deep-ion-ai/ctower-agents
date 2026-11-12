import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, expect, test } from "vitest";
import { App } from "../App";
import { FakeDaemonApi } from "../api/fake";
import type { BranchCommits, DiffFile, DiffPatch, DiffSummary } from "../protocol/gen";
import { terms } from "../test/fakeXterm";
import { dashboard, session, workItem } from "../test/fixtures";
import { LARGE_LINES } from "./diff";
import { DIFF_OLD } from "./DiffViewer";

// Visualizador de diff (#168). O realce (highlight.js) roda de verdade.

const WI = workItem({ gh_number: 32, title: "diff", branch: "feat/32-diff" });
const S = session({ id: "s-32", work_item_id: 32 });

function f(p: Partial<DiffFile> & { path: string }): DiffFile {
  return { old_path: null, status: "modified", additions: 1, deletions: 1, binary: false, untracked: false, ...p };
}

const FILES: DiffFile[] = [
  f({ path: "src/lib.rs", additions: 2, deletions: 2 }),
  f({ path: "img.png", status: "added", additions: null, deletions: null, binary: true }),
  f({ path: "novo.md", status: "added", additions: 1, deletions: 0, untracked: true }),
  f({ path: "c.txt", old_path: "b.txt", status: "renamed", additions: 0, deletions: 0 }),
  f({ path: "gerado.json", additions: LARGE_LINES, deletions: 5 }),
];

const PATCH = [
  "diff --git a/src/lib.rs b/src/lib.rs",
  "--- a/src/lib.rs",
  "+++ b/src/lib.rs",
  "@@ -1,2 +1,3 @@",
  " fn main() {",
  '-    let x = "<b>velho</b>";',
  '+    let x = "<b>novo</b>";',
  "+    // mais",
  "",
].join("\n");

const COMMITS: BranchCommits = {
  base_ref: "origin/develop",
  merge_base: "aaaaaaaa11111111",
  truncated: false,
  commits: [
    { sha: "c2c2c2c2c2c2", short: "c2c2c2c", subject: "segundo", author: "Fulano", date: Date.now() - 60_000, merge: false },
    { sha: "c1c1c1c1c1c1", short: "c1c1c1c", subject: "primeiro", author: "Fulano", date: Date.now() - 120_000, merge: false },
  ],
};

function summary(p: Partial<DiffSummary> = {}): DiffSummary {
  return { from: "aaaaaaaa11111111", to: null, base_ref: "origin/develop", files: FILES, truncated: false, ...p };
}

function base() {
  return new FakeDaemonApi()
    .on("dashboard.get", dashboard({ live_sessions: [S] }, [WI]))
    .on("session.peek", { data: "" })
    .on("shell.list", [])
    .on("recovery.list", [])
    .on("files.shared", [])
    .on("git.diff", (p: { mode: string; commit?: string }) =>
      p.mode === "commit" ? summary({ to: p.commit!, base_ref: null, files: [FILES[0]] }) : summary(),
    )
    .on("git.diff_file", (p: { path: string }): DiffPatch => ({ path: p.path, patch: p.path === "src/lib.rs" ? PATCH : "", too_large: false }))
    .on("git.commits", COMMITS);
}

const at = () => ({ name: "diff" as const, env: "wsl", workItem: 32, title: "#32" });
const files = () => screen.getByRole("navigation", { name: "Arquivos do diff" });
const fileBtn = (name: RegExp) => within(files()).getByRole("button", { name });

beforeEach(() => localStorage.clear());

test("branch: lista com status e +/−, patch do primeiro arquivo realçado e escapado", async () => {
  const api = base();
  render(<App api={api} initial={at()} />);
  const main = await screen.findByRole("main", { name: /Diff · #32 diff/ });
  expect(api.callsTo("git.diff")[0].params).toEqual({ work_item_id: 32, mode: "branch" });
  expect(await within(main).findByText(/origin\/develop \(merge-base aaaaaaaa\) → working tree/)).toBeInTheDocument();
  expect(files().querySelector(".dv-count")?.textContent).toBe("5 arquivos · +3003 −7");
  expect(within(files()).getByText("b.txt → c.txt")).toBeInTheDocument();
  const table = await within(main).findByRole("table", { name: "Diff de src/lib.rs" });
  expect(api.callsTo("git.diff_file")[0].params).toEqual({ work_item_id: 32, mode: "branch", path: "src/lib.rs" });
  // O <b> do código é texto, não HTML; o realce chega por cima.
  expect(table.querySelector("b")).toBeNull();
  expect(table).toHaveTextContent('let x = "<b>novo</b>";');
  await waitFor(() => expect(table.querySelector(".hljs-keyword")).not.toBeNull());
  expect(table.querySelector("tr.dv-add")).not.toBeNull();
  expect(table.querySelector("tr.dv-del")).not.toBeNull();
});

test("binário e diff grande aparecem resumidos; o grande abre se o usuário pedir", async () => {
  const api = base();
  const user = userEvent.setup();
  render(<App api={api} initial={at()} />);
  await screen.findByRole("table", { name: "Diff de src/lib.rs" });
  await user.click(fileBtn(/img\.png/));
  expect(await screen.findByText("Arquivo binário: o conteúdo não é mostrado.")).toBeInTheDocument();
  await user.click(fileBtn(/gerado\.json/));
  expect(screen.getByText(/Diff grande: 3005 linhas mudadas/)).toBeInTheDocument();
  const pedidos = () => api.callsTo("git.diff_file").map((c) => c.params.path);
  expect(pedidos()).toEqual(["src/lib.rs"]);
  await user.click(screen.getByRole("button", { name: "Mostrar mesmo assim" }));
  await waitFor(() => expect(pedidos()).toEqual(["src/lib.rs", "gerado.json"]));
  // Renomeação manda o caminho antigo; não rastreado leva a marca.
  await user.click(fileBtn(/c\.txt/));
  await waitFor(() => expect(api.callsTo("git.diff_file").slice(-1)[0]?.params).toMatchObject({ path: "c.txt", old_path: "b.txt" }));
  await user.click(fileBtn(/novo\.md/));
  expect(await screen.findByText(/adicionado · não rastreado/)).toBeInTheDocument();
});

test("lado a lado (lembrado) e Recarregar pede de novo ao ambiente", async () => {
  const api = base();
  const user = userEvent.setup();
  render(<App api={api} initial={at()} />);
  await screen.findByRole("table", { name: "Diff de src/lib.rs" });
  await user.click(screen.getByRole("button", { name: "Lado a lado" }));
  expect(localStorage.getItem("ct.diff.view")).toBe("split");
  const table = screen.getByRole("table", { name: "Diff de src/lib.rs" });
  expect(table).toHaveClass("dv-split");
  // Linha velha e nova na mesma linha da tabela.
  const row = [...table.querySelectorAll("tr")].find((r) => r.textContent?.includes("velho"));
  expect(row?.textContent).toContain("novo");
  await user.click(screen.getByRole("button", { name: "Recarregar" }));
  await waitFor(() => expect(api.callsTo("git.diff")).toHaveLength(2));
  await waitFor(() => expect(api.callsTo("git.diff_file")).toHaveLength(2));
});

test("commit: lista os commits, escolhe o mais novo e troca pelo seletor", async () => {
  const api = base();
  const user = userEvent.setup();
  render(<App api={api} initial={at()} />);
  await screen.findByRole("table", { name: "Diff de src/lib.rs" });
  await user.click(screen.getByRole("button", { name: "Commit" }));
  await waitFor(() =>
    expect(api.callsTo("git.diff").slice(-1)[0]?.params).toEqual({ work_item_id: 32, mode: "commit", commit: "c2c2c2c2c2c2" }),
  );
  const sel = screen.getByRole("combobox", { name: "Commit" });
  expect(within(sel).getAllByRole("option").map((o) => o.textContent)).toEqual([
    expect.stringMatching(/^c2c2c2c · segundo · Fulano/),
    expect.stringMatching(/^c1c1c1c · primeiro · Fulano/),
  ]);
  await user.selectOptions(sel, "c1c1c1c1c1c1");
  await waitFor(() => expect(api.callsTo("git.diff").slice(-1)[0]?.params).toMatchObject({ commit: "c1c1c1c1c1c1" }));
  await waitFor(() =>
    expect(api.callsTo("git.diff_file").slice(-1)[0]?.params).toEqual({
      work_item_id: 32,
      mode: "commit",
      commit: "c1c1c1c1c1c1",
      path: "src/lib.rs",
    }),
  );
  expect(api.callsTo("git.commits")).toHaveLength(1);
});

test("erro do daemon aparece com Tentar de novo; ambiente antigo pede atualização", async () => {
  const api = base().on("git.diff", () => {
    throw new Error("o work item #32 não tem worktree neste ambiente");
  });
  const { unmount } = render(<App api={api} initial={at()} />);
  expect(await screen.findByRole("alert")).toHaveTextContent("não tem worktree");
  expect(screen.getByRole("button", { name: "Tentar de novo" })).toBeInTheDocument();
  unmount();
  const old = base().on("git.diff", () => {
    throw new Error("método desconhecido: git.diff (-32601)");
  });
  render(<App api={old} initial={at()} />);
  expect(await screen.findByText(DIFF_OLD)).toBeInTheDocument();
  expect(screen.queryByRole("button", { name: "Tentar de novo" })).toBeNull();
});

test("sem mudanças: avisa no lugar do patch", async () => {
  const api = base().on("git.diff", summary({ files: [] }));
  const user = userEvent.setup();
  render(<App api={api} initial={at()} />);
  expect(await screen.findByText("Nenhuma mudança.")).toBeInTheDocument();
  await user.click(screen.getByRole("button", { name: "Não commitado" }));
  expect(await screen.findByText("Nada por commitar na worktree.")).toBeInTheDocument();
  expect(api.callsTo("git.diff").slice(-1)[0]?.params).toEqual({ work_item_id: 32, mode: "uncommitted" });
});

test("o ícone Ver diff do cabeçalho da sessão abre a aba do diff", async () => {
  terms.length = 0;
  const api = base();
  const user = userEvent.setup();
  render(<App api={api} />);
  await user.click(await screen.findByRole("button", { name: /^Terminal/ }));
  await user.click(await screen.findByRole("treeitem", { name: /#32 diff/ }));
  const bar = await screen.findByRole("region", { name: "Sessão" });
  await user.click(within(bar).getByRole("button", { name: "Ver diff" }));
  expect(await screen.findByRole("main", { name: /Diff · #32 diff/ })).toBeInTheDocument();
  expect(api.callsTo("git.diff")[0].params).toEqual({ work_item_id: 32, mode: "branch" });
});

