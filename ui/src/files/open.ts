// Abrir um `.md` do ambiente no visualizador (#167): a aba de cada arquivo
// e as regras de caminho que a UI aplica antes de pedir ao daemon (que
// confere de novo e é quem decide). Funções puras, testadas em open.test.ts.
import type { OpenTab, Screen } from "../state/nav";

export type FileScreen = Extract<Screen, { name: "file" }>;

/** Protocolo do ambiente com `file.read`, `files.shared` e `docs.list`. */
export const FILES_PROTOCOL = "2.13";

/** De onde o arquivo foi aberto: a sessão (worktree e docs) ou o projeto (só docs). */
export type FileScope = { session: string } | { projectId: number };

/** Nome do arquivo (último segmento). */
export const basename = (path: string) => path.replace(/\/+$/, "").split("/").pop() || path;

/** Pasta do caminho (sem a barra final); `""` se não houver. */
export const dirname = (path: string) => {
  const i = path.replace(/\/+$/, "").lastIndexOf("/");
  return i <= 0 ? (i === 0 ? "/" : "") : path.slice(0, i);
};

/** Parâmetros do `file.read` para o escopo. */
export const readParams = (scope: FileScope, path: string) =>
  "session" in scope ? { session_id: scope.session, path } : { project_id: scope.projectId, path };

/** Id estável da aba: o mesmo arquivo do mesmo ambiente volta à aba aberta. */
export const fileTabId = (env: string, path: string) => `file:${env}:${path}`;

/** Aba do visualizador do arquivo `path` do ambiente `env`. */
export function fileTab(env: string, path: string, scope: FileScope, title?: string): OpenTab {
  const screen: FileScreen = {
    name: "file",
    env,
    path,
    ...("session" in scope ? { session: scope.session } : { projectId: scope.projectId }),
    ...(title ? { title } : {}),
  };
  return { id: fileTabId(env, path), title: title || basename(path), screen };
}

/** Escopo do endereço da aba. */
export const scopeOf = (s: FileScreen): FileScope =>
  s.session !== undefined ? { session: s.session } : { projectId: s.projectId ?? 0 };

/** Junta `rel` à pasta `base` (POSIX), resolvendo `.` e `..`. */
export function joinPath(base: string, rel: string): string {
  const abs = rel.startsWith("/");
  const parts = (abs ? rel : `${base}/${rel}`).split("/");
  const out: string[] = [];
  for (const p of parts) {
    if (p === "" || p === ".") continue;
    if (p === "..") out.pop();
    else out.push(p);
  }
  return `/${out.join("/")}`;
}

/**
 * Link relativo de dentro do documento para outro `.md` (`./b.md`,
 * `../specs/c.md#secao`): o caminho absoluto no ambiente, ou `null` se o
 * link não é um `.md` local.
 */
export function linkedMd(href: string, currentPath: string): string | null {
  if (!href || /^[a-z][a-z0-9+.-]*:/i.test(href) || href.startsWith("#") || href.startsWith("//")) return null;
  let path = href.split("#")[0].split("?")[0];
  try {
    path = decodeURIComponent(path);
  } catch {
    return null;
  }
  if (!/\.md$/i.test(path)) return null;
  return joinPath(dirname(currentPath), path);
}

/** Tamanho legível: "812 B", "3,4 KiB". */
export function sizeText(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const k = bytes / 1024;
  return `${k < 10 ? k.toFixed(1).replace(".", ",") : Math.round(k)} KiB`;
}
