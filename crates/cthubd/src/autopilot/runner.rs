//! O laço do agendador do piloto automático (#183) no processo do hub:
//! monta a fotografia ([`snapshot`]), decide ([`super::plan::decide`]) e
//! executa as ações pelo canal reverso ([`EnvPort`]), gravando os eventos.
//! Roda a cada minuto e a cada leitura nova do plano ([`Scheduler::kick`]),
//! sem depender da UI aberta.

use super::plan::{self, Action, Outcome, ProjectPlan, ResumeVia, Snapshot};
use crate::HubState;
use ct_core::{AutonomyState, SessionState};
use ct_http::rpc::{invalid, RpcResult};
use ct_ledger::{
    AutonomyWip, AutopilotEvent, AutopilotSession, AutopilotSettings, Hub, ParkedSession,
    RankingFilter, SharedHub, RANKING_MAX,
};
use serde::Serialize;
use serde_json::{json, Value};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;
use tokio::sync::Notify;

/// Intervalo do ciclo.
pub const TICK: Duration = Duration::from_secs(60);
/// Espera depois de subir o hub antes do primeiro ciclo: os ambientes
/// reabrem o canal.
const FIRST_TICK: Duration = Duration::from_secs(20);
/// Um início que o ambiente ainda não sincronizou conta como sessão viva
/// por este tempo.
const PENDING_START_MS: i64 = 10 * 60_000;

/// Texto do `session.resume` da sessão estacionada que encerrou.
/// O mesmo texto do `session.continue` do ambiente
/// (`ct_sessions::remote::CONTINUE_TEXT`).
const RESUME_PROMPT: &str =
    "O limite do plano renovou. Continue a issue de onde parou, sem perguntar.";

/// Estado do laço no `HubState`.
#[derive(Debug, Default)]
pub struct Scheduler {
    kick: Notify,
    last_tick_at: AtomicI64,
}

impl Scheduler {
    /// Pede um ciclo já (leitura nova do plano, pausa retomada).
    pub fn kick(&self) {
        self.kick.notify_one();
    }

    /// Último ciclo (ms); `None` = ainda não rodou.
    pub fn last_tick_at(&self) -> Option<i64> {
        Some(self.last_tick_at.load(Ordering::Relaxed)).filter(|t| *t > 0)
    }
}

/// O que o agendador usa do canal reverso: dá para trocar nos testes.
pub trait EnvPort: Send + Sync {
    fn online(&self, env_id: &str) -> bool;
    fn call(
        &self,
        env_id: String,
        method: &'static str,
        params: Value,
    ) -> Pin<Box<dyn Future<Output = RpcResult> + Send + '_>>;
}

/// O canal de verdade, pelo `call_env` do `/ws` (limite de trabalho em
/// andamento e modelo da issue aplicados lá).
struct HubPort(HubState);

impl EnvPort for HubPort {
    fn online(&self, env_id: &str) -> bool {
        self.0.channels.is_online(env_id)
    }

    fn call(
        &self,
        env_id: String,
        method: &'static str,
        params: Value,
    ) -> Pin<Box<dyn Future<Output = RpcResult> + Send + '_>> {
        Box::pin(crate::ws::call_env(&self.0, env_id, method, params))
    }
}

/// Sobe o laço em segundo plano.
pub fn spawn(st: HubState) {
    tokio::spawn(async move {
        let port = HubPort(st.clone());
        tokio::time::sleep(FIRST_TICK).await;
        loop {
            let now = ct_core::now_ms();
            match tick(&st.hub, &port, now).await {
                Ok(n) if n > 0 => st.autopilot_changed(),
                Ok(_) => {}
                Err(e) => tracing::warn!("piloto automático: ciclo falhou: {e}"),
            }
            // Resumo da manhã (#185): no horário da política, uma vez por
            // dia útil; vira o card da Visão geral e a Web Push.
            let hub = st.hub.clone();
            match tokio::task::spawn_blocking(move || super::digest::run(&hub.lock(), now)).await {
                Ok(Ok(Some(d))) => {
                    tracing::info!(
                        "piloto automático: resumo da manhã gerado ({} projetos)",
                        d.projects.len()
                    );
                    st.autopilot_changed();
                    st.push_kick();
                }
                Ok(Ok(None)) => {}
                Ok(Err(e)) => tracing::warn!("piloto automático: resumo da manhã falhou: {e}"),
                Err(e) => tracing::warn!("piloto automático: resumo da manhã falhou: {e}"),
            }
            st.autopilot.last_tick_at.store(now, Ordering::Relaxed);
            tokio::select! {
                _ = tokio::time::sleep(TICK) => {}
                _ = st.autopilot.kick.notified() => {
                    // Junta avisos seguidos (vários pushes com plano).
                    tokio::time::sleep(Duration::from_secs(2)).await;
                }
            }
        }
    });
}

/// `reason` do evento `started` da sessão sonda (conta sem leitura do
/// plano, veja [`plan`]).
pub const PROBE_REASON: &str = "sem_leitura_sonda";

