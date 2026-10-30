import { maintenanceTab, projectFilterKey } from "../../maintenance/sections";
import type { HubProjectView } from "../../protocol/hub-gen";
import { useNav } from "../../state/nav";
import { useTower } from "../../state/tower";
import { keyOf } from "./form";

/**
 * Limpeza do projeto (#22) como seção das Configurações do projeto: um
 * atalho, não uma segunda Limpeza. Abre Manutenção › Limpeza já filtrada
 * pelo projeto, num clique, no primeiro ambiente deste app que tem o
 * projeto (o seletor de lá troca de ambiente sem perder o filtro). As
 * regras, o plano e o limpar ficam só lá.
 */
export function CleanupCard({ project: p }: { project: HubProjectView }) {
  const { envs, dashboards, status } = useTower();
  const nav = useNav();
  const pr = p.project;
  const ref = { owner: pr.gh_owner, repo: pr.gh_repo };
  const want = projectFilterKey(ref);
  const has = (env: string) =>
    !!dashboards[env]?.projects.some((x) => projectFilterKey({ owner: x.project.gh_owner, repo: x.project.gh_repo }) === want);
  const local = envs.filter((e) => has(e.id));
  // O conectado primeiro; sem ambiente com o projeto, o critério padrão da Manutenção.
  const env = (local.find((e) => status[e.id]?.connected) ?? local[0])?.id;

  return (
    <section className="hub-card" aria-labelledby={`pc-cleanup-${pr.id}`}>
      <div className="hub-card-head">
        <h3 id={`pc-cleanup-${pr.id}`}>Limpeza</h3>
      </div>
      <p className="muted">
        Worktrees, <span className="mono">target/</span> e branches dos itens concluídos de{" "}
        <span className="mono">{keyOf(pr.gh_owner, pr.gh_repo)}</span>: o plano, as regras e o limpar ficam em Manutenção ›
        Limpeza, por ambiente.
        {local.length === 0 && " Nenhum ambiente deste app tem o projeto agora."}
      </p>
      <div>
        <button type="button" className="btn" onClick={() => nav.openTab(maintenanceTab("cleanup", env, ref))}>
          Abrir a limpeza do projeto ›
        </button>
      </div>
    </section>
  );
}
