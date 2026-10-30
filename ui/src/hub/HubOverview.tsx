import { useState, type ReactNode } from "react";
import type { HealthSignal, HubHealth } from "../protocol/hub-gen";
import { useNav } from "../state/nav";
import { useTower } from "../state/tower";
import { ago } from "../util/time";
import { plural } from "../shell/summary";
import { GestorLauncher } from "./HubGestor";
import { kindText, SEVERITY_TEXT, SEVERITY_TONE, where } from "./HubHealth";
import { itemShort } from "../util/item";
import { hubProjectOf } from "../util/hub";
import { WHY_SHORT, valueText, type DecideResult, type ProposalsData } from "./HubProposals";
import { actionableSignals, warningAction, type HubSection } from "./sections";
import { syncText } from "../project/settings";
import { projectAt } from "../project/sections";
import { isJira } from "./tracker";
import { DocsBackupReadOnly } from "../maintenance/DocsBackup";

/** Quantas propostas e sinais a Visão geral lista; o resto fica na seção. */
export const OVERVIEW_LIMIT = 6;

/**
 * Visão geral do Hub (#313): só o que pede ação. Avisos de `hub.status` com
 * o botão que leva à seção que resolve (os de backup, a Manutenção), propostas pendentes com Aprovar e
 * Recusar, sinais de saúde de aviso e críticos em tabela e o resumo dos
 * ambientes. "Abrir sessão do gestor" fica no cabeçalho.
 */
export function HubOverview({
  now,
  proposals,
  health,
  healthError,
  open,
}: {
  now: number;
  proposals: ProposalsData;
  health: HubHealth | null;
  healthError: string | null;
  open(s: HubSection): void;
}) {
  const { web } = useTower();
  return (
    <>
      <div className="hub-page-head">
        <h1 id="hub-h1">Visão geral</h1>
        <span className="muted">O que pede a sua ação no hub agora</span>
        <GestorLauncher />
      </div>
      <div className="hub-cards">
        <Warnings />
        <PendingProposals data={proposals} open={open} />
        {/* PWA (#444): sem Manutenção, o estado do backup dos docs fica aqui, só leitura. */}
        {web && <DocsBackupReadOnly now={now} />}
      </div>
      <Signals health={health} error={healthError} open={open} />
      <TrackerSync now={now} />
      <EnvsSummary now={now} open={open} />
    </>
  );
}

function Card({
  id,
  title,
  more,
  children,
}: {
  id: string;
  title: string;
  more?: { label: string; onClick(): void };
  children: ReactNode;
}) {
  return (
    <section className="hub-card" aria-labelledby={id}>
      <div className="hub-card-head">
        <h2 id={id}>{title}</h2>
        {more && (
          <button type="button" className="btn btn-sm" onClick={more.onClick}>
            {more.label}
          </button>
        )}
      </div>
      {children}
    </section>
  );
}

/** Cada aviso com o botão que leva aonde se resolve (backup: Manutenção, #338). */
function Warnings() {
  const { hub, web } = useTower();
  const nav = useNav();
  const warnings = hub.health?.warnings ?? [];
  return (
    <Card id="hub-ov-avisos" title={`Avisos · ${warnings.length}`}>
      {warnings.length === 0 ? (
        <p className="hub-card-empty muted">Sem avisos: backup em dia e nada em quarentena.</p>
      ) : (
        <ul className="hub-card-list">
          {warnings.map((w, i) => {
            const a = warningAction(w.kind, web);
            return (
              <li key={`${w.kind}:${i}`} data-kind={w.kind}>
                <span className="sp-dot" data-tone="attention" aria-hidden="true" />
                <span className="hub-card-text">{w.message}</span>
                {a && (
                  <button type="button" className="btn btn-sm" onClick={() => nav.openTab(a.tab)}>
                    {a.label}
                  </button>
                )}
              </li>
            );
          })}
        </ul>
      )}
    </Card>
  );
}

