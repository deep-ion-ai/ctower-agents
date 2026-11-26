//! O laço do motor de execução de épico (#298) no processo do hub, no
//! formato do agendador do piloto ([`crate::autopilot::runner`]): para cada
//! execução ativa, monta a fotografia ([`snapshot`]), decide
//! ([`super::plan::decide`]) e aplica as ações pelo canal reverso
//! ([`EnvPort`]), gravando os eventos da execução. Cuida também da sessão
//! de plano: inicia em `planejando`/`replanejando` e encerra quando o plano
//! chega. Roda a cada minuto e quando chamado ([`Engine::kick`]).
//!
//! Disparo manual: não depende do modo autônomo do projeto nem da política
//! de orçamento; da conta usa só a leitura do plano (janela esgotada) para
//! estacionar e retomar.

use super::plan::{self, Action, ItemSnap, SessionSnap, Snapshot};
use super::prompt::{self, PlanIssue, PlanVars};
use crate::autopilot::plan::{exhausted, Closing, ResumeVia, SendKind};
use crate::autopilot::runner::{env_protocol, EnvPort};
use crate::HubState;
use ct_core::{EpicNodeState, EpicRunStatus, PrState};
use ct_hub_store::{EpicNodePatch, EpicRun, EpicRunEvent, Hub, SharedHub};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;
use tokio::sync::Notify;

/// Intervalo do ciclo.
pub const TICK: Duration = Duration::from_secs(60);
const FIRST_TICK: Duration = Duration::from_secs(20);
/// Protocolo do ambiente com a sessão de plano (#299).
pub const PLAN_PROTOCOL: (u32, u32) = (2, 25);
/// Protocolo do ambiente com `epic_run` no `session.start` e o
/// `worktree.remove` (#298).
pub const RUN_PROTOCOL: (u32, u32) = (2, 26);
/// Sessão de plano iniciada que o hub ainda não conhece conta como viva por
/// este tempo.
const PLAN_START_GRACE_MS: i64 = 10 * 60_000;

/// Texto do `session.resume` da sessão estacionada que encerrou.
const RESUME_PROMPT: &str =
    "Execução de épico do Control Tower: continue a sub-issue de onde parou, sem perguntar.";

/// Estado do laço no `HubState`.
#[derive(Debug, Default)]
pub struct Engine {
    kick: Notify,
    last_tick_at: AtomicI64,
}

impl Engine {
    /// Pede um ciclo já (execução nova, aprovação, retomada).
    pub fn kick(&self) {
        self.kick.notify_one();
    }

    pub fn last_tick_at(&self) -> Option<i64> {
        Some(self.last_tick_at.load(Ordering::Relaxed)).filter(|t| *t > 0)
    }
}

/// Sobe o laço em segundo plano.
pub fn spawn(st: HubState) {
    tokio::spawn(async move {
        let port = crate::autopilot::runner::HubPort(st.clone());
        tokio::time::sleep(FIRST_TICK).await;
        loop {
            let now = ct_core::now_ms();
            match tick(&st.hub, &port, now).await {
                Ok(t) => {
                    if t.changed > 0 {
                        st.epic_runs_changed();
                    }
                    if t.decisions > 0 {
                        st.push_kick();
                    }
                }
                Err(e) => tracing::warn!("execução de épico: ciclo falhou: {e}"),
            }
            st.epic_runs.last_tick_at.store(now, Ordering::Relaxed);
            tokio::select! {
                _ = tokio::time::sleep(TICK) => {}
                _ = st.epic_runs.kick.notified() => {
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
            }
        }
    });
}

fn ledger_err(e: ct_ledger::LedgerError) -> String {
    e.to_string()
}

fn transient(code: i64) -> bool {
    code == ct_http::sync::CHANNEL_ENV_OFFLINE || code == ct_http::sync::CHANNEL_TIMEOUT
}

/// O que um ciclo mudou.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct TickResult {
    /// Eventos gravados.
    pub changed: usize,
    /// Pedidos de decisão novos.
    pub decisions: usize,
}

/// O contexto de uma execução no ciclo.
struct RunCtx {
    run: EpicRun,
    project: ct_hub_store::HubProject,
    /// Branch de integração do épico (a do work item do épico).
    wip: Option<String>,
    epic_title: String,
}

/// Um ciclo de todas as execuções ativas.
pub async fn tick(hub: &SharedHub, port: &dyn EnvPort, now: i64) -> Result<TickResult, String> {
    let runs = hub.lock().active_epic_runs().map_err(ledger_err)?;
    let mut out = TickResult::default();
    let mut protocols: BTreeMap<String, Option<(u32, u32)>> = BTreeMap::new();
    for run in runs {
        let Some(ctx) = run_ctx(&hub.lock(), run).map_err(ledger_err)? else {
            continue;
        };
        let env = ctx.run.env_id.clone();
        let protocol = if port.online(&env) {
            match protocols.get(&env) {
                Some(p) => *p,
                None => {
                    let p = env_protocol(port, &env).await;
                    protocols.insert(env.clone(), p);
                    p
                }
            }
        } else {
            None
        };
        let r = tick_run(hub, port, &ctx, protocol, now).await?;
        out.changed += r.changed;
        out.decisions += r.decisions;
    }
    Ok(out)
}

fn run_ctx(h: &Hub, run: EpicRun) -> ct_ledger::Result<Option<RunCtx>> {
    let Some(project) = h.find_project(&run.owner, &run.repo)? else {
        return Ok(None);
    };
    let wip = h
        .find_work_item(&run.owner, &run.repo, run.epic)?
        .and_then(|w| w.branch);
    let epic_title = h
        .list_issues(project.id)?
        .into_iter()
        .find(|i| i.gh_number == run.epic)
        .map(|i| i.title)
        .unwrap_or_default();
    Ok(Some(RunCtx {
        run,
        project,
        wip,
        epic_title,
    }))
}

/// Grava o evento, sem repetir o último igual (mesmo tipo, nó e texto).
fn record(h: &Hub, e: EpicRunEvent) -> ct_ledger::Result<bool> {
    let recent = h.epic_run_events_tail(e.run_id, 20)?;
    if recent
        .iter()
        .rev()
        .find(|x| x.kind == e.kind && x.number == e.number)
        .is_some_and(|x| x.message == e.message && x.reason == e.reason)
        && matches!(
            e.kind.as_str(),
            "start_failed" | "refused" | "continue_failed" | "plan_failed"
        )
    {
        return Ok(false);
    }
    h.add_epic_run_event(&e)?;
    Ok(true)
}

