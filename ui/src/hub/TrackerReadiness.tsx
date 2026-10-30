import type { HubProject, HubTrackerStatus } from "../protocol/hub-gen";
import { ago } from "../util/time";
import { readinessChecks, type ReadinessCheck } from "./tracker";

function Checks({ title, checks, error }: { title: string; checks: ReadinessCheck[]; error: string | null }) {
  return (
    <div className="readiness-group">
      <h4>{title}</h4>
      <ul className="readiness" aria-label={title}>
        {checks.map((c) => (
          <li key={c.label} data-ok={c.ok ? "true" : "false"}>
            <span className="hub-dot" data-tone={c.ok ? "ok" : "error"} aria-hidden="true" />{" "}
            <span>
              {c.label}
              {c.ok ? (c.value ? <span className="muted mono"> · {c.value}</span> : null) : <span className="sr-only"> (falta)</span>}
              {!c.ok && c.fix && <span className="hub-card-sub">{c.fix}</span>}
            </span>
          </li>
        ))}
      </ul>
      {error && <p className="hub-card-sub start-error">{error}</p>}
    </div>
  );
}

/**
 * Prontidão do ambiente de sync para o projeto Jira + GitLab (spec §8.4):
 * o que ele conferiu no último ciclo (jira-cli, `.config.yml`, comando de
 * credencial, `jira me`, glab e API do GitLab) e, para cada item que falta,
 * a instrução de configuração. Com um ciclo lendo o Jira, o progresso
 * (`readiness.progress`, enviado no máximo a cada 30 s; com `total` 0, ainda
 * listando as chaves). Sem relato ainda: aguardando o ambiente de
 * sync. O hub não alcança o Jira nem o GitLab; só mostra o relato.
 */
export function TrackerReadiness({
  project,
  status,
  now,
}: {
  project: HubProject;
  /** Último `tracker_status` aceito; `null` = o ambiente de sync ainda não relatou. */
  status: HubTrackerStatus | null;
  now: number;
}) {
  if (!status) {
    return (
      <p className="muted hub-card-msg" role="status">
        Aguardando o ambiente de sync: a prontidão do jira-cli e do glab chega no primeiro ciclo de sync dele.
      </p>
    );
  }
  const r = status.readiness;
  const c = readinessChecks(r, project);
  const missing = [...c.jira, ...c.gitlab].filter((x) => !x.ok).length;
  // Ciclo lendo agora (ausente no ambiente anterior ao progresso).
  const progress = r.progress ?? null;
  return (
    <div className="hub-card-msg" aria-label={`Prontidão de ${status.env_name}`}>
      <p className="hub-card-sub">
        Conferido por <strong>{status.env_name}</strong> {r.checked_at > 0 ? ago(r.checked_at, now) : ago(status.received_at, now)}
        {" · "}
        {missing === 0 ? "tudo pronto" : `${missing} item(ns) a configurar no ambiente`}
      </p>
      {progress && (
        <p className="hub-card-sub" role="status">
          {progress.total > 0
            ? `Sincronizando… ${progress.done} de ${progress.total} issues lidas`
            : `Sincronizando… listando as issues, ${progress.done} até agora`}
          {progress.started_at > 0 && <span className="muted"> · começou {ago(progress.started_at, now)}</span>}
        </p>
      )}
      <div className="readiness-cols">
        <Checks title="Jira" checks={c.jira} error={r.jira.error} />
        <Checks title="GitLab" checks={c.gitlab} error={r.gitlab.error} />
      </div>
    </div>
  );
}
