//! Limpeza do ambiente após a conclusão do work item (épico #284, issue
//! #287): o job de verificação, o acionamento manual e o resumo ao hub.
//!
//! - Job ([`spawn_periodic`]): no boot e a cada `cleanup.check_interval_hours`
//!   monta o plano ([`ct_sessions::cleanup::Cleaner::plan`]) sem apagar nada
//!   e guarda o resultado. Com `cleanup.auto`, executa em seguida as etapas
//!   pendentes (nunca as bloqueadas).
//! - Manual, sempre disponível (mesmo com o job desligado): `cleanup.status`,
//!   `cleanup.plan` e `cleanup.run` no `/ws`; `POST /cleanup/plan` e
//!   `POST /cleanup/run` para o `ctower clean`.
//! - Cada plano novo sai no push `cleanup.status` e, com hub, o resumo
//!   (contagem e bytes) vai no próximo envio ([`HubSender::set_cleanup`]).
//! - Cada execução (fora do dry-run) vai para o log e para
//!   `<data>/cleanup/runs.jsonl`: o que saiu, o que ficou e por quê, e o
//!   espaço liberado.
//!
//! Work item concluído com sessão aberta sai no log a cada verificação do
//! job (a etapa "sessões" fica bloqueada até alguém encerrá-la, #285).
//!
//! Uma verificação ou execução por vez: quem chega espera a anterior.

use crate::hub_sync::HubSender;
use crate::AppState;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use ct_sessions::cleanup::{
    human_bytes, Cleaner, CleanupPlan, CleanupRun, CleanupScope, StepKind, StepResult,
};
use ct_sessions::config::CleanupConfig;
use ct_sessions::{Sessions, SessionsError};
use serde::Serialize;
use serde_json::{json, Value};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::broadcast;

/// Estado da limpeza deste ambiente (`cleanup.status` e push `cleanup.status`).
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct CleanupStatus {
    pub config: CleanupConfig,
    /// Último plano montado (job ou manual); `None` = ainda não verificou.
    pub plan: Option<CleanupPlan>,
    /// Falha da última verificação (some na próxima que der certo).
    pub check_error: Option<String>,
    /// Última execução de verdade (sem dry-run).
    pub last_run: Option<CleanupRun>,
    /// Há uma verificação ou execução em andamento.
    pub busy: bool,
}

/// Erro de `cleanup.run`: pedido inválido ou falha ao executar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CleanupError {
    Invalid(String),
    Failed(String),
}

impl CleanupError {
    pub fn message(&self) -> &str {
        match self {
            CleanupError::Invalid(m) | CleanupError::Failed(m) => m,
        }
    }

    fn rpc(self) -> (i64, String) {
        match self {
            CleanupError::Invalid(m) => ct_http::rpc::invalid(m),
            CleanupError::Failed(m) => (-32000, m),
        }
    }

    fn http(self) -> Response {
        match self {
            CleanupError::Invalid(m) => (StatusCode::BAD_REQUEST, m).into_response(),
            CleanupError::Failed(m) => (StatusCode::INTERNAL_SERVER_ERROR, m).into_response(),
        }
    }
}

impl From<SessionsError> for CleanupError {
    fn from(e: SessionsError) -> Self {
        match e {
            SessionsError::Invalid(m) => CleanupError::Invalid(m),
            e => CleanupError::Failed(e.to_string()),
        }
    }
}

#[derive(Default)]
struct Inner {
    plan: Option<CleanupPlan>,
    check_error: Option<String>,
    last_run: Option<CleanupRun>,
}

pub struct Cleanup {
    sessions: Sessions,
    cleaner: Arc<Cleaner>,
    state: Mutex<Inner>,
    /// Uma verificação ou execução por vez.
    busy: tokio::sync::Mutex<()>,
    events: broadcast::Sender<Value>,
    hub: Option<Arc<HubSender>>,
    /// Registro das execuções (JSON Lines).
    log_path: PathBuf,
    /// Relógio (os testes o adiantam para passar das idades mínimas).
    clock: fn() -> i64,
}

impl Cleanup {
    pub fn new(sessions: Sessions, cleaner: Cleaner, events: broadcast::Sender<Value>) -> Self {
        let log_path = sessions
            .config()
            .dirs
            .data
            .join("cleanup")
            .join("runs.jsonl");
        Cleanup {
            sessions,
            cleaner: Arc::new(cleaner),
            state: Mutex::default(),
            busy: tokio::sync::Mutex::new(()),
            events,
            hub: None,
            log_path,
            clock: ct_core::now_ms,
        }
    }