fn ev(run: &EpicRun, now: i64, kind: &str, number: Option<i64>, message: String) -> EpicRunEvent {
    EpicRunEvent {
        run_id: run.id,
        at: now,
        kind: kind.into(),
        number,
        message,
        ..Default::default()
    }
}

/// Fotografia da execução para a decisão; `None` = sem plano aprovado ou
/// sem a `wip`.
pub(crate) fn snapshot(
    h: &Hub,
    port: &dyn EnvPort,
    ctx: &RunCtxView,
    now: i64,
) -> ct_ledger::Result<Option<Snapshot>> {
    let run = ctx.run;
    let (Some(v), Some(wip)) = (run.plan_version, ctx.wip) else {
        return Ok(None);
    };
    let Some(plan) = h.epic_plan(run.id, v)? else {
        return Ok(None);
    };
    let p = ctx.project;
    let nodes = h.epic_nodes(run.id)?;
    let numbers: BTreeSet<i64> = plan
        .nodes
        .iter()
        .map(|n| n.number)
        .chain(nodes.iter().map(|n| n.number))
        .collect();
    let issues: BTreeMap<i64, ct_ledger::IssueRow> = h
        .list_issues(p.id)?
        .into_iter()
        .map(|i| (i.gh_number, i))
        .collect();
    let closes: BTreeMap<i64, ct_hub_store::CloseState> = h
        .autopilot_close_states()?
        .into_iter()
        .map(|c| (c.work_item_id, c))
        .collect();
    let mut items = BTreeMap::new();
    for n in &numbers {
        let issue = issues.get(n);
        let wi = h.find_work_item(&run.owner, &run.repo, *n)?;
        let fingerprint = issue.map(|i| {
            ct_hub_store::issue_fingerprint(
                &i.title,
                i.updated_at,
                wi.as_ref().and_then(|w| w.priority).map(|p| p.as_str()),
                &i.labels,
            )
        });
        let mut item = ItemSnap {
            title: issue.map(|i| i.title.clone()),
            fingerprint,
            ..Default::default()
        };
        if let Some(w) = &wi {
            let prs = h.prs_for_work_item(w.id)?;
            item.merged = prs
                .iter()
                .find(|pr| pr.state == PrState::Merged && pr.base == wip)
                .map(|pr| pr.number);
            if let Some(pr) = prs
                .into_iter()
                .find(|pr| pr.state == PrState::Open && pr.base == wip)
            {
                item.open = Some(Closing {
                    work_item_id: w.id,
                    integration: wip.to_string(),
                    checks: h.pr_checks(p.id, pr.number)?,
                    feedback: h.pr_feedback(p.id, pr.number)?,
                    state: closes.get(&w.id).cloned().unwrap_or_default(),
                    pr,
                });
            }
        }
        items.insert(*n, item);
    }
    let cited: BTreeSet<i64> = plan
        .edges
        .iter()
        .map(|e| e.blocker)
        .filter(|b| !numbers.contains(b))
        .collect();
    let external = h
        .autopilot_blocker_states(p.id, &p.base_branch, &cited)?
        .into_iter()
        .map(|(n, s)| (n, s == ct_hub_store::BlockerState::Ready))
        .collect();
    let mut sessions = BTreeMap::new();
    for id in nodes.iter().filter_map(|n| n.session_id.clone()) {
        if let Some(i) = h.epic_session_info(&id)? {
            let human_input_at = h.last_human_input(&id)?;
            sessions.insert(
                id,
                SessionSnap {
                    state: i.state,
                    idle_since: (i.state == ct_core::SessionState::Idle).then_some(i.idle_since),
                    human_input_at,
                },
            );
        }
    }
    let account = h
        .list_env_plans()?
        .into_iter()
        .find(|(env, _, _)| *env == run.env_id)
        .and_then(|(_, _, r)| r.account.map(|a| a.key));
    let exhausted = match account {
        Some(key) => exhausted(&crate::autopilot::account_plan(h, &key, now)?),
        None => None,
    };
    let settings = h.autopilot_settings()?;
    let pending: HashSet<_> = h
        .epic_decisions(run.id, true)?
        .into_iter()
        .map(|d| (d.number, d.origin))
        .collect();
    // Integração (#302): a sessão e o PR do épico (`wip` → base).
    let mut integration = plan::IntegrationSnap::default();
    if let Some(id) = &run.integration_session {
        if let Some(i) = h.epic_session_info(id)? {
            integration.session = Some(SessionSnap {
                state: i.state,
                idle_since: (i.state == ct_core::SessionState::Idle).then_some(i.idle_since),
                human_input_at: None,
            });
        }
    }
    if let Some(w) = h.find_work_item(&run.owner, &run.repo, run.epic)? {
        let prs: Vec<_> = h
            .prs_for_work_item(w.id)?
            .into_iter()
            .filter(|pr| pr.head == wip && pr.base == p.base_branch)
            .collect();
        integration.merged = prs.iter().any(|pr| pr.state == PrState::Merged);
        if let Some(pr) = prs.into_iter().find(|pr| pr.state == PrState::Open) {
            integration.open = Some(Closing {
                work_item_id: w.id,
                integration: p.base_branch.clone(),
                checks: h.pr_checks(p.id, pr.number)?,
                feedback: h.pr_feedback(p.id, pr.number)?,
                state: closes.get(&w.id).cloned().unwrap_or_default(),
                pr,
            });
        }
    }
    let mut handback_at = BTreeMap::new();
    for n in &nodes {
        if let Some(at) = h.epic_node_event_at(run.id, n.number, "handback")? {
            handback_at.insert(n.number, at);
        }
    }
    Ok(Some(Snapshot {
        integration,
        handback_at,
        now,
        run: run.clone(),
        plan: Some(plan),
        nodes,
        wip: wip.to_string(),
        items,
        external,
        sessions,
        env_online: port.online(&run.env_id),
        exhausted,
        pending,
        max_ci_attempts: settings.max_ci_attempts,
        idle_timeout_min: settings.idle_timeout_min,
    }))
}

