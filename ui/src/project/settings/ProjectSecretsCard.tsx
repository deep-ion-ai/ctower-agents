import { useState } from "react";
import { useNav } from "../../state/nav";
import type { HubProject } from "../../protocol/hub-gen";
import { DELIVERY_TEXT, HubSecrets } from "../../hub/HubSecrets";
import { hubTab } from "../../hub/sections";
import { projectSecretsOf, type ProjectSecrets } from "./useSecrets";

/**
 * Secrets e variáveis do projeto e a entrega por ambiente. "Gerenciar" abre
 * no cartão o editor do escopo do projeto (`HubSecrets project=`, #8); ao
 * abrir e ao fechar, o resumo é relido.
 */
export function ProjectSecretsCard({ project: pr, secrets }: { project: HubProject; secrets: ProjectSecrets }) {
  const nav = useNav();
  const [editing, setEditing] = useState(false);
  const { own, delivery } = projectSecretsOf(secrets, pr.gh_owner, pr.gh_repo);
  return (
    <section className="hub-card" aria-labelledby={`pc-sec-${pr.id}`}>
      <div className="hub-card-head">
        <h3 id={`pc-sec-${pr.id}`}>Secrets e variáveis do projeto · {own.length}</h3>
        <button
          type="button"
          className="btn btn-sm"
          aria-expanded={editing}
          onClick={() => {
            setEditing(!editing);
            void secrets.reload();
          }}
        >
          Gerenciar secrets do projeto
        </button>
        <button type="button" className="btn btn-sm" onClick={() => nav.openTab(hubTab("secrets"))}>
          Abrir Secrets e variáveis
        </button>
      </div>
      {editing ? (
        <HubSecrets project={{ owner: pr.gh_owner, repo: pr.gh_repo }} />
      ) : secrets.error ? (
        <p className="muted hub-card-msg">Secrets indisponíveis: {secrets.error}</p>
      ) : !secrets.list ? (
        <p className="muted hub-card-msg">Carregando…</p>
      ) : (
        <>
          {own.length === 0 ? (
            <p className="muted hub-card-empty">Nenhum secret ou variável próprio; o projeto herda os globais.</p>
          ) : (
            <div className="table-wrap">
              <table className="hub-card-table" aria-label={`Secrets e variáveis de ${pr.name}`}>
                <thead>
                  <tr>
                    <th scope="col">Nome</th>
                    <th scope="col">Tipo</th>
                    <th scope="col">Entrega</th>
                  </tr>
                </thead>
                <tbody>
                  {own.map((i) => {
                    // Chegou ao ambiente que confirmou uma revisão igual ou posterior à da entrada.
                    const got = delivery.filter(
                      (d) => d.state !== "no_key" && d.applied_rev !== null && d.applied_rev >= i.rev,
                    ).length;
                    return (
                      <tr key={i.name}>
                        <td className="mono">{i.name}</td>
                        <td>{i.kind === "secret" ? "secret" : "variável"}</td>
                        <td className="nowrap">
                          {got}/{delivery.length} ambientes · rev {i.rev}
                        </td>
                      </tr>
                    );
                  })}
                </tbody>
              </table>
            </div>
          )}
          {delivery.length > 0 && (
            <ul className="hub-envs" aria-label={`Entrega dos secrets de ${pr.name} por ambiente`}>
              {delivery.map((d) => (
                <li key={d.env_id}>
                  <strong>{d.env_name}</strong>
                  <span className="hub-state" data-state={d.state}>
                    {DELIVERY_TEXT[d.state]}
                  </span>
                </li>
              ))}
            </ul>
          )}
        </>
      )}
    </section>
  );
}
