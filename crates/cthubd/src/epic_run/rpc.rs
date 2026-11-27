//! `hub.epic_run.*` (#298): disparo, aprovação do plano, controle e
//! acompanhamento das execuções de épico, e as respostas aos pedidos de
//! decisão. O app desktop e a UI web chamam igual.

use crate::autopilot::runner::{EnvPort, HubPort};
use crate::HubState;
use ct_core::{EpicNodeState, EpicRunStatus, PrState};
use ct_http::rpc::{i64_param, invalid, str_param, RpcResult};
use ct_hub_store::{
    EpicApproval, EpicDecision, EpicNode, EpicNodePatch, EpicPlan, EpicRun, EpicRunEvent, Hub,
    NewEpicRun,
};
use ct_ledger::LedgerError;
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;

/// Resposta de `hub.epic_run.status`.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct EpicRunView {
    pub run: EpicRun,
    /// A versão mais recente do plano (rascunho ou aprovada).
    pub plan: Option<EpicPlan>,
    /// A versão aprovada em vigor, quando o mais recente é outro rascunho.
    pub approved: Option<EpicPlan>,
    pub nodes: Vec<EpicNode>,
    /// Os pedidos de decisão, do mais antigo ao mais novo.
    pub decisions: Vec<EpicDecision>,
    /// Os 100 eventos mais recentes, do mais antigo ao mais novo.
    pub events: Vec<EpicRunEvent>,
    /// Título das sub-issues no espelho, por número.
    pub titles: BTreeMap<i64, String>,
    /// Último ciclo do motor (ms).
    pub last_tick_at: Option<i64>,
    /// O PR do épico (`wip` → base), do espelho: o aberto ou, sem ele, o
    /// mais recente (#341). `None` = ainda não aberto.
    #[serde(default)]
    pub epic_pr: Option<ct_ledger::PrRow>,
}

fn ledger(e: LedgerError) -> (i64, String) {
    match e {
        LedgerError::Invalid(m) | LedgerError::Conflict(m) => invalid(m),
        LedgerError::NotFound(what, id) => (-32004, format!("{what} não encontrado: {id}")),
        e => (-32000, e.to_string()),
    }
}

fn run_of(h: &Hub, p: &Value) -> Result<EpicRun, (i64, String)> {
    let id = i64_param(p, "run_id")?;
    h.epic_run(id)
        .map_err(ledger)?
        .ok_or_else(|| (-32004, format!("execução de épico não encontrada: {id}")))
}

fn opt_str(p: &Value, k: &str) -> Result<Option<String>, (i64, String)> {
    match p.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) if !s.trim().is_empty() => Ok(Some(s.trim().to_string())),
        Some(_) => Err(invalid(format!("{k} deve ser texto ou null"))),
    }
}

fn opt_u32(p: &Value, k: &str) -> Result<Option<u32>, (i64, String)> {
    match p.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .map(Some)
            .ok_or_else(|| invalid(format!("{k} deve ser um inteiro positivo"))),
    }
}

/// O estado da execução para a UI.
pub fn view(h: &Hub, run: EpicRun, last_tick_at: Option<i64>) -> ct_ledger::Result<EpicRunView> {
    let plan = h.latest_epic_plan(run.id)?;
    let approved = match (run.plan_version, &plan) {
        (Some(v), Some(p)) if p.version != v => h.epic_plan(run.id, v)?,
        _ => None,
    };
    let project = h.find_project(&run.owner, &run.repo)?;
    let titles = match &project {
        Some(p) => h
            .list_issues(p.id)?
            .into_iter()
            .filter(|i| i.parent_number == Some(run.epic) || i.gh_number == run.epic)
            .map(|i| (i.gh_number, i.title))
            .collect(),
        None => BTreeMap::new(),
    };
    let epic_pr = match (&project, h.find_work_item(&run.owner, &run.repo, run.epic)?) {
        (Some(p), Some(w)) => match w.branch.as_deref() {
            Some(wip) => {
                let mut prs: Vec<_> = h
                    .prs_for_work_item(w.id)?
                    .into_iter()
                    .filter(|pr| pr.head == wip && pr.base == p.base_branch)
                    .collect();
                prs.sort_by_key(|pr| (pr.state == PrState::Open, pr.number));
                prs.pop()
            }
            None => None,
        },
        _ => None,
    };
    Ok(EpicRunView {
        nodes: h.epic_nodes(run.id)?,
        decisions: h.epic_decisions(run.id, false)?,
        events: h.epic_run_events_tail(run.id, 100)?,
        plan,
        approved,
        titles,
        last_tick_at,
        epic_pr,
        run,
    })
}

/// O status para onde a execução volta (todas mescladas = integração).
fn back_status(h: &Hub, run: &EpicRun) -> ct_ledger::Result<EpicRunStatus> {
    let nodes = h.epic_nodes(run.id)?;
    let all = !nodes.is_empty() && nodes.iter().all(|n| n.state == EpicNodeState::Mesclada);
    Ok(if all {
        EpicRunStatus::Integrando
    } else {
        EpicRunStatus::Executando
    })
}

fn event(h: &Hub, run: &EpicRun, kind: &str, number: Option<i64>, message: String) {
    let _ = h.add_epic_run_event(&EpicRunEvent {
        run_id: run.id,
        at: ct_core::now_ms(),
        kind: kind.into(),
        number,
        message,
        ..Default::default()
    });
}

/// Sessões vivas dirigidas pelo motor numa execução: (nó, sessão).
fn live_sessions(h: &Hub, run: &EpicRun) -> ct_ledger::Result<Vec<(i64, String)>> {
    let mut out = Vec::new();
    for n in h.epic_nodes(run.id)? {
        let Some(id) = n.session_id.clone() else {
            continue;
        };
        if matches!(n.state, EpicNodeState::EmExecucao | EpicNodeState::EmPr)
            && h.epic_session_info(&id)?.is_some_and(|i| i.state.is_live())
        {
            out.push((n.number, id));
        }
    }
    Ok(out)
}

