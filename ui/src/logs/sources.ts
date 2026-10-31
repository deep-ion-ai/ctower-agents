// Fontes da tela Logs (#65) e o que cada uma faz: o hub (`hub.logs.*`), um
// ambiente (`logs.*`, pelo hub na web) e a própria UI (buffer local). Funções
// puras, testadas em sources.test.ts.
import { levelRank, type LogLevel, type LogLine } from "./types";
import { mask } from "./mask";

export type LogSource = { kind: "hub" } | { kind: "ui" } | { kind: "env"; env: string };

/** Chave estável da fonte (`hub`, `ui`, `env:<id>`): valor do seletor e da contagem do modo seguir. */
export const sourceKey = (s: LogSource): string => (s.kind === "env" ? `env:${s.env}` : s.kind);

export function parseSourceKey(k: string): LogSource | null {
  if (k === "hub" || k === "ui") return { kind: k };
  if (k.startsWith("env:") && k.length > 4) return { kind: "env", env: k.slice(4) };
  return null;
}

/** Filtro da tela: nível mínimo e texto. */
export interface LogFilter {
  level: LogLevel | "";
  q: string;
}

/** Parâmetros de `logs.tail` para o filtro (sem os vazios). */
export function tailParams(f: LogFilter, extra: { limit?: number; before?: number } = {}): Record<string, unknown> {
  const p: Record<string, unknown> = { ...extra };
  if (f.level) p.level = f.level;
  if (f.q.trim()) p.q = f.q.trim();
  return p;
}

/** A linha passa no filtro (o mesmo critério dos daemons). */
export function matches(l: LogLine, f: LogFilter): boolean {
  if (f.level && levelRank(l.level) > levelRank(f.level)) return false;
  const q = f.q.trim().toLowerCase();
  if (!q) return true;
  return (
    l.message.toLowerCase().includes(q) ||
    l.target.toLowerCase().includes(q) ||
    (!!l.fields && JSON.stringify(l.fields).toLowerCase().includes(q))
  );
}

/** Junta linhas novas às atuais, sem repetir `id` e sem passar de `max` (corta as mais antigas). */
export function appendLines(cur: LogLine[], add: LogLine[], max: number): { lines: LogLine[]; dropped: number } {
  const last = cur.length ? cur[cur.length - 1].id : -Infinity;
  const fresh = add.filter((l) => l.id > last);
  const all = fresh.length ? [...cur, ...fresh] : cur;
  const dropped = Math.max(0, all.length - max);
  return { lines: dropped ? all.slice(dropped) : all, dropped };
}

const pad = (n: number, w = 2) => String(n).padStart(w, "0");

/** `14:03:07.120` no fuso local. */
export function clock(ts: number): string {
  const d = new Date(ts);
  return `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}.${pad(d.getMilliseconds(), 3)}`;
}

/** `chave=valor` dos campos, numa linha. */
export function fieldsText(fields?: Record<string, unknown>): string {
  if (!fields) return "";
  return Object.entries(fields)
    .map(([k, v]) => `${k}=${typeof v === "string" ? v : JSON.stringify(v)}`)
    .join(" ");
}

/** Uma linha em texto (cópia e `.log`): `2026-10-31T14:03:07.120Z WARN alvo: mensagem k=v`. */
export function lineText(l: LogLine): string {
  const f = fieldsText(l.fields);
  return mask(`${new Date(l.ts).toISOString()} ${l.level.toUpperCase().padEnd(5)} ${l.target}: ${l.message}${f ? ` ${f}` : ""}`);
}

/** Conteúdo do Exportar: `.log` (texto) ou `.jsonl` (uma linha JSON por evento). */
export function exportContent(lines: LogLine[], format: "log" | "jsonl"): string {
  if (format === "log") return lines.map(lineText).join("\n") + (lines.length ? "\n" : "");
  return (
    lines
      .map(({ previous: _p, ...l }) => mask(JSON.stringify(l)))
      .join("\n") + (lines.length ? "\n" : "")
  );
}

/** Nome do arquivo exportado: `ctower-logs-hub-20261031-1403.jsonl`. */
export function exportName(sourceLabel: string, format: "log" | "jsonl", now = new Date()): string {
  const slug =
    sourceLabel
      .normalize("NFD")
      .replace(/[̀-ͯ]/g, "")
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, "-")
      .replace(/^-|-$/g, "") || "logs";
  const stamp = `${now.getFullYear()}${pad(now.getMonth() + 1)}${pad(now.getDate())}-${pad(now.getHours())}${pad(now.getMinutes())}`;
  return `ctower-logs-${slug}-${stamp}.${format}`;
}

/**
 * Erro da fonte em texto para a tela: ambiente offline (código -32030 do
 * hub, ou o ambiente desconectado), log desligado no daemon (-32040) e
 * daemon anterior à tela Logs (método desconhecido).
 */
export type SourceProblem = { kind: "offline" } | { kind: "disabled" } | { kind: "outdated" } | { kind: "error"; message: string };

export function classifyError(message: string): SourceProblem {
  const m = message.toLowerCase();
  if (m.includes("ambiente offline") || m.includes("desconectado") || m.includes("sem canal")) return { kind: "offline" };
  if (m.includes("log em arquivo desligado")) return { kind: "disabled" };
  if (m.includes("método desconhecido") || m.includes("indisponível pelo")) return { kind: "outdated" };
  return { kind: "error", message };
}