    /// Envio ao hub, que leva o resumo de cada plano novo.
    pub fn with_hub(mut self, hub: Option<Arc<HubSender>>) -> Self {
        self.hub = hub;
        self
    }

    /// Relógio do plano e das idades mínimas (testes).
    pub fn with_clock(mut self, clock: fn() -> i64) -> Self {
        self.clock = clock;
        self
    }

    pub fn config(&self) -> &CleanupConfig {
        self.cleaner.config()
    }

    pub fn log_path(&self) -> &std::path::Path {
        &self.log_path
    }

    pub fn status(&self) -> CleanupStatus {
        self.status_with(self.busy.try_lock().is_err())
    }

    fn status_with(&self, busy: bool) -> CleanupStatus {
        let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        CleanupStatus {
            config: self.config().clone(),
            plan: st.plan.clone(),
            check_error: st.check_error.clone(),
            last_run: st.last_run.clone(),
            busy,
        }
    }

    /// Push do estado ao fim de uma verificação (quem chama ainda segura o
    /// `busy`, mas já terminou).
    fn push(&self) {
        let _ = self
            .events
            .send(json!({ "event": "cleanup.status", "data": self.status_with(false) }));
    }

    /// Recalcula o plano, guarda, avisa as UIs e passa o resumo ao hub.
    /// Nunca apaga nada.
    pub async fn check(&self) -> Result<CleanupPlan, String> {
        let _guard = self.busy.lock().await;
        self.check_locked().await
    }

    async fn check_locked(&self) -> Result<CleanupPlan, String> {
        let (sessions, cleaner, now) = (
            self.sessions.clone(),
            Arc::clone(&self.cleaner),
            (self.clock)(),
        );
        let r = tokio::task::spawn_blocking(move || cleaner.plan(&sessions, now))
            .await
            .map_err(|e| e.to_string())
            .and_then(|r| r.map_err(|e| e.to_string()));
        {
            let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            match &r {
                Ok(p) => {
                    st.plan = Some(p.clone());
                    st.check_error = None;
                }
                Err(e) => st.check_error = Some(e.clone()),
            }
        }
        if let (Ok(p), Some(h)) = (&r, &self.hub) {
            h.set_cleanup(p.summary());
        }
        self.push();
        r
    }

    /// Executa o escopo (ou só relata, com `dry_run`). Fora do dry-run,
    /// registra a execução e monta o plano de novo.
    pub async fn run(
        &self,
        scope: CleanupScope,
        dry_run: bool,
    ) -> Result<CleanupRun, CleanupError> {
        let _guard = self.busy.lock().await;
        let (sessions, cleaner, now) = (
            self.sessions.clone(),
            Arc::clone(&self.cleaner),
            (self.clock)(),
        );
        let run = tokio::task::spawn_blocking(move || cleaner.run(&sessions, scope, dry_run, now))
            .await
            .map_err(|e| CleanupError::Failed(e.to_string()))??;
        if !dry_run {
            self.record(&run);
            self.state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .last_run = Some(run.clone());
            if let Err(e) = self.check_locked().await {
                tracing::warn!("limpeza: plano depois da execução falhou: {e}");
            }
        }
        Ok(run)
    }

    /// Log e `runs.jsonl` de uma execução.
    fn record(&self, run: &CleanupRun) {
        for it in &run.items {
            for s in &it.steps {
                let what = format!(
                    "limpeza {}/{}#{} · {}",
                    it.owner,
                    it.repo,
                    it.gh_number,
                    s.kind.label()
                );
                match s.result {
                    StepResult::Feita => tracing::info!(
                        "{what}: feita, {} liberado(s); saiu: {:?}; ficou: {:?}",
                        human_bytes(s.bytes),
                        s.removed,
                        s.kept
                    ),
                    StepResult::Falhou => {
                        tracing::warn!("{what}: falhou: {}", s.reason.as_deref().unwrap_or(""))
                    }
                    StepResult::Bloqueada => {
                        tracing::debug!("{what}: bloqueada: {}", s.reason.as_deref().unwrap_or(""))
                    }
                    StepResult::Simulada => {}
                }
            }
        }
        let (feitas, falhas) = (run.count(StepResult::Feita), run.count(StepResult::Falhou));
        if feitas > 0 || falhas > 0 {
            tracing::info!(
                "limpeza: {feitas} etapa(s) feita(s), {falhas} com falha, {} bloqueada(s); {} \
                 liberado(s)",
                run.count(StepResult::Bloqueada),
                human_bytes(run.freed_bytes)
            );
        }
        let line = match serde_json::to_string(run) {
            Ok(l) => l,
            Err(e) => return tracing::warn!("limpeza: registro da execução falhou: {e}"),
        };
        let write = || -> std::io::Result<()> {
            if let Some(dir) = self.log_path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let mut f = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.log_path)?;
            writeln!(f, "{line}")
        };
        if let Err(e) = write() {
            tracing::warn!(
                "limpeza: registro em {} falhou: {e}",
                self.log_path.display()
            );
        }
    }
}

