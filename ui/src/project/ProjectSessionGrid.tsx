import "../control-room/control-room.css";
import { DENSITIES, useGridPrefs } from "../control-room/grid";
import { TileGrid, type ProjectMatch } from "../control-room/TileGrid";

/**
 * Grade de sessões vivas do projeto (#356): o mesmo `TileGrid` da Sala de
 * controle, filtrado pelo projeto, com as mesmas preferências de densidade e
 * agrupamento. Abrir o terminal é a ação do tile; recuperar e descartar ficam
 * na lista por ambiente (`ProjectSessions`), logo abaixo.
 */
export function ProjectSessionGrid({ now, only }: { now: number; only: ProjectMatch }) {
  const [grid, setGrid] = useGridPrefs();
  return (
    <div className="pj-session-grid">
      <div className="control-head">
        <span className="control-spacer" />
        <button
          type="button"
          className="btn btn-sm"
          aria-pressed={grid.group}
          onClick={() => setGrid({ group: !grid.group })}
        >
          Agrupar por ambiente
        </button>
        <div role="group" aria-label="Densidade da grade" className="control-seg">
          {DENSITIES.map((d) => (
            <button
              key={d.id}
              type="button"
              className="btn btn-sm"
              aria-pressed={grid.density === d.id}
              onClick={() => setGrid({ density: d.id })}
            >
              {d.label}
            </button>
          ))}
        </div>
      </div>
      <TileGrid now={now} only={only} group={grid.group} density={grid.density} />
    </div>
  );
}