/// `hub.epic_run.*`. `web` = vindo do `/web/ws`.
pub(crate) async fn call(st: &HubState, method: &str, p: Value, web: bool) -> RpcResult {
    let now = ct_core::now_ms();
    let started_by = if web { "web" } else { "desktop" };
    // Os que falam com o ambiente agora: interromper, cancelar e reiniciar.
    match method {
        "hub.epic_run.interrupt" => return interrupt(st, &p, now).await,
        "hub.epic_run.cancel" => return cancel(st, &p, now).await,
        "hub.epic_run.decide" => return decide(st, &p, started_by, now).await,
        _ => {}
    }
    let (st2, method2) = (st.clone(), method.to_string());
    let (v, changed) =
        tokio::task::spawn_blocking(move || -> Result<(Value, bool), (i64, String)> {
            let h = st2.hub.lock();
            let last = st2.epic_runs.last_tick_at();
            match method2.as_str() {
                "hub.epic_run.list" => {
                    let runs = match (opt_str(&p, "owner")?, opt_str(&p, "repo")?) {
                        (Some(o), Some(r)) => {
                            let pid = h
                                .find_project(&o, &r)
                                .map_err(ledger)?
                                .ok_or_else(|| invalid(format!("projeto {o}/{r} não está no hub")))?
                                .id;
                            let epic = i64_param(&p, "epic")?;
                            h.epic_runs(pid, epic).map_err(ledger)?
                        }
                        // Com `ended_since` (ms), também as encerradas desde
                        // então: o histórico do índice (#341).
                        _ => match p.get("ended_since").and_then(Value::as_i64) {
                            Some(since) => h.epic_runs_with_history(since).map_err(ledger)?,
                            None => h.active_epic_runs().map_err(ledger)?,
                        },
                    };
                    Ok((json!(runs), false))
                }
                "hub.epic_run.status" => {
                    let run = run_of(&h, &p)?;
                    Ok((json!(view(&h, run, last).map_err(ledger)?), false))
                }
                "hub.epic_run.events" => {
                    let run = run_of(&h, &p)?;
                    let after = p.get("after").and_then(Value::as_i64).unwrap_or(0);
                    let limit = p
                        .get("limit")
                        .and_then(Value::as_u64)
                        .unwrap_or(200)
                        .min(1000);
                    Ok((
                        json!(h
                            .epic_run_events(run.id, after, limit as usize)
                            .map_err(ledger)?),
                        false,
                    ))
                }
                "hub.epic_run.start" => {
                    let owner = str_param(&p, "owner")?;
                    let repo = str_param(&p, "repo")?;
                    let epic = i64_param(&p, "epic")?;
                    let env_id = str_param(&p, "env_id")?.to_string();
                    let project =
                        h.find_project(owner, repo)
                            .map_err(ledger)?
                            .ok_or_else(|| {
                                invalid(format!("projeto {owner}/{repo} não está no hub"))
                            })?;
                    if !h
                        .list_envs()
                        .map_err(ledger)?
                        .iter()
                        .any(|e| e.env_id == env_id)
                    {
                        return Err(invalid(format!("ambiente desconhecido: {env_id}")));
                    }
                    if h.epic_sub_issues(project.id, epic)
                        .map_err(ledger)?
                        .is_empty()
                    {
                        return Err(invalid(format!(
                            "{}: sem sub-issues no espelho do hub",
                            project.item_ref(epic)
                        )));
                    }
                    // Sem a `wip`, o motor pede ao ambiente que a crie
                    // antes da sessão de plano (#341).
                    let run = h
                        .create_epic_run(
                            &NewEpicRun {
                                project_id: project.id,
                                epic,
                                env_id,
                                started_by: started_by.into(),
                                max_parallel: opt_u32(&p, "max_parallel")?,
                                plan_model: opt_str(&p, "plan_model")?,
                                plan_effort: opt_str(&p, "plan_effort")?,
                            },
                            now,
                        )
                        .map_err(ledger)?;
                    event(
                        &h,
                        &run,
                        "created",
                        None,
                        format!("execução disparada ({started_by})"),
                    );
                    Ok((json!(run), true))
                }
                "hub.epic_run.approve" => {
                    let run = run_of(&h, &p)?;
                    let version = i64_param(&p, "version")?;
                    let project = h
                        .find_project(&run.owner, &run.repo)
                        .map_err(ledger)?
                        .ok_or_else(|| invalid("projeto da execução não está no hub"))?;
                    // Impressão digital de cada sub-issue agora, congelada.
                    let issues = h.list_issues(project.id).map_err(ledger)?;
                    let mut fingerprints = BTreeMap::new();
                    for i in &issues {
                        let prio = h
                            .find_work_item(&run.owner, &run.repo, i.gh_number)
                            .map_err(ledger)?
                            .and_then(|w| w.priority);
                        fingerprints.insert(
                            i.gh_number,
                            ct_hub_store::issue_fingerprint(
                                &i.title,
                                i.updated_at,
                                prio.map(|p| p.as_str()),
                                &i.labels,
                            ),
                        );
                    }
                    let code_review = match p.get("code_review") {
                        None | Some(Value::Null) => run.code_review,
                        Some(Value::Bool(b)) => *b,
                        Some(_) => return Err(invalid("code_review deve ser true ou false")),
                    };
                    let run = h
                        .approve_epic_plan(
                            run.id,
                            &EpicApproval {
                                version,
                                approved_by: started_by.into(),
                                max_parallel: opt_u32(&p, "max_parallel")?
                                    .unwrap_or(run.max_parallel),
                                code_review,
                                fingerprints,
                            },
                            now,
                        )
                        .map_err(ledger)?;
                    event(
                        &h,
                        &run,
                        "plan_approved",
                        None,
                        format!(
                            "plano v{version} aprovado (paralelismo {}, /code-review {})",
                            run.max_parallel,
                            if run.code_review {
                                "ligado"
                            } else {
                                "desligado"
                            }
                        ),
                    );
                    Ok((json!(run), true))
                }
                "hub.epic_run.replan" => {
                    let run = run_of(&h, &p)?;
                    let reason = opt_str(&p, "reason")?;
                    let to = if run.plan_version.is_none() {
                        EpicRunStatus::Planejando
                    } else {
                        EpicRunStatus::Replanejando
                    };
                    let run = h
                        .set_epic_run_status(run.id, to, None, None, now)
                        .map_err(ledger)?;
                    event(
                        &h,
                        &run,
                        "replan_requested",
                        None,
                        match reason {
                            Some(r) => format!("outro plano pedido: {r}"),
                            None => "outro plano pedido".into(),
                        },
                    );
                    Ok((json!(run), true))
                }
                "hub.epic_run.pause" => {
                    let run = run_of(&h, &p)?;
                    let reason = opt_str(&p, "reason")?;
                    let run = h
                        .set_epic_run_status(
                            run.id,
                            EpicRunStatus::Pausado,
                            Some(reason.as_deref().unwrap_or("pausa manual")),
                            None,
                            now,
                        )
                        .map_err(ledger)?;
                    event(
                        &h,
                        &run,
                        "paused",
                        None,
                        "pausada: nada novo inicia, e as sessões estacionam ao fim do turno".into(),
                    );
                    Ok((json!(run), true))
                }
                "hub.epic_run.resume" => {
                    let run = run_of(&h, &p)?;
                    if !matches!(
                        run.status,
                        EpicRunStatus::Pausado | EpicRunStatus::Estacionado
                    ) {
                        return Err(invalid(format!(
                            "a execução não está pausada ({})",
                            run.status
                        )));
                    }
                    let to = back_status(&h, &run).map_err(ledger)?;
                    let run = h
                        .set_epic_run_status(run.id, to, None, None, now)
                        .map_err(ledger)?;
                    event(
                        &h,
                        &run,
                        "unpaused",
                        None,
                        "retomada pelo desenvolvedor".into(),
                    );
                    Ok((json!(run), true))
                }
                "hub.epic_run.set_parallel" => {
                    let run = run_of(&h, &p)?;
                    let n = opt_u32(&p, "max_parallel")?
                        .ok_or_else(|| invalid("parâmetro obrigatório: max_parallel"))?;
                    let run = h.set_epic_run_parallel(run.id, n, now).map_err(ledger)?;
                    event(&h, &run, "parallel_set", None, format!("paralelismo: {n}"));
                    Ok((json!(run), true))
                }
                "hub.epic_run.set_code_review" => {
                    let run = run_of(&h, &p)?;
                    let on = p
                        .get("code_review")
                        .and_then(Value::as_bool)
                        .ok_or_else(|| {
                            invalid("parâmetro obrigatório: code_review (true ou false)")
                        })?;
                    let run = h
                        .set_epic_run_code_review(run.id, on, now)
                        .map_err(ledger)?;
                    event(
                        &h,
                        &run,
                        "code_review_set",
                        None,
                        format!(
                            "/code-review na integração: {}",
                            if on { "ligado" } else { "desligado" }
                        ),
                    );
                    Ok((json!(run), true))
                }
                "hub.epic_run.set_node" => {
                    let run = run_of(&h, &p)?;
                    let n = i64_param(&p, "number")?;
                    let agent = ct_hub_store::IssueAgent {
                        model: opt_str(&p, "model")?,
                        effort: opt_str(&p, "effort")?,
                    };
                    let plan = h
                        .set_epic_plan_node_agent(run.id, n, &agent)
                        .map_err(ledger)?;
                    Ok((json!(plan), true))
                }
                "hub.epic_run.handback" => {
                    let run = run_of(&h, &p)?;
                    let n = i64_param(&p, "number")?;
                    let node = h.restore_epic_node(run.id, n, now).map_err(ledger)?;
                    // O pedido de merge sai de novo (#341): o anterior pode ter
                    // caído num diálogo que o desenvolvedor acabou de fechar.
                    if let Some(w) = h.find_work_item(&run.owner, &run.repo, n).map_err(ledger)? {
                        h.forget_close_merge(w.id).map_err(ledger)?;
                    }
                    event(
                        &h,
                        &run,
                        "handback",
                        Some(n),
                        format!("#{n} devolvida ao motor ({})", node.state),
                    );
                    Ok((json!(node), true))
                }
                m => Err((-32601, format!("método desconhecido: {m}"))),
            }
        })
        .await
        .map_err(|e| (-32603, e.to_string()))??;
    // Propostas da versão aprovada (#300): viram sub-issues do épico agora,
    // depois da aprovação.
    if method == "hub.epic_run.approve" {
        if let (Some(id), Some(version)) = (
            v.get("id").and_then(Value::as_i64),
            v.get("plan_version").and_then(Value::as_i64),
        ) {
            let (hub, gh) = (st.hub.clone(), st.github.clone());
            let _ = tokio::task::spawn_blocking(move || {
                create_proposals(&hub, id, version, |run, p| match &gh {
                    Some(g) => create_github_sub_issue(g.client(), run, p).map(Some),
                    None => Ok(None),
                })
            })
            .await;
        }
    }
    if changed {
        st.epic_runs_changed();
        st.epic_runs.kick();
    }
    Ok(v)
}