/// Monta o plano logo após o boot e depois a cada `every`; com
/// `cleanup.auto`, executa as etapas pendentes em seguida.
pub fn spawn_periodic(cleanup: Arc<Cleanup>, every: Duration) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            match cleanup.check().await {
                Ok(p) => {
                    warn_open_sessions(&p);
                    if p.pending_items > 0 {
                        tracing::info!(
                            "limpeza: {} work item(s) com limpeza pendente ({})",
                            p.pending_items,
                            human_bytes(p.pending_bytes)
                        );
                    }
                    if cleanup.config().auto && p.pending_items > 0 {
                        if let Err(e) = cleanup.run(CleanupScope::default(), false).await {
                            tracing::warn!("limpeza automática falhou: {}", e.message());
                        }
                    }
                }
                Err(e) => tracing::warn!("limpeza: verificação falhou: {e}"),
            }
            tokio::time::sleep(every).await;
        }
    })
}

/// Aviso no log de cada work item concluído com sessão ainda aberta (#285):
/// a limpeza nunca a encerra sem ação explícita (ou
/// `cleanup.end_idle_sessions_hours`), então alguém precisa saber.
fn warn_open_sessions(plan: &CleanupPlan) {
    for it in plan.items.iter().filter(|i| !i.live_sessions.is_empty()) {
        tracing::warn!(
            "limpeza {}/{}#{}: issue concluída, sessão ainda aberta ({}); encerre-a para seguir \
             a limpeza",
            it.owner,
            it.repo,
            it.gh_number,
            it.live_sessions.join(", ")
        );
    }
}

/// Escopo e `dry_run` de `cleanup.run`: `work_item_id` e `step` opcionais
/// (a etapa exige o work item), `dry_run` padrão `false`.
pub fn run_params(p: &Value) -> Result<(CleanupScope, bool), CleanupError> {
    let work_item_id = match p.get("work_item_id") {
        None | Some(Value::Null) => None,
        Some(v) => Some(
            v.as_i64()
                .ok_or_else(|| CleanupError::Invalid("work_item_id inválido".into()))?,
        ),
    };
    let step = match p.get("step") {
        None | Some(Value::Null) => None,
        Some(v) => Some(
            v.as_str()
                .unwrap_or_default()
                .parse::<StepKind>()
                .map_err(CleanupError::Invalid)?,
        ),
    };
    let dry_run = match p.get("dry_run") {
        None | Some(Value::Null) => false,
        Some(v) => v
            .as_bool()
            .ok_or_else(|| CleanupError::Invalid("dry_run deve ser booleano".into()))?,
    };
    Ok((CleanupScope { work_item_id, step }, dry_run))
}

/// Limpeza desativada neste daemon.
pub const DISABLED: &str = "limpeza desativada neste ambiente";

/// `cleanup.*` do `/ws`.
pub(crate) async fn rpc(
    cleanup: Option<Arc<Cleanup>>,
    method: &str,
    p: &Value,
) -> ct_http::rpc::RpcResult {
    let c = cleanup.ok_or_else(|| (-32000, DISABLED.to_string()))?;
    match method {
        "cleanup.status" => Ok(json!(c.status())),
        "cleanup.plan" => c.check().await.map(|p| json!(p)).map_err(|e| (-32000, e)),
        "cleanup.run" => {
            let (scope, dry_run) = run_params(p).map_err(CleanupError::rpc)?;
            c.run(scope, dry_run)
                .await
                .map(|r| json!(r))
                .map_err(CleanupError::rpc)
        }
        _ => Err((-32601, format!("método desconhecido: {method}"))),
    }
}

