/** Mensagem de erro de uma chamada (o Tauri rejeita com texto). */
export const errText = (e: unknown) => (e instanceof Error ? e.message : String(e));

/** O hub recusou pedindo `confirm: true` (exclusão, remoção com sessões vivas). */
export const needsConfirm = (msg: string) => msg.includes("confirm: true");
