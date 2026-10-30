import { useState } from "react";
import type { ProjectView } from "./TreeControls";

const KEY = "ct.project.view";

/** Lê a visão guardada na sessão do navegador; qualquer falha cai na árvore. */
function read(): ProjectView {
  try {
    return window.sessionStorage.getItem(KEY) === "board" ? "board" : "tree";
  } catch {
    return "tree";
  }
}

/** Escolha Árvore | Kanban: persiste durante a sessão da tela (sessionStorage, com fallback em memória). */
export function useViewChoice(): [ProjectView, (v: ProjectView) => void] {
  const [view, setView] = useState<ProjectView>(read);
  const set = (v: ProjectView) => {
    setView(v);
    try {
      window.sessionStorage.setItem(KEY, v);
    } catch {
      /* sem storage: vale só enquanto a tela está aberta */
    }
  };
  return [view, set];
}