/// Visão emprestada do contexto, para a fotografia.
pub(crate) struct RunCtxView<'a> {
    pub run: &'a EpicRun,
    pub project: &'a ct_hub_store::HubProject,
    pub wip: Option<&'a str>,
}

/// O prompt da sessão de uma sub-issue.
fn node_prompt(ctx: &RunCtx, n: i64, title: &str, wip: &str) -> String {
    let p = &ctx.project;
    let r = p.item_ref(n);
    let epic = p.item_ref(ctx.run.epic);
    let closes = if r.contains('#') {
        format!("Closes #{n}")
    } else {
        r.clone()
    };
    format!(
        "Sessão da execução de épico do Control Tower: implemente a sub-issue {r} ({title}) do \
         épico {epic} ({etitle}) no repositório {owner}/{repo}. A branch de trabalho já saiu da \
         {wip}.\nAntes de começar, consulte decisões anteriores com `ledger_list_decisions`. \
         Registre toda decisão relevante com `ledger_record_decision`. Siga as regras do projeto \
         e do épico do system prompt.\nAo terminar, com os testes passando, abra o PR da sua \
         branch para {wip} (nunca para outra base), com a linha `{closes}` no corpo. Não mescle: \
         o hub acompanha o CI e pede o merge. Se o plano estiver errado (falta um pré-requisito, \
         o escopo não cabe), pare e explique.",
        etitle = ctx.epic_title,
        owner = p.gh_owner,
        repo = p.gh_repo,
    )
}

/// O prompt da sessão de integração (#302): as sub-issues mescladas e o
/// corpo do PR com um `Closes` por item.
fn integration_prompt(h: &Hub, ctx: &RunCtx, wip: &str) -> ct_ledger::Result<String> {
    let run = &ctx.run;
    let p = &ctx.project;
    let merged: Vec<i64> = h
        .epic_nodes(run.id)?
        .into_iter()
        .filter(|n| n.state == EpicNodeState::Mesclada)
        .map(|n| n.number)
        .collect();
    let mut closes: Vec<String> = merged
        .iter()
        .map(|n| prompt::closes_line(&p.item_ref(*n), *n))
        .collect();
    closes.push(prompt::closes_line(&p.item_ref(run.epic), run.epic));
    Ok(prompt::integration_prompt(&prompt::IntegrationVars {
        owner: run.owner.clone(),
        repo: run.repo.clone(),
        epic_ref: p.item_ref(run.epic),
        epic_title: ctx.epic_title.clone(),
        wip: wip.to_string(),
        base: p.base_branch.clone(),
        merged: merged.iter().map(|n| p.item_ref(*n)).collect(),
        closes,
        code_review: run.code_review,
    }))
}

