// Visualizador de diff (#168): funções puras da tela, testadas em
// diff.test.ts. O daemon do ambiente faz o diff (`git.diff`,
// `git.diff_file`, `git.commits`); aqui só a aba, os parâmetros e a leitura
// do patch unificado para as visões unificada e lado a lado.
import type { DiffFile, DiffStatus, EnvInfo } from "../protocol/gen";
import type { OpenTab, Screen } from "../state/nav";
import { versionNewer } from "../util/version";

export type DiffScreen = Extract<Screen, { name: "diff" }>;

/** Protocolo do ambiente com `git.diff`, `git.diff_file` e `git.commits`. */
export const DIFF_PROTOCOL = "2.15";

/** Branch (merge-base até a working tree), não commitado ou um commit. */
export type DiffMode = "branch" | "uncommitted" | "commit";

/** Acima disso de linhas mudadas, o patch só abre se o usuário pedir. */
export const LARGE_LINES = 3000;

export const MODE_LABEL: Record<DiffMode, string> = {
  branch: "Branch",
  uncommitted: "Não commitado",
  commit: "Commit",
};

/** Id estável: o diff do mesmo work item do mesmo ambiente volta à aba aberta. */
export const diffTabId = (env: string, workItem: number) => `diff:${env}:${workItem}`;

/** Aba do diff do work item; `label` é o que aparece depois de "Diff · " (ex.: `#12`). */
export function diffTab(env: string, workItem: number, label?: string): OpenTab {
  const screen: DiffScreen = { name: "diff", env, workItem, ...(label ? { title: label } : {}) };
  return { id: diffTabId(env, workItem), title: diffTitle(screen), screen };
}

export const diffTitle = (s: Pick<DiffScreen, "title">) => (s.title ? `Diff · ${s.title}` : "Diff");

/** Parâmetros do `git.diff` (e, com `path`, do `git.diff_file`). */
export function diffParams(workItem: number, mode: DiffMode, commit: string | null) {
  return { work_item_id: workItem, mode, ...(mode === "commit" && commit ? { commit } : {}) };
}

const atLeast = (protocol: string, want: string) => !versionNewer(want, protocol);

/**
 * Dá para pedir o diff do work item neste ambiente? `null` = sim; senão o
 * motivo (dica do botão desabilitado). `connected`: o ambiente está online.
 */
export function diffBlocked(
  wi: { worktree_path: string | null; gh_number: number } | undefined,
  info: EnvInfo | undefined,
  connected: boolean,
  envName: string,
): string | null {
  if (!wi) return "Sem work item neste ambiente.";
  if (wi.gh_number === 0) return "O item do projeto (#0) usa o clone principal: não há branch para comparar.";
  if (!wi.worktree_path) return `Sem worktree no ambiente ${envName}.`;
  if (!connected) return `Ambiente ${envName} desconectado.`;
  // Sem o `env.info` (na web, que não o lê): o ambiente responde, e um
  // anterior ao método mostra o aviso de atualizar na própria tela.
  if (info && !atLeast(info.protocol, DIFF_PROTOCOL)) {
    return `Atualize o ambiente ${envName} (protocolo ${info.protocol}; o diff pede ${DIFF_PROTOCOL} ou maior).`;
  }
  return null;
}

export const STATUS_LETTER: Record<DiffStatus, string> = {
  added: "A",
  modified: "M",
  deleted: "D",
  renamed: "R",
  copied: "C",
  typechange: "T",
  unmerged: "U",
};

export const STATUS_TEXT: Record<DiffStatus, string> = {
  added: "adicionado",
  modified: "modificado",
  deleted: "removido",
  renamed: "renomeado",
  copied: "copiado",
  typechange: "tipo mudou",
  unmerged: "em conflito",
};

/** Linhas mudadas (0 em binário). */
export const changedLines = (f: DiffFile) => (f.additions ?? 0) + (f.deletions ?? 0);

/** Uma linha do patch. `meta`: "\ No newline at end of file", "Binary files…". */
export interface PatchLine {
  kind: "add" | "del" | "ctx" | "meta";
  text: string;
  oldNo: number | null;
  newNo: number | null;
}

export interface Hunk {
  /** A linha `@@ -a,b +c,d @@ contexto`. */
  header: string;
  lines: PatchLine[];
}

export interface ParsedPatch {
  hunks: Hunk[];
  /** Linhas fora dos hunks que dizem algo (binário, modo, renomeação). */
  notes: string[];
}

const HUNK = /^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@/;
const NOTE = /^(Binary files |GIT binary patch|old mode |new mode |deleted file mode |new file mode |similarity index |rename from |rename to |copy from |copy to )/;