fn ledger_err(e: ct_ledger::LedgerError) -> String {
    e.to_string()
}

/// Fotografia do hub em `now`.
pub fn snapshot(hub: &Hub, port: &dyn EnvPort, now: i64) -> ct_ledger::Result<Snapshot> {
    let mut s = Snapshot {
        now,
        settings: hub.autopilot_settings()?,
        ..Default::default()
    };
    let accounts: std::collections::BTreeMap<String, String> = hub
        .list_env_plans()?
        .into_iter()
        .filter_map(|(env, _, r)| r.account.map(|a| (env, a.key)))
        .collect();
    for e in hub.list_envs()? {
        s.envs.insert(
            e.env_id.clone(),
            plan::EnvSnap {
                online: port.online(&e.env_id),
                autonomous: e.autonomous,
                account: accounts.get(&e.env_id).cloned(),
                name: e.name,
            },
        );
    }
    let keys: std::collections::BTreeSet<&String> = accounts.values().collect();
    for key in keys {
        let view = super::account_plan(hub, key, now)?;
        let decision = hub
            .autopilot_policy(key)?
            .map(|p| ct_core::plan::budget::evaluate(&p, &view, now));
        s.accounts.insert(
            key.clone(),
            plan::AccountSnap {
                decision,
                exhausted: plan::exhausted(&view),
            },
        );
    }
    s.sessions = hub.autopilot_sessions()?;
    for (i, (env_id, owner, repo, n)) in hub
        .autopilot_pending_starts(now - PENDING_START_MS)?
        .into_iter()
        .enumerate()
    {
        s.sessions.push(AutopilotSession {
            session_id: format!("pendente-{i}"),
            env_id,
            state: SessionState::Starting,
            owner,
            repo,
            gh_number: n,
            started_at: now,
        });
    }
    s.parked = hub.list_parked()?;
    for p in hub.list_projects()? {
        if !p.autonomy.is_on() {
            continue;
        }
        let Some(env_id) = p.autonomy.env_id.clone() else {
            continue;
        };
        let conflict = p
            .autonomy_report
            .as_ref()
            .filter(|r| r.state == AutonomyState::Conflict)
            .map(|r| {
                r.message
                    .clone()
                    .unwrap_or_else(|| format!("{} em conflito com a base", r.branch))
            });
        let autonomy_error = p
            .autonomy_report
            .as_ref()
            .filter(|r| r.state == AutonomyState::Error)
            .map(|r| {
                r.message
                    .clone()
                    .unwrap_or_else(|| format!("erro em {}", r.branch))
            });
        let filter = RankingFilter {
            project: Some((p.gh_owner.clone(), p.gh_repo.clone())),
            priority: None,
        };
        let mut items = Vec::new();
        for r in hub.ranking(&filter, RANKING_MAX)?.items {
            let model = hub
                .issue_agent(&p.gh_owner, &p.gh_repo, r.number)?
                .and_then(|a| a.model);
            items.push(plan::ItemSnap {
                number: r.number,
                title: r.title,
                kind: r.kind,
                status: r.status,
                priority: r.priority,
                model,
                approved: hub.is_approved(&p.gh_owner, &p.gh_repo, r.number)?,
            });
        }
        s.projects.push(plan::ProjectSnap {
            wip: hub.project_wip(p.id)?,
            approval_label: p.approval.active_label().map(String::from),
            owner: p.gh_owner,
            repo: p.gh_repo,
            env_id,
            conflict,
            autonomy_error,
            items,
        });
    }
    Ok(s)
}

fn event(kind: &str, scope: String, message: String, at: i64) -> AutopilotEvent {
    AutopilotEvent {
        at,
        kind: kind.into(),
        scope,
        message,
        ..Default::default()
    }
}

fn parked_event(kind: &str, p: &ParkedSession, message: String, at: i64) -> AutopilotEvent {
    AutopilotEvent {
        account_key: Some(p.account_key.clone()),
        env_id: Some(p.env_id.clone()),
        owner: Some(p.owner.clone()),
        repo: Some(p.repo.clone()),
        gh_number: Some(p.gh_number),
        session_id: Some(p.session_id.clone()),
        reason: Some(p.window.clone()),
        ..event(
            kind,
            format!("item:{}/{}#{}", p.owner, p.repo, p.gh_number),
            message,
            at,
        )
    }
}

/// Código estável da falha de início, pelo erro do ambiente ou do hub.
fn start_error_reason(code: i64) -> &'static str {
    match code {
        c if c == ct_http::sync::AUTONOMY_CONFLICT => "conflict",
        c if c == crate::wip::WIP_LIMIT_REACHED => "wip_limit",
        c if c == ct_http::sync::APPROVAL_REQUIRED => "approval_required",
        c if c == ct_http::sync::CHANNEL_ENV_OFFLINE => "env_offline",
        c if c == ct_http::sync::CHANNEL_TIMEOUT => "timeout",
        -32602 => "invalid_params",
        _ => "env_error",
    }
}

