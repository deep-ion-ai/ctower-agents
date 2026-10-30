import type { WorkItemStatus } from "../protocol/gen";

/** Status por texto e tom (a cor nunca é o único sinal). */
export const STATUS: Record<WorkItemStatus, { text: string; tone: string }> = {
  backlog: { text: "backlog", tone: "neutral" },
  em_execucao: { text: "em execução", tone: "running" },
  aguardando_voce: { text: "aguardando você", tone: "attention" },
  em_revisao: { text: "em revisão", tone: "running" },
  aguardando_epico: { text: "aguardando o épico", tone: "neutral" },
  concluido: { text: "concluído", tone: "done" },
  bloqueado: { text: "bloqueado", tone: "error" },
  pausado: { text: "pausado", tone: "neutral" },
};