function PendingProposals({ data, open }: { data: ProposalsData; open(s: HubSection): void }) {
  const nav = useNav();
  const { hub } = useTower();
  const [busy, setBusy] = useState(false);
  const [msg, setMsg] = useState<DecideResult | null>(null);
  const { pending } = data;
  const shown = pending.slice(0, OVERVIEW_LIMIT);
  const note = msg ?? (data.error ? { text: data.error, error: true } : null);

  const decide = async (id: number, approve: boolean) => {
    setBusy(true);
    setMsg(null);
    setMsg(await data.decide([id], approve));
    setBusy(false);
  };

  return (
    <Card
      id="hub-ov-propostas"
      title={`Propostas do gestor · ${pending.length}`}
      more={{ label: "Ver todas", onClick: () => open("proposals") }}
    >
      {pending.length === 0 ? (
        <p className="hub-card-empty muted">Nenhuma proposta pendente.</p>
      ) : (
        <ul className="hub-card-list">
          {shown.map((p) => (
            <li key={p.id} aria-label={`Proposta #${p.id}`}>
              <span className="badge">{p.kind === "priority" ? "prioridade" : "status"}</span>
              <span className="hub-card-text">
                <button
                  type="button"
                  className="linklike mono"
                  title={p.ref ?? `${p.owner}/${p.repo}#${p.number}`}
                  onClick={() => nav.go({ name: "project", owner: p.owner, repo: p.repo, item: { ghNumber: p.number } })}
                >
                  {itemShort(hubProjectOf(hub.projects, p.owner, p.repo), p.number)}
                </button>
                {p.title && <> {p.title}</>} <span className="muted">→</span>{" "}
                <strong>{valueText(p.kind, p.proposed, hubProjectOf(hub.projects, p.owner, p.repo))}</strong>
                <span className="hub-card-sub">
                  {p.justification.length > WHY_SHORT
                    ? `${p.justification.slice(0, WHY_SHORT).trimEnd()}…`
                    : p.justification}
                </span>
              </span>
              <button
                type="button"
                className="btn btn-sm"
                disabled={busy}
                aria-label={`Recusar a proposta #${p.id}`}
                onClick={() => decide(p.id, false)}
              >
                Recusar
              </button>
              <button
                type="button"
                className="btn btn-sm btn-primary"
                disabled={busy}
                aria-label={`Aprovar a proposta #${p.id}`}
                onClick={() => decide(p.id, true)}
              >
                Aprovar
              </button>
            </li>
          ))}
          {pending.length > shown.length && (
            <li className="muted">e mais {pending.length - shown.length} em Propostas do gestor</li>
          )}
        </ul>
      )}
      {note && (
        <p role={note.error ? "alert" : "status"} className={`hub-card-msg ${note.error ? "start-error" : "state-on"}`}>
          {note.text}
        </p>
      )}
    </Card>
  );
}

/** Ação de um sinal: o item (ou o projeto) na tela Projeto, ou os ambientes. */
function SignalAction({ s, open }: { s: HealthSignal; open(sec: HubSection): void }) {
  const nav = useNav();
  const { hub } = useTower();
  // Jira sem sync: o ambiente de sync se troca no detalhe do projeto.
  if (s.kind === "rastreador_sem_sync" && s.owner && s.repo) {
    const project = { owner: s.owner, repo: s.repo };
    return (
      <button
        type="button"
        className="btn btn-sm"
        aria-label={`Ambiente de sync de ${s.owner}/${s.repo}`}
        // Configurações › Repositório e rastreador do próprio projeto (#18).
        onClick={() => nav.go(projectAt({ name: "project", ...project }, "settings", "tracker"))}
      >
        Ambiente de sync
      </button>
    );
  }
  if (s.owner && s.repo) {
    const owner = s.owner;
    const repo = s.repo;
    const item = s.number !== null ? { ghNumber: s.number } : undefined;
    return (
      <button
        type="button"
        className="btn btn-sm"
        aria-label={`Abrir ${where(s, hubProjectOf(hub.projects, owner, repo))}`}
        onClick={() => nav.go({ name: "project", owner, repo, ...(item ? { item } : {}) })}
      >
        {item ? "Abrir item" : "Abrir projeto"}
      </button>
    );
  }
  if (s.env) {
    return (
      <button type="button" className="btn btn-sm" onClick={() => open("envs")}>
        Ver ambientes
      </button>
    );
  }
  return null;
}