/// Cria as propostas da versão `version` do plano como sub-issues do
/// épico, com `create` (`Ok(Some(n))` = criada como #n; `Ok(None)` = o hub
/// não cria neste rastreador). Grava um evento por proposta:
/// `proposal_created`, `proposal_manual` ou `proposal_failed`. Só em
/// projeto do GitHub; nos outros, todas pedem a criação manual.
pub(crate) fn create_proposals(
    hub: &ct_hub_store::SharedHub,
    run_id: i64,
    version: i64,
    create: impl Fn(&EpicRun, &ct_hub_store::EpicProposal) -> Result<Option<i64>, String>,
) {
    let (run, plan, github) = {
        let h = hub.lock();
        let Ok(Some(run)) = h.epic_run(run_id) else {
            return;
        };
        let Ok(Some(plan)) = h.epic_plan(run_id, version) else {
            return;
        };
        let github = h
            .find_project(&run.owner, &run.repo)
            .ok()
            .flatten()
            .is_some_and(|p| p.tracker.kind() == ct_core::TrackerKind::Github);
        (run, plan, github)
    };
    for p in &plan.proposals {
        let r = if github { create(&run, p) } else { Ok(None) };
        let (kind, message) = match r {
            Ok(Some(n)) => (
                "proposal_created",
                format!(
                    "proposta \"{}\" criada como #{n} (sub-issue do épico)",
                    p.title
                ),
            ),
            Ok(None) => (
                "proposal_manual",
                format!(
                    "proposta \"{}\" aprovada: crie a sub-issue no rastreador e replaneje para \
                     incluí-la",
                    p.title
                ),
            ),
            Err(e) => (
                "proposal_failed",
                format!("a criação da proposta \"{}\" falhou: {e}", p.title),
            ),
        };
        event(&hub.lock(), &run, kind, None, message);
    }
}

