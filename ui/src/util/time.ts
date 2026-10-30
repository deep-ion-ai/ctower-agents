const MIN = 60_000;
const HOUR = 60 * MIN;
const DAY = 24 * HOUR;

/** Duração legível: "< 1 min", "12 min", "2 h 10 min", "3 d" (a tela atualiza a cada 30 s). */
export function duration(ms: number): string {
  const d = Math.max(0, ms);
  if (d < MIN) return "< 1 min";
  if (d < HOUR) return `${Math.floor(d / MIN)} min`;
  if (d < DAY) {
    const h = Math.floor(d / HOUR);
    const m = Math.floor((d % HOUR) / MIN);
    return m ? `${h} h ${m} min` : `${h} h`;
  }
  return `${Math.floor(d / DAY)} d`;
}

/** "há 12 min" (ou "agora" abaixo de 1 min). */
export function ago(ts: number, now: number): string {
  return now - ts < MIN ? "agora" : `há ${duration(now - ts)}`;
}
