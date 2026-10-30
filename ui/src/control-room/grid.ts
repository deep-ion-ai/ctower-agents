// Disposição da grade da Sala de controle (#316): agrupar por projeto e
// densidade. Fica por máquina, em localStorage, como a do shell.
import { useCallback, useState } from "react";

/** Colunas da grade: automática (pela largura), 2 ou 3. */
export type Density = "auto" | "2" | "3";

export interface GridPrefs {
  group: boolean;
  density: Density;
}

export const DEFAULT_GRID: GridPrefs = { group: false, density: "auto" };
export const DENSITIES: { id: Density; label: string }[] = [
  { id: "auto", label: "Automática" },
  { id: "2", label: "2 colunas" },
  { id: "3", label: "3 colunas" },
];

const KEY = "ctower.control.grid";

export function loadGrid(): GridPrefs {
  let v: Partial<GridPrefs> | null = null;
  try {
    v = JSON.parse(localStorage.getItem(KEY) ?? "null");
  } catch {
    // Sem armazenamento: o padrão.
  }
  return {
    group: typeof v?.group === "boolean" ? v.group : DEFAULT_GRID.group,
    density: DENSITIES.some((d) => d.id === v?.density) ? v!.density! : DEFAULT_GRID.density,
  };
}

export function useGridPrefs(): [GridPrefs, (p: Partial<GridPrefs>) => void] {
  const [prefs, setPrefs] = useState(loadGrid);
  const set = useCallback((p: Partial<GridPrefs>) => {
    setPrefs((old) => {
      const next = { ...old, ...p };
      try {
        localStorage.setItem(KEY, JSON.stringify(next));
      } catch {
        // Sem armazenamento: vale só nesta execução.
      }
      return next;
    });
  }, []);
  return [prefs, set];
}

/** Visões do painel lateral; `null` = tudo (faixa, grade e o fim do dia). */
export type ControlView = "attention" | "running" | "done";