/// Cria a issue no GitHub e a liga como sub-issue do épico.
fn create_github_sub_issue(
    gh: &ct_gh::GhClient,
    run: &EpicRun,
    p: &ct_hub_store::EpicProposal,
) -> Result<i64, String> {
    let mut body = p.body.trim().to_string();
    if !body.is_empty() {
        body.push_str("\n\n");
    }
    body.push_str(&format!("Parte do épico #{}.", run.epic));
    if let Some(f) = p.from {
        body.push_str(&format!(" Divide a #{f}."));
    }
    let v = gh
        .post_json(
            &format!("/repos/{}/{}/issues", run.owner, run.repo),
            &json!({ "title": p.title, "body": body }),
        )
        .map_err(|e| e.to_string())?;
    let number = v
        .get("number")
        .and_then(Value::as_i64)
        .ok_or("resposta do GitHub sem number")?;
    let id = v
        .get("id")
        .and_then(Value::as_i64)
        .ok_or("resposta do GitHub sem id")?;
    gh.post_json(
        &format!(
            "/repos/{}/{}/issues/{}/sub_issues",
            run.owner, run.repo, run.epic
        ),
        &json!({ "sub_issue_id": id }),
    )
    .map_err(|e| format!("#{number} criada, mas não foi ligada ao épico: {e}"))?;
    Ok(number)
}

/// "Interromper agora": pausa e encerra já as sessões dirigidas pelo motor;
/// os nós estacionam e voltam com `session.resume` na retomada.
async fn interrupt(st: &HubState, p: &Value, now: i64) -> RpcResult {
    let (run, live) = {
        let h = st.hub.lock();
        let run = run_of(&h, p)?;
        let run = if run.status == EpicRunStatus::Pausado {
            run
        } else {
            h.set_epic_run_status(
                run.id,
                EpicRunStatus::Pausado,
                Some("interrompida"),
                None,
                now,
            )
            .map_err(ledger)?
        };
        let live = live_sessions(&h, &run).map_err(ledger)?;
        (run, live)
    };
    let port = HubPort(st.clone());
    let mut killed = 0;
    for (n, id) in live {
        if port
            .call(
                run.env_id.clone(),
                "session.kill",
                json!({ "session_id": id }),
            )
            .await
            .is_ok()
        {
            killed += 1;
            let h = st.hub.lock();
            h.set_epic_node_state(
                run.id,
                n,
                EpicNodeState::Estacionada,
                Some("interrompida"),
                now,
            )
            .map_err(ledger)?;
        }
    }
    {
        let h = st.hub.lock();
        event(
            &h,
            &run,
            "interrupted",
            None,
            format!("interrompida agora: {killed} sessão(ões) encerrada(s)"),
        );
    }
    st.epic_runs_changed();
    Ok(json!(run))
}

/// Cancela: o motor para de dirigir; a sessão de plano viva é encerrada.
/// As sessões das sub-issues ficam como estão (a limpeza é a de sempre).
async fn cancel(st: &HubState, p: &Value, now: i64) -> RpcResult {
    let run = {
        let h = st.hub.lock();
        let run = run_of(&h, p)?;
        let run = h
            .set_epic_run_status(run.id, EpicRunStatus::Cancelado, None, None, now)
            .map_err(ledger)?;
        event(&h, &run, "cancelled", None, "execução cancelada".into());
        run
    };
    if let Some(id) = &run.plan_session {
        let port = HubPort(st.clone());
        let _ = port
            .call(
                run.env_id.clone(),
                "session.kill",
                json!({ "session_id": id }),
            )
            .await;
        let _ = st.hub.lock().set_epic_run_plan_session(run.id, None, now);
    }
    st.epic_runs_changed();
    Ok(json!(run))
}

