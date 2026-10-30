import { useEffect, useState } from "react";
import type { HealthKind, HealthSignal, HubHealth } from "../protocol/hub-gen";
import { useNav } from "../state/nav";
import { useTower } from "../state/tower";
import { changeLabel, itemFullRef, trackerLabel } from "../util/item";
import { hubProjectOf } from "../util/hub";
import type { HubProject } from "../protocol/hub-gen";
import { errText } from "./calls";

export const KIND_TEXT: Record<HealthKind, string> = {
  parado: "parado",
  esperando_voce: "esperando você",
  bloqueado: "bloqueado",
  pausado_longo: "pausado há muito tempo",
  pr_sem_andamento: "PR sem andamento",
  decisoes_sem_revisao: "decisões sem revisão",
  sessoes_falhas: "sessões falhas",
  sessao_interrompida: "sessão interrompida",
  p0_sem_andamento: "P0 sem andamento",
  epico_sem_sessao: "épico sem sessão",
  nao_triado: "issues não triadas",
  ambiente_mudo: "ambiente sem enviar",
  github_sem_sync: "GitHub sem sync",
  rastreador_sem_sync: "Jira sem sync",
};

export const SEVERITY_TEXT = { critico: "crítico", aviso: "aviso", info: "info" } as const;
export const SEVERITY_TONE = { critico: "error", aviso: "attention", info: "neutral" } as const;

/** `hub.health.get` relido junto com a visão do hub (a cada envio dos ambientes). */
export function useHubHealth(): { health: HubHealth | null; error: string | null } {
  const { api, hub } = useTower();
  const [health, setHealth] = useState<HubHealth | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    if (!hub.connected) return;
    let alive = true;
    api.hubCall<HubHealth>("hub.health.get", { min_severity: "info" }).then(
      (h) => {
        if (!alive) return;
        setHealth(h);
        setError(null);
      },
      (e) => alive && setError(errText(e)),
    );
    return () => {
      alive = false;
    };
  }, [api, hub.connected, hub.dashboard]);
  return { health, error };
}

/** Contagem por gravidade dos sinais de um projeto. */
export function projectCounts(h: HubHealth | null, owner: string, repo: string) {
  const out = { critico: 0, aviso: 0, info: 0 };
  for (const s of h?.signals ?? []) {
    if (s.owner?.toLowerCase() === owner.toLowerCase() && s.repo?.toLowerCase() === repo.toLowerCase()) {
      out[s.severity] += 1;
    }
  }
  return out;
}

/**
 * Texto do tipo de sinal no provedor do projeto (#352): "MR sem andamento"
 * no GitLab, "GitHub sem sync" ou "Jira sem sync" pelo rastreador.
 */
export function kindText(s: Pick<HealthSignal, "kind">, project?: HubProject | null): string {
  if (s.kind === "pr_sem_andamento") return `${changeLabel(project)} sem andamento`;
  if ((s.kind === "github_sem_sync" || s.kind === "rastreador_sem_sync") && project) return `${trackerLabel(project)} sem sync`;
  return KIND_TEXT[s.kind];
}

/** Onde: a referência pronta do hub (`owner/repo#n` ou `PROJ-n`), o projeto ou o ambiente. */
export function where(s: HealthSignal, project?: HubProject | null): string {
  if (s.owner && s.repo) {
    if (s.number === null) return `${s.owner}/${s.repo}`;
    return s.ref ?? itemFullRef(project ?? { gh_owner: s.owner, gh_repo: s.repo }, s.number);
  }
  return s.env ?? "hub";
}

/**
 * Saúde e riscos do portfólio (spec do gestor §5 e §11): os sinais do hub
 * por gravidade, com link para o projeto do item. Calculados no hub a cada
 * leitura, com os limiares de `[health]` do `cthubd`.
 */
export function HealthPanel({ health, error }: { health: HubHealth | null; error: string | null }) {
  const nav = useNav();
  const { hub } = useTower();
  const projectOf = (s: HealthSignal) => hubProjectOf(hub.projects, s.owner, s.repo);
  const [showInfo, setShowInfo] = useState(false);
  const signals = (health?.signals ?? []).filter((s) => showInfo || s.severity !== "info");
  const c = health?.counts;
  return (
    <section aria-labelledby="hub-saude" className="panel settings-section">
      <h2 id="hub-saude">
        Saúde e riscos{c ? ` · ${c.critico} crítico(s), ${c.aviso} aviso(s)` : ""}
      </h2>
      {error && (
        <p role="alert" className="start-error">
          {error}
        </p>
      )}
      {health && signals.length === 0 ? (
        <p className="muted">Nenhum sinal{showInfo ? "" : " de aviso ou crítico"}: nada parado, bloqueado ou falhando.</p>
      ) : (
        <ul className="hub-list health">
          {signals.map((s, i) => (
            <li key={`${s.kind}:${where(s)}:${i}`} data-severity={s.severity}>
              <span className="badge" data-tone={SEVERITY_TONE[s.severity]}>
                {SEVERITY_TEXT[s.severity]}
              </span>
              <span>{kindText(s, projectOf(s))}</span>
              {s.owner && s.repo ? (
                <button
                  type="button"
                  className="linklike mono"
                  onClick={() => nav.go({ name: "project", owner: s.owner!, repo: s.repo! })}
                >
                  {where(s, projectOf(s))}
                </button>
              ) : (
                <span className="mono">{where(s)}</span>
              )}
              {s.priority && <span className="prio" data-p={s.priority}>{s.priority}</span>}
              {s.title && <span className="muted">{s.title}</span>}
              <span>
                {s.value}
                {s.threshold ? <span className="muted"> (limiar {s.threshold})</span> : null}
              </span>
              {s.detail && <span className="muted small">{s.detail}</span>}
            </li>
          ))}
        </ul>
      )}
      <label className="ctl">
        <input type="checkbox" checked={showInfo} onChange={(e) => setShowInfo(e.target.checked)} />
        mostrar informativos
      </label>
    </section>
  );
}
