import type { SessionMode } from "../protocol/gen";

/**
 * Modelos aceitos pelo daemon em `session.start` e `session.start_project`
 * (aliases do Claude Code). Espelha `CLAUDE_MODELS` em ct-core/src/model.rs.
 */
export const CLAUDE_MODELS: { value: string; label: string }[] = [
  { value: "fable", label: "Fable" },
  { value: "opus", label: "Opus" },
  { value: "sonnet", label: "Sonnet" },
  { value: "haiku", label: "Haiku" },
];

/**
 * Níveis de esforço aceitos em `--effort` (#182), do menor para o maior.
 * Espelha `CLAUDE_EFFORTS` em ct-core/src/model.rs.
 */
export const CLAUDE_EFFORTS: { value: string; label: string }[] = [
  { value: "low", label: "low" },
  { value: "medium", label: "medium" },
  { value: "high", label: "high" },
  { value: "xhigh", label: "xhigh" },
  { value: "max", label: "max" },
];

/** Valor do select para "esforço padrão" (sem `--effort`). */
export const DEFAULT_EFFORT_VALUE = "";

/** "Opus · esforço high", "Sonnet" ou "sem modelo": o modelo da issue (#182). */
export const issueAgentLabel = (a: { model?: string | null; effort?: string | null } | null | undefined) =>
  a?.model ? `${modelLabel(a.model)}${a.effort ? ` · esforço ${a.effort}` : ""}` : "sem modelo";

export const SESSION_MODES: { value: SessionMode; label: string; hint: string }[] = [
  { value: "native", label: "native", hint: "sessão direta" },
  { value: "subagents", label: "subagents", hint: "delega cada tarefa a subagentes" },
];

/** Valor do select para "padrão do Claude Code" (sem `--model`). */
export const DEFAULT_MODEL_VALUE = "";

export const modelLabel = (model: string | null) =>
  model ? (CLAUDE_MODELS.find((m) => m.value === model)?.label ?? model) : "modelo padrão";

/** "Opus · subagents": o que a sessão usa, para tiles e detalhe. */
export const agentLabel = (s: { model: string | null; mode: SessionMode }) => `${modelLabel(s.model)} · ${s.mode}`;