/** Lê o patch unificado de um arquivo (`git diff` com prefixos `a/` e `b/`). */
export function parsePatch(patch: string): ParsedPatch {
  const hunks: Hunk[] = [];
  const notes: string[] = [];
  let cur: Hunk | null = null;
  let oldNo = 0;
  let newNo = 0;
  const lines = patch.split("\n");
  // O patch termina em "\n": a última "linha" é vazia.
  if (lines.length > 0 && lines[lines.length - 1] === "") lines.pop();
  for (const line of lines) {
    const h = HUNK.exec(line);
    if (h) {
      cur = { header: line, lines: [] };
      hunks.push(cur);
      oldNo = Number(h[1]);
      newNo = Number(h[2]);
      continue;
    }
    if (!cur) {
      if (NOTE.test(line)) notes.push(line);
      continue;
    }
    const c = line[0];
    if (c === "+") cur.lines.push({ kind: "add", text: line.slice(1), oldNo: null, newNo: newNo++ });
    else if (c === "-") cur.lines.push({ kind: "del", text: line.slice(1), oldNo: oldNo++, newNo: null });
    else if (c === " " || line === "") cur.lines.push({ kind: "ctx", text: line.slice(1), oldNo: oldNo++, newNo: newNo++ });
    else if (c === "\\") cur.lines.push({ kind: "meta", text: line.slice(1).trim(), oldNo: null, newNo: null });
    else if (line.startsWith("diff --git ")) cur = null;
  }
  return { hunks, notes };
}

/** Linha da visão lado a lado: antigo à esquerda, novo à direita. */
export interface SplitRow {
  left: PatchLine | null;
  right: PatchLine | null;
}

/**
 * Pares da visão lado a lado: o contexto nos dois lados; cada bloco de
 * remoções seguido de adições vira pares (a primeira removida com a
 * primeira adicionada…), e o que sobra fica sozinho no seu lado.
 */
export function splitRows(lines: PatchLine[]): SplitRow[] {
  const rows: SplitRow[] = [];
  let i = 0;
  while (i < lines.length) {
    const l = lines[i];
    if (l.kind === "ctx") {
      rows.push({ left: l, right: l });
      i++;
      continue;
    }
    if (l.kind === "meta") {
      rows.push({ left: l, right: l });
      i++;
      continue;
    }
    const dels: PatchLine[] = [];
    const adds: PatchLine[] = [];
    while (i < lines.length && lines[i].kind === "del") dels.push(lines[i++]);
    while (i < lines.length && lines[i].kind === "add") adds.push(lines[i++]);
    for (let k = 0; k < Math.max(dels.length, adds.length); k++) {
      rows.push({ left: dels[k] ?? null, right: adds[k] ?? null });
    }
  }
  return rows;
}

/** Linguagens do realce de sintaxe (highlight.js), pela extensão ou pelo nome. */
const BY_EXT: Record<string, string> = {
  rs: "rust",
  ts: "typescript",
  tsx: "typescript",
  mts: "typescript",
  cts: "typescript",
  js: "javascript",
  jsx: "javascript",
  mjs: "javascript",
  cjs: "javascript",
  json: "json",
  css: "css",
  scss: "scss",
  html: "xml",
  htm: "xml",
  xml: "xml",
  svg: "xml",
  vue: "xml",
  md: "markdown",
  markdown: "markdown",
  py: "python",
  go: "go",
  java: "java",
  kt: "kotlin",
  kts: "kotlin",
  sh: "bash",
  bash: "bash",
  zsh: "bash",
  yml: "yaml",
  yaml: "yaml",
  toml: "ini",
  ini: "ini",
  sql: "sql",
  c: "c",
  h: "c",
  cpp: "cpp",
  cc: "cpp",
  hpp: "cpp",
  cs: "csharp",
  rb: "ruby",
  php: "php",
  swift: "swift",
  ps1: "powershell",
};

const BY_NAME: Record<string, string> = {
  dockerfile: "dockerfile",
  makefile: "makefile",
  "cargo.lock": "ini",
};

/** Linguagem do arquivo, ou `null` (sem realce). */
export function languageOf(path: string): string | null {
  const name = (path.split("/").pop() ?? path).toLowerCase();
  if (BY_NAME[name]) return BY_NAME[name];
  const dot = name.lastIndexOf(".");
  if (dot <= 0) return null;
  return BY_EXT[name.slice(dot + 1)] ?? null;
}

/** "a.txt → b.txt" numa renomeação; o caminho, no resto. */
export const fileLabel = (f: Pick<DiffFile, "path" | "old_path">) =>
  f.old_path && f.old_path !== f.path ? `${f.old_path} → ${f.path}` : f.path;

/** Chave do patch no cache da tela. */
export const patchKey = (mode: DiffMode, commit: string | null, f: Pick<DiffFile, "path" | "old_path">) =>
  `${mode}:${mode === "commit" ? (commit ?? "") : ""}:${f.old_path ?? ""}:${f.path}`;