/// Responde um pedido de decisão e aplica o efeito no nó (veja #300 para o
/// replanejamento).
async fn decide(st: &HubState, p: &Value, by: &str, now: i64) -> RpcResult {
    let id = i64_param(p, "decision_id")?;
    let answer = str_param(p, "answer")?.to_string();
    // "aguardar" (#300): a sub-issue espera este bloqueante.
    let blocker = match p.get("blocker") {
        None | Some(Value::Null) => None,
        Some(v) => Some(
            v.as_i64()
                .filter(|n| *n > 0)
                .ok_or_else(|| invalid("blocker deve ser o número de uma issue"))?,
        ),
    };
    let (run, d, kill) = {
        let h = st.hub.lock();
        if let Some(b) = blocker {
            let pending = h.epic_decision(id).map_err(ledger)?;
            if answer != "aguardar" || pending.as_ref().and_then(|d| d.number) == Some(b) {
                return Err(invalid(
                    "blocker só com a resposta aguardar, e diferente da própria sub-issue",
                ));
            }
        }
        let d = h
            .answer_epic_decision(id, &answer, by, now)
            .map_err(ledger)?;
        let run = h
            .epic_run(d.run_id)
            .map_err(ledger)?
            .ok_or_else(|| invalid("execução do pedido não encontrada"))?;
        let kill = apply_answer(&h, &run, &d, blocker, now).map_err(ledger)?;
        let detail = match blocker {
            Some(b) => format!(" (esperando a #{b})"),
            None => String::new(),
        };
        // O motivo do pedido vai junto: a sessão de plano o recebe no
        // replanejamento.
        event(
            &h,
            &run,
            "decided",
            d.number,
            format!(
                "pedido {} ({}) respondido: {answer}{detail}. Motivo: {}",
                d.id, d.origin, d.text
            ),
        );
        (run, d, kill)
    };
    if let Some(sid) = kill {
        let _ = HubPort(st.clone())
            .call(
                run.env_id.clone(),
                "session.kill",
                json!({ "session_id": sid }),
            )
            .await;
    }
    st.epic_runs_changed();
    st.epic_runs.kick();
    Ok(json!(d))
}