/// Executa um ciclo; devolve quantos eventos gravou.
pub async fn tick(hub: &SharedHub, port: &dyn EnvPort, now: i64) -> Result<usize, String> {
    let outcome = {
        let h = hub.lock();
        plan::decide(&snapshot(&h, port, now).map_err(ledger_err)?)
    };
    let mut events: Vec<AutopilotEvent> = outcome
        .refusals
        .iter()
        .map(|r| AutopilotEvent {
            reason: Some(r.reason.clone()),
            account_key: r.account.clone(),
            env_id: r.env_id.clone(),
            owner: r.owner.clone(),
            repo: r.repo.clone(),
            ..event("refused", r.scope.clone(), r.message.clone(), now)
        })
        .collect();
    for a in outcome.actions {
        match a {
            Action::Park(p) => {
                hub.lock().park_session(&p).map_err(ledger_err)?;
                let m = format!(
                    "{}/{}#{}: limite do plano atingido ({}); a sessão fica estacionada e é \
                     retomada depois do reset",
                    p.owner,
                    p.repo,
                    p.gh_number,
                    window_label(&p.window)
                );
                events.push(parked_event("parked", &p, m, now));
            }
            Action::Unpark { parked, message } => {
                hub.lock()
                    .unpark_session(&parked.session_id)
                    .map_err(ledger_err)?;
                events.push(parked_event("unparked", &parked, message, now));
            }
            Action::Resume { parked, via } => {
                let (method, params) = match via {
                    ResumeVia::Continue => (
                        "session.continue",
                        json!({ "session_id": parked.session_id }),
                    ),
                    ResumeVia::Resume => (
                        "session.resume",
                        json!({ "session_id": parked.session_id, "prompt": RESUME_PROMPT }),
                    ),
                };
                let slug = format!("{}/{}#{}", parked.owner, parked.repo, parked.gh_number);
                match port.call(parked.env_id.clone(), method, params).await {
                    Ok(_) => {
                        hub.lock()
                            .unpark_session(&parked.session_id)
                            .map_err(ledger_err)?;
                        events.push(parked_event(
                            "resumed",
                            &parked,
                            format!("{slug}: sessão retomada depois do reset do plano"),
                            now,
                        ));
                    }
                    // Canal caiu entre a fotografia e a chamada: tenta no
                    // próximo ciclo.
                    Err((c, _))
                        if c == ct_http::sync::CHANNEL_ENV_OFFLINE
                            || c == ct_http::sync::CHANNEL_TIMEOUT => {}
                    Err((_, m)) => {
                        hub.lock()
                            .unpark_session(&parked.session_id)
                            .map_err(ledger_err)?;
                        events.push(parked_event(
                            "resume_failed",
                            &parked,
                            format!("{slug}: a retomada falhou ({method}): {m}"),
                            now,
                        ));
                    }
                }
            }
            Action::Start(i) => {
                let slug = format!("{}/{}#{}", i.owner, i.repo, i.number);
                // Sem `prompt` (o ambiente monta o autônomo, #191) e nunca
                // com `override_approval` (#124); o modelo e o esforço da
                // issue entram no `call_env` (#182). O `title` sai do espelho
                // do hub: logo depois da atribuição, o do ambiente ainda não
                // tem a issue, e sem ele o `session.start` é recusado.
                let params = json!({
                    "owner": i.owner,
                    "repo": i.repo,
                    "gh_number": i.number,
                    "title": i.title,
                    "autonomous": true,
                });
                let base = AutopilotEvent {
                    account_key: Some(i.account.clone()),
                    env_id: Some(i.env_id.clone()),
                    owner: Some(i.owner.clone()),
                    repo: Some(i.repo.clone()),
                    gh_number: Some(i.number),
                    ..Default::default()
                };
                match port.call(i.env_id.clone(), "session.start", params).await {
                    Ok(v) => {
                        let sid = v.get("id").and_then(Value::as_str).map(String::from);
                        // A sonda sem leitura do plano fica distinguível no
                        // histórico: `reason` = `sem_leitura_sonda`.
                        let (reason, extra) = if i.probe {
                            (
                                Some(PROBE_REASON.to_string()),
                                " (sonda: a conta não tem leitura do plano; esta sessão traz a \
                                 primeira)",
                            )
                        } else {
                            (None, "")
                        };
                        events.push(AutopilotEvent {
                            session_id: sid,
                            reason,
                            ..event(
                                "started",
                                format!("project:{}/{}", i.owner, i.repo),
                                format!("{slug}: sessão autônoma iniciada com {}{extra}", i.model),
                                now,
                            )
                            .merge(&base)
                        });
                    }
                    Err((code, m)) => {
                        events.push(AutopilotEvent {
                            reason: Some(start_error_reason(code).into()),
                            ..event(
                                "start_failed",
                                format!("item:{slug}"),
                                format!("{slug}: o início foi recusado: {m}"),
                                now,
                            )
                            .merge(&base)
                        });
                    }
                }
            }
        }
    }
    let h = hub.lock();
    let mut recorded = 0;
    for e in &events {
        if h.record_autopilot_event(e).map_err(ledger_err)?.is_some() {
            recorded += 1;
        }
    }
    Ok(recorded)
}

