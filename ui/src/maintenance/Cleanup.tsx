import { useCallback, useEffect, useState, type ReactNode } from "react";
import type { CleanupItem, CleanupRun, CleanupStatus, StepKind, StepPlan } from "../protocol/gen";
import { useTower } from "../state/tower";
import { useNow } from "../App";
import { ago } from "../util/time";
import { humanBytes } from "../util/bytes";
import { changeLabel, itemFullRef, type ProviderProject } from "../util/item";
import type { Project } from "../protocol/gen";

export const STEP_LABEL: Record<StepKind, string> = {
  sessoes: "sessões",
  worktree: "worktree",
  target: "target/",
  branch_local: "branch local",
  branch_remota: "branch remota",
};

/** Rótulo do sinal de conclusão; o `pr_*` leva o nome da mudança na forja ("MR mesclado" no GitLab, #352). */
export function signalLabel(signal: NonNullable<CleanupItem["completion"]>["signal"], project?: ProviderProject | null): string {
  switch (signal) {
    case "work_item_concluido":
      return "work item concluído";
    case "pr_mesclado":
      return `${changeLabel(project)} mesclado`;
    case "pr_fechado":
      return `${changeLabel(project)} fechado`;
    case "issue_fechada":
      return "issue fechada";
  }
}

const DAY = 24 * 3_600_000;

/**
 * O que pôs o work item no plano: o sinal de conclusão ou, no em andamento
 * (`completion` nulo, #265), o worktree inativo (só o `target/` sai).
 */
export function itemReason(i: CleanupItem, now: number, project?: ProviderProject | null): string {
  if (i.completion) return signalLabel(i.completion.signal, project);
  if (i.last_activity_at == null) return "em andamento, worktree inativo";
  const days = Math.floor((now - i.last_activity_at) / DAY);
  return `em andamento, worktree inativo há ${days} dia(s)`;
}

const hasPending = (i: CleanupItem) => i.steps.some((s) => s.state === "pendente");
const pendingBytes = (i: CleanupItem) =>
  i.steps.filter((s) => s.state === "pendente").reduce((n, s) => n + s.bytes, 0);
/** `owner/repo#n`, ou a chave do Jira no projeto Jira (#352). */
const itemName = (i: { owner: string; repo: string; gh_number: number }, project?: Project) =>
  itemFullRef(project ?? { gh_owner: i.owner, gh_repo: i.repo }, i.gh_number);

/** "N limpezas pendentes · X GB" (ou "nenhuma limpeza pendente"). */
export function pendingText(items: number, bytes: number): string {
  if (items === 0) return "nenhuma limpeza pendente";
  return `${items} ${items === 1 ? "limpeza pendente" : "limpezas pendentes"} · ${humanBytes(bytes)}`;
}

function runText(r: CleanupRun): string {
  const steps = r.items.flatMap((i) => i.steps);
  const n = (k: string) => steps.filter((s) => s.result === k).length;
  return `${n("feita")} etapa(s) feita(s), ${n("bloqueada")} bloqueada(s), ${n("falhou")} com falha · liberado: ${humanBytes(r.freed_bytes)}`;
}

/** Estado da limpeza de um ambiente na UI: `null` = carregando; `false` = daemon sem suporte. */
type EnvCleanup = CleanupStatus | false | null;

/** Sessão viva de issue concluída com o encerramento esperando confirmação. */
interface Ending {
  env: string;
  session: string;
}

/** Sessões vivas do work item (daemon antigo não manda o campo). */
const liveSessions = (i: CleanupItem) => i.live_sessions ?? [];

/** Pedido de limpeza esperando confirmação. */
interface Confirm {
  env: string;
  /** `null` = o plano inteiro do ambiente. */
  item: CleanupItem | null;
}

/**
 * Limpeza do ambiente após a conclusão das issues (issue #287): o último
 * plano de cada ambiente ("N limpezas pendentes · X GB"), a lista por work
 * item com as etapas pendentes e as bloqueadas com o motivo, e os botões
 * "Verificar agora" (`cleanup.plan`) e "Limpar" (`cleanup.run`, tudo ou um
 * item). Vale mesmo com o job do ambiente desligado.
 *
 * Sessão viva de issue concluída nunca é encerrada pela limpeza (issue
 * #285): aparece em destaque, com "Encerrar sessão" (`session.kill`, com
 * confirmação), e a etapa "sessões" fica bloqueada até lá.
 *
 * Em Manutenção › Limpeza (#337, antes nas Configurações) mostra o
 * ambiente escolhido (`env`); as regras (`[cleanup]`) ficam nas
 * Configurações (`onSettings`).
 */
