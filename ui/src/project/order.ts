// Prioridade entre projetos (`rank` do hub, desempata o ranking do
// portfólio): arrastar, Alt+↑/↓ e os botões "subir/descer". Saiu da lista do
// Hub › Projetos (#314) para o índice da atividade Projetos (#16); as duas
// listas usam o mesmo gancho enquanto a seção do Hub existir.
import { useState, type DragEvent, type HTMLAttributes, type KeyboardEvent } from "react";
import type { HubProject } from "../protocol/hub-gen";
import { useTower } from "../state/tower";
import { keyOf, type SettingsRun } from "./settings";

/** Onde soltar o projeto arrastado: antes ou depois do alvo. */
export interface Drop {
  key: string;
  where: "before" | "after";
}

export interface ProjectOrder {
  /** Vizinhos de `pr` na ordem do hub (os botões do detalhe desligam sem eles). */
  neighbors(pr: HubProject): { prev?: HubProject; next?: HubProject };
  /** Sobe ou desce um lugar. */
  step(pr: HubProject, up: boolean): void;
  /** Alt+↑/↓ no elemento focável da linha. */
  onKey(e: KeyboardEvent, pr: HubProject): void;
  /** Atributos de arrasto do `<li>` da linha (`data-drop` e `data-dragging` para o CSS). */
  itemProps(pr: HubProject): HTMLAttributes<HTMLLIElement> & { draggable: boolean; "data-drop"?: string; "data-dragging"?: string };
}

/**
 * Ordem dos `projects` (na ordem do hub). `enabled` = pode reordenar (não na
 * web, que é só leitura); `settings` dá o `run` (erro e mensagem de quem
 * hospeda) e o `busy` (sem mover outro enquanto um movimento roda).
 */
export function useProjectOrder(projects: HubProject[], settings: SettingsRun, enabled: boolean): ProjectOrder {
  const { api } = useTower();
  const { busy, run } = settings;
  const [dragging, setDragging] = useState<string | null>(null);
  const [drop, setDrop] = useState<Drop | null>(null);

  // `pr` vai para antes ou depois de `other`.
  const move = (pr: HubProject, to: Drop["where"], other: HubProject) =>
    void run(async () => {
      await api.hubCall("hub.projects.move", {
        owner: pr.gh_owner,
        repo: pr.gh_repo,
        [to]: { owner: other.gh_owner, repo: other.gh_repo },
      });
      const i = projects.findIndex((p) => p.id === pr.id);
      const j = projects.findIndex((p) => p.id === other.id);
      return `${pr.name} ${j < i ? "subiu" : "desceu"} na ordem dos projetos.`;
    });

  const neighbors = (pr: HubProject) => {
    const i = projects.findIndex((p) => p.id === pr.id);
    return { prev: projects[i - 1], next: projects[i + 1] };
  };

  const step = (pr: HubProject, up: boolean) => {
    const { prev, next } = neighbors(pr);
    if (up && prev) move(pr, "before", prev);
    if (!up && next) move(pr, "after", next);
  };

  const onKey = (e: KeyboardEvent, pr: HubProject) => {
    if (!enabled || !e.altKey || (e.key !== "ArrowUp" && e.key !== "ArrowDown") || busy) return;
    e.preventDefault();
    step(pr, e.key === "ArrowUp");
  };

  // Arrasto (HTML5): metade de cima do alvo = antes; de baixo = depois.
  const dropWhere = (e: DragEvent<HTMLElement>): Drop["where"] => {
    const r = e.currentTarget.getBoundingClientRect();
    return e.clientY < r.top + r.height / 2 ? "before" : "after";
  };
  const onDrop = (target: HubProject, where: Drop["where"]) => {
    const from = projects.find((p) => keyOf(p.gh_owner, p.gh_repo) === dragging);
    setDragging(null);
    setDrop(null);
    if (!from || from.id === target.id || busy) return;
    const { prev, next } = neighbors(target);
    // Soltar ao lado de onde já está não muda a ordem.
    if ((where === "before" && prev?.id === from.id) || (where === "after" && next?.id === from.id)) return;
    move(from, where, target);
  };

  const itemProps = (pr: HubProject) => {
    const key = keyOf(pr.gh_owner, pr.gh_repo);
    return {
      draggable: enabled && !busy,
      "data-drop": drop?.key === key ? drop.where : undefined,
      "data-dragging": dragging === key ? "true" : undefined,
      onDragStart: (e: DragEvent<HTMLLIElement>) => {
        setDragging(key);
        try {
          e.dataTransfer.effectAllowed = "move";
          e.dataTransfer.setData("text/plain", key);
        } catch {
          // Sem dataTransfer (testes): o estado basta.
        }
      },
      onDragOver: (e: DragEvent<HTMLLIElement>) => {
        if (!dragging || dragging === key) return;
        e.preventDefault();
        const where = dropWhere(e);
        if (drop?.key !== key || drop.where !== where) setDrop({ key, where });
      },
      onDragLeave: () => {
        if (drop?.key === key) setDrop(null);
      },
      onDrop: (e: DragEvent<HTMLLIElement>) => {
        e.preventDefault();
        onDrop(pr, dropWhere(e));
      },
      onDragEnd: () => {
        setDragging(null);
        setDrop(null);
      },
    };
  };

  return { neighbors, step, onKey, itemProps };
}