trait Merge {
    fn merge(self, base: &AutopilotEvent) -> AutopilotEvent;
}

impl Merge for AutopilotEvent {
    /// Completa os campos de contexto vazios com os de `base`.
    fn merge(mut self, base: &AutopilotEvent) -> AutopilotEvent {
        self.account_key = self.account_key.or_else(|| base.account_key.clone());
        self.env_id = self.env_id.or_else(|| base.env_id.clone());
        self.owner = self.owner.or_else(|| base.owner.clone());
        self.repo = self.repo.or_else(|| base.repo.clone());
        self.gh_number = self.gh_number.or(base.gh_number);
        self
    }
}

fn window_label(w: &str) -> &'static str {
    match w {
        "seven_day" => "semanal",
        _ => "janela de 5h",
    }
}

/// Resposta de `hub.autopilot.status`: o estado do agendador e o que ele
/// faria agora, para o painel (#185).
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct AutopilotStatus {
    pub settings: AutopilotSettings,
    /// Último ciclo (ms); `None` = ainda não rodou desde que o hub subiu.
    pub last_tick_at: Option<i64>,
    pub now: i64,
    /// Contas com política e a decisão do orçamento agora (sem a chave na
    /// web).
    pub accounts: Vec<AutopilotAccountStatus>,
    /// Projetos com o modo autônomo ligado, na ordem do agendador.
    pub projects: Vec<ProjectPlan>,
    /// Sessões de implementação vivas nos ambientes autônomos.
    pub sessions: Vec<AutopilotSession>,
    pub parked: Vec<ParkedSession>,
    /// Eventos mais recentes, do mais novo para o mais velho.
    pub events: Vec<AutopilotEvent>,
    /// A branch de integração de cada projeto autônomo, com as issues à
    /// frente da base (#185).
    pub wip: Vec<AutonomyWip>,
}

#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct AutopilotAccountStatus {
    /// Chave da conta; na web, `conta-N`.
    pub account: String,
    pub decision: ct_core::plan::budget::BudgetDecision,
    /// Janela esgotada agora (limite real): `five_hour` ou `seven_day`.
    pub exhausted_window: Option<String>,
    pub exhausted_until: Option<i64>,
}

/// Monta o status (sem efeito nenhum). Na web, as chaves de conta viram
/// `conta-N` e saem dos eventos e das sessões estacionadas.
pub fn status(
    hub: &Hub,
    port: &dyn EnvPort,
    last_tick_at: Option<i64>,
    now: i64,
    web: bool,
) -> ct_ledger::Result<AutopilotStatus> {
    let s = snapshot(hub, port, now)?;
    let Outcome { mut projects, .. } = plan::decide(&s);
    let autonomous: std::collections::BTreeSet<&str> = s
        .envs
        .iter()
        .filter(|(_, e)| e.autonomous)
        .map(|(id, _)| id.as_str())
        .collect();
    let sessions = s
        .sessions
        .iter()
        .filter(|x| {
            autonomous.contains(x.env_id.as_str()) && !x.session_id.starts_with("pendente-")
        })
        .cloned()
        .collect();
    let mut accounts: Vec<AutopilotAccountStatus> = s
        .accounts
        .iter()
        .filter_map(|(k, a)| {
            Some(AutopilotAccountStatus {
                account: k.clone(),
                decision: a.decision.clone()?,
                exhausted_window: a.exhausted.as_ref().map(|e| e.window.clone()),
                exhausted_until: a.exhausted.as_ref().map(|e| e.resets_at),
            })
        })
        .collect();
    let mut parked = s.parked.clone();
    let mut events = hub.list_autopilot_events(50, None)?;
    if web {
        let alias: std::collections::BTreeMap<String, String> = s
            .accounts
            .keys()
            .enumerate()
            .map(|(i, k)| (k.clone(), format!("conta-{}", i + 1)))
            .collect();
        let anon = |k: &str| alias.get(k).cloned().unwrap_or_else(|| "conta".into());
        for a in &mut accounts {
            a.account = anon(&a.account);
        }
        for p in &mut projects {
            p.account = p.account.as_deref().map(anon);
        }
        for p in &mut parked {
            p.account_key = anon(&p.account_key);
        }
        for e in &mut events {
            if let Some(k) = e.account_key.take() {
                let a = anon(&k);
                e.scope = e
                    .scope
                    .replace(&format!("account:{k}"), &format!("account:{a}"));
                e.account_key = Some(a);
            }
        }
    }
    Ok(AutopilotStatus {
        settings: s.settings,
        last_tick_at,
        now,
        accounts,
        projects,
        sessions,
        parked,
        events,
        wip: hub.autonomy_wip(None)?,
    })
}