export function Cleanup({ env, onSettings }: { env: string; onSettings: () => void }) {
  const { envs, status, api, dashboards } = useTower();
  const now = useNow();
  const [state, setState] = useState<Record<string, EnvCleanup>>({});
  const [busy, setBusy] = useState<Record<string, string>>({});
  const [errors, setErrors] = useState<Record<string, string>>({});
  const [results, setResults] = useState<Record<string, string>>({});
  const [confirm, setConfirm] = useState<Confirm | null>(null);
  const [ending, setEnding] = useState<Ending | null>(null);

  const connected = envs.filter((e) => e.id === env && status[e.id]?.connected).map((e) => e.id);
  const key = connected.join(",");

  const load = useCallback(
    async (env: string) => {
      try {
        const st = await api.call<CleanupStatus>(env, "cleanup.status");
        setState((all) => ({ ...all, [env]: st }));
      } catch {
        setState((all) => ({ ...all, [env]: false }));
      }
    },
    [api],
  );

  useEffect(() => {
    for (const env of key ? key.split(",") : []) void load(env);
  }, [key, load]);

  // Plano novo (job, verificação ou execução em outra tela): o push traz o estado inteiro.
  useEffect(
    () =>
      api.onPush((e) => {
        if (e.event === "cleanup.status") setState((all) => ({ ...all, [e.env]: e.data as CleanupStatus }));
      }),
    [api],
  );

  const act = async (env: string, what: string, f: () => Promise<void>) => {
    setBusy((all) => ({ ...all, [env]: what }));
    setErrors((all) => ({ ...all, [env]: "" }));
    try {
      await f();
    } catch (err) {
      setErrors((all) => ({ ...all, [env]: String(err) }));
    } finally {
      setBusy((all) => ({ ...all, [env]: "" }));
      await load(env);
    }
  };

  const check = (env: string) =>
    act(env, "check", async () => {
      await api.call(env, "cleanup.plan");
      setResults((all) => ({ ...all, [env]: "" }));
    });

  const clean = (c: Confirm) => {
    setConfirm(null);
    return act(c.env, "run", async () => {
      const r = await api.call<CleanupRun>(c.env, "cleanup.run", c.item ? { work_item_id: c.item.work_item_id } : {});
      setResults((all) => ({ ...all, [c.env]: runText(r) }));
    });
  };

  // Encerramento explícito da sessão aberta; o plano é refeito em seguida.
  const endSession = (e: Ending) => {
    setEnding(null);
    return act(e.env, "end", async () => {
      await api.call(e.env, "session.kill", { session_id: e.session });
      await api.call(e.env, "cleanup.plan");
    });
  };

  return (
    <section className="panel settings-section set-stack" aria-labelledby="maint-cleanup">
      <h2 id="maint-cleanup">Plano de limpeza</h2>
      {envs.filter((e) => e.id === env).map((e) => {
        const on = !!status[e.id]?.connected;
        const st = state[e.id];
        const plan = st ? st.plan : null;
        const pending = plan?.items.filter(hasPending) ?? [];
        const blocked = plan?.items.filter((i) => !hasPending(i)) ?? [];
        const open = (plan?.items ?? []).flatMap((i) => liveSessions(i).map((session) => ({ item: i, session })));
        const working = !!busy[e.id];
        const proj = (i: CleanupItem) => dashboards[e.id]?.projects.find((p) => p.project.id === i.project_id)?.project;
        return (
          <div key={e.id} className="env-row cleanup-row" role="group" aria-label={`Limpeza de ${e.name}`}>
            <div className="fact">
              <span className="label">Ambiente</span>
              <span className="strong">{e.name}</span>
            </div>
            <div className="fact">
              <span className="label">Pendentes</span>
              {!on ? (
                <span className="muted">sem conexão</span>
              ) : st === false ? (
                <span className="muted">daemon sem suporte a limpeza</span>
              ) : !plan ? (
                <span className="muted">{st?.check_error ? "verificação falhou" : "ainda não verificado"}</span>
              ) : (
                <span className={plan.pending_items > 0 ? "strong" : "muted"} role="status">
                  {pendingText(Number(plan.pending_items), Number(plan.pending_bytes))}
                </span>
              )}
            </div>
            <div className="fact">
              <span className="label">Última verificação</span>
              <span>{plan ? ago(plan.checked_at, now) : "—"}</span>
            </div>
            <div className="fact">
              <span className="label">Execução automática</span>
              <span>{st ? (st.config.auto ? "ligada" : "desligada") : "—"}</span>
            </div>
            <div className="env-actions">
              <button type="button" className="btn" disabled={!on || st === false || working} onClick={() => check(e.id)}>
                {busy[e.id] === "check" ? "Verificando…" : "Verificar agora"}
                <span className="sr-only"> {e.name}</span>
              </button>
              <button
                type="button"
                className="btn btn-primary"
                disabled={!on || working || pending.length === 0}
                onClick={() => setConfirm({ env: e.id, item: null })}
              >
                {busy[e.id] === "run" ? "Limpando…" : "Limpar"}
                <span className="sr-only"> tudo em {e.name}</span>
              </button>
            </div>
            {st && st.check_error && on && <div className="update-state wide state-off">erro na verificação: {st.check_error}</div>}
            {errors[e.id] && (
              <div className="update-state wide state-off" role="alert">
                {errors[e.id]}
              </div>
            )}
            {results[e.id] && <div className="update-state wide state-on">{results[e.id]}</div>}
            {open.length > 0 && (
              <ul className="wide cleanup-items" aria-label={`Sessões abertas de issues concluídas em ${e.name}`}>
                {open.map(({ item: i, session }) => (
                  <li key={session} className="cleanup-item cleanup-open">
                    <div className="cleanup-item-head">
                      <span className="mono strong">{itemName(i, proj(i))}</span>
                      <span role="alert">issue concluída, sessão ainda aberta</span>
                      <span className="mono muted small">{session}</span>
                      <span className="cleanup-item-action">
                        {ending?.env === e.id && ending.session === session ? (
                          <span role="group" aria-label="Confirmar encerramento" className="confirm">
                            <span>Encerrar mata o processo do agente.</span>
                            <button type="button" className="btn btn-danger" onClick={() => endSession(ending)}>
                              Confirmar
                            </button>
                            <button type="button" className="btn" onClick={() => setEnding(null)}>
                              Cancelar
                            </button>
                          </span>
                        ) : (
                          <button
                            type="button"
                            className="btn btn-danger"
                            disabled={!on || working}
                            onClick={() => setEnding({ env: e.id, session })}
                          >
                            Encerrar sessão <span className="sr-only">{session}</span>
                          </button>
                        )}
                      </span>
                    </div>
                  </li>
                ))}
              </ul>
            )}
            {pending.length > 0 && (
              <ul className="wide cleanup-items" aria-label={`Limpezas pendentes em ${e.name}`}>
                {pending.map((i) => (
                  <Item
                    key={i.work_item_id}
                    item={i}
                    project={proj(i)}
                    now={now}
                    action={
                      <button
                        type="button"
                        className="btn"
                        disabled={!on || working}
                        onClick={() => setConfirm({ env: e.id, item: i })}
                      >
                        Limpar <span className="sr-only">{itemName(i, proj(i))}</span>
                      </button>
                    }
                  />
                ))}
              </ul>
            )}
            {blocked.length > 0 && (
              <details className="wide">
                <summary>
                  {blocked.length} concluído(s) com etapas bloqueadas
                </summary>
                <ul className="cleanup-items" aria-label={`Limpezas bloqueadas em ${e.name}`}>
                  {blocked.map((i) => (
                    <Item key={i.work_item_id} item={i} project={proj(i)} now={now} />
                  ))}
                </ul>
              </details>
            )}
            {confirm?.env === e.id && (
              <div role="group" aria-label="Confirmar limpeza" className="confirm-box wide">
                <p>
                  {confirm.item
                    ? `Limpar ${itemName(confirm.item, proj(confirm.item))}? Saem as etapas pendentes (${humanBytes(pendingBytes(confirm.item))}).`
                    : `Limpar ${pending.length} work item(s) em ${e.name}? Saem as etapas pendentes (${humanBytes(Number(plan?.pending_bytes ?? 0))}).`}{" "}
                  As bloqueadas ficam, e nada com mudança não commitada ou não enviada é apagado.
                </p>
                <div className="form-actions">
                  <button type="button" className="btn" onClick={() => setConfirm(null)}>
                    Cancelar
                  </button>
                  <button type="button" className="btn btn-danger" onClick={() => clean(confirm)}>
                    Limpar
                  </button>
                </div>
              </div>
            )}
          </div>
        );
      })}
      <p className="muted hint">
        O ambiente monta o plano no boot e a cada <span className="mono">check_interval_hours</span>, sem apagar nada
        enquanto <span className="mono">auto</span> estiver desligado. Sessão aberta de issue concluída só é encerrada à
        mão, salvo <span className="mono">end_idle_sessions_hours</span>. O worktree só sai limpo, enviado e sem sessão
        aberta; de worktree que fica (em andamento ou com trabalho por salvar), só o{" "}
        <span className="mono">target/</span> sai, depois de <span className="mono">target_idle_days</span> dias sem
        atividade. No terminal: <span className="mono">ctower clean --dry-run</span> e{" "}
        <span className="mono">ctower clean [--item n]</span>.
      </p>
      <div>
        <button type="button" className="link-btn" onClick={onSettings}>
          Ajustes de limpeza ›
        </button>
      </div>
    </section>
  );
}

function stepText(s: StepPlan): string {
  if (s.state === "pendente") return `${STEP_LABEL[s.kind]}: ${humanBytes(s.bytes)}`;
  return `${STEP_LABEL[s.kind]}: ${s.reason ?? "bloqueada"}`;
}

function Item({ item, project, now, action }: { item: CleanupItem; project?: Project; now: number; action?: ReactNode }) {
  const steps = item.steps.filter((s) => s.state !== "feita");
  return (
    <li className="cleanup-item">
      <div className="cleanup-item-head">
        <span className="mono strong">{itemName(item, project)}</span>
        {item.title && <span>{item.title}</span>}
        <span className="muted small">{itemReason(item, now, project)}</span>
        {action && <span className="cleanup-item-action">{action}</span>}
      </div>
      <ul className="cleanup-steps">
        {steps.map((s) => (
          <li key={s.kind} data-state={s.state} title={s.targets.join("\n") || undefined}>
            {stepText(s)}
          </li>
        ))}
      </ul>
    </li>
  );
}
