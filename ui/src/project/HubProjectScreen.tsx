import { useCallback, useEffect, useRef, useState } from "react";
import type { HubTree, HubTreeNode } from "../protocol/hub-gen";
import { useNav, type ItemIntent } from "../state/nav";
import { useTower } from "../state/tower";
import { HubSecrets } from "../hub/HubSecrets";
import { hubClones, liveSessions } from "./clones";
import { ProjectView } from "./ProjectView";
import { projectKey } from "./tabs";
import { cacheTree, cachedTree } from "./ui-store";
import { trackerLabel } from "../util/item";
import { splitProjectKey } from "../util/hub";

const same = (a: string, b: string) => a.toLowerCase() === b.toLowerCase();

/**
 * Tela Projeto pela visão do hub (spec do ledger centralizado §7): árvore,
 * status e resumo consolidados de todos os ambientes. Terminal e sessões
 * vivas continuam em cada daemon; a modal de início escolhe o ambiente (o
 * da atribuição) e manda `owner/repo` e o número.
 */
export function HubProjectScreen({ owner, repo, item }: { owner: string; repo: string; item?: ItemIntent }) {
  const tower = useTower();
  const { api, hub, web } = tower;
  const nav = useNav();
  const key = projectKey({ owner, repo });
  const [tree, setTree] = useState<HubTree | null>(() => cachedTree<HubTree>(key));
  const [error, setError] = useState<string | null>(null);
  // Secrets e variáveis do projeto (#165): seção aberta sob demanda.
  const [showSecrets, setShowSecrets] = useState(false);

  // Só a resposta da última chamada vale: uma antiga (ou de outro projeto) é descartada.
  const seq = useRef(0);

  const load = useCallback(async () => {
    if (!hub.connected) return;
    const id = ++seq.current;
    try {
      const t = await api.hubCall<HubTree>("hub.tree.get", { owner, repo });
      if (id !== seq.current) return;
      cacheTree(projectKey({ owner, repo }), t);
      setTree(t);
      setError(null);
    } catch (e) {
      if (id === seq.current) setError(String(e));
    }
  }, [api, hub.connected, owner, repo]);

  const shownKey = useRef(key);
  useEffect(() => {
    if (shownKey.current === key) return;
    shownKey.current = key;
    seq.current++;
    setTree(cachedTree<HubTree>(key));
  }, [key]);

  // A visão do hub muda com os envios dos ambientes: recarrega junto.
  useEffect(() => {
    void load();
  }, [hub.dashboard, load]);

  const clones = hubClones(tower, owner, repo);
  /** Sessões vivas do item em cada ambiente conectado (para abrir o terminal). */
  const liveOf = (gh: number) =>
    liveSessions(clones, gh).map((l) => ({ env: l.env, name: l.name, session: l.session.id }));
  // A árvore de outro projeto (troca em andamento) não serve.
  const mine = tree && same(tree.project.gh_owner, owner) && same(tree.project.gh_repo, repo) ? tree : null;
  const p = mine?.project ?? null;
  const projects = hub.dashboard?.projects.map((x) => x.project) ?? [];

  return (
    <ProjectView<HubTreeNode>
      addr={{ name: "project", owner, repo }}
      uiKey={key}
      name={p?.name ?? `${owner}/${repo}`}
      project={p}
      roots={mine?.roots ?? null}
      syncedAt={mine?.sync?.last_ok_at ?? null}
      error={error}
      item={item}
      liveOf={liveOf}
      liveText={(n) => {
        const envs = n.summary.live_envs.length > 0 ? n.summary.live_envs : liveOf(n.issue.gh_number).map((l) => l.name);
        return envs.length > 0 ? envs.join(", ") : null;
      }}
      whereOf={(n) => n.summary.live_envs}
      // Web: só com um ambiente online (canal reverso) que tenha o projeto.
      canStart={clones.length > 0}
      startBase={{ owner, repo }}
      emptyText={`Nenhuma issue no espelho do ${trackerLabel(p)} do hub ainda.`}
      reviewOf={(gh) => {
        // Web: a Revisão da web é a lista, com "Marcar como revisadas" (WebReview).
        if (web) return null;
        for (const c of clones) {
          const wi = c.items.find((w) => w.gh_number === gh);
          if (wi) return { name: "review", env: c.env, workItemId: wi.id };
        }
        return null;
      }}
      onReload={() => void load()}
      docs={clones}
      headExtra={
        <>
          <span className="hub-badge" title="Visão consolidada de todos os ambientes">
            visão do hub
          </span>
          {hub.connected && !web && (
            <button type="button" className="btn" aria-expanded={showSecrets} onClick={() => setShowSecrets((v) => !v)}>
              Secrets e variáveis
            </button>
          )}
        </>
      }
      banner={
        clones.length === 0 &&
        mine && (
          <p className="banner" role="status">
            {web
              ? "Nenhum ambiente com este projeto está online agora: dá para acompanhar, mas iniciar sessões volta quando um deles reconectar ao hub."
              : "Nenhum ambiente conectado tem este projeto atribuído: dá para acompanhar, mas não iniciar sessões. Atribua o projeto a um ambiente na tela Hub."}
          </p>
        )
      }
      below={
        showSecrets &&
        hub.connected &&
        !web && <HubSecrets project={{ owner: p?.gh_owner ?? owner, repo: p?.gh_repo ?? repo }} />
      }
      picker={
        projects.length > 1 ? (
          <label className="pj-pick">
            <span className="sr-only">Trocar de projeto</span>
            <select
              value={`${p?.gh_owner ?? owner}/${p?.gh_repo ?? repo}`}
              onChange={(e) => {
                // Na última barra: o `owner` do GitLab tem barras (`grupo/projeto`).
                nav.go({ name: "project", ...splitProjectKey(e.target.value) });
              }}
            >
              {projects.map((x) => (
                <option key={x.id} value={`${x.gh_owner}/${x.gh_repo}`}>
                  {x.name}
                </option>
              ))}
            </select>
          </label>
        ) : (
          <p className="pj-name">
            {owner}/{repo}
          </p>
        )
      }
    />
  );
}
