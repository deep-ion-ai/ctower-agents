// Links do console (OSC 8 e URLs em texto puro): Ctrl+clique (Cmd no macOS)
// abre no navegador pelo `api.openUrl`, sem confirmação. Só http e https.

/** URL normalizada, se for http(s); senão `null`. */
export function safeUrl(uri: string): string | null {
  let u: URL;
  try {
    u = new URL(uri.trim());
  } catch {
    return null;
  }
  return u.protocol === "http:" || u.protocol === "https:" ? u.href : null;
}

export const isMac = () => /Mac|iPhone|iPad/.test(navigator.platform);

/** Ctrl+clique (Cmd+clique no macOS). Clique simples não abre: evita abrir ao selecionar. */
export function isOpenClick(e: Pick<MouseEvent, "ctrlKey" | "metaKey" | "button">, mac = isMac()): boolean {
  return e.button === 0 && (mac ? e.metaKey : e.ctrlKey);
}

/** Tratador comum aos dois tipos de link; `onError` recebe a falha ao abrir. */
export function linkActivator(open: (url: string) => Promise<void>, onError: (e: unknown) => void = () => {}) {
  return (e: Pick<MouseEvent, "ctrlKey" | "metaKey" | "button">, uri: string) => {
    if (!isOpenClick(e)) return;
    const url = safeUrl(uri);
    if (url) open(url).catch(onError);
  };
}

/** Caminho `.md` achado numa linha do console (#167): `start`/`end` em colunas (0-based, `end` exclusivo). */
export interface MdPathMatch {
  start: number;
  end: number;
  path: string;
}

// Absoluto (`/…`), `./…` ou relativo (`docs/a.md`, `a.md`); nunca dentro
// de uma URL (o caractere antes não pode ser `/`, `:` nem parte do nome).
const MD_PATH = /(?<![\w./~:@+-])((?:\/|\.\/)?[\w@+-][\w.@+-]*(?:\/[\w.@+-]+)*\.md)(?![\w/-]|\.\w)/gi;

/** Caminhos `.md` na linha (texto do buffer do xterm). */
export function mdPathsIn(line: string): MdPathMatch[] {
  const out: MdPathMatch[] = [];
  for (const m of line.matchAll(MD_PATH)) {
    const i = m.index ?? 0;
    out.push({ start: i, end: i + m[0].length, path: m[0] });
  }
  return out;
}

/**
 * Caminho absoluto do `.md` para abrir no visualizador, se estiver numa das
 * pastas da sessão (`roots`: a pasta da sessão primeiro, base dos
 * relativos). Fora delas, ou com `..`, `null` (não vira link). O daemon
 * confere de novo ao ler.
 */
export function mdLinkTarget(path: string, roots: string[]): string | null {
  const clean = roots.map((r) => r.replace(/\/+$/, "")).filter((r) => r !== "");
  if (clean.length === 0 || path.split("/").includes("..")) return null;
  if (!path.startsWith("/")) return `${clean[0]}/${path.replace(/^\.\//, "")}`;
  return clean.some((r) => path.startsWith(`${r}/`)) ? path : null;
}
