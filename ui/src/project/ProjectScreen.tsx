import { useCallback, useEffect, useRef, useState } from "react";
import { hubTab } from "../hub/sections";
import type { ProjectTree, TreeNode } from "../protocol/gen";
import { settingsTab } from "../settings/sections";
import { useNav, type ItemIntent } from "../state/nav";
import { useTower } from "../state/tower";
import { changeLabel, trackerLabel } from "../util/item";
import { useNow } from "../App";
import { itemTitle } from "../util/item";
import { localClone, liveSessions } from "./clones";
import { ProjectView, useGoPage } from "./ProjectView";
import { ProjectOverview } from "./ProjectOverview";
import { ProjectSyncCard } from "./OverviewCards";
import { syncPlan } from "./overview";
import { ProjectSessions } from "./ProjectSessions";
import type { ProjectPage } from "./sections";
import { projectSessions, sessionCounts } from "./sessions";
import { syncText } from "./settings";
import { withSummaries } from "./summary";
import { projectKey } from "./tabs";
import { cacheTree, cachedTree } from "./ui-store";

/**
 * Tela Projeto pelos dados locais de um ambiente (hub fora: visão parcial).
 * Lê `tree.get` e entrega à visão comum (`ProjectView`).
 */
export function ProjectScreen(props: {
  env?: string;
  projectId?: number;
  item?: ItemIntent;
  /** Aba do projeto (#17); sem ela, Itens. A seção das Configurações não serve aqui (sem o hub). */
  view?: ProjectPage;
}) {
  const tower = useTower();
  const { envs, dashboards, api, hub } = tower;
  const nav = useNav();
  const now = useNow();

  // Sem seleção: o primeiro projeto do primeiro ambiente com dashboard.
  const env = props.env ?? envs.find((e) => dashboards[e.id]?.projects.length)?.id;
  const projectId = props.projectId ?? (env ? dashboards[env]?.projects[0]?.project.id : undefined);
  const key = projectKey({ env, projectId });
  // Hooks antes do retorno antecipado (sem projeto): o endereço vale mesmo sem ele.
  const goPage = useGoPage({ name: "project", env, projectId });

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
        {/* O cadastro é no Hub: sem ele no ar, o caminho é configurar a conexão. */}
        <p className="empty">
          {hub.connected
            ? "Nenhum projeto cadastrado. O cadastro fica no Hub, em Projetos e atribuições."
            : "Nenhum projeto cadastrado. O cadastro fica no Hub, que está fora do ar ou sem conexão configurada."}
        </p>
        <div className="form-actions">
          {hub.connected ? (
            <button
              type="button"
              className="btn btn-primary"
              onClick={() => nav.openTab(hubTab("projects", { register: { url: "" } }))}
            >
              + Novo projeto
            </button>
          ) : (
            <button type="button" className="btn btn-primary" onClick={() => nav.openTab(settingsTab({ section: "connections" }))}>
              Configurar o hub
            </button>
          )}
        </div>
      </main>
    );
  }

  const projects = envs.flatMap((e) =>
    (dashboards[e.id]?.projects ?? []).map((p) => ({ env: e.id, project: p.project })),
  );
  const clone = localClone(tower, env, projectId);
  const clones = clone ? [clone] : [];
  const liveOf = (gh: number) =>
    liveSessions(clones, gh).map((l) => ({ env: l.env, name: l.name, session: l.session.id }));
  const mine = tree && tree.project.id === projectId ? tree : null;
  const p = mine?.project ?? clone?.project ?? null;
  const name = p?.name ?? "Projeto";
  const sessions = () => projectSessions(clones, dashboards);
  const sync = mine?.sync ?? dash?.sync.find((x) => x.project_id === projectId) ?? null;

  return (
    <ProjectView<TreeNode>
      addr={{ name: "project", env, projectId, ...(p ? { owner: p.gh_owner, repo: p.gh_repo } : {}) }}
      page={props.item ? "items" : props.view}
      pages={{
        // Versão local (hub fora): sem atribuições nem estado consolidado.
        overview: () => {
          const groups = sessions();
          return (
            <ProjectOverview
              state={null}
              sync={{ ...syncText(sync, now), error: sync?.last_error ?? null }}
              assignments={null}
              localEnv={clone?.name ?? env}
              live={groups.flatMap((g) =>
                g.live.map((s) => ({
                  id: s.id,
                  label: itemTitle({ gh_number: s.gh, title: s.title }, p),
                  envName: g.name,
                  open: () => nav.openTerminal({ env, session: s.id }),
                })),
              )}
              counts={sessionCounts(groups)}
              onPage={goPage}
              // Hub fora (#19): só o sync, no próprio ambiente. Saúde e gestor
              // dependem do hub (os sinais são calculados lá; o gestor lê o hub).
              extra={<ProjectSyncCard plan={syncPlan(clones, null)} change={changeLabel(p)} />}
            />
          );
        },
        sessions: () => (
          <ProjectSessions
            groups={sessions()}
            project={p}
            now={now}
            emptyText="O ambiente deste projeto não está conectado."
          />
        ),
        // As configurações do projeto ficam no hub (cadastro, atribuições,
        // regras, secrets, docs): sem ele, só o que o ambiente guarda.
        settings: () => (
          <div className="pj-settings-off">
            <p className="muted" role="status">
              As configurações do projeto ficam no hub, que está fora do ar ou sem conexão configurada. Elas voltam
              aqui quando ele responder; o ambiente segue com a última configuração recebida.
            </p>
            <div className="form-actions">
              <button
                type="button"
                className="btn"
                onClick={() => nav.openTab(settingsTab({ section: "local-projects", env }))}
              >
                Projetos locais de {clone?.name ?? env}
              </button>
              {!hub.connected && (
                <button type="button" className="btn" onClick={() => nav.openTab(settingsTab({ section: "connections" }))}>
                  Configurar o hub
                </button>
              )}
            </div>
          </div>
        ),
      }}
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
      picker={
        projects.length > 1 ? (
          <label className="pj-pick">
            <span className="sr-only">Trocar de projeto</span>
            <select
              value={`${env}|${projectId}`}
              onChange={(e) => {
                const [en, id] = e.target.value.split("|");
                nav.go({ name: "project", env: en, projectId: Number(id) });
              }}
            >
              {projects.map((x) => (
                <option key={`${x.env}|${x.project.id}`} value={`${x.env}|${x.project.id}`}>
                  {x.project.name}
                </option>
              ))}
            </select>
          </label>
        ) : (
          <p className="pj-name">{p ? `${p.gh_owner}/${p.gh_repo}` : name}</p>
        )
      }
    />
  );
}
