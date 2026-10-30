import { useCallback, useEffect, useRef, useState } from "react";
import type { ProjectTree, TreeNode } from "../protocol/gen";
import { useNav, type ItemIntent } from "../state/nav";
import { useTower } from "../state/tower";
import { trackerLabel } from "../util/item";
import { localClone, liveSessions } from "./clones";
import { ProjectTrail } from "./ProjectIndex";
import { ProjectView } from "./ProjectView";
import { withSummaries } from "./summary";
import { projectKey } from "./tabs";
import { cacheTree, cachedTree } from "./ui-store";

/**
 * Tela Projeto pelos dados locais de um ambiente (hub fora: visão parcial).
 * Lê `tree.get` e entrega à visão comum (`ProjectView`).
 */
export function ProjectScreen(props: { env?: string; projectId?: number; item?: ItemIntent }) {
  const tower = useTower();
  const { dashboards, api, hub } = tower;
  const nav = useNav();

  // Sem projeto não se chega aqui: o endereço sem chave é o índice (#16), e
  // o do hub com o hub fora sem projeto local correspondente cai no aviso abaixo.
  const { env, projectId } = props;
  const key = projectKey({ env, projectId });

  const [tree, setTree] = useState<ProjectTree | null>(() => cachedTree<ProjectTree>(key));
  const [error, setError] = useState<string | null>(null);

  // Só a resposta da última chamada vale: uma antiga (ou de outro projeto) é descartada.
  const seq = useRef(0);

  const load = useCallback(async () => {
    if (!env || projectId === undefined) return;
    const id = ++seq.current;
    try {
      const t = withSummaries(await api.call<ProjectTree>(env, "tree.get", { project_id: projectId }));
      if (id !== seq.current) return;
      cacheTree(projectKey({ env, projectId }), t);
      setTree(t);
      setError(null);
    } catch (e) {
      if (id === seq.current) setError(String(e));
    }
  }, [api, env, projectId]);

  // Troca de projeto: a árvore do anterior sai (a última deste, se houver, entra).
  const shownKey = useRef(key);
  useEffect(() => {
    if (shownKey.current === key) return;
    shownKey.current = key;
    seq.current++;
    setTree(cachedTree<ProjectTree>(key));
  }, [key]);

  // Carrega e recarrega quando o dashboard do ambiente muda (pushes de status).
  const dash = env ? dashboards[env] : undefined;
  useEffect(() => {
    void load();
  }, [dash, load]);

  if (!env || projectId === undefined) {
    return (
      <main className="page">
        <h1 className="section-title">Projeto</h1>
        <p className="empty">
          {hub.connected
            ? "Este projeto não está em nenhum ambiente conectado."
            : "Este projeto não está em nenhum ambiente conectado, e o Hub, que tem a visão consolidada, está fora do ar."}
        </p>
        <div className="form-actions">
          <button type="button" className="btn btn-primary" onClick={() => nav.go({ name: "project" })}>
            Ver todos os projetos
          </button>
        </div>
      </main>
    );
  }

  const clone = localClone(tower, env, projectId);
  const clones = clone ? [clone] : [];
  const liveOf = (gh: number) =>
    liveSessions(clones, gh).map((l) => ({ env: l.env, name: l.name, session: l.session.id }));
  const mine = tree && tree.project.id === projectId ? tree : null;
  const p = mine?.project ?? clone?.project ?? null;
  const name = p?.name ?? "Projeto";

  return (
    <ProjectView<TreeNode>
      addr={{ name: "project", env, projectId, ...(p ? { owner: p.gh_owner, repo: p.gh_repo } : {}) }}
      uiKey={key}
      name={name}
      project={p}
      roots={mine?.roots ?? null}
      syncedAt={mine?.sync?.last_ok_at ?? null}
      error={error}
      item={props.item}
      liveOf={liveOf}
      liveText={(n) => (liveOf(n.issue.gh_number).length > 0 ? (clone?.name ?? env) : null)}
      canStart={clone !== null}
      startBase={{ env, projectId }}
      emptyText={`Nenhuma issue aberta no espelho do ${trackerLabel(p)}. Use "Sincronizar agora".`}
      reviewOf={(gh) => {
        const wi = clone?.items.find((w) => w.gh_number === gh);
        return wi ? { name: "review", env, workItemId: wi.id } : null;
      }}
      onReload={() => void load()}
      docs={clones}
      // Trilha "Projetos › X" (#16): trocar de projeto é pelo índice.
      picker={<ProjectTrail name={name} title={p ? `${p.gh_owner}/${p.gh_repo}` : undefined} />}
    />
  );
}