function Signals({
  health,
  error,
  open,
}: {
  health: HubHealth | null;
  error: string | null;
  open(s: HubSection): void;
}) {
  const { hub } = useTower();
  const projectOf = (s: HealthSignal) => hubProjectOf(hub.projects, s.owner, s.repo);
  const signals = actionableSignals(health);
  const shown = signals.slice(0, OVERVIEW_LIMIT);
  return (
    <Card
      id="hub-ov-saude"
      title={`Saúde dos projetos · ${plural(signals.length, "sinal", "sinais")}`}
      more={{ label: "Ver todos", onClick: () => open("health") }}
    >
      {error && (
        <p role="alert" className="hub-card-msg start-error">
          {error}
        </p>
      )}
      {signals.length === 0 ? (
        <p className="hub-card-empty muted">
          {health ? "Nenhum sinal de aviso ou crítico: nada parado, bloqueado ou falhando." : "Lendo os sinais…"}
        </p>
      ) : (
        <div className="table-wrap">
          <table className="hub-card-table" aria-label="Sinais de saúde">
            <thead>
              <tr>
                <th scope="col">Sinal</th>
                <th scope="col">Item</th>
                <th scope="col">Medida / limiar</th>
                <th scope="col">Ambiente</th>
                <th scope="col">
                  <span className="sr-only">Ação</span>
                </th>
              </tr>
            </thead>
            <tbody>
              {shown.map((s, i) => (
                <tr key={`${s.kind}:${s.owner}/${s.repo}#${s.number}:${s.env}:${i}`}>
                  <td>
                    <span className="badge" data-tone={SEVERITY_TONE[s.severity]} title={SEVERITY_TEXT[s.severity]}>
                      {kindText(s, projectOf(s))}
                    </span>
                  </td>
                  <td>
                    {s.owner && s.repo ? (
                      <>
                        <span className="mono">{s.number !== null ? itemShort(projectOf(s), s.number) : `${s.owner}/${s.repo}`}</span>
                        {s.title && <span className="muted"> {s.title}</span>}
                        {s.number === null && s.detail && <span className="hub-card-sub">{s.detail}</span>}
                      </>
                    ) : (
                      <span className="muted">—</span>
                    )}
                  </td>
                  <td className="nowrap">
                    {s.value}
                    {s.threshold && <span className="muted"> · {s.threshold}</span>}
                  </td>
                  <td>{s.env ?? <span className="muted">—</span>}</td>
                  <td className="hub-card-action">
                    <SignalAction s={s} open={open} />
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
          {signals.length > shown.length && (
            <p className="hub-card-empty muted">e mais {signals.length - shown.length} em Saúde dos projetos</p>
          )}
        </div>
      )}
    </Card>
  );
}

/**
 * Último sync (ou o erro dele) de cada projeto Jira, relatado pelo ambiente
 * de sync (#348): o hub não sincroniza esses projetos. Some sem projeto Jira.
 */
function TrackerSync({ now }: { now: number }) {
  const { hub } = useTower();
  const nav = useNav();
  const jira = hub.projects.filter((p) => isJira(p.project));
  if (jira.length === 0) return null;
  return (
    <Card id="hub-ov-jira" title={`Sync dos projetos Jira · ${jira.length}`}>
      <ul className="hub-card-list">
        {jira.map((p) => {
          const pr = p.project;
          const sync = syncText(p.sync, now);
          const env = pr.sync_env_id
            ? (p.assignments.find((a) => a.env_id === pr.sync_env_id)?.env_name ?? pr.sync_env_id)
            : null;
          return (
            <li key={pr.id} aria-label={`Sync de ${pr.name}`}>
              <span className="sp-dot" data-tone={env ? (sync.tone === "ok" ? "done" : sync.tone) : "attention"} aria-hidden="true" />
              <span className="hub-card-text">
                <strong>{pr.name}</strong> {env ? sync.text : "sem ambiente de sync"}
                <span className="hub-card-sub">
                  {env ? `pelo ambiente de sync ${env}` : "atribua o projeto ou escolha o ambiente de sync"}
                  {p.sync?.last_error ? `: ${p.sync.last_error}` : ""}
                </span>
              </span>
              <button
                type="button"
                className="btn btn-sm"
                // Configurações › Repositório e rastreador do próprio projeto (#18).
                onClick={() =>
                  nav.go(projectAt({ name: "project", owner: pr.gh_owner, repo: pr.gh_repo }, "settings", "tracker"))
                }
              >
                Abrir <span className="sr-only">{pr.name}</span>
              </button>
            </li>
          );
        })}
      </ul>
    </Card>
  );
}

function EnvsSummary({ now, open }: { now: number; open(s: HubSection): void }) {
  const { hub } = useTower();
  return (
    <Card
      id="hub-ov-ambientes"
      title={`Ambientes · ${hub.envs.length}`}
      more={{ label: "Gerenciar", onClick: () => open("envs") }}
    >
      {hub.envs.length === 0 ? (
        <p className="hub-card-empty muted">Nenhum ambiente ligado ao hub.</p>
      ) : (
        <ul className="hub-envs">
          {hub.envs.map((e) => (
            <li key={e.env_id}>
              <span
                className="sp-dot"
                data-tone={e.last_seen_at === null ? "error" : e.pending > 0 ? "attention" : "done"}
                aria-hidden="true"
              />
              <strong>{e.name}</strong>
              <span className="muted">
                último envio {e.last_seen_at ? ago(e.last_seen_at, now) : "nunca"} ·{" "}
                {plural(e.pending, "pendência", "pendências")}
              </span>
            </li>
          ))}
        </ul>
      )}
    </Card>
  );
}
