// Máscara de tokens e secrets (#65), a mesma do `ct_http::logs::mask`: o log
// da UI passa por aqui antes de ir ao buffer, ao arquivo e à exportação, e as
// linhas dos daemons de novo ao exportar.
import type { LogLine } from "./types";

const MASK = "***";

/** Usuário e senha na URL (`https://user:senha@host`). */
const USERINFO = /\b([a-z][a-z0-9+.-]*:\/\/)[^/\s:@]+:[^/\s@]+@/gi;
/** `token=x`, `?secret=x`, `"password": "x"`. */
const PAIRS =
  /(["']?[A-Za-z0-9_.-]*(?:token|secret|passw(?:or)?d|api[_-]?key|credential|authorization|private[_-]?key)[A-Za-z0-9_.-]*["']?)(\s*=\s*|\s*:\s*["'])([^\s"'&,;}]+)/gi;
/** `Bearer x`, `Basic x`. */
const AUTH = /\b(bearer|basic)\s+[A-Za-z0-9._~+/=-]+/gi;
/** Valores com cara de token: GitHub, GitLab, Anthropic/OpenAI, Slack, age, JWT e os 64 hex do Control Tower. */
const VALUES = new RegExp(
  [
    "\\b(?:gh[pousr]_[A-Za-z0-9]{20,}",
    "github_pat_[A-Za-z0-9_]{20,}",
    "glpat-[A-Za-z0-9_-]{16,}",
    "sk-ant-[A-Za-z0-9_-]{16,}",
    "sk-[A-Za-z0-9_-]{20,}",
    "xox[abprs]-[A-Za-z0-9-]{10,}",
    "AGE-SECRET-KEY-1[0-9A-Z]{20,}",
    "eyJ[A-Za-z0-9_-]{8,}\\.[A-Za-z0-9_-]{8,}\\.[A-Za-z0-9_-]+",
    "[0-9a-fA-F]{64})\\b",
  ].join("|"),
  "g",
);
/** Campo estruturado cujo valor é secret. */
const FIELD = /token|secret|passw(or)?d|api[_-]?key|credential|authorization|private[_-]?key/i;

export function mask(s: string): string {
  return s
    .replace(USERINFO, (_m, scheme: string) => `${scheme}${MASK}@`)
    .replace(PAIRS, (_m, k: string, sep: string) => `${k}${sep}${MASK}`)
    .replace(AUTH, (_m, scheme: string) => `${scheme} ${MASK}`)
    .replace(VALUES, MASK);
}

function maskValue(name: string, v: unknown): unknown {
  if (FIELD.test(name)) return MASK;
  if (typeof v === "string") return mask(v);
  if (Array.isArray(v)) return v.map((x) => maskValue("", x));
  if (v && typeof v === "object")
    return Object.fromEntries(Object.entries(v as Record<string, unknown>).map(([k, x]) => [k, maskValue(k, x)]));
  return v;
}

export function maskLine(l: LogLine): LogLine {
  const out: LogLine = { ...l, message: mask(l.message) };
  if (l.fields) out.fields = maskValue("", l.fields) as Record<string, unknown>;
  return out;
}
