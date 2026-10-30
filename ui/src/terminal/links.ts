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
