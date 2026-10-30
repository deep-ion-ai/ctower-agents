// Fim da tela de uma sessão para os tiles. O Claude Code redesenha a tela
// com movimentos de cursor, então o texto cru não serve: a saída passa por um
// terminal invisível (@xterm/headless) e lemos as linhas resultantes.
import { Terminal } from "@xterm/headless";

/** Linhas do "chrome" do Claude Code que não dizem o que a sessão está fazendo. */
function isChrome(line: string): boolean {
  const t = line.trim();
  return (
    /^[─━╌┄\-]{10,}$/.test(t) || // bordas da caixa de entrada
    /^❯\s*$/.test(t) || // prompt vazio
    t.startsWith("⏵") || // modo de permissão
    /for agents|shift\+tab|for shortcuts/.test(t)
  );
}

/** Últimas `n` linhas com conteúdo da tela, sem a caixa de entrada vazia nem o rodapé. */
export async function screenTail(raw: string, n: number, cols = 120, rows = 40): Promise<string> {
  const term = new Terminal({ cols, rows, scrollback: 500, allowProposedApi: true });
  try {
    await new Promise<void>((resolve) => term.write(raw, resolve));
    const buf = term.buffer.active;
    const lines: string[] = [];
    for (let i = 0; i < buf.length; i++) {
      const l = buf.getLine(i)?.translateToString(true) ?? "";
      if (l.trim()) lines.push(l.trimEnd());
    }
    while (lines.length && isChrome(lines[lines.length - 1])) lines.pop();
    return lines.slice(-n).join("\n");
  } finally {
    term.dispose();
  }
}
