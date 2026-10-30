import { useNow } from "../App";
import type { Project } from "../protocol/gen";
import { useTower } from "../state/tower";
import { configLine } from "../util/hub";

/** Estado da atribuição como o ambiente a aplicou (clone ou validação). */
function applied(p: Project): string {
  if (p.config_rev === null) return "local (anterior ao hub)";
  if (p.location_state === "ok") return "aplicada";
  if (p.location_state === "error") return `com erro: ${p.location_error ?? "?"}`;
  return "pendente";
}

/**
 * Projetos do ambiente, somente leitura: são cadastrados e atribuídos no hub
 * (#178); o ambiente só mostra o cache da configuração vinda dele e a idade
 * dela. O ambiente vem do seletor de escopo das Configurações (#315). O que
 * ficou fora do hub está em Manutenção › Limpeza (#337).
 */
export function ProjectsSettings({ env }: { env: string }) {
  const { status, dashboards, infos } = useTower();
  const now = useNow();
  const current = status[env]?.connected ? env : undefined;

  if (!current) {
    return <p className="muted">Conecte o ambiente para ver os projetos atribuídos a ele.</p>;
  }

  const projects: Project[] = (dashboards[current]?.projects ?? []).map((p) => p.project);
  const unlinked = infos[current]?.hub_config?.linked === false;
  const config = unlinked ? null : configLine(infos[current], now);

  return (
    <div className="set-stack">
      {unlinked && (
        <p role="status" className="state-off">
          Ambiente não ligado a um hub: configure <span className="mono">hub_url</span> e grave o token com{" "}
          <span className="mono">ctower hub set-token</span>. Sem hub, o ambiente não tem projetos nem inicia sessões.
        </p>
      )}
      {config && (
        <p role="status" className={config.warn ? "state-off" : "muted"}>
          {config.text[0].toUpperCase() + config.text.slice(1)}
          {config.text.endsWith("pull falhou") ? `: ${infos[current]?.hub_config?.last_error}` : ""}
        </p>
      )}
      <div className="table-wrap">
        <table>
          <thead>
            <tr>
              <th>Nome</th>
              <th>Repositório</th>
              <th>Caminho na distro</th>
              <th>Base</th>
              <th>Protegidas</th>
              <th>Atribuição</th>
            </tr>
          </thead>
          <tbody>
            {projects.map((p) => (
              <tr key={p.id}>
                <td className="strong">{p.name}</td>
                <td className="mono">
                  {p.gh_owner}/{p.gh_repo}
                </td>
                <td className="mono">{p.repo_path}</td>
                <td className="mono">{p.base_branch}</td>
                <td className="mono">{p.protected_branches.join(", ")}</td>
                <td>{applied(p)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      <p className="muted hint">
        Projetos são cadastrados, editados e atribuídos aos ambientes no hub (<span className="mono">cthubd</span>). O
        ambiente puxa a configuração dos atribuídos a ele e não cria nem edita projetos.
      </p>
    </div>
  );
}
