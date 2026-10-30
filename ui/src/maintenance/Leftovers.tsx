import { useEffect, useState } from "react";
import { useNow } from "../App";
import type { Project } from "../protocol/gen";
import type { ProjectRef } from "../hub/sections";
import { useTower } from "../state/tower";
import { ago } from "../util/time";
import { projectFilterKey } from "./sections";

/**
 * Manutenção › Limpeza › Fora do hub (#337, antes em Configurações ›
 * Projetos locais): projetos desatribuídos no hub e os locais anteriores a
 * ele, cujo clone e worktrees ficaram no disco do ambiente `env`. O
 * ambiente não os apaga: a lista é o que resta limpar à mão. Com `project`
 * (o filtro da Limpeza, #22), só a linha desse projeto, se houver.
 */
export function Leftovers({ env, project }: { env: string; project?: ProjectRef }) {
  const { status, infos, api } = useTower();
  const now = useNow();
  const current = status[env]?.connected ? env : undefined;
  // `null` = ainda não lido.
  const [all, setOutside] = useState<Project[] | null>(null);
  const filterKey = project ? projectFilterKey(project) : null;
  const outside =
    all && filterKey !== null
      ? all.filter((p) => projectFilterKey({ owner: p.gh_owner, repo: p.gh_repo }) === filterKey)
      : all;
  const rev = current ? infos[current]?.hub_config?.config_rev : undefined;

  // Relê quando a configuração do ambiente muda.
  useEffect(() => {
    setOutside(null);
    if (!current) return;
    let alive = true;
    api.call<Project[]>(current, "projects.list", { unassigned: true }).then(
      (list) => alive && setOutside(list),
      () => alive && setOutside([]),
    );
    return () => {
      alive = false;
    };
  }, [api, current, rev]);

  return (
    <section className="panel settings-section set-stack" aria-labelledby="maint-leftovers">
      <h2 id="maint-leftovers">Fora do hub · o que restou para limpar à mão</h2>
      {!current ? (
        <p className="muted">Conecte o ambiente para ver o que ficou fora do hub.</p>
      ) : outside === null ? (
        <p className="muted">Carregando…</p>
      ) : outside.length === 0 ? (
        <p className="muted">
          {project ? `${project.owner}/${project.repo} não está fora do hub neste ambiente.` : "Nada fora do hub neste ambiente."}
        </p>
      ) : (
        <div className="table-wrap">
          <table aria-label="Projetos fora do hub">
            <thead>
              <tr>
                <th>Nome</th>
                <th>Repositório</th>
                <th>Clone na distro</th>
                <th>Situação</th>
              </tr>
            </thead>
            <tbody>
              {outside.map((p) => (
                <tr key={p.id}>
                  <td className="strong">{p.name}</td>
                  <td className="mono">
                    {p.gh_owner}/{p.gh_repo}
                  </td>
                  <td className="mono">{p.repo_path}</td>
                  <td>
                    {p.config_rev === null
                      ? "projeto local fora do hub"
                      : `atribuição removida${p.unassigned_at ? ` ${ago(p.unassigned_at, now)}` : ""}`}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
      <p className="muted hint">
        O ambiente não apaga clones nem worktrees de projetos fora do hub: remova-os à mão quando não precisar mais. Para
        voltar a executar um desses projetos, atribua-o a este ambiente no hub.
      </p>
    </section>
  );
}