/// O efeito da resposta no nó; devolve a sessão a encerrar (reiniciar).
pub(crate) fn apply_answer(
    h: &Hub,
    run: &EpicRun,
    d: &EpicDecision,
    blocker: Option<i64>,
    now: i64,
) -> ct_ledger::Result<Option<String>> {
    let answer = d.answer.as_deref().unwrap_or_default();
    if answer == "replanejar" {
        if run.status.can_transition(EpicRunStatus::Replanejando) {
            h.set_epic_run_status(run.id, EpicRunStatus::Replanejando, None, None, now)?;
        }
        return Ok(None);
    }
    let Some(n) = d.number else {
        return Ok(None);
    };
    let Some(node) = h.epic_node(run.id, n)? else {
        return Ok(None);
    };
    let blocked = node.state == EpicNodeState::Bloqueada;
    match answer {
        "assumir" if node.state.can_transition(EpicNodeState::Assumida) => {
            h.set_epic_node_state(run.id, n, EpicNodeState::Assumida, None, now)?;
        }
        "mais_tentativas" if blocked => {
            // Zera as tentativas do fechamento e volta a acompanhar o PR.
            if let Some(w) = h.find_work_item(&run.owner, &run.repo, n)? {
                h.clear_autopilot_close(w.id)?;
            }
            let to = if node.pr.is_some() {
                EpicNodeState::EmPr
            } else {
                EpicNodeState::EmExecucao
            };
            h.set_epic_node_state(run.id, n, to, None, now)?;
            h.update_epic_node(
                run.id,
                n,
                &EpicNodePatch {
                    attempts: Some(0),
                    ..Default::default()
                },
                now,
            )?;
        }
        "reiniciar" if blocked => {
            h.set_epic_node_state(run.id, n, EpicNodeState::Pronta, None, now)?;
            h.update_epic_node(
                run.id,
                n,
                &EpicNodePatch {
                    session_id: Some(None),
                    attempts: Some(0),
                    ..Default::default()
                },
                now,
            )?;
            return Ok(node.session_id);
        }
        "manter" => {
            if let Some(p) = h.find_project(&run.owner, &run.repo)? {
                if let Some(i) = h.list_issues(p.id)?.into_iter().find(|i| i.gh_number == n) {
                    let prio = h
                        .find_work_item(&run.owner, &run.repo, n)?
                        .and_then(|w| w.priority);
                    let fp = ct_hub_store::issue_fingerprint(
                        &i.title,
                        i.updated_at,
                        prio.map(|p| p.as_str()),
                        &i.labels,
                    );
                    h.set_epic_plan_fingerprint(run.id, n, &fp)?;
                }
            }
        }
        // Com o bloqueante, a sub-issue espera por ele (aresta manual na
        // versão em vigor) e a sessão sai: volta nova quando ficar pronta.
        // Sem, fica bloqueada até outra decisão.
        "aguardar" => {
            if let Some(b) = blocker {
                h.add_epic_plan_blocker(run.id, n, b, "aguardar: decisão do desenvolvedor", now)?;
                if blocked {
                    h.set_epic_node_state(run.id, n, EpicNodeState::Pendente, None, now)?;
                    h.update_epic_node(
                        run.id,
                        n,
                        &EpicNodePatch {
                            session_id: Some(None),
                            ..Default::default()
                        },
                        now,
                    )?;
                    return Ok(node.session_id);
                }
            }
        }
        _ => {}
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ct_hub_store::{EpicPlanDraft, EpicPlanNodeInput, NewHubProject, SharedHub};
    use ct_ledger::{GithubSnapshot, IssueRow};

    fn state(branch: bool) -> HubState {
        let st = HubState::new(
            SharedHub::new(Hub::open_in_memory().unwrap()),
            "ui",
            "hub",
            "hub-id",
        );
        {
            let h = st.hub.lock();
            let pid = h
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
            let issue = |n: i64, parent: Option<i64>| IssueRow {
                gh_number: n,
                title: format!("ACME-{n}"),
                state: "open".into(),
                url: String::new(),
                updated_at: 1,
                parent_number: parent,
                ..Default::default()
            };
            h.apply_github(
                &GithubSnapshot {
                    project_id: pid,
                    issues: vec![issue(10, None), issue(11, Some(10))],
                    ..Default::default()
                },
                "env-a",
            )
            .unwrap();
            let branch = if branch { "'wip/10-x'" } else { "NULL" };
            h.conn()
                .execute_batch(&format!(
                    "INSERT INTO env (env_id, name, created_at, last_seen_at) VALUES ('env-a', 'a', 1, 1);
                     INSERT INTO work_item (project_id, gh_number, kind, status, branch, created_at)
                       VALUES ({pid}, 10, 'epic', 'em_execucao', {branch}, 1);"
                ))
                .unwrap();
        }
        st
    }

    async fn rpc(st: &HubState, m: &str, p: Value) -> RpcResult {
        call(st, m, p, false).await
    }

    #[tokio::test]
    async fn dispara_aprova_pausa_retoma_e_cancela_com_eventos() {
        let st = state(true);
        let start = json!({ "owner": "acme", "repo": "acme-web", "epic": 10, "env_id": "env-a" });
        // Ambiente desconhecido e épico sem sub-issues: recusados.
        let mut bad = start.clone();
        bad["env_id"] = json!("env-x");
        assert!(rpc(&st, "hub.epic_run.start", bad).await.is_err());
        let mut bad = start.clone();
        bad["epic"] = json!(11);
        assert!(rpc(&st, "hub.epic_run.start", bad).await.is_err());
        let run = rpc(&st, "hub.epic_run.start", start.clone()).await.unwrap();
        let id = run["id"].as_i64().unwrap();
        assert_eq!(run["status"], "planejando");
        // Uma ativa por épico.
        assert!(rpc(&st, "hub.epic_run.start", start).await.is_err());
        st.hub
            .lock()
            .save_epic_plan_draft(
                id,
                &EpicPlanDraft {
                    nodes: vec![EpicPlanNodeInput {
                        number: 11,
                        model: Some("haiku".into()),
                        ..Default::default()
                    }],
                    ..Default::default()
                },
                1,
            )
            .unwrap();
        let r = rpc(
            &st,
            "hub.epic_run.approve",
            json!({ "run_id": id, "version": 1, "max_parallel": 3, "code_review": true }),
        )
        .await
        .unwrap();
        assert_eq!(
            (r["status"].as_str(), r["max_parallel"].as_i64()),
            (Some("executando"), Some(3))
        );
        let v = rpc(&st, "hub.epic_run.status", json!({ "run_id": id }))
            .await
            .unwrap();
        assert_eq!(
            v["plan"]["nodes"][0]["fingerprint"].as_str().map(str::len),
            Some(16)
        );
        assert_eq!(v["titles"]["11"], "ACME-11");
        let r = rpc(&st, "hub.epic_run.pause", json!({ "run_id": id }))
            .await
            .unwrap();
        assert_eq!(r["status"], "pausado");
        assert!(rpc(&st, "hub.epic_run.pause", json!({ "run_id": id }))
            .await
            .is_err());
        let r = rpc(&st, "hub.epic_run.resume", json!({ "run_id": id }))
            .await
            .unwrap();
        assert_eq!(r["status"], "executando");
        assert!(rpc(&st, "hub.epic_run.resume", json!({ "run_id": id }))
            .await
            .is_err());
        let r = rpc(
            &st,
            "hub.epic_run.set_parallel",
            json!({ "run_id": id, "max_parallel": 1 }),
        )
        .await
        .unwrap();
        assert_eq!(r["max_parallel"], 1);
        let r = rpc(&st, "hub.epic_run.cancel", json!({ "run_id": id }))
            .await
            .unwrap();
        assert_eq!(r["status"], "cancelado");
        let kinds: Vec<String> = st
            .hub
            .lock()
            .epic_run_events(id, 0, 50)
            .unwrap()
            .into_iter()
            .map(|e| e.kind)
            .collect();
        assert_eq!(
            kinds,
            vec![
                "created",
                "plan_approved",
                "paused",
                "unpaused",
                "parallel_set",
                "cancelled"
            ]
        );
        assert!(rpc(&st, "hub.epic_run.list", Value::Null)
            .await
            .unwrap()
            .as_array()
            .unwrap()
            .is_empty());
        // Com `ended_since`, a cancelada aparece no histórico (#341).
        let hist = rpc(&st, "hub.epic_run.list", json!({ "ended_since": 0 }))
            .await
            .unwrap();
        assert_eq!(hist.as_array().unwrap().len(), 1, "{hist}");
        assert_eq!(hist[0]["status"], "cancelado");
        let futuro = rpc(&st, "hub.epic_run.list", json!({ "ended_since": i64::MAX }))
            .await
            .unwrap();
        assert!(futuro.as_array().unwrap().is_empty());
    }

    /// #341: o `/code-review` da integração liga e desliga depois da
    /// aprovação, até a sessão de integração começar.
    #[tokio::test]
    async fn code_review_muda_ate_a_integracao_comecar() {
        let st = state(true);
        let run = rpc(
            &st,
            "hub.epic_run.start",
            json!({ "owner": "acme", "repo": "acme-web", "epic": 10, "env_id": "env-a" }),
        )
        .await
        .unwrap();
        let id = run["id"].as_i64().unwrap();
        assert_eq!(run["code_review"], false);
        let r = rpc(
            &st,
            "hub.epic_run.set_code_review",
            json!({ "run_id": id, "code_review": true }),
        )
        .await
        .unwrap();
        assert_eq!(r["code_review"], true);
        let ev = st.hub.lock().epic_run_events(id, 0, 50).unwrap();
        assert!(ev
            .iter()
            .any(|e| e.kind == "code_review_set" && e.message.contains("ligado")));
        // Sem o valor: recusa.
        assert!(
            rpc(&st, "hub.epic_run.set_code_review", json!({ "run_id": id }))
                .await
                .is_err()
        );
        // Com a integração já começada: recusa, com o motivo.
        st.hub
            .lock()
            .conn()
            .execute(
                "UPDATE epic_run SET integration_session = 's-int' WHERE id = ?1",
                [id],
            )
            .unwrap();
        let e = rpc(
            &st,
            "hub.epic_run.set_code_review",
            json!({ "run_id": id, "code_review": false }),
        )
        .await
        .unwrap_err();
        assert!(e.1.contains("integração já começou"), "{e:?}");
    }

    /// #341: a visão traz o PR do épico (`wip` → base), o aberto antes do
    /// fechado.
    #[tokio::test]
    async fn visao_traz_o_pr_do_epico() {
        let st = state(true);
        let run = rpc(
            &st,
            "hub.epic_run.start",
            json!({ "owner": "acme", "repo": "acme-web", "epic": 10, "env_id": "env-a" }),
        )
        .await
        .unwrap();
        let id = run["id"].as_i64().unwrap();
        let v = rpc(&st, "hub.epic_run.status", json!({ "run_id": id }))
            .await
            .unwrap();
        assert!(v["epic_pr"].is_null(), "{v}");
        {
            let h = st.hub.lock();
            let pid = h.find_project("acme", "acme-web").unwrap().unwrap().id;
            let wi = h
                .find_work_item("acme", "acme-web", 10)
                .unwrap()
                .unwrap()
                .id;
            h.conn()
                .execute_batch(&format!(
                    "INSERT INTO pr_cache (project_id, number, work_item_id, head, base, state, url,
                                           updated_at, synced_at)
                     VALUES ({pid}, 90, {wi}, 'wip/10-x', 'develop', 'closed', 'u90', 1, 1),
                            ({pid}, 91, {wi}, 'wip/10-x', 'develop', 'open', 'u91', 2, 2),
                            ({pid}, 92, {wi}, 'feat/10-y', 'develop', 'open', 'u92', 3, 3);"
                ))
                .unwrap();
        }
        let v = rpc(&st, "hub.epic_run.status", json!({ "run_id": id }))
            .await
            .unwrap();
        assert_eq!(v["epic_pr"]["number"], 91, "{v}");
        assert_eq!(v["epic_pr"]["url"], "u91");
    }

    /// #341: o épico sem `wip` dispara; o motor a cria antes do plano.
    #[tokio::test]
    async fn sem_wip_dispara() {
        let st = state(false);
        let run = rpc(
            &st,
            "hub.epic_run.start",
            json!({ "owner": "acme", "repo": "acme-web", "epic": 10, "env_id": "env-a" }),
        )
        .await
        .unwrap();
        assert_eq!(run["status"], "planejando");
    }

    #[tokio::test]
    async fn decisoes_assumir_mais_tentativas_e_manter() {
        let st = state(true);
        let run = rpc(
            &st,
            "hub.epic_run.start",
            json!({ "owner": "acme", "repo": "acme-web", "epic": 10, "env_id": "env-a" }),
        )
        .await
        .unwrap();
        let id = run["id"].as_i64().unwrap();
        {
            let h = st.hub.lock();
            h.save_epic_plan_draft(
                id,
                &EpicPlanDraft {
                    nodes: vec![EpicPlanNodeInput {
                        number: 11,
                        model: Some("haiku".into()),
                        ..Default::default()
                    }],
                    ..Default::default()
                },
                1,
            )
            .unwrap();
        }
        rpc(
            &st,
            "hub.epic_run.approve",
            json!({ "run_id": id, "version": 1 }),
        )
        .await
        .unwrap();
        let ask = |origin| {
            let h = st.hub.lock();
            for to in [
                EpicNodeState::Pronta,
                EpicNodeState::EmExecucao,
                EpicNodeState::Bloqueada,
            ] {
                let _ = h.set_epic_node_state(id, 11, to, None, 2);
            }
            h.add_epic_decision(id, Some(11), origin, "x", 2)
                .unwrap()
                .id
        };
        let d = ask(ct_core::EpicDecisionOrigin::CiEsgotado);
        assert!(rpc(
            &st,
            "hub.epic_run.decide",
            json!({ "decision_id": d, "answer": "talvez" })
        )
        .await
        .is_err());
        rpc(
            &st,
            "hub.epic_run.decide",
            json!({ "decision_id": d, "answer": "mais_tentativas" }),
        )
        .await
        .unwrap();
        assert_eq!(
            st.hub.lock().epic_node(id, 11).unwrap().unwrap().state,
            EpicNodeState::EmExecucao
        );
        let d = ask(ct_core::EpicDecisionOrigin::SemProgresso);
        rpc(
            &st,
            "hub.epic_run.decide",
            json!({ "decision_id": d, "answer": "assumir" }),
        )
        .await
        .unwrap();
        assert_eq!(
            st.hub.lock().epic_node(id, 11).unwrap().unwrap().state,
            EpicNodeState::Assumida
        );
        // Pedido de merge já enviado (#341): a devolução o esquece, para o
        // motor pedir de novo; as tentativas ficam.
        let wi = {
            let h = st.hub.lock();
            h.set_work_item_branch_if_missing("acme", "acme-web", 11, "feat/11-x")
                .unwrap();
            let wi = h
                .find_work_item("acme", "acme-web", 11)
                .unwrap()
                .unwrap()
                .id;
            h.save_autopilot_close(
                &ct_hub_store::CloseState {
                    work_item_id: wi,
                    attempts: 2,
                    merge_head: Some("abc".into()),
                    last_sent_at: Some(5),
                    ..Default::default()
                },
                5,
            )
            .unwrap();
            wi
        };
        let n = rpc(
            &st,
            "hub.epic_run.handback",
            json!({ "run_id": id, "number": 11 }),
        )
        .await
        .unwrap();
        assert_eq!(n["state"], "em_execucao");
        let c = st
            .hub
            .lock()
            .autopilot_close_states()
            .unwrap()
            .into_iter()
            .find(|c| c.work_item_id == wi)
            .unwrap();
        assert_eq!((c.merge_head, c.attempts), (None, 2));
        // Replanejar leva a execução a replanejando.
        let d = st
            .hub
            .lock()
            .add_epic_decision(id, None, ct_core::EpicDecisionOrigin::IssueAlterada, "x", 3)
            .unwrap()
            .id;
        rpc(
            &st,
            "hub.epic_run.decide",
            json!({ "decision_id": d, "answer": "replanejar" }),
        )
        .await
        .unwrap();
        assert_eq!(
            st.hub.lock().epic_run(id).unwrap().unwrap().status,
            EpicRunStatus::Replanejando
        );
    }

    #[tokio::test]
    async fn aguardar_com_bloqueante_espera_e_replanejar_vai_ao_plano() {
        let st = state(true);
        let run = rpc(
            &st,
            "hub.epic_run.start",
            json!({ "owner": "acme", "repo": "acme-web", "epic": 10, "env_id": "env-a" }),
        )
        .await
        .unwrap();
        let id = run["id"].as_i64().unwrap();
        st.hub
            .lock()
            .save_epic_plan_draft(
                id,
                &EpicPlanDraft {
                    nodes: vec![EpicPlanNodeInput {
                        number: 11,
                        model: Some("haiku".into()),
                        ..Default::default()
                    }],
                    ..Default::default()
                },
                1,
            )
            .unwrap();
        rpc(
            &st,
            "hub.epic_run.approve",
            json!({ "run_id": id, "version": 1 }),
        )
        .await
        .unwrap();
        let d = {
            let h = st.hub.lock();
            for to in [
                EpicNodeState::Pronta,
                EpicNodeState::EmExecucao,
                EpicNodeState::Bloqueada,
            ] {
                h.set_epic_node_state(id, 11, to, None, 2).unwrap();
            }
            h.add_epic_decision(
                id,
                Some(11),
                ct_core::EpicDecisionOrigin::PlanoErrado,
                "falta a #20",
                2,
            )
            .unwrap()
            .id
        };
        // Bloqueante igual à sub-issue: recusado sem responder.
        assert!(rpc(
            &st,
            "hub.epic_run.decide",
            json!({ "decision_id": d, "answer": "aguardar", "blocker": 11 })
        )
        .await
        .is_err());
        rpc(
            &st,
            "hub.epic_run.decide",
            json!({ "decision_id": d, "answer": "aguardar", "blocker": 20 }),
        )
        .await
        .unwrap();
        {
            let h = st.hub.lock();
            let n = h.epic_node(id, 11).unwrap().unwrap();
            assert_eq!((n.state, n.session_id), (EpicNodeState::Pendente, None));
            let plan = h.epic_plan(id, 1).unwrap().unwrap();
            assert!(plan.edges.iter().any(|e| (e.number, e.blocker) == (11, 20)));
        }
        // Replanejar: a execução vai a replanejando, com o motivo nos eventos.
        let d = st
            .hub
            .lock()
            .add_epic_decision(
                id,
                Some(11),
                ct_core::EpicDecisionOrigin::PlanoErrado,
                "escopo errado",
                3,
            )
            .unwrap()
            .id;
        rpc(
            &st,
            "hub.epic_run.decide",
            json!({ "decision_id": d, "answer": "replanejar" }),
        )
        .await
        .unwrap();
        let h = st.hub.lock();
        assert_eq!(
            h.epic_run(id).unwrap().unwrap().status,
            EpicRunStatus::Replanejando
        );
        let ev = h.epic_run_events_tail(id, 1).unwrap();
        assert!(
            ev[0].message.contains("Motivo: escopo errado"),
            "{:?}",
            ev[0]
        );
    }

    #[test]
    fn propostas_viram_sub_issues_depois_da_aprovacao() {
        let st = state(true);
        let id = {
            let h = st.hub.lock();
            let pid = h.find_project("acme", "acme-web").unwrap().unwrap().id;
            let r = h
                .create_epic_run(
                    &NewEpicRun {
                        project_id: pid,
                        epic: 10,
                        env_id: "env-a".into(),
                        started_by: "fulano".into(),
                        max_parallel: None,
                        plan_model: None,
                        plan_effort: None,
                    },
                    1,
                )
                .unwrap();
            let prop = |t: &str| ct_hub_store::EpicProposal {
                kind: "nova".into(),
                title: t.into(),
                body: String::new(),
                from: None,
            };
            h.save_epic_plan_draft(
                r.id,
                &EpicPlanDraft {
                    nodes: vec![EpicPlanNodeInput {
                        number: 11,
                        model: Some("haiku".into()),
                        ..Default::default()
                    }],
                    proposals: vec![prop("Migração"), prop("Docs")],
                    ..Default::default()
                },
                1,
            )
            .unwrap();
            r.id
        };
        // Antes de aprovar, nada é criado: só a aprovação chama.
        let calls = std::sync::Mutex::new(Vec::new());
        create_proposals(&st.hub, id, 1, |_, p| {
            calls.lock().unwrap().push(p.title.clone());
            if p.title == "Docs" {
                Err("403".into())
            } else {
                Ok(Some(40))
            }
        });
        assert_eq!(*calls.lock().unwrap(), vec!["Migração", "Docs"]);
        let kinds: Vec<String> = st
            .hub
            .lock()
            .epic_run_events(id, 0, 50)
            .unwrap()
            .into_iter()
            .map(|e| e.kind)
            .collect();
        assert_eq!(kinds, vec!["proposal_created", "proposal_failed"]);
    }
}
