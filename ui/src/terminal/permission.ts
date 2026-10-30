// Pedido de permissão em texto (#365): o que a ferramenta quer fazer, para a
// faixa abaixo do console e o cartão "Precisa de você".
import type { PermissionRequest } from "../protocol/gen";

/** Corte do argumento na faixa (o pedido inteiro fica no `title`). */
export const ARG_MAX = 160;

/** Argumento principal de cada ferramenta conhecida do Claude Code. */
const MAIN_ARG: Record<string, string> = {
  Bash: "command",
  Read: "file_path",
  Edit: "file_path",
  MultiEdit: "file_path",
  Write: "file_path",
  NotebookEdit: "notebook_path",
  WebFetch: "url",
  WebSearch: "query",
  Glob: "pattern",
  Grep: "pattern",
};

/** Argumento da ferramenta: o principal, se conhecido; senão o JSON. */
export function permissionArg(p: Pick<PermissionRequest, "tool_name" | "tool_input">): string {
  const input = p.tool_input;
  if (input === null || input === undefined) return "";
  if (typeof input !== "object") return String(input);
  const key = p.tool_name ? MAIN_ARG[p.tool_name] : undefined;
  const main = key ? (input as Record<string, unknown>)[key] : undefined;
  if (typeof main === "string") return main;
  const json = JSON.stringify(input);
  return json === "{}" ? "" : json;
}

/** "Bash: git push origin main" (cortado em `max`). */
export function permissionText(p: Pick<PermissionRequest, "tool_name" | "tool_input">, max = ARG_MAX): string {
  const tool = p.tool_name ?? "Ferramenta";
  const arg = permissionArg(p).replace(/\s+/g, " ").trim();
  if (!arg) return `${tool} pede permissão`;
  return `${tool}: ${arg.length > max ? arg.slice(0, max - 1) + "…" : arg}`;
}

/** "1:05" até o pedido voltar ao menu do terminal. */
export function remaining(expiresAt: number, now: number): string {
  const s = Math.max(0, Math.ceil((expiresAt - now) / 1000));
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
}