/// `hub.autopilot.status|events|pause|resume|set` (#183) e
/// `hub.autopilot.digest|wip` (#185). `web` = vindo do
/// `/web/ws` (sem as chaves de conta).
pub(crate) async fn call(st: &HubState, method: &str, p: Value, web: bool) -> RpcResult {
    let e = |e: ct_ledger::LedgerError| (-32000, e.to_string());
    let now = ct_core::now_ms();
    let (st2, method2) = (st.clone(), method.to_string());
    let (v, changed) =
        tokio::task::spawn_blocking(move || -> Result<(Value, bool), (i64, String)> {
            let hub = st2.hub.lock();
            match method2.as_str() {
                "hub.autopilot.status" => {
                    let port = HubPort(st2.clone());
                    let v =
                        status(&hub, &port, st2.autopilot.last_tick_at(), now, web).map_err(e)?;
                    Ok((json!(v), false))
                }
                "hub.autopilot.events" => {
                    let limit = p.get("limit").and_then(Value::as_u64).unwrap_or(50) as u32;
                    let before = p.get("before").and_then(Value::as_i64);
                    let mut v = hub.list_autopilot_events(limit, before).map_err(e)?;
                    if web {
                        for ev in &mut v {
                            if ev.account_key.take().is_some() && ev.scope.starts_with("account:") {
                                ev.scope = "account".into();
                            }
                        }
                    }
                    Ok((json!(v), false))
                }
                "hub.autopilot.pause" | "hub.autopilot.resume" => {
                    let paused = method2 == "hub.autopilot.pause";
                    let (s, changed) = hub.set_autopilot_paused(paused, now).map_err(e)?;
                    if changed {
                        let (kind, m) = if paused {
                            (
                                "paused",
                                "piloto automático pausado: nenhuma sessão nova inicia",
                            )
                        } else {
                            ("unpaused", "piloto automático retomado")
                        };
                        hub.record_autopilot_event(&event(kind, "hub".into(), m.into(), now))
                            .map_err(e)?;
                    }
                    Ok((json!(s), changed))
                }
                // Resumo da manhã (#185): os mais recentes; na web, sem a
                // chave da conta.
                "hub.autopilot.digest" => {
                    let limit = p.get("limit").and_then(Value::as_u64).unwrap_or(1) as u32;
                    let mut v = hub.list_digests(limit).map_err(e)?;
                    if web {
                        super::digest::anonymize(&hub, &mut v).map_err(e)?;
                    }
                    Ok((json!(v), false))
                }
                // Branch de integração dos projetos autônomos (#185), para a
                // Visão geral do projeto; `owner` e `repo` filtram.
                "hub.autopilot.wip" => {
                    let owner = p.get("owner").and_then(Value::as_str);
                    let repo = p.get("repo").and_then(Value::as_str);
                    let only = owner.zip(repo);
                    Ok((json!(hub.autonomy_wip(only).map_err(e)?), false))
                }
                "hub.autopilot.set" => {
                    let n = p
                        .get("max_sessions_per_env")
                        .and_then(Value::as_u64)
                        .ok_or_else(|| invalid("parâmetro obrigatório: max_sessions_per_env"))?;
                    let s = hub
                        .set_autopilot_max_per_env(n.min(u32::MAX as u64) as u32, now)
                        .map_err(e)?;
                    Ok((json!(s), true))
                }
                _ => Err((-32601, format!("método desconhecido: {method2}"))),
            }
        })
        .await
        .map_err(|e| (-32603, e.to_string()))??;
    if changed {
        st.autopilot_changed();
        st.autopilot.kick();
    }
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ct_core::plan::budget::BudgetPolicy;
    use ct_core::plan::{EnvPlanReport, PlanAccount, PlanReading, PlanWindow};
    use ct_core::{ApprovalConfig, AutonomyConfig, Priority};
    use ct_ledger::{GithubSnapshot, HubProjectPatch, IssueAgent, IssueRow, NewHubProject};
    use std::sync::Mutex;

    /// Sábado, 2026-11-14 12:00 em GMT-3: fim de semana, faixa aberta.
    const SAB: i64 = 1_794_668_400_000;
    const H: i64 = 3_600_000;

    #[derive(Default)]
    struct FakePort {
        offline: bool,
        calls: Mutex<Vec<(String, String, Value)>>,
        /// Resposta de `session.start`; `None` = `{ "id": "s-<n>" }`.
        start_error: Option<(i64, String)>,
    }

    impl EnvPort for FakePort {
        fn online(&self, _: &str) -> bool {
            !self.offline
        }
        fn call(
            &self,
            env_id: String,
            method: &'static str,
            params: Value,
        ) -> Pin<Box<dyn Future<Output = RpcResult> + Send + '_>> {
            let n = params.get("gh_number").and_then(Value::as_i64).unwrap_or(0);
            self.calls
                .lock()
                .unwrap()
                .push((env_id, method.into(), params));
            let r = match (&self.start_error, method) {
                (Some(e), "session.start") => Err(e.clone()),
                (_, "session.start") => Ok(json!({ "id": format!("s-{n}") })),
                _ => Ok(json!({})),
            };
            Box::pin(async move { r })
        }
    }

    fn issue(n: i64, labels: &[&str]) -> IssueRow {
        IssueRow {
            gh_number: n,
            title: format!("ACME-{n}"),
            state: "open".into(),
            labels: labels.iter().map(|s| s.to_string()).collect(),
            url: String::new(),
            updated_at: 1,
            ..Default::default()
        }
    }

    fn reading(hub: &Hub, five: f64, now: i64) {
        hub.set_env_plan(
            "env-auto",
            &EnvPlanReport {
                account: Some(PlanAccount {
                    key: "conta-a".into(),
                    email: None,
                    org_name: None,
                    subscription_type: None,
                }),
                statusline: Default::default(),
                latest: Some(PlanReading {
                    read_at: now - 1000,
                    five_hour: Some(PlanWindow {
                        used_percentage: five,
                        resets_at: now + 2 * H,
                    }),
                    seven_day: Some(PlanWindow {
                        used_percentage: 10.0,
                        resets_at: now + 3 * 24 * H,
                    }),
                }),
            },
        )
        .unwrap();
    }

    /// Hub com `acme/acme-web` no modo autônomo em `env-auto`, a issue 123
    /// aprovada, P1 e com modelo, e a 124 sem rótulo.
    fn hub() -> SharedHub {
        let hub = Hub::open_in_memory().unwrap();
        hub.ingest("env-auto", "env-auto", &[]).unwrap();
        let p = hub
            .add_project(&NewHubProject {
                owner: "acme".into(),
                repo: "acme-web".into(),
                name: "acme-web".into(),
                base_branch: "develop".into(),
                protected_branches: vec!["develop".into(), "main".into()],
            })
            .unwrap();
        hub.set_env_autonomous("env-auto", true).unwrap();
        hub.add_assignment("acme", "acme-web", "env-auto", None)
            .unwrap();
        hub.update_project(
            "acme",
            "acme-web",
            &HubProjectPatch {
                autonomy: Some(AutonomyConfig {
                    enabled: true,
                    env_id: Some("env-auto".into()),
                    ..Default::default()
                }),
                approval: Some(ApprovalConfig {
                    label: "aprovada".into(),
                    required: true,
                }),
                ..Default::default()
            },
        )
        .unwrap();
        hub.apply_github(
            &GithubSnapshot {
                project_id: p.project.id,
                issues: vec![issue(123, &["aprovada"]), issue(124, &[])],
                prs: vec![],
                ..Default::default()
            },
            "hub-id",
        )
        .unwrap();
        for n in [123, 124] {
            hub.set_priority(
                "acme",
                "acme-web",
                n,
                Some(Priority::P1),
                None,
                None,
                "hub-id",
            )
            .unwrap();
            hub.set_model(
                "acme",
                "acme-web",
                n,
                &IssueAgent {
                    model: Some("opus".into()),
                    effort: Some("high".into()),
                },
                None,
                None,
                "hub-id",
            )
            .unwrap();
        }
        hub.set_autopilot_policy("conta-a", Some(&BudgetPolicy::default()))
            .unwrap();
        reading(&hub, 10.0, SAB);
        SharedHub::new(hub)
    }

    #[tokio::test]
    async fn inicia_sem_prompt_e_registra_o_evento() {
        let hub = hub();
        let port = FakePort::default();
        let n = tick(&hub, &port, SAB).await.unwrap();
        assert_eq!(n, 1);
        let calls = port.calls.lock().unwrap().clone();
        assert_eq!(calls.len(), 1);
        let (env, method, params) = &calls[0];
        assert_eq!(
            (env.as_str(), method.as_str()),
            ("env-auto", "session.start")
        );
        assert_eq!(
            params,
            &json!({ "owner": "acme", "repo": "acme-web", "gh_number": 123,
                     "title": "ACME-123", "autonomous": true })
        );
        assert!(params.get("prompt").is_none() && params.get("override_approval").is_none());
        let ev = hub.lock().list_autopilot_events(10, None).unwrap();
        assert_eq!(ev[0].kind, "started");
        assert_eq!(ev[0].session_id.as_deref(), Some("s-123"));
        assert_eq!(ev[0].gh_number, Some(123));

        // O ambiente ainda não sincronizou a sessão: o próximo ciclo não
        // inicia outra no mesmo ambiente.
        port.calls.lock().unwrap().clear();
        tick(&hub, &port, SAB + 60_000).await.unwrap();
        assert!(port.calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn conta_sem_leitura_inicia_uma_sonda_so() {
        let hub = hub();
        // O ambiente conhece a conta, mas nenhuma sessão gravou o plano.
        hub.lock()
            .set_env_plan(
                "env-auto",
                &EnvPlanReport {
                    account: Some(PlanAccount {
                        key: "conta-a".into(),
                        email: None,
                        org_name: None,
                        subscription_type: None,
                    }),
                    statusline: Default::default(),
                    latest: None,
                },
            )
            .unwrap();
        hub.lock().set_autopilot_max_per_env(2, SAB).unwrap();
        let port = FakePort::default();
        tick(&hub, &port, SAB).await.unwrap();
        assert_eq!(port.calls.lock().unwrap().len(), 1);
        let ev = hub.lock().list_autopilot_events(10, None).unwrap();
        assert_eq!(ev[0].kind, "started");
        assert_eq!(ev[0].reason.as_deref(), Some(PROBE_REASON));
        assert!(ev[0].message.contains("sonda"));
        // Com a sonda viva (ainda não sincronizada), o ambiente tem vaga,
        // mas a conta espera a leitura dela: nenhum outro início.
        port.calls.lock().unwrap().clear();
        tick(&hub, &port, SAB + 60_000).await.unwrap();
        assert!(port.calls.lock().unwrap().is_empty());
        let ev = hub.lock().list_autopilot_events(10, None).unwrap();
        assert_eq!(ev[0].kind, "refused");
        assert_eq!(ev[0].reason.as_deref(), Some("budget:no_reading"));
    }

    #[tokio::test]
    async fn erro_na_branch_de_integracao_recusa_uma_vez() {
        use ct_ledger::{OutboxEntry, ProjectChange, SyncChange};
        let hub = hub();
        let report = OutboxEntry {
            seq: 1,
            created_at: 1,
            change: SyncChange::Project(ProjectChange {
                owner: "acme".into(),
                repo: "acme-web".into(),
                name: "acme-web".into(),
                repo_path: "/tmp/acme-web".into(),
                base_branch: "develop".into(),
                protected_branches: vec![],
                created_at: 1,
                hlc: 1,
                state: None,
                error: None,
                applied_rev: Some(1),
                tracker: Default::default(),
                forge: Default::default(),
                branch_pattern: None,
                attribution: true,
                autonomy: Some(ct_core::AutonomyReport {
                    state: AutonomyState::Error,
                    branch: "wip/autonoma".into(),
                    base: "develop".into(),
                    message: Some("push de wip/autonoma recusado (403)".into()),
                    conflicts: vec![],
                    pr_url: None,
                    pr_pending: false,
                    at: 1,
                }),
            }),
        };
        hub.lock()
            .ingest("env-auto", "env-auto", &[report])
            .unwrap();
        let port = FakePort::default();
        for i in 0..3 {
            tick(&hub, &port, SAB + i * 60_000).await.unwrap();
        }
        assert!(port.calls.lock().unwrap().is_empty());
        let ev = hub.lock().list_autopilot_events(10, None).unwrap();
        assert_eq!(ev.len(), 1, "{ev:?}");
        assert_eq!(ev[0].kind, "refused");
        assert_eq!(ev[0].reason.as_deref(), Some("autonomy_error"));
        assert_eq!(ev[0].scope, "project:acme/acme-web");
        assert!(ev[0].message.contains("403"));
        let w = hub.lock().autonomy_wip(None).unwrap();
        assert_eq!(
            w[0].error.as_deref(),
            Some("push de wip/autonoma recusado (403)")
        );
    }

    #[tokio::test]
    async fn pausado_nao_inicia_e_retomar_volta() {
        let hub = hub();
        let port = FakePort::default();
        hub.lock().set_autopilot_paused(true, SAB).unwrap();
        tick(&hub, &port, SAB).await.unwrap();
        assert!(port.calls.lock().unwrap().is_empty());
        hub.lock().set_autopilot_paused(false, SAB).unwrap();
        tick(&hub, &port, SAB).await.unwrap();
        assert_eq!(port.calls.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn recusa_repetida_vira_um_evento_so() {
        let hub = hub();
        reading(&hub.lock(), 96.0, SAB);
        let port = FakePort::default();
        for i in 0..3 {
            tick(&hub, &port, SAB + i * 60_000).await.unwrap();
        }
        assert!(port.calls.lock().unwrap().is_empty());
        let ev = hub.lock().list_autopilot_events(10, None).unwrap();
        assert_eq!(ev.len(), 1, "{ev:?}");
        assert_eq!(ev[0].kind, "refused");
        assert_eq!(ev[0].reason.as_deref(), Some("budget:five_hour_over_cap"));
        assert_eq!(ev[0].scope, "account:conta-a");
    }

    #[tokio::test]
    async fn falha_do_ambiente_vira_evento_com_o_codigo() {
        let hub = hub();
        let port = FakePort {
            start_error: Some((
                ct_http::sync::APPROVAL_REQUIRED,
                "aguardando aprovação".into(),
            )),
            ..Default::default()
        };
        tick(&hub, &port, SAB).await.unwrap();
        tick(&hub, &port, SAB + 60_000).await.unwrap();
        let ev = hub.lock().list_autopilot_events(10, None).unwrap();
        assert_eq!(ev.len(), 1, "{ev:?}");
        assert_eq!(ev[0].kind, "start_failed");
        assert_eq!(ev[0].reason.as_deref(), Some("approval_required"));
    }

    #[tokio::test]
    async fn estaciona_no_limite_e_retoma_depois_do_reset() {
        let hub = hub();
        // Sessão autônoma ociosa da issue 123 no ambiente autônomo.
        {
            let h = hub.lock();
            let wi = h.find_work_item("acme", "acme-web", 123).unwrap().unwrap();
            h.conn()
                .execute(
                    "INSERT INTO session (id, env_id, work_item_id, tool, cwd, state, started_at,
                                          last_event_at)
                     VALUES ('s1', 'env-auto', ?1, 'claude', '/tmp/acme-web', 'idle', 1, 1)",
                    [wi.id],
                )
                .unwrap();
            reading(&h, 100.0, SAB);
        }
        let port = FakePort::default();
        tick(&hub, &port, SAB).await.unwrap();
        let parked = hub.lock().list_parked().unwrap();
        assert_eq!(parked.len(), 1);
        assert_eq!(parked[0].resume_at, SAB + 2 * H + plan::RESUME_MARGIN_MS);
        assert!(port.calls.lock().unwrap().is_empty());

        // Depois do reset: `session.continue` e nenhum início no ciclo.
        let after = parked[0].resume_at + 1;
        tick(&hub, &port, after).await.unwrap();
        let calls = port.calls.lock().unwrap().clone();
        assert_eq!(calls.len(), 1, "{calls:?}");
        assert_eq!(calls[0].1, "session.continue");
        assert_eq!(calls[0].2, json!({ "session_id": "s1" }));
        assert!(hub.lock().list_parked().unwrap().is_empty());
        let kinds: Vec<String> = hub
            .lock()
            .list_autopilot_events(10, None)
            .unwrap()
            .into_iter()
            .map(|e| e.kind)
            .collect();
        assert!(kinds.contains(&"parked".to_string()), "{kinds:?}");
        assert!(kinds.contains(&"resumed".to_string()), "{kinds:?}");
    }

    #[tokio::test]
    async fn status_mostra_fila_e_esconde_a_conta_na_web() {
        let hub = hub();
        let port = FakePort::default();
        let s = status(&hub.lock(), &port, None, SAB, false).unwrap();
        assert_eq!(s.projects.len(), 1);
        let p = &s.projects[0];
        assert_eq!(p.next, Some(123));
        assert_eq!(p.account.as_deref(), Some("conta-a"));
        let skip: Vec<_> = p.queue.iter().map(|q| q.skip.clone()).collect();
        assert_eq!(skip, vec![None, Some("not_approved".into())]);
        assert_eq!(s.accounts[0].account, "conta-a");
        // Status não tem efeito.
        assert!(port.calls.lock().unwrap().is_empty());

        let w = status(&hub.lock(), &port, None, SAB, true).unwrap();
        assert_eq!(w.accounts[0].account, "conta-1");
        assert_eq!(w.projects[0].account.as_deref(), Some("conta-1"));
    }

    #[tokio::test]
    async fn ambiente_offline_nao_chama_o_canal() {
        let hub = hub();
        let port = FakePort {
            offline: true,
            ..Default::default()
        };
        tick(&hub, &port, SAB).await.unwrap();
        assert!(port.calls.lock().unwrap().is_empty());
        let ev = hub.lock().list_autopilot_events(10, None).unwrap();
        assert_eq!(ev[0].reason.as_deref(), Some("env_offline"));
    }

    #[tokio::test]
    async fn resumo_e_wip_pelo_ws_sem_a_conta_na_web() {
        let hub = hub();
        let s = status(&hub.lock(), &FakePort::default(), None, SAB, false).unwrap();
        assert_eq!(s.wip.len(), 1);
        assert_eq!(s.wip[0].branch, "wip/autonoma");
        // Segunda 09:00 em GMT-3, a semana seguinte ao `SAB`.
        let seg = SAB + 2 * 24 * H - 3 * H;
        reading(&hub.lock(), 10.0, seg);
        super::super::digest::run(&hub.lock(), seg)
            .unwrap()
            .unwrap();
        let st = HubState::new(hub, "ui", "hub", "hub-id");
        let desk = call(&st, "hub.autopilot.digest", json!({}), false)
            .await
            .unwrap();
        assert_eq!(desk[0]["usage"][0]["account"], "conta-a");
        let web = call(&st, "hub.autopilot.digest", json!({ "limit": 5 }), true)
            .await
            .unwrap();
        assert_eq!(web[0]["usage"][0]["account"], "conta-1");
        assert_eq!(web[0]["projects"][0]["repo"], "acme-web");
        let wip = call(
            &st,
            "hub.autopilot.wip",
            json!({ "owner": "acme", "repo": "acme-web" }),
            true,
        )
        .await
        .unwrap();
        assert_eq!(wip[0]["base"], "develop");
        assert!(crate::web::web_allowed("hub.autopilot.digest"));
        assert!(crate::web::web_allowed("hub.autopilot.wip"));
    }
}