/// Um ciclo de uma execução.
async fn tick_run(
    hub: &SharedHub,
    port: &dyn EnvPort,
    ctx: &RunCtx,
    protocol: Option<(u32, u32)>,
    now: i64,
) -> Result<TickResult, String> {
    let run = &ctx.run;
    let mut out = TickResult::default();
    let env = run.env_id.clone();
    let slug = |n: i64| format!("{}/{}#{n}", run.owner, run.repo);
    let rec = |e: EpicRunEvent, out: &mut TickResult| -> Result<(), String> {
        if record(&hub.lock(), e).map_err(ledger_err)? {
            out.changed += 1;
        }
        Ok(())
    };
    let Some(wip) = ctx.wip.clone() else {
        rec(
            EpicRunEvent {
                reason: Some("sem_wip".into()),
                ..ev(
                    run,
                    now,
                    "refused",
                    None,
                    format!(
                        "o épico {} ainda não tem branch de integração no hub: inicie uma \
                         sessão do épico uma vez",
                        slug(run.epic)
                    ),
                )
            },
            &mut out,
        )?;
        return Ok(out);
    };

    // Sessão de plano.
    match run.status {
        EpicRunStatus::Planejando | EpicRunStatus::Replanejando => {
            plan_session(hub, port, ctx, &wip, protocol, now, &mut out).await?;
        }
        _ => {
            if let Some(id) = run.plan_session.clone() {
                if port.online(&env) {
                    match port
                        .call(env.clone(), "session.kill", json!({ "session_id": id }))
                        .await
                    {
                        Err((c, _)) if transient(c) => {}
                        _ => {
                            hub.lock()
                                .set_epic_run_plan_session(run.id, None, now)
                                .map_err(ledger_err)?;
                            rec(
                                ev(
                                    run,
                                    now,
                                    "plan_session_ended",
                                    None,
                                    "plano gravado: a sessão de plano foi encerrada".into(),
                                ),
                                &mut out,
                            )?;
                        }
                    }
                }
            }
        }
    }

    // Motor.
    let snap = {
        let h = hub.lock();
        snapshot(
            &h,
            port,
            &RunCtxView {
                run,
                project: &ctx.project,
                wip: Some(&wip),
            },
            now,
        )
        .map_err(ledger_err)?
    };
    let Some(snap) = snap else {
        return Ok(out);
    };
    let outcome = plan::decide(&snap);
    let run_ok = protocol.is_some_and(|p| p >= RUN_PROTOCOL);
    for a in outcome.actions {
        match a {
            Action::Status {
                to,
                reason,
                resume_at,
                message,
            } => {
                hub.lock()
                    .set_epic_run_status(run.id, to, reason.as_deref(), resume_at, now)
                    .map_err(ledger_err)?;
                rec(
                    EpicRunEvent {
                        reason: Some(to.as_str().into()),
                        ..ev(run, now, "status", None, message)
                    },
                    &mut out,
                )?;
            }
            Action::Reschedule { resume_at, message } => {
                hub.lock()
                    .set_epic_run_resume_at(run.id, resume_at, now)
                    .map_err(ledger_err)?;
                rec(ev(run, now, "rescheduled", None, message), &mut out)?;
            }
            Action::Node {
                number,
                to,
                pr,
                reason,
                message,
            } => {
                let h = hub.lock();
                h.set_epic_node_state(run.id, number, to, reason.as_deref(), now)
                    .map_err(ledger_err)?;
                if pr.is_some() {
                    h.update_epic_node(
                        run.id,
                        number,
                        &EpicNodePatch {
                            pr: Some(pr),
                            ..Default::default()
                        },
                        now,
                    )
                    .map_err(ledger_err)?;
                }
                drop(h);
                let kind = if to == EpicNodeState::Assumida {
                    "assumed"
                } else {
                    "node"
                };
                rec(
                    EpicRunEvent {
                        reason: Some(to.as_str().into()),
                        ..ev(run, now, kind, Some(number), message)
                    },
                    &mut out,
                )?;
            }
            Action::Start {
                number,
                title,
                model,
                effort,
                message,
            } => {
                if !run_ok {
                    rec(
                        EpicRunEvent {
                            reason: Some("env_outdated".into()),
                            ..ev(
                                run,
                                now,
                                "refused",
                                None,
                                format!(
                                    "o ambiente da execução é anterior ao protocolo {}.{}: \
                                     atualize-o para iniciar as sub-issues",
                                    RUN_PROTOCOL.0, RUN_PROTOCOL.1
                                ),
                            )
                        },
                        &mut out,
                    )?;
                    continue;
                }
                let params = json!({
                    "owner": run.owner,
                    "repo": run.repo,
                    "gh_number": number,
                    "kind": "issue",
                    "title": title,
                    "prompt": node_prompt(ctx, number, &title, &wip),
                    "model": model,
                    "effort": effort,
                    "epic_run": { "base": wip },
                });
                match port.call(env.clone(), "session.start", params).await {
                    Ok(v) => {
                        let h = hub.lock();
                        h.set_epic_node_state(run.id, number, EpicNodeState::EmExecucao, None, now)
                            .map_err(ledger_err)?;
                        h.update_epic_node(
                            run.id,
                            number,
                            &EpicNodePatch {
                                session_id: Some(
                                    v.get("id").and_then(Value::as_str).map(String::from),
                                ),
                                worktree: Some(
                                    v.get("cwd").and_then(Value::as_str).map(String::from),
                                ),
                                attempts: Some(0),
                                reason: Some(None),
                                ..Default::default()
                            },
                            now,
                        )
                        .map_err(ledger_err)?;
                        drop(h);
                        rec(ev(run, now, "started", Some(number), message), &mut out)?;
                    }
                    Err((c, _)) if transient(c) => {}
                    Err((_, m)) => rec(
                        ev(
                            run,
                            now,
                            "start_failed",
                            Some(number),
                            format!("{}: o início da sessão falhou: {m}", slug(number)),
                        ),
                        &mut out,
                    )?,
                }
            }
            Action::Unpark {
                number,
                session_id,
                via,
                message,
            } => {
                let (method, params) = match via {
                    ResumeVia::Continue => {
                        ("session.continue", json!({ "session_id": session_id }))
                    }
                    ResumeVia::Resume => (
                        "session.resume",
                        json!({ "session_id": session_id, "prompt": RESUME_PROMPT }),
                    ),
                };
                match port.call(env.clone(), method, params).await {
                    Err((c, _)) if transient(c) => continue,
                    Ok(_) => {}
                    Err((_, m)) => {
                        rec(
                            ev(
                                run,
                                now,
                                "continue_failed",
                                Some(number),
                                format!("{}: a retomada falhou ({method}): {m}", slug(number)),
                            ),
                            &mut out,
                        )?;
                    }
                }
                hub.lock()
                    .restore_epic_node(run.id, number, now)
                    .map_err(ledger_err)?;
                rec(ev(run, now, "resumed", Some(number), message), &mut out)?;
            }
            Action::Send {
                number,
                session_id,
                kind,
                close,
                text,
                message,
            } => {
                let (kind_ev, reason) = match kind {
                    SendKind::Feedback(r) => ("feedback_sent", r),
                    SendKind::Merge => ("merge_requested", crate::autopilot::plan::MERGE_GREEN),
                };
                match port
                    .call(
                        env.clone(),
                        "session.continue",
                        json!({ "session_id": session_id, "text": text }),
                    )
                    .await
                {
                    Ok(_) => {
                        let h = hub.lock();
                        h.save_autopilot_close(&close, now).map_err(ledger_err)?;
                        h.update_epic_node(
                            run.id,
                            number,
                            &EpicNodePatch {
                                attempts: Some(close.attempts),
                                ..Default::default()
                            },
                            now,
                        )
                        .map_err(ledger_err)?;
                        drop(h);
                        rec(
                            EpicRunEvent {
                                reason: Some(reason.into()),
                                ..ev(run, now, kind_ev, Some(number), message)
                            },
                            &mut out,
                        )?;
                    }
                    Err((c, _)) if transient(c) => {}
                    Err((_, m)) => rec(
                        ev(
                            run,
                            now,
                            "continue_failed",
                            Some(number),
                            format!("{}: o envio ao agente falhou: {m}", slug(number)),
                        ),
                        &mut out,
                    )?,
                }
            }
            Action::Track { close, .. } => {
                hub.lock()
                    .save_autopilot_close(&close, now)
                    .map_err(ledger_err)?;
            }
            Action::Rebase {
                number,
                session_id,
                head,
                text,
                message,
            } => {
                let mut sent_at = None;
                if let Some(id) = session_id {
                    match port
                        .call(
                            env.clone(),
                            "session.continue",
                            json!({ "session_id": id, "text": text }),
                        )
                        .await
                    {
                        Ok(_) => sent_at = Some(now),
                        Err((c, _)) if transient(c) => continue,
                        Err((_, m)) => {
                            rec(
                                ev(
                                    run,
                                    now,
                                    "continue_failed",
                                    Some(number),
                                    format!("{}: o pedido de rebase falhou: {m}", slug(number)),
                                ),
                                &mut out,
                            )?;
                            continue;
                        }
                    }
                }
                hub.lock()
                    .update_epic_node(
                        run.id,
                        number,
                        &EpicNodePatch {
                            rebase_head: Some(Some(head)),
                            rebase_at: Some(sent_at),
                            ..Default::default()
                        },
                        now,
                    )
                    .map_err(ledger_err)?;
                rec(
                    ev(run, now, "rebase_requested", Some(number), message),
                    &mut out,
                )?;
            }
            Action::RebaseDone { number } => {
                hub.lock()
                    .update_epic_node(
                        run.id,
                        number,
                        &EpicNodePatch {
                            rebase_head: Some(None),
                            rebase_at: Some(None),
                            ..Default::default()
                        },
                        now,
                    )
                    .map_err(ledger_err)?;
            }
            Action::End {
                number,
                session_id,
                message,
            } => match port
                .call(
                    env.clone(),
                    "session.kill",
                    json!({ "session_id": session_id }),
                )
                .await
            {
                Err((c, _)) if transient(c) => {}
                _ => rec(ev(run, now, "ended", Some(number), message), &mut out)?,
            },
            Action::RemoveWorktree {
                number,
                worktree,
                message: _,
            } => {
                if !run_ok {
                    continue;
                }
                let r = port
                    .call(
                        env.clone(),
                        "worktree.remove",
                        json!({ "owner": run.owner, "repo": run.repo, "gh_number": number }),
                    )
                    .await;
                let (kind, reason, msg) = match r {
                    Err((c, _)) if transient(c) => continue,
                    Ok(v) if v.get("removed").and_then(Value::as_bool) == Some(true) => (
                        "worktree_removed",
                        None,
                        format!("{}: worktree {worktree} removido", slug(number)),
                    ),
                    Ok(v) => {
                        let why = v
                            .get("reason")
                            .and_then(Value::as_str)
                            .unwrap_or("motivo desconhecido")
                            .to_string();
                        (
                            "worktree_kept",
                            Some(format!("worktree mantido: {why}")),
                            format!("{}: o worktree {worktree} ficou: {why}", slug(number)),
                        )
                    }
                    Err((_, m)) => (
                        "worktree_kept",
                        Some(format!("worktree mantido: {m}")),
                        format!("{}: a remoção do worktree falhou: {m}", slug(number)),
                    ),
                };
                // Uma vez: com qualquer resultado, o nó fica sem worktree e o
                // motivo do bloqueio aparece nele.
                hub.lock()
                    .update_epic_node(
                        run.id,
                        number,
                        &EpicNodePatch {
                            worktree: Some(None),
                            reason: reason.map(Some),
                            ..Default::default()
                        },
                        now,
                    )
                    .map_err(ledger_err)?;
                rec(ev(run, now, kind, Some(number), msg), &mut out)?;
            }
            Action::StartIntegration { message } => {
                if !run_ok {
                    continue;
                }
                let prompt = integration_prompt(&hub.lock(), ctx, &wip).map_err(ledger_err)?;
                let params = json!({
                    "owner": run.owner,
                    "repo": run.repo,
                    "gh_number": run.epic,
                    "kind": "epic",
                    "title": ctx.epic_title,
                    "prompt": prompt,
                    "model": run.plan_model.as_deref().unwrap_or(prompt::PLAN_MODEL_DEFAULT),
                    "effort": run.plan_effort.as_deref().unwrap_or(prompt::PLAN_EFFORT_DEFAULT),
                    "epic_run": { "base": wip },
                });
                match port.call(env.clone(), "session.start", params).await {
                    Ok(v) => {
                        let id = v.get("id").and_then(Value::as_str).map(String::from);
                        hub.lock()
                            .set_epic_run_integration_session(run.id, id.as_deref(), now)
                            .map_err(ledger_err)?;
                        rec(ev(run, now, "integration_started", None, message), &mut out)?;
                    }
                    Err((c, _)) if transient(c) => {}
                    Err((_, m)) => rec(
                        ev(
                            run,
                            now,
                            "start_failed",
                            None,
                            format!("o início da sessão de integração falhou: {m}"),
                        ),
                        &mut out,
                    )?,
                }
            }
            Action::ClearIntegration { message } => {
                hub.lock()
                    .set_epic_run_integration_session(run.id, None, now)
                    .map_err(ledger_err)?;
                rec(ev(run, now, "integration_failed", None, message), &mut out)?;
            }
            Action::EndIntegration {
                session_id,
                message,
            } => {
                if let Err((c, _)) = port
                    .call(
                        env.clone(),
                        "session.kill",
                        json!({ "session_id": session_id }),
                    )
                    .await
                {
                    if transient(c) {
                        continue;
                    }
                }
                hub.lock()
                    .set_epic_run_integration_session(run.id, None, now)
                    .map_err(ledger_err)?;
                rec(ev(run, now, "integration_ended", None, message), &mut out)?;
            }
            Action::IntegrationSend {
                session_id,
                close,
                text,
                message,
            } => match port
                .call(
                    env.clone(),
                    "session.continue",
                    json!({ "session_id": session_id, "text": text }),
                )
                .await
            {
                Ok(_) => {
                    hub.lock()
                        .save_autopilot_close(&close, now)
                        .map_err(ledger_err)?;
                    rec(ev(run, now, "feedback_sent", None, message), &mut out)?;
                }
                Err((c, _)) if transient(c) => {}
                Err((_, m)) => rec(
                    ev(
                        run,
                        now,
                        "continue_failed",
                        None,
                        format!("o envio à sessão de integração falhou: {m}"),
                    ),
                    &mut out,
                )?,
            },
            Action::Ask {
                number,
                origin,
                block,
                text,
            } => {
                let h = hub.lock();
                let before = h.epic_decisions(run.id, true).map_err(ledger_err)?.len();
                let d = h
                    .add_epic_decision(run.id, number, origin, &text, now)
                    .map_err(ledger_err)?;
                let fresh = h.epic_decisions(run.id, true).map_err(ledger_err)?.len() > before;
                if let (true, Some(n)) = (block, number) {
                    if h.epic_node(run.id, n)
                        .map_err(ledger_err)?
                        .is_some_and(|x| x.state.can_transition(EpicNodeState::Bloqueada))
                    {
                        h.set_epic_node_state(
                            run.id,
                            n,
                            EpicNodeState::Bloqueada,
                            Some(origin.as_str()),
                            now,
                        )
                        .map_err(ledger_err)?;
                    }
                }
                drop(h);
                if fresh {
                    out.decisions += 1;
                    rec(
                        EpicRunEvent {
                            reason: Some(origin.as_str().into()),
                            ..ev(run, now, "decision_requested", number, d.text)
                        },
                        &mut out,
                    )?;
                }
            }
        }
    }
    Ok(out)
}

