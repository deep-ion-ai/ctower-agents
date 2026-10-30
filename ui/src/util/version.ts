// Comparação de versões `X.Y.Z` do workspace (a mesma no hub e nos
// ambientes): sufixo de pré-release ou build é ignorado.

const parts = (v: string): number[] =>
  v
    .trim()
    .replace(/^v/, "")
    .split(/[-+]/)[0]
    .split(".")
    .map((n) => Number.parseInt(n, 10) || 0);

/** `a` é mais nova que `b` (ex.: "0.15.0" > "0.14.2"). Ilegível vale 0. */
export function versionNewer(a: string, b: string): boolean {
  const x = parts(a);
  const y = parts(b);
  for (let i = 0; i < Math.max(x.length, y.length); i++) {
    const d = (x[i] ?? 0) - (y[i] ?? 0);
    if (d !== 0) return d > 0;
  }
  return false;
}
