import "../control-room/control-room.css";
import { Footer } from "../control-room/Footer";
import type { ProjectMatch } from "../control-room/TileGrid";

/** Fim do dia do projeto (#356): "Em revisão" e "Concluídas hoje" só deste projeto, na Visão geral. */
export function ProjectDayEnd({ now, only }: { now: number; only: ProjectMatch }) {
  return (
    <section className="hub-card pj-day-end" aria-label="Fim do dia do projeto">
      <Footer now={now} only={only} />
    </section>
  );
}