/// Inicia (ou reinicia) a sessão de plano da execução em `planejando` ou
/// `replanejando`.
async fn plan_session(
    hub: &SharedHub,
    port: &dyn EnvPort,
    ctx: &RunCtx,
    wip: &str,
    protocol: Option<(u32, u32)>,
    now: i64,
    out: &mut TickResult,
) -> Result<(), String> {
    let run = &ctx.run;
    if let Some(id) = &run.plan_session {
        let info = hub.lock().epic_session_info(id).map_err(ledger_err)?;
        let alive = match &info {
            Some(i) => i.state.is_live(),
            None => now - run.updated_at < PLAN_START_GRACE_MS,
        };
        if alive {
            return Ok(());
        }
        let h = hub.lock();
        h.set_epic_run_plan_session(run.id, None, now)
            .map_err(ledger_err)?;
        if record(
            &h,
            ev(
                run,
                now,
                "plan_failed",
                None,
                "a sessão de plano terminou sem gravar o plano: o hub inicia outra".into(),
            ),
        )
        .map_err(ledger_err)?
        {
            out.changed += 1;
        }
        return Ok(());
    }
    if !port.online(&run.env_id) {
        return Ok(());
    }
    if !protocol.is_some_and(|p| p >= PLAN_PROTOCOL) {
        if record(
            &hub.lock(),
            EpicRunEvent {
                reason: Some("env_outdated".into()),
                ..ev(
                    run,
                    now,
                    "refused",
                    None,
                    format!(
                        "o ambiente da execução é anterior ao protocolo {}.{}: atualize-o para \
                         a sessão de plano",
                        PLAN_PROTOCOL.0, PLAN_PROTOCOL.1
                    ),
                )
            },
        )
        .map_err(ledger_err)?
        {
            out.changed += 1;
        }
        return Ok(());
    }
    let vars = {
        let h = hub.lock();
        let subs = h
            .epic_sub_issues(ctx.project.id, run.epic)
            .map_err(ledger_err)?;
        let issues = h
            .list_issues(ctx.project.id)
            .map_err(ledger_err)?
            .into_iter()
            .filter(|i| subs.contains(&i.gh_number))
            .map(|i| PlanIssue {
                number: i.gh_number,
                title: i.title,
                state: i.state,
            })
            .collect();
        let manual = h
            .autopilot_dependencies(ctx.project.id)
            .map_err(ledger_err)?
            .into_iter()
            .filter(|d| d.source == ct_hub_store::DEP_SOURCE_MANUAL && subs.contains(&d.number))
            .map(|d| (d.number, d.blocker, d.removed))
            .collect();
        PlanVars {
            owner: run.owner.clone(),
            repo: run.repo.clone(),
            epic_ref: ctx.project.item_ref(run.epic),
            epic_title: ctx.epic_title.clone(),
            wip: wip.to_string(),
            issues,
            manual,
        }
    };
    let params = json!({
        "owner": run.owner,
        "repo": run.repo,
        "prompt": prompt::plan_prompt(&vars),
        "epic_plan": run.epic,
        "model": run.plan_model.as_deref().unwrap_or(prompt::PLAN_MODEL_DEFAULT),
        "effort": run.plan_effort.as_deref().unwrap_or(prompt::PLAN_EFFORT_DEFAULT),
    });
    match port
        .call(run.env_id.clone(), "session.start_project", params)
        .await
    {
        Ok(v) => {
            let id = v.get("id").and_then(Value::as_str).map(String::from);
            let h = hub.lock();
            h.set_epic_run_plan_session(run.id, id.as_deref(), now)
                .map_err(ledger_err)?;
            if record(
                &h,
                ev(
                    run,
                    now,
                    "plan_started",
                    None,
                    "sessão de plano iniciada".into(),
                ),
            )
            .map_err(ledger_err)?
            {
                out.changed += 1;
            }
        }
        Err((c, _)) if transient(c) => {}
        Err((_, m)) => {
            if record(
                &hub.lock(),
                ev(
                    run,
                    now,
                    "plan_failed",
                    None,
                    format!("o início da sessão de plano falhou: {m}"),
                ),
            )
            .map_err(ledger_err)?
            {
                out.changed += 1;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ct_http::rpc::RpcResult;
    use ct_hub_store::{EpicApproval, EpicPlanDraft, EpicPlanNodeInput, NewEpicRun, NewHubProject};
    use ct_ledger::{GithubSnapshot, IssueRow};
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::Mutex;

    const T: i64 = 1_800_000_000_000;

    #[derive(Default)]
    struct FakePort {
        calls: Mutex<Vec<(String, Value)>>,
        /// Resposta do `worktree.remove`.
        removal: Option<Value>,
    }

    impl FakePort {
        fn methods(&self) -> Vec<String> {
            self.calls
                .lock()
                .unwrap()
                .iter()
                .map(|(m, _)| m.clone())
                .collect()
        }
        fn call_of(&self, method: &str) -> Option<Value> {
            self.calls
                .lock()
                .unwrap()
                .iter()
                .find(|(m, _)| m == method)
                .map(|(_, p)| p.clone())
        }
    }

    impl EnvPort for FakePort {
        fn online(&self, _: &str) -> bool {
            true
        }
        fn call(
            &self,
            _env: String,
            method: &'static str,
            params: Value,
        ) -> Pin<Box<dyn Future<Output = RpcResult> + Send + '_>> {
            let n = params.get("gh_number").and_then(Value::as_i64).unwrap_or(0);
            if method != "env.info" {
                self.calls
                    .lock()
                    .unwrap()
                    .push((method.to_string(), params.clone()));
            }
            let r = match method {
                "env.info" => json!({ "protocol": "2.26" }),
                "session.start" => json!({ "id": format!("s-{n}"), "cwd": format!("/wt/{n}") }),
                "session.start_project" => json!({ "id": "s-plano" }),
                "worktree.remove" => self.removal.clone().unwrap_or(json!({ "removed": true })),
                _ => json!({}),
            };
            Box::pin(async move { Ok(r) })
        }
    }

    fn issue(n: i64, parent: Option<i64>) -> IssueRow {
        IssueRow {
            gh_number: n,
            title: format!("ACME-{n}"),
            state: "open".into(),
            url: String::new(),
            updated_at: 1,
            parent_number: parent,
            ..Default::default()
        }
    }

    /// Hub com o épico 10 (branch `wip/10-x`), as sub-issues 11 e 12 e o
    /// ambiente `env-a`.
    fn setup(hub: &Hub) -> i64 {
        let pid = hub
            .add_project(&NewHubProject {
                owner: "acme".into(),
                repo: "acme-web".into(),
                name: "acme-web".into(),
                base_branch: "develop".into(),
                protected_branches: vec![],
            })
            .unwrap()
            .project
            .id;
        hub.apply_github(
            &GithubSnapshot {
                project_id: pid,
                issues: vec![issue(10, None), issue(11, Some(10)), issue(12, Some(10))],
                ..Default::default()
            },
            "env-a",
        )
        .unwrap();
        hub.conn()
            .execute_batch(&format!(
                "INSERT INTO env (env_id, name, created_at, last_seen_at) VALUES ('env-a', 'a', 1, 1);
                 INSERT INTO work_item (project_id, gh_number, kind, status, branch, created_at)
                   VALUES ({pid}, 10, 'epic', 'em_execucao', 'wip/10-x', 1);"
            ))
            .unwrap();
        pid
    }

    fn new_run(pid: i64) -> NewEpicRun {
        NewEpicRun {
            project_id: pid,
            epic: 10,
            env_id: "env-a".into(),
            started_by: "fulano".into(),
            max_parallel: None,
            plan_model: None,
            plan_effort: None,
        }
    }

    /// Execução aprovada com 11 e 12 (12 bloqueada pela 11).
    fn approved(hub: &Hub, pid: i64) -> EpicRun {
        let run = hub.create_epic_run(&new_run(pid), T).unwrap();
        hub.set_dependency_manual(pid, 12, 11, ct_hub_store::ManualEdit::Add, "", T)
            .unwrap();
        let node = |n: i64| EpicPlanNodeInput {
            number: n,
            model: Some("sonnet".into()),
            effort: Some("low".into()),
            areas: vec![],
            reason: String::new(),
        };
        hub.save_epic_plan_draft(
            run.id,
            &EpicPlanDraft {
                nodes: vec![node(11), node(12)],
                ..Default::default()
            },
            T,
        )
        .unwrap();
        hub.approve_epic_plan(
            run.id,
            &EpicApproval {
                version: 1,
                approved_by: "fulano".into(),
                max_parallel: 2,
                code_review: false,
                fingerprints: Default::default(),
            },
            T,
        )
        .unwrap()
    }

    fn kinds(hub: &Hub, run: i64) -> Vec<String> {
        hub.epic_run_events(run, 0, 100)
            .unwrap()
            .into_iter()
            .map(|e| e.kind)
            .collect()
    }

    #[tokio::test]
    async fn sessao_de_plano_inicia_e_encerra_quando_o_plano_chega() {
        let hub = SharedHub::new(Hub::open_in_memory().unwrap());
        let pid = setup(&hub.lock());
        let run = hub.lock().create_epic_run(&new_run(pid), T).unwrap();
        let port = FakePort::default();
        tick(&hub, &port, T).await.unwrap();
        let p = port.call_of("session.start_project").unwrap();
        assert_eq!(p["epic_plan"], 10);
        assert_eq!(p["model"], "opus");
        assert!(
            p["prompt"].as_str().unwrap().contains("- #11 ACME-11"),
            "{p}"
        );
        let r = hub.lock().epic_run(run.id).unwrap().unwrap();
        assert_eq!(r.plan_session.as_deref(), Some("s-plano"));
        // Viva (ainda desconhecida, na folga): não inicia outra.
        tick(&hub, &port, T + 60_000).await.unwrap();
        assert_eq!(
            port.methods()
                .iter()
                .filter(|m| *m == "session.start_project")
                .count(),
            1
        );
        // O plano chegou: a sessão é encerrada.
        hub.lock()
            .save_epic_plan_draft(
                run.id,
                &EpicPlanDraft {
                    nodes: vec![EpicPlanNodeInput {
                        number: 11,
                        model: Some("haiku".into()),
                        ..Default::default()
                    }],
                    ..Default::default()
                },
                T + 1,
            )
            .unwrap();
        tick(&hub, &port, T + 120_000).await.unwrap();
        assert_eq!(
            port.call_of("session.kill").unwrap()["session_id"],
            "s-plano"
        );
        let r = hub.lock().epic_run(run.id).unwrap().unwrap();
        assert_eq!(r.plan_session, None);
        assert_eq!(
            kinds(&hub.lock(), run.id),
            vec!["plan_started", "plan_session_ended"]
        );
    }

    #[tokio::test]
    async fn inicia_da_wip_com_o_modelo_e_continua_depois_de_reabrir_o_hub() {
        let d = tempfile::tempdir().unwrap();
        let db = d.path().join("hub.db");
        let (pid, run) = {
            let hub = Hub::open(&db).unwrap();
            let pid = setup(&hub);
            let run = approved(&hub, pid);
            (pid, run)
        };
        let _ = pid;
        let hub = SharedHub::new(Hub::open(&db).unwrap());
        let port = FakePort::default();
        // Ciclo 1: a 11 fica pronta (a 12 espera a 11).
        tick(&hub, &port, T + 1).await.unwrap();
        // Ciclo 2: inicia a 11.
        tick(&hub, &port, T + 2).await.unwrap();
        let p = port.call_of("session.start").unwrap();
        assert_eq!(p["gh_number"], 11);
        assert_eq!(p["epic_run"]["base"], "wip/10-x");
        assert_eq!(
            (p["model"].as_str(), p["effort"].as_str()),
            (Some("sonnet"), Some("low"))
        );
        assert!(p["prompt"].as_str().unwrap().contains("Closes #11"), "{p}");
        let n = hub.lock().epic_node(run.id, 11).unwrap().unwrap();
        assert_eq!(n.state, EpicNodeState::EmExecucao);
        assert_eq!(n.session_id.as_deref(), Some("s-11"));
        assert_eq!(n.worktree.as_deref(), Some("/wt/11"));
        // "Reinício" do hub: reabre o banco e segue do estado gravado, sem
        // iniciar a 11 de novo nem a 12 (bloqueada).
        drop(hub);
        let hub = SharedHub::new(Hub::open(&db).unwrap());
        let port = FakePort::default();
        tick(&hub, &port, T + 3).await.unwrap();
        assert!(
            port.call_of("session.start").is_none(),
            "{:?}",
            port.methods()
        );
        assert_eq!(
            hub.lock().epic_node(run.id, 12).unwrap().unwrap().state,
            EpicNodeState::Pendente
        );
    }

    #[tokio::test]
    async fn worktree_mesclado_sai_uma_vez_e_o_bloqueio_fica_no_no() {
        for (removal, reason) in [
            (json!({ "removed": true }), None),
            (
                json!({ "removed": false, "reason": "mudança não commitada: x.txt" }),
                Some("worktree mantido: mudança não commitada: x.txt"),
            ),
        ] {
            let hub = SharedHub::new(Hub::open_in_memory().unwrap());
            let pid = setup(&hub.lock());
            let run = approved(&hub.lock(), pid);
            {
                let h = hub.lock();
                for to in [
                    EpicNodeState::Pronta,
                    EpicNodeState::EmExecucao,
                    EpicNodeState::EmPr,
                    EpicNodeState::Mesclada,
                ] {
                    h.set_epic_node_state(run.id, 11, to, None, T).unwrap();
                }
                h.update_epic_node(
                    run.id,
                    11,
                    &EpicNodePatch {
                        worktree: Some(Some("/wt/11".into())),
                        ..Default::default()
                    },
                    T,
                )
                .unwrap();
            }
            let port = FakePort {
                removal: Some(removal),
                ..Default::default()
            };
            tick(&hub, &port, T + 1).await.unwrap();
            let p = port.call_of("worktree.remove").unwrap();
            assert_eq!(
                (p["gh_number"].as_i64(), p["repo"].as_str()),
                (Some(11), Some("acme-web"))
            );
            let n = hub.lock().epic_node(run.id, 11).unwrap().unwrap();
            assert_eq!((n.worktree, n.reason.as_deref()), (None, reason));
            // A execução segue: a 12 ficou pronta e inicia no ciclo seguinte.
            tick(&hub, &port, T + 2).await.unwrap();
            assert_eq!(
                port.methods()
                    .iter()
                    .filter(|m| *m == "worktree.remove")
                    .count(),
                1
            );
            assert_eq!(port.call_of("session.start").unwrap()["gh_number"], 12);
            let k = kinds(&hub.lock(), run.id);
            assert!(
                k.contains(&if reason.is_none() {
                    "worktree_removed".to_string()
                } else {
                    "worktree_kept".to_string()
                }),
                "{k:?}"
            );
        }
    }

    #[tokio::test]
    async fn entrada_humana_no_hub_assume_o_no() {
        let hub = SharedHub::new(Hub::open_in_memory().unwrap());
        let pid = setup(&hub.lock());
        let run = approved(&hub.lock(), pid);
        {
            let h = hub.lock();
            for to in [EpicNodeState::Pronta, EpicNodeState::EmExecucao] {
                h.set_epic_node_state(run.id, 11, to, None, T).unwrap();
            }
            h.update_epic_node(
                run.id,
                11,
                &EpicNodePatch {
                    session_id: Some(Some("s-11".into())),
                    ..Default::default()
                },
                T,
            )
            .unwrap();
            let wi = h
                .find_work_item("acme", "acme-web", 10)
                .unwrap()
                .unwrap()
                .id;
            h.conn()
                .execute_batch(&format!(
                    "INSERT INTO session (id, env_id, work_item_id, tool, cwd, state, started_at,
                                          last_event_at)
                       VALUES ('s-11', 'env-a', {wi}, 'claude', '/wt/11', 'running', {T}, {T});
                     INSERT INTO event (uid, env_id, hlc, ts, project_id, work_item_id,
                                        session_id, type, payload)
                       VALUES ('u-1', 'env-a', 1, {t}, {pid}, {wi}, 's-11', 'human_input', '{{}}');",
                    t = T + 5
                ))
                .unwrap();
            assert_eq!(h.last_human_input("s-11").unwrap(), Some(T + 5));
        }
        let port = FakePort::default();
        tick(&hub, &port, T + 10).await.unwrap();
        let n = hub.lock().epic_node(run.id, 11).unwrap().unwrap();
        assert_eq!(
            (n.state, n.assumed_since),
            (EpicNodeState::Assumida, Some(T + 10))
        );
        assert!(kinds(&hub.lock(), run.id).contains(&"assumed".to_string()));
    }

    #[tokio::test]
    async fn integracao_inicia_a_sessao_do_epico_com_o_corpo_do_pr() {
        let hub = SharedHub::new(Hub::open_in_memory().unwrap());
        let pid = setup(&hub.lock());
        let run = approved(&hub.lock(), pid);
        {
            let h = hub.lock();
            for n in [11, 12] {
                for to in [
                    EpicNodeState::Pronta,
                    EpicNodeState::EmExecucao,
                    EpicNodeState::Mesclada,
                ] {
                    h.set_epic_node_state(run.id, n, to, None, T).unwrap();
                }
            }
        }
        let port = FakePort::default();
        // Ciclo 1: integrando; ciclo 2: a sessão de integração.
        tick(&hub, &port, T + 1).await.unwrap();
        assert_eq!(
            hub.lock().epic_run(run.id).unwrap().unwrap().status,
            EpicRunStatus::Integrando
        );
        tick(&hub, &port, T + 2).await.unwrap();
        let p = port.call_of("session.start").unwrap();
        assert_eq!(
            (p["gh_number"].as_i64(), p["kind"].as_str()),
            (Some(10), Some("epic"))
        );
        assert_eq!(p["epic_run"]["base"], "wip/10-x");
        let prompt = p["prompt"].as_str().unwrap();
        for l in [
            "Closes #11",
            "Closes #12",
            "Closes #10",
            "Não rode o /code-review",
        ] {
            assert!(prompt.contains(l), "{l}\n{prompt}");
        }
        assert_eq!(
            hub.lock()
                .epic_run(run.id)
                .unwrap()
                .unwrap()
                .integration_session
                .as_deref(),
            Some("s-10")
        );
    }
}