/// `POST /cleanup/plan` (token de UI): `ctower clean --dry-run`.
pub(crate) async fn plan_http(State(st): State<AppState>, headers: HeaderMap) -> Response {
    if !crate::bearer_ok(&headers, &st.ui_token) {
        return crate::unauthorized();
    }
    let Some(c) = st.cleanup.clone() else {
        return (StatusCode::NOT_FOUND, DISABLED).into_response();
    };
    match c.check().await {
        Ok(p) => axum::Json(p).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
    }
}

/// `POST /cleanup/run` (token de UI), corpo como o de `cleanup.run`:
/// `ctower clean`.
pub(crate) async fn run_http(
    State(st): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    if !crate::bearer_ok(&headers, &st.ui_token) {
        return crate::unauthorized();
    }
    let Some(c) = st.cleanup.clone() else {
        return (StatusCode::NOT_FOUND, DISABLED).into_response();
    };
    let p: Value = if body.is_empty() {
        json!({})
    } else {
        match serde_json::from_slice(&body) {
            Ok(v) => v,
            Err(e) => {
                return (StatusCode::BAD_REQUEST, format!("corpo inválido: {e}")).into_response()
            }
        }
    };
    let (scope, dry_run) = match run_params(&p) {
        Ok(x) => x,
        Err(e) => return e.http(),
    };
    match c.run(scope, dry_run).await {
        Ok(r) => axum::Json(r).into_response(),
        Err(e) => e.http(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{serve_hub, Srv, UI};
    use ct_core::{WorkItemKind, WorkItemStatus};
    use ct_sessions::cleanup::{CleanupStep, StepContext, StepOutcome, StepPlan};
    use ct_sessions::test_support::FakeClaude;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Etapa de teste: pendente enquanto `file` existir; `run` o apaga.
    /// Conta as avaliações e as execuções.
    struct ApagaArquivo {
        kind: StepKind,
        file: PathBuf,
        plans: Arc<AtomicUsize>,
        runs: Arc<AtomicUsize>,
    }

    impl CleanupStep for ApagaArquivo {
        fn kind(&self) -> StepKind {
            self.kind
        }
        fn plan(&self, _: &StepContext<'_>) -> ct_sessions::Result<StepPlan> {
            self.plans.fetch_add(1, Ordering::SeqCst);
            Ok(if self.file.exists() {
                StepPlan::pendente(self.kind, 1 << 30, vec![self.file.display().to_string()])
            } else {
                StepPlan::feita(self.kind)
            })
        }
        fn run(&self, _: &StepContext<'_>) -> ct_sessions::Result<StepOutcome> {
            self.runs.fetch_add(1, Ordering::SeqCst);
            std::fs::remove_file(&self.file)?;
            Ok(StepOutcome {
                freed_bytes: 1 << 30,
                removed: vec![self.file.display().to_string()],
                kept: vec![],
            })
        }
    }

    struct Bloqueada(Arc<AtomicUsize>);

    impl CleanupStep for Bloqueada {
        fn kind(&self) -> StepKind {
            StepKind::BranchRemota
        }
        fn plan(&self, _: &StepContext<'_>) -> ct_sessions::Result<StepPlan> {
            Ok(StepPlan::bloqueada(
                StepKind::BranchRemota,
                "push depois do head do PR",
            ))
        }
        fn run(&self, _: &StepContext<'_>) -> ct_sessions::Result<StepOutcome> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(StepOutcome::default())
        }
    }

    struct Fixture {
        srv: Srv,
        file: PathBuf,
        plans: Arc<AtomicUsize>,
        runs: Arc<AtomicUsize>,
        blocked_runs: Arc<AtomicUsize>,
        work_item_id: i64,
    }

    /// Idades mínimas no passado: o relógio do plano anda 2 dias.
    fn depois() -> i64 {
        ct_core::now_ms() + 48 * 3_600_000
    }

    /// Ambiente com o work item #7 concluído e um worktree falso
    /// (`<tmp>/wt/lixo.bin`) que a etapa de teste apaga.
    fn fixture() -> Fixture {
        let srv = Srv::new(FakeClaude::exit(0));
        let file = srv.env.tmp.path().join("wt").join("lixo.bin");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, b"x").unwrap();
        let work_item_id = {
            let l = srv.env.sessions.ledger();
            let wi = l
                .ensure_work_item(srv.env.project_id, 7, WorkItemKind::Issue)
                .unwrap();
            l.set_work_item_status(wi.id, WorkItemStatus::Concluido, "teste")
                .unwrap();
            wi.id
        };
        Fixture {
            srv,
            file,
            plans: Arc::default(),
            runs: Arc::default(),
            blocked_runs: Arc::default(),
            work_item_id,
        }
    }

    impl Fixture {
        fn cleanup(&self, auto: bool) -> Arc<Cleanup> {
            let cfg = CleanupConfig {
                auto,
                ..CleanupConfig::default()
            };
            let cleaner = Cleaner::new(
                cfg,
                vec![
                    Box::new(ApagaArquivo {
                        kind: StepKind::Worktree,
                        file: self.file.clone(),
                        plans: Arc::clone(&self.plans),
                        runs: Arc::clone(&self.runs),
                    }),
                    Box::new(Bloqueada(Arc::clone(&self.blocked_runs))),
                ],
            );
            Arc::new(
                Cleanup::new(
                    self.srv.env.sessions.clone(),
                    cleaner,
                    self.srv.state.events.clone(),
                )
                .with_clock(depois),
            )
        }
    }

    async fn wait(what: &str, f: impl Fn() -> bool) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while !f() {
            assert!(tokio::time::Instant::now() < deadline, "prazo: {what}");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    /// Critério de aceite do #287: o job roda no boot e no intervalo, monta
    /// o plano e, com `cleanup.auto = false`, não apaga nada.
    #[tokio::test(flavor = "multi_thread")]
    async fn job_roda_no_boot_e_no_intervalo_sem_apagar_nada() {
        let fx = fixture();
        let c = fx.cleanup(false);
        let mut pushes = fx.srv.state.events.subscribe();
        let job = spawn_periodic(Arc::clone(&c), Duration::from_millis(150));
        // Boot: a primeira verificação sai sem esperar o intervalo.
        wait("plano do boot", || c.status().plan.is_some()).await;
        let primeiro = c.status().plan.unwrap();
        assert_eq!(
            (primeiro.pending_items, primeiro.pending_bytes),
            (1, 1 << 30)
        );
        let it = &primeiro.items[0];
        assert_eq!(it.gh_number, 7);
        assert_eq!(
            it.steps[1].reason.as_deref(),
            Some("push depois do head do PR")
        );
        let push = tokio::time::timeout(Duration::from_secs(5), pushes.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(push["event"], "cleanup.status");
        assert_eq!(push["data"]["plan"]["pending_items"], 1);
        // Intervalo: o plano é montado de novo.
        wait("plano do intervalo", || {
            c.status()
                .plan
                .is_some_and(|p| p.checked_at > primeiro.checked_at)
        })
        .await;
        wait("três verificações", || {
            fx.plans.load(Ordering::SeqCst) >= 3
        })
        .await;
        job.abort();
        // Nada foi executado nem apagado.
        assert_eq!(fx.runs.load(Ordering::SeqCst), 0);
        assert_eq!(fx.blocked_runs.load(Ordering::SeqCst), 0);
        assert!(fx.file.exists());
        assert!(c.status().last_run.is_none());
        assert!(!c.log_path().exists());
    }

    /// `cleanup.auto = true`: o job executa as pendentes, nunca as bloqueadas,
    /// e registra a execução.
    #[tokio::test(flavor = "multi_thread")]
    async fn job_com_auto_executa_so_as_pendentes() {
        let fx = fixture();
        let c = fx.cleanup(true);
        let job = spawn_periodic(Arc::clone(&c), Duration::from_secs(3600));
        // `run` grava `last_run` antes de remontar o plano: espera os dois.
        wait("execução automática e plano de depois", || {
            let st = c.status();
            st.last_run.is_some() && st.plan.is_some_and(|p| p.pending_items == 0)
        })
        .await;
        job.abort();
        assert_eq!(fx.runs.load(Ordering::SeqCst), 1);
        assert_eq!(fx.blocked_runs.load(Ordering::SeqCst), 0);
        assert!(!fx.file.exists());
        let st = c.status();
        let run = st.last_run.unwrap();
        assert_eq!(run.freed_bytes, 1 << 30);
        assert_eq!(run.count(StepResult::Feita), 1);
        assert_eq!(run.count(StepResult::Bloqueada), 1);
        // O plano de depois já não tem o worktree pendente.
        assert_eq!(st.plan.unwrap().pending_items, 0);
        let log = std::fs::read_to_string(c.log_path()).unwrap();
        assert_eq!(log.lines().count(), 1);
        assert!(log.contains("push depois do head do PR"), "{log}");
    }

    /// Acionamento manual pelo `/ws`, com o job desligado: plano, dry-run,
    /// uma etapa e o plano inteiro.
    #[tokio::test(flavor = "multi_thread")]
    async fn rpc_plan_e_run_com_o_job_desligado() {
        let fx = fixture();
        let c = fx.cleanup(false);
        let call = |m: &'static str, p: Value| {
            let c = Arc::clone(&c);
            async move { rpc(Some(c), m, &p).await }
        };
        let st = call("cleanup.status", json!({})).await.unwrap();
        assert_eq!(st["plan"], Value::Null);
        assert_eq!(st["config"]["auto"], false);
        let p = call("cleanup.plan", json!({})).await.unwrap();
        assert_eq!(p["pending_items"], 1);
        assert_eq!(p["items"][0]["steps"][0]["state"], "pendente");
        assert_eq!(p["items"][0]["steps"][1]["state"], "bloqueada");

        let r = call("cleanup.run", json!({ "dry_run": true }))
            .await
            .unwrap();
        assert_eq!(r["dry_run"], true);
        assert_eq!(r["items"][0]["steps"][0]["result"], "simulada");
        assert!(fx.file.exists());
        assert_eq!(fx.runs.load(Ordering::SeqCst), 0);

        let e = call("cleanup.run", json!({ "step": "worktree" }))
            .await
            .unwrap_err();
        assert_eq!(e.0, -32602, "{e:?}");
        let e = call("cleanup.run", json!({ "step": "lixo", "work_item_id": 1 }))
            .await
            .unwrap_err();
        assert!(e.1.contains("etapa inválida"), "{e:?}");

        let r = call(
            "cleanup.run",
            json!({ "work_item_id": fx.work_item_id, "step": "worktree" }),
        )
        .await
        .unwrap();
        assert_eq!(r["items"][0]["steps"][0]["result"], "feita");
        assert_eq!(r["freed_bytes"], 1u64 << 30);
        assert!(!fx.file.exists());
        // O plano foi refeito depois da execução.
        let st = call("cleanup.status", json!({})).await.unwrap();
        assert_eq!(st["plan"]["pending_items"], 0);
        assert_eq!(st["last_run"]["freed_bytes"], 1u64 << 30);

        let e = rpc(None, "cleanup.plan", &json!({})).await.unwrap_err();
        assert_eq!(e.1, DISABLED);
    }

    /// `POST /cleanup/plan` e `/cleanup/run` (ctower clean), com o token de UI.
    #[tokio::test(flavor = "multi_thread")]
    async fn http_plan_e_run() {
        let mut fx = fixture();
        let (s, _) = fx.srv.post("/cleanup/plan", Some(UI), &[], json!({})).await;
        assert_eq!(s, StatusCode::NOT_FOUND);
        fx.srv.state.cleanup = Some(fx.cleanup(false));
        let (s, _) = fx.srv.post("/cleanup/plan", None, &[], json!({})).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED);
        let (s, v) = fx.srv.post("/cleanup/plan", Some(UI), &[], json!({})).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(v["items"][0]["gh_number"], 7);
        let (s, _) = fx
            .srv
            .post("/cleanup/run", Some(UI), &[], json!({ "dry_run": "sim" }))
            .await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        let (s, v) = fx
            .srv
            .post("/cleanup/run", Some(UI), &[], json!({ "dry_run": true }))
            .await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(v["freed_bytes"], 1u64 << 30);
        assert!(fx.file.exists());
        let (s, v) = fx.srv.post("/cleanup/run", Some(UI), &[], json!({})).await;
        assert_eq!(s, StatusCode::OK, "{v}");
        assert_eq!(v["items"][0]["steps"][0]["result"], "feita");
        assert!(!fx.file.exists());
    }

    /// Etapa "sessões" de verdade (#285) pelo `/ws`: sessão viva de issue
    /// concluída bloqueia a limpeza e nunca é encerrada pelo `cleanup.run`;
    /// depois do encerramento explícito (`session.kill`), a etapa arquiva a
    /// sessão.
    #[tokio::test(flavor = "multi_thread")]
    async fn sessao_viva_de_issue_concluida_so_sai_com_acao_explicita() {
        let srv = Srv::new(FakeClaude::sleep(30));
        let env = &srv.env;
        let s = env.sessions.start(env.request(3, "A")).unwrap();
        env.wait_fake_output(&s.id);
        env.sessions
            .ledger()
            .set_work_item_status(s.work_item_id, WorkItemStatus::Concluido, "teste")
            .unwrap();
        let c = Arc::new(
            Cleanup::new(
                env.sessions.clone(),
                Cleaner::standard(CleanupConfig {
                    auto: true,
                    ..CleanupConfig::default()
                }),
                srv.state.events.clone(),
            )
            .with_clock(depois),
        );
        let call = |m: &'static str, p: Value| {
            let c = Arc::clone(&c);
            async move { rpc(Some(c), m, &p).await.unwrap() }
        };
        let p = call("cleanup.plan", json!({})).await;
        let it = &p["items"][0];
        assert_eq!(it["live_sessions"], json!([s.id]));
        assert_eq!(it["steps"][0]["kind"], "sessoes");
        assert_eq!(it["steps"][0]["state"], "bloqueada");
        assert!(it["steps"][0]["reason"]
            .as_str()
            .unwrap()
            .starts_with("issue concluída, sessão ainda aberta"));
        // Nem a execução (como a do job com `auto`) encerra a sessão.
        let job = spawn_periodic(Arc::clone(&c), Duration::from_secs(3600));
        wait("verificação do job", || c.status().plan.is_some()).await;
        job.abort();
        let r = call("cleanup.run", json!({})).await;
        assert_eq!(r["items"][0]["steps"][0]["result"], "bloqueada");
        assert!(env.sessions.pty(&s.id).is_some());
        assert!(env
            .sessions
            .ledger()
            .get_session(&s.id)
            .unwrap()
            .state
            .is_live());

        // Ação explícita: encerrar a sessão (o `session.kill` da UI).
        env.sessions.kill(&s.id).unwrap();
        env.wait_state(&s.id, ct_core::SessionState::Exited);
        let p = call("cleanup.plan", json!({})).await;
        assert_eq!(p["items"][0]["live_sessions"], json!([]));
        assert_eq!(p["items"][0]["steps"][0]["state"], "pendente");
        let r = call(
            "cleanup.run",
            json!({ "work_item_id": s.work_item_id, "step": "sessoes" }),
        )
        .await;
        assert_eq!(r["items"][0]["steps"][0]["result"], "feita");
        assert_eq!(
            env.sessions.ledger().get_session(&s.id).unwrap().state,
            ct_core::SessionState::Archived
        );
    }

    /// O resumo do plano vai ao hub no envio seguinte, mesmo com o outbox
    /// vazio.
    #[tokio::test(flavor = "multi_thread")]
    async fn resumo_vai_ao_hub() {
        let fx = fixture();
        let hub = ct_ledger::SharedHub::new(ct_ledger::Hub::open_in_memory().unwrap());
        let tok = hub.lock().add_legacy_env_token("a").unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        serve_hub(hub.clone(), listener).await;
        let sender = Arc::new(HubSender::new(
            fx.srv.env.sessions.clone(),
            crate::hub_sync::HubTarget { url, token: tok },
            Duration::from_secs(5),
        ));
        let push = |s: &Arc<HubSender>| {
            let s = Arc::clone(s);
            tokio::task::spawn_blocking(move || s.push_pending())
        };
        push(&sender).await.unwrap().unwrap();
        assert_eq!(hub.lock().list_envs().unwrap()[0].cleanup, None);

        let c = Cleanup::new(
            fx.srv.env.sessions.clone(),
            Cleaner::new(
                CleanupConfig::default(),
                vec![Box::new(ApagaArquivo {
                    kind: StepKind::Worktree,
                    file: fx.file.clone(),
                    plans: Arc::default(),
                    runs: Arc::default(),
                })],
            ),
            fx.srv.state.events.clone(),
        )
        .with_clock(depois)
        .with_hub(Some(Arc::clone(&sender)));
        let plan = c.check().await.unwrap();
        // Outbox vazio: o resumo novo vai num envio sem mudanças.
        assert_eq!(push(&sender).await.unwrap().unwrap(), 0);
        let got = hub.lock().list_envs().unwrap()[0].cleanup.unwrap();
        assert_eq!(got.checked_at, plan.checked_at);
        assert_eq!((got.items, got.bytes), (1, 1 << 30));
    }
}
