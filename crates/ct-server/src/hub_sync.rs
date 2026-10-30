//! Envio ambiente → hub (spec do ledger centralizado §5 e §8).
//!
//! Cada daemon manda o seu `sync_outbox` em lotes para `POST /sync/push` do
//! hub, autenticado com o token de sync do ambiente. O hub aplica cada lote
//! de forma idempotente e devolve o último `seq` aplicado; o ambiente apaga
//! do outbox o que foi confirmado. Hub fora do ar: o outbox cresce e o envio
//! tenta de novo com backoff exponencial (até 5 min), sem bloquear nada. O
//! hub é o `cthubd`; não há mais envio em processo: a máquina do hub roda um
//! `ctowerd` comum, que envia ao `cthubd` local por HTTP.
//!
//! Há no máximo um envio em voo: o laço de fundo e o pedido manual
//! (`hub.sync.now`, `ctower hub sync`) passam por `flush`, e quem chega com
//! um envio em andamento espera por ele e recebe o mesmo resultado. O pedido
//! manual puxa antes a configuração do hub (`hub_config`), para o relato da
//! aplicação já ir neste envio.
//!
//! Logo depois de um envio que deixou o outbox vazio, ainda dentro do envio
//! em voo (nunca em paralelo a outro), roda a poda (spec do hub §6): no
//! máximo uma vez a cada [`PRUNE_EVERY`], ou sempre no pedido manual.

use crate::AppState;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
pub use ct_http::sync::{DecisionsQuery, DecisionsReply, PushRequest, PushResponse};
use ct_ledger::{HubDecision, OutboxEntry};
use ct_sessions::Sessions;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

/// Mudanças por lote.
pub const BATCH: u32 = 200;
/// Intervalo entre envios com o hub no ar.
pub const PUSH_EVERY: Duration = Duration::from_secs(5);
/// Teto do backoff com o hub fora do ar.
pub const MAX_BACKOFF: Duration = Duration::from_secs(5 * 60);

/// Intervalo mínimo entre podas automáticas (o critério é em dias).
pub const PRUNE_EVERY: Duration = Duration::from_secs(10 * 60);
/// Retenção padrão da poda (`retention_days`).
pub const DEFAULT_RETENTION_DAYS: u32 = 30;

/// Timeout da consulta do MCP ao hub: o agente nunca fica esperando muito.
pub const QUERY_TIMEOUT: Duration = Duration::from_secs(5);

/// Hub para onde vão as mudanças deste ambiente (`hub_url` e o token de sync).
pub struct HubTarget {
    pub url: String,
    pub token: String,
}

/// Estado do envio, para `env.info` e a UI.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct HubSyncStatus {
    /// Mudanças esperando no outbox.
    pub pending: i64,
    pub oldest_pending_at: Option<i64>,
    pub last_attempt_at: Option<i64>,
    /// Último envio confirmado pelo hub.
    pub last_ok_at: Option<i64>,
    pub last_error: Option<String>,
    /// Falhas seguidas (base do backoff).
    pub failures: u32,
    /// Há um envio em andamento.
    pub syncing: bool,
    /// Poda após o ack (spec do hub §6).
    pub prune: PruneStatus,
}

/// Estado da poda, para `env.info` e a UI.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct PruneStatus {
    /// `0` = poda desligada.
    pub retention_days: u32,
    /// Work items já podados neste ambiente (o histórico deles só está no
    /// hub).
    pub pruned_work_items: i64,
    /// Última vez que a poda rodou (com o outbox vazio).
    pub last_run_at: Option<i64>,
    /// Work items podados na última vez.
    pub last_pruned: u32,
    /// Falha da última poda, ou dos candidatos e arquivos que não saíram.
    pub last_error: Option<String>,
}

/// Resultado de `hub.sync.now` / `POST /hub/sync` (`ctower hub sync`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct HubSyncResult {
    /// O hub confirmou tudo: sem erro, e o envio só termina com o outbox
    /// vazio. O que entra no outbox depois (ex.: o relato de um clone em
    /// segundo plano) fica em `pending`, para o envio seguinte, e não torna
    /// a rodada uma falha.
    pub ok: bool,
    /// Lotes confirmados pelo hub.
    pub batches: u32,
    /// Mudanças confirmadas.
    pub sent: i64,
    /// Último `seq` do outbox confirmado nesta rodada.
    pub acked_seq: Option<i64>,
    /// Mudanças que ficaram no outbox.
    pub pending: i64,
    /// Motivo da falha (hub inacessível, token recusado...).
    pub error: Option<String>,
    /// Reaproveitou um envio que já estava em andamento.
    pub joined: bool,
    /// Pull da configuração feito antes do envio.
    pub config: Option<crate::hub_config::ConfigPullResult>,
    /// Work items podados logo depois deste envio.
    #[serde(default)]
    pub pruned: u32,
}

/// O que um envio conseguiu, inclusive o progresso antes de uma falha.
#[derive(Debug, Clone, Default)]
struct Flushed {
    batches: u32,
    sent: i64,
    acked_seq: Option<i64>,
    error: Option<String>,
    pruned: u32,
}

/// Estado em memória da poda.
#[derive(Debug, Default)]
struct PruneState {
    /// Quando rodou pela última vez (base do intervalo).
    ran: Option<std::time::Instant>,
    last_run_at: Option<i64>,
    last_pruned: u32,
    last_error: Option<String>,
}

/// Envio em voo: quem chega durante ele espera o resultado.
#[derive(Default)]
struct Flight {
    result: Mutex<Option<Flushed>>,
    done: Condvar,
}

/// Encerra o envio em voo mesmo se `push_all` entrar em pânico, para quem
/// espera não ficar preso.
struct FlightGuard<'a> {
    sender: &'a HubSender,
    flight: Arc<Flight>,
}

impl Drop for FlightGuard<'_> {
    fn drop(&mut self) {
        let mut r = self.flight.result.lock().unwrap_or_else(|e| e.into_inner());
        if r.is_none() {
            *r = Some(Flushed {
                error: Some("envio interrompido".into()),
                ..Flushed::default()
            });
        }
        drop(r);
        self.flight.done.notify_all();
        *self
            .sender
            .inflight
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = None;
    }
}

pub struct HubSender {
    pub(crate) sessions: Sessions,
    pub(crate) target: HubTarget,
    pub(crate) agent: ureq::Agent,
    query_agent: ureq::Agent,
    state: Mutex<HubSyncStatus>,
    inflight: Mutex<Option<Arc<Flight>>>,
    /// Pull da configuração (`hub_config`).
    pub(crate) config: crate::hub_config::ConfigOptions,
    pub(crate) pull: Mutex<crate::hub_config::PullState>,
    pub(crate) pull_lock: Mutex<()>,
    /// Uma aplicação de atribuições (clone) por vez, fora do `pull_lock`.
    pub(crate) apply_lock: Mutex<()>,
    /// Chave do ambiente e cache dos secrets entregues pelo hub (#184).
    pub(crate) env_cache: ct_secrets::EnvCache,
    /// `retention_days` da poda; `0` desliga.
    retention_days: u32,
    /// Relógio da poda (os testes o adiantam).
    pub(crate) clock: fn() -> i64,
    prune: Mutex<PruneState>,
    /// O hub já registrou este ambiente nesta execução (envio vazio ou com
    /// mudanças). Ele só lista ambientes que já enviaram algo, então um
    /// ambiente novo, de outbox vazio, precisa se apresentar.
    announced: AtomicBool,
    /// Resumo do último plano de limpeza (issue #287), em todo envio.
    cleanup: Mutex<Option<ct_ledger::CleanupSummary>>,
    /// Resumo novo ainda não confirmado pelo hub: com o outbox vazio, vai
    /// num envio sem mudanças.
    cleanup_dirty: AtomicBool,
}

impl HubSender {
    pub fn new(sessions: Sessions, target: HubTarget, timeout: Duration) -> Self {
        let env_cache = crate::env_secrets::env_cache(&sessions.config().dirs);
        HubSender {
            sessions,
            target,
            agent: ureq::AgentBuilder::new().timeout(timeout).build(),
            query_agent: ureq::AgentBuilder::new()
                .timeout(QUERY_TIMEOUT.min(timeout))
                .build(),
            state: Mutex::new(HubSyncStatus::default()),
            inflight: Mutex::new(None),
            config: Default::default(),
            pull: Mutex::default(),
            pull_lock: Mutex::new(()),
            apply_lock: Mutex::new(()),
            env_cache,
            retention_days: DEFAULT_RETENTION_DAYS,
            clock: ct_core::now_ms,
            prune: Mutex::default(),
            announced: AtomicBool::new(false),
            cleanup: Mutex::new(None),
            cleanup_dirty: AtomicBool::new(false),
        }
    }

    /// Resumo do último plano de limpeza, levado ao hub no próximo envio
    /// (mesmo com o outbox vazio).
    pub fn set_cleanup(&self, summary: ct_ledger::CleanupSummary) {
        *self.cleanup.lock().unwrap_or_else(|e| e.into_inner()) = Some(summary);
        self.cleanup_dirty.store(true, Ordering::SeqCst);
    }

    /// `retention_days` da poda (`0` desliga).
    pub fn with_retention_days(mut self, days: u32) -> Self {
        self.retention_days = days;
        self
    }

    /// Como o pull aplica a configuração (`repos_root`, intervalo, `gh`).
    pub fn with_config(mut self, config: crate::hub_config::ConfigOptions) -> Self {
        self.config = config;
        self
    }

    /// Envia tudo o que está pendente, lote a lote. Retorna quantas mudanças
    /// o hub confirmou. Síncrono: chame fora do runtime.
    pub fn push_pending(&self) -> Result<usize, String> {
        let (f, _) = self.flush(false);
        match f.error {
            None => Ok(f.sent as usize),
            Some(e) => Err(e),
        }
    }

    /// Sob demanda (`hub.sync.now`): puxa a configuração, dispara a
    /// aplicação das atribuições em segundo plano (o envio não espera clone)
    /// e envia o outbox, ignorando o backoff e reaproveitando o envio em
    /// andamento, se houver. Síncrono: chame fora do runtime.
    pub fn sync_now(self: &Arc<Self>) -> HubSyncResult {
        let config = self.pull_config();
        // A validação de um clone existente roda aqui, antes do envio, para
        // o relato ir neste lote: em segundo plano ela podia terminar depois
        // do envio e o `ok` vinha falso sem erro nenhum. Só o clone novo
        // (que pode demorar) fica em segundo plano, para o envio seguinte.
        self.apply_existing();
        self.spawn_apply();
        let (f, joined) = self.flush(true);
        let pending = self
            .sessions
            .ledger()
            .outbox_stats()
            .map(|o| o.pending)
            .unwrap_or(0);
        HubSyncResult {
            // Sem erro, o envio esvaziou o outbox: o que está em `pending`
            // entrou depois dele (a aplicação em segundo plano corre em
            // paralelo) e não é falha desta rodada.
            ok: f.error.is_none() && config.error.is_none(),
            batches: f.batches,
            sent: f.sent,
            acked_seq: f.acked_seq,
            pending,
            error: f.error.or_else(|| {
                config
                    .error
                    .as_ref()
                    .map(|e| format!("pull da configuração: {e}"))
            }),
            joined,
            config: Some(config),
            pruned: f.pruned,
        }
    }

    /// Um envio por vez: o primeiro a chegar envia; os demais esperam por
    /// ele e recebem o mesmo resultado (`true` = reaproveitado). Com o
    /// outbox zerado, poda antes de encerrar o envio em voo (`force` ignora
    /// o [`PRUNE_EVERY`]).
    fn flush(&self, force: bool) -> (Flushed, bool) {
        let (flight, leader) = {
            let mut cur = self.inflight.lock().unwrap_or_else(|e| e.into_inner());
            match &*cur {
                Some(f) => (Arc::clone(f), false),
                None => {
                    let f = Arc::new(Flight::default());
                    *cur = Some(Arc::clone(&f));
                    (f, true)
                }
            }
        };
        if !leader {
            let mut r = flight.result.lock().unwrap_or_else(|e| e.into_inner());
            while r.is_none() {
                r = flight.done.wait(r).unwrap_or_else(|e| e.into_inner());
            }
            return (r.clone().unwrap_or_default(), true);
        }
        let guard = FlightGuard {
            sender: self,
            flight,
        };
        let now = ct_core::now_ms();
        let mut f = self.push_all();
        if f.error.is_none() {
            f.pruned = self.prune_after_push(force);
        }
        {
            let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            st.last_attempt_at = Some(now);
            match &f.error {
                None => {
                    st.last_ok_at = Some(now);
                    st.last_error = None;
                    st.failures = 0;
                }
                Some(e) => {
                    st.last_error = Some(e.clone());
                    st.failures = st.failures.saturating_add(1);
                }
            }
        }
        *guard
            .flight
            .result
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(f.clone());
        drop(guard);
        (f, false)
    }

    /// Poda depois de um envio sem erro, se o outbox ficou vazio (a poda
    /// confere de novo dentro da transação dela). Devolve quantos work items
    /// saíram.
    fn prune_after_push(&self, force: bool) -> u32 {
        if self.retention_days == 0 {
            return 0;
        }
        let mut st = self.prune.lock().unwrap_or_else(|e| e.into_inner());
        if !force && st.ran.is_some_and(|t| t.elapsed() < PRUNE_EVERY) {
            return 0;
        }
        match self.sessions.ledger().outbox_stats() {
            Ok(o) if o.pending == 0 => {}
            _ => return 0,
        }
        let now = (self.clock)();
        match self.sessions.prune(self.retention_days, now) {
            Ok(o) if o.report.skipped.is_some() => {
                // Mudança nova entre a conferência e a transação: tenta no
                // próximo envio.
                0
            }
            Ok(o) => {
                let r = &o.report;
                let n = r.work_items.len() as u32;
                if n > 0 || !r.projects.is_empty() {
                    tracing::info!(
                        "poda: {n} work item(s) e {} projeto(s) desatribuído(s) saíram do \
                         ambiente (já no hub; retention_days = {}): {:?} {:?}",
                        r.projects.len(),
                        self.retention_days,
                        r.work_items,
                        r.projects
                    );
                }
                let mut problems: Vec<String> =
                    r.failed.iter().map(|(k, e)| format!("{k}: {e}")).collect();
                problems.extend(o.file_errors.iter().cloned());
                for p in &problems {
                    tracing::warn!("poda: {p}");
                }
                st.ran = Some(std::time::Instant::now());
                st.last_run_at = Some(now);
                st.last_pruned = n;
                st.last_error = (!problems.is_empty()).then(|| problems.join("; "));
                n
            }
            Err(e) => {
                tracing::warn!("poda falhou: {e}");
                st.ran = Some(std::time::Instant::now());
                st.last_run_at = Some(now);
                st.last_pruned = 0;
                st.last_error = Some(e.to_string());
                0
            }
        }
    }

    fn push_all(&self) -> Flushed {
        let mut f = Flushed::default();
        if let Err(e) = self.push_batches(&mut f) {
            f.error = Some(e);
        }
        f
    }

    fn push_batches(&self, f: &mut Flushed) -> Result<(), String> {
        let env_id = self.sessions.ledger().env_id().to_string();
        loop {
            let batch = self
                .sessions
                .ledger()
                .outbox_pending(BATCH)
                .map_err(|e| e.to_string())?;
            let Some(last) = batch.last().map(|e| e.seq) else {
                if !self.announced.load(Ordering::Relaxed)
                    || self.cleanup_dirty.load(Ordering::SeqCst)
                {
                    self.send(&env_id, Vec::new(), 0)?;
                    self.announced.store(true, Ordering::Relaxed);
                }
                return Ok(());
            };
            let n = batch.len();
            let total = self
                .sessions
                .ledger()
                .outbox_stats()
                .map_err(|e| e.to_string())?
                .pending;
            let acked = self.send(&env_id, batch, total - n as i64)?;
            // O hub confirma até `acked`; nunca além do que foi enviado.
            let acked = acked.min(last);
            self.sessions
                .ledger()
                .outbox_ack(acked)
                .map_err(|e| e.to_string())?;
            self.announced.store(true, Ordering::Relaxed);
            f.batches += 1;
            f.sent += n as i64;
            f.acked_seq = Some(acked);
            if (n as u32) < BATCH {
                return Ok(());
            }
        }
    }

    fn send(&self, env_id: &str, entries: Vec<OutboxEntry>, pending: i64) -> Result<i64, String> {
        let HubTarget { url, token } = &self.target;
        // Baixa a marca antes de ler o resumo: um resumo novo que chegar
        // durante o envio a levanta de novo e vai no próximo.
        let dirty = self.cleanup_dirty.swap(false, Ordering::SeqCst);
        let cleanup = *self.cleanup.lock().unwrap_or_else(|e| e.into_inner());
        let body = serde_json::to_string(&PushRequest {
            env_id: env_id.to_string(),
            entries,
            pending,
            cleanup,
        })
        .expect("PushRequest serializa");
        let r = self.post_push(url, token, body);
        if r.is_err() && dirty {
            self.cleanup_dirty.store(true, Ordering::SeqCst);
        }
        r
    }

    fn post_push(&self, url: &str, token: &str, body: String) -> Result<i64, String> {
        let resp = self
            .agent
            .post(&format!("{}/sync/push", url.trim_end_matches('/')))
            .set("Authorization", &format!("Bearer {token}"))
            .set("Content-Type", "application/json")
            .send_string(&body);
        match resp {
            Ok(r) => r
                .into_json::<PushResponse>()
                .map(|r| r.last_seq)
                .map_err(|e| format!("resposta inválida do hub: {e}")),
            Err(ureq::Error::Status(code, r)) => {
                Err(status_error(code, &r.into_string().unwrap_or_default()))
            }
            Err(ureq::Error::Transport(t)) => Err(format!("hub inacessível: {t}")),
        }
    }

    /// Decisões do escopo consultadas no hub (MCP `ledger_list_decisions`).
    /// Erro (hub fora, work item ainda não enviado) = o chamador usa as locais.
    pub fn query_decisions(&self, q: &DecisionsQuery) -> Result<Vec<HubDecision>, String> {
        let HubTarget { url, token } = &self.target;
        let resp = self
            .query_agent
            .post(&format!(
                "{}/sync/query/decisions",
                url.trim_end_matches('/')
            ))
            .set("Authorization", &format!("Bearer {token}"))
            .set("Content-Type", "application/json")
            .send_string(&serde_json::to_string(q).expect("DecisionsQuery serializa"));
        match resp {
            Ok(r) => r
                .into_json::<DecisionsReply>()
                .map(|r| r.decisions)
                .map_err(|e| format!("resposta inválida do hub: {e}")),
            Err(ureq::Error::Status(404, _)) => Err("work item ainda não chegou ao hub".into()),
            Err(ureq::Error::Status(code, r)) => {
                Err(status_error(code, &r.into_string().unwrap_or_default()))
            }
            Err(ureq::Error::Transport(t)) => Err(format!("hub inacessível: {t}")),
        }
    }

    /// Chamada a `/agent/v1/<path>` do hub com o token de agente (lido do
    /// arquivo a cada vez), para as ferramentas `portfolio_*` do gestor.
    /// O agente nunca vê o token. `session` vai em `X-Ctower-Session`.
    pub fn agent_call(
        &self,
        method: &str,
        path: &str,
        query: &[(&str, String)],
        body: Option<&Value>,
        session: &str,
    ) -> Result<Value, String> {
        let token = agent_token(&self.sessions.config().dirs).ok_or(NO_AGENT_TOKEN)?;
        let env_id = self.sessions.ledger().env_id().to_string();
        let url = format!("{}/agent/v1/{path}", self.target.url.trim_end_matches('/'));
        let mut req = self
            .query_agent
            .request(method, &url)
            .set("Authorization", &format!("Bearer {token}"))
            .set(ct_http::sync::ENV_ID_HEADER, &env_id)
            .set("X-Ctower-Session", session);
        for (k, v) in query {
            req = req.query(k, v);
        }
        let resp = match body {
            Some(b) => req
                .set("Content-Type", "application/json")
                .send_string(&b.to_string()),
            None => req.call(),
        };
        match resp {
            Ok(r) => r
                .into_json::<Value>()
                .map_err(|e| format!("resposta inválida do hub: {e}")),
            Err(ureq::Error::Status(401, _)) => Err(
                "hub recusou o token de agente (401): gere outro com `cthubd agent-token` no hub \
                 e grave com `ctower hub set-agent-token`"
                    .into(),
            ),
            Err(ureq::Error::Status(code, r)) => {
                let body = r.into_string().unwrap_or_default();
                Err(format!("hub respondeu {code}: {}", body.trim()))
            }
            Err(ureq::Error::Transport(t)) => Err(format!("hub indisponível: {t}")),
        }
    }

    /// O hub responde ao `/healthz` (para `ledger_get_context` do gestor).
    pub fn reachable(&self) -> bool {
        self.query_agent
            .get(&format!(
                "{}/healthz",
                self.target.url.trim_end_matches('/')
            ))
            .call()
            .is_ok()
    }

    pub fn status(&self) -> HubSyncStatus {
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner()).clone();
        {
            let l = self.sessions.ledger();
            if let Ok(o) = l.outbox_stats() {
                st.pending = o.pending;
                st.oldest_pending_at = o.oldest_created_at;
            }
            st.prune.pruned_work_items = l.pruned_work_items().unwrap_or(0);
        }
        {
            let p = self.prune.lock().unwrap_or_else(|e| e.into_inner());
            st.prune.retention_days = self.retention_days;
            st.prune.last_run_at = p.last_run_at;
            st.prune.last_pruned = p.last_pruned;
            st.prune.last_error = p.last_error.clone();
        }
        st.syncing = self
            .inflight
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_some();
        st
    }

    /// Espera até o próximo envio: `PUSH_EVERY` com o hub no ar, dobrando a
    /// cada falha seguida até `MAX_BACKOFF`.
    pub fn next_delay(&self) -> Duration {
        let failures = self
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .failures;
        PUSH_EVERY
            .checked_mul(1u32 << failures.min(16))
            .unwrap_or(MAX_BACKOFF)
            .min(MAX_BACKOFF)
    }
}

/// Erro HTTP do hub em texto legível para a UI e a CLI.
pub(crate) fn status_error(code: u16, body: &str) -> String {
    let body = body.trim();
    match code {
        401 => "hub recusou o token de sync (401): gere outro com `cthubd add-env` \
                no hub e grave com `ctower hub set-token`"
            .into(),
        403 => format!("hub recusou o token de sync (403): {body}"),
        _ if body.is_empty() => format!("hub respondeu {code}"),
        _ => format!("hub respondeu {code}: {body}"),
    }
}

/// Laço de envio do daemon. Na primeira volta, põe no outbox o ledger que
/// existia antes da replicação (primeira conexão ao hub).
pub fn spawn_sender(sender: Arc<HubSender>) {
    tokio::spawn(async move {
        let s = Arc::clone(&sender);
        match tokio::task::spawn_blocking(move || s.sessions.ledger().ensure_backfilled()).await {
            Ok(Ok(n)) if n > 0 => {
                tracing::info!("hub: {n} mudança(s) do ledger existente no outbox")
            }
            Ok(Ok(_)) => {}
            Ok(Err(e)) => tracing::warn!("hub: backfill do outbox falhou: {e}"),
            Err(e) => tracing::warn!("hub: backfill do outbox falhou: {e}"),
        }
        loop {
            let s = Arc::clone(&sender);
            match tokio::task::spawn_blocking(move || s.push_pending()).await {
                Ok(Ok(n)) if n > 0 => tracing::debug!("hub: {n} mudança(s) enviada(s)"),
                Ok(Ok(_)) => {}
                Ok(Err(e)) => tracing::warn!("hub: envio falhou: {e}"),
                Err(e) => tracing::warn!("hub: envio falhou: {e}"),
            }
            tokio::time::sleep(sender.next_delay()).await;
        }
    });
}

/// Sem `hub_url` ou sem `hub.token`: nada a enviar nem a puxar, e o
/// ambiente não inicia sessões (spec §7).
/// Sem `hub-agent.token`, o gestor não inicia (spec do gestor §2.3).
pub const NO_AGENT_TOKEN: &str = "ambiente sem token de agente do hub: gere com \"cthubd \
agent-token <nome>\" e grave com \"ctower hub set-agent-token\"";

/// Token de agente gravado no ambiente, lido a cada chamada (trocar o
/// arquivo não pede restart). `None` se ausente ou vazio.
pub fn agent_token(dirs: &ct_sessions::Dirs) -> Option<String> {
    std::fs::read_to_string(dirs.hub_agent_token_file())
        .ok()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
}

pub const NOT_LINKED: &str =
    "ambiente não ligado a um hub: configure hub_url e grave o token com \"ctower hub set-token\"";

/// Envio sob demanda fora do runtime (`hub.sync.now` e `POST /hub/sync`).
pub(crate) async fn run_sync_now(sender: Option<Arc<HubSender>>) -> Result<HubSyncResult, String> {
    let s = sender.ok_or_else(|| NOT_LINKED.to_string())?;
    tokio::task::spawn_blocking(move || s.sync_now())
        .await
        .map_err(|e| e.to_string())
}

/// `POST /hub/sync` (token de UI): `ctower hub sync`. Responde 200 com o
/// resultado mesmo quando o hub não confirmou tudo (`ok: false`).
pub(crate) async fn sync_now_http(State(st): State<AppState>, headers: HeaderMap) -> Response {
    if !crate::bearer_ok(&headers, &st.ui_token) {
        return crate::unauthorized();
    }
    match run_sync_now(st.hub_sender.clone()).await {
        Ok(r) => axum::Json(r).into_response(),
        Err(e) if st.hub_sender.is_none() => (StatusCode::NOT_FOUND, e).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{serve_hub, DeadHub, Srv};
    use axum::body::Bytes;
    use ct_core::WorkItemKind;
    use ct_ledger::{DecisionScope, Hub, SharedHub};
    use ct_sessions::test_support::FakeClaude;

    fn sender(srv: &Srv, target: HubTarget) -> Arc<HubSender> {
        Arc::new(HubSender::new(
            srv.env.sessions.clone(),
            target,
            Duration::from_secs(5),
        ))
    }

    fn remote(url: &str, token: &str) -> HubTarget {
        HubTarget {
            url: url.to_string(),
            token: token.to_string(),
        }
    }

    async fn push(s: &Arc<HubSender>) -> Result<usize, String> {
        let s = Arc::clone(s);
        tokio::task::spawn_blocking(move || s.push_pending())
            .await
            .unwrap()
    }

    /// Dois ambientes e um `cthubd`: hub fora, mudanças esperam no outbox
    /// com backoff; hub volta, tudo chega e o outbox esvazia.
    #[tokio::test(flavor = "multi_thread")]
    async fn dois_ambientes_e_hub_que_cai_e_volta() {
        let hub = new_hub();
        let tok_a = hub.lock().add_legacy_env_token("a").unwrap();
        let tok_b = hub.lock().add_legacy_env_token("b").unwrap();

        // O hub ainda não está no ar (a porta fica presa pelo teste).
        let dead = DeadHub::new();
        let url = dead.url.clone();

        let a = Srv::new(FakeClaude::exit(0));
        let b = Srv::new(FakeClaude::exit(0));
        for (srv, n) in [(&a, 10), (&b, 10)] {
            let l = srv.env.sessions.ledger();
            l.ensure_work_item(srv.env.project_id, n, WorkItemKind::Issue)
                .unwrap();
        }
        let sa = sender(&a, remote(&url, &tok_a));
        let sb = sender(&b, remote(&url, &tok_b));

        let antes = sa.status().pending;
        assert!(antes > 0);
        assert!(push(&sa).await.is_err());
        assert!(push(&sa).await.is_err());
        let st = sa.status();
        assert_eq!(st.pending, antes);
        assert_eq!(st.failures, 2);
        assert!(st.last_error.unwrap().contains("hub inacessível"));
        assert!(st.last_ok_at.is_none());
        assert_eq!(sa.next_delay(), PUSH_EVERY * 4);

        // O hub sobe.
        serve_hub(hub.clone(), dead.revive()).await;

        assert_eq!(push(&sa).await.unwrap() as i64, antes);
        push(&sb).await.unwrap();
        let st = sa.status();
        assert_eq!((st.pending, st.failures), (0, 0));
        assert!(st.last_ok_at.is_some() && st.last_error.is_none());
        assert_eq!(sa.next_delay(), PUSH_EVERY);

        // Consulta de decisões pelo mesmo token de sync.
        let q = |number| DecisionsQuery {
            owner: "deep-ion-ai".into(),
            repo: "demo".into(),
            number,
            scope: DecisionScope::Project,
        };
        let s2 = Arc::clone(&sa);
        let (ok, falta) = tokio::task::spawn_blocking(move || {
            (s2.query_decisions(&q(10)), s2.query_decisions(&q(999)))
        })
        .await
        .unwrap();
        assert_eq!(ok.unwrap().len(), 0);
        assert!(falta.unwrap_err().contains("ainda não chegou"));

        let h = hub.lock();
        let w = h
            .find_work_item("deep-ion-ai", "demo", 10)
            .unwrap()
            .unwrap();
        let envs: std::collections::BTreeSet<_> = h
            .list_events(w.id, 100)
            .unwrap()
            .into_iter()
            .map(|e| e.env_id)
            .collect();
        assert_eq!(envs.len(), 2, "eventos de a e b no mesmo work item");
        let p = h.find_project("deep-ion-ai", "demo").unwrap().unwrap();
        assert_eq!(h.project_locations(p.id).unwrap().len(), 2);
        assert!(h.list_envs().unwrap().iter().all(|e| e.pending == 0));
        let mut nomes: Vec<_> = h.list_envs().unwrap().into_iter().map(|e| e.name).collect();
        nomes.sort();
        assert_eq!(nomes, vec!["a", "b"]);
    }

    async fn sync_now(s: &Arc<HubSender>) -> HubSyncResult {
        let s = Arc::clone(s);
        tokio::task::spawn_blocking(move || s.sync_now())
            .await
            .unwrap()
    }

    /// Ambiente novo, de outbox vazio: o hub só o lista depois de um envio,
    /// então o primeiro envio se apresenta mesmo sem mudanças (uma vez só).
    #[tokio::test(flavor = "multi_thread")]
    async fn ambiente_de_outbox_vazio_se_apresenta_ao_hub() {
        let srv = Srv::new(FakeClaude::exit(0));
        // Esvazia o outbox num hub descartável.
        let antigo = new_hub();
        let tok = antigo.lock().add_legacy_env_token("a").unwrap();
        let url = hub_up(&antigo).await;
        push(&sender(&srv, remote(&url, &tok))).await.unwrap();
        assert_eq!(srv.env.sessions.ledger().outbox_stats().unwrap().pending, 0);

        let hub = new_hub();
        let tok = hub.lock().add_legacy_env_token("a").unwrap();
        let url = hub_up(&hub).await;
        assert!(hub.lock().list_envs().unwrap().is_empty());

        let s = sender(&srv, remote(&url, &tok));
        assert_eq!(push(&s).await.unwrap(), 0);
        let envs = hub.lock().list_envs().unwrap();
        assert_eq!(envs.len(), 1);
        assert_eq!(envs[0].name, "a");
        assert_eq!(envs[0].last_seq, 0);
    }

    /// `cthubd` de verdade escutando numa porta livre; devolve a URL.
    async fn hub_up(hub: &SharedHub) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        serve_hub(hub.clone(), listener).await;
        url
    }

    /// Hub com `deep-ion-ai/demo` cadastrado (o hub nunca cria projeto a
    /// partir de um push).
    fn new_hub() -> SharedHub {
        let hub = Hub::open_in_memory().unwrap();
        hub.add_project(&ct_ledger::NewHubProject {
            owner: "deep-ion-ai".into(),
            repo: "demo".into(),
            name: "demo".into(),
            base_branch: "develop".into(),
            protected_branches: vec![],
        })
        .unwrap();
        SharedHub::new(hub)
    }

    /// Ambiente com o work item #10 no outbox.
    fn env_with_changes() -> Srv {
        let a = Srv::new(FakeClaude::exit(0));
        a.env
            .sessions
            .ledger()
            .ensure_work_item(a.env.project_id, 10, WorkItemKind::Issue)
            .unwrap();
        a
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn sync_now_esvazia_o_outbox_e_o_hub_recebe() {
        let hub = new_hub();
        let tok = hub.lock().add_legacy_env_token("a").unwrap();
        let url = hub_up(&hub).await;
        let a = env_with_changes();
        let sa = sender(&a, remote(&url, &tok));
        let antes = sa.status().pending;
        assert!(antes > 0);

        let r = sync_now(&sa).await;
        assert!(r.ok, "{r:?}");
        assert_eq!((r.sent, r.pending, r.batches), (antes, 0, 1));
        assert!(r.acked_seq.is_some() && r.error.is_none());
        assert!(!r.joined);
        assert!(hub
            .lock()
            .find_work_item("deep-ion-ai", "demo", 10)
            .unwrap()
            .is_some());
        let st = sa.status();
        assert!(st.last_ok_at.is_some() && st.last_attempt_at.is_some() && !st.syncing);

        // De novo: nada a enviar, e continua ok.
        let r = sync_now(&sa).await;
        assert!(r.ok);
        assert_eq!((r.sent, r.batches, r.acked_seq), (0, 0, None));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn sync_now_com_hub_fora_mantem_o_outbox() {
        let dead = DeadHub::new();
        let a = env_with_changes();
        let sa = sender(&a, remote(&dead.url, "t"));
        let antes = sa.status().pending;
        let r = sync_now(&sa).await;
        assert!(!r.ok);
        assert!(
            r.error.as_deref().unwrap().contains("hub inacessível"),
            "{r:?}"
        );
        assert_eq!((r.sent, r.pending), (0, antes));
        let st = sa.status();
        assert_eq!(st.failures, 1);
        assert!(st.last_error.unwrap().contains("hub inacessível"));
        assert!(st.last_attempt_at.is_some());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn sync_now_com_token_recusado() {
        let hub = new_hub();
        hub.lock().add_legacy_env_token("a").unwrap();
        let url = hub_up(&hub).await;
        let a = env_with_changes();
        let sa = sender(&a, remote(&url, "errado"));
        let antes = sa.status().pending;
        let r = sync_now(&sa).await;
        assert!(!r.ok);
        let e = r.error.unwrap();
        assert!(e.contains("recusou o token") && e.contains("401"), "{e}");
        assert_eq!(r.pending, antes);
    }

    /// Duas chamadas ao mesmo tempo: um único envio, o segundo reaproveita.
    #[tokio::test(flavor = "multi_thread")]
    async fn sync_now_concorrente_reaproveita_o_envio() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let hits = Arc::new(AtomicUsize::new(0));
        let h = Arc::clone(&hits);
        let app = axum::Router::new().route(
            "/sync/push",
            axum::routing::post(move |body: Bytes| {
                let h = Arc::clone(&h);
                async move {
                    h.fetch_add(1, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(400)).await;
                    let req: PushRequest = serde_json::from_slice(&body).unwrap();
                    let last_seq = req.entries.iter().map(|e| e.seq).max().unwrap_or(0);
                    axum::Json(PushResponse { last_seq })
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let a = env_with_changes();
        let sa = sender(&a, remote(&url, "t"));
        let s1 = Arc::clone(&sa);
        let first = tokio::task::spawn_blocking(move || s1.sync_now());
        // Espera o primeiro envio começar antes de pedir o segundo.
        while hits.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(sa.status().syncing);
        let second = sync_now(&sa).await;
        let first = first.await.unwrap();
        assert_eq!(hits.load(Ordering::SeqCst), 1, "um único POST");
        assert!(first.ok && second.ok);
        assert!(!first.joined && second.joined);
        assert_eq!(first.sent, second.sent);
        assert!(!sa.status().syncing);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn post_hub_sync_exige_token_de_ui_e_envio_configurado() {
        let mut srv = Srv::new(FakeClaude::exit(0));
        let (st, _) = srv
            .post(
                "/hub/sync",
                Some(crate::test_util::HOOK),
                &[],
                serde_json::json!({}),
            )
            .await;
        assert_eq!(st, StatusCode::UNAUTHORIZED);
        let (st, _) = srv
            .post(
                "/hub/sync",
                Some(crate::test_util::UI),
                &[],
                serde_json::json!({}),
            )
            .await;
        assert_eq!(st, StatusCode::NOT_FOUND);

        let hub = new_hub();
        let tok = hub.lock().add_legacy_env_token("a").unwrap();
        let url = hub_up(&hub).await;
        srv.state.hub_sender = Some(sender(&srv, remote(&url, &tok)));
        let (st, v) = srv
            .post(
                "/hub/sync",
                Some(crate::test_util::UI),
                &[],
                serde_json::json!({}),
            )
            .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(v["ok"], true);
        assert!(v.get("local").is_none());
    }

    /// Relógio 31 dias à frente: tudo o que está fechado ficou velho.
    fn daqui_a_31_dias() -> i64 {
        ct_core::now_ms() + 31 * 24 * 60 * 60 * 1000
    }

    /// Item fechado e confirmado pelo hub é podado logo depois do envio que
    /// zerou o outbox (hub fora: nada sai); o hub guarda o histórico. Reaberto, o item volta por
    /// chave natural e o hub funde com o que já tinha.
    #[tokio::test(flavor = "multi_thread")]
    async fn poda_depois_do_ack_e_hub_funde_o_reaberto() {
        let hub = new_hub();
        let tok = hub.lock().add_legacy_env_token("a").unwrap();
        let url = hub_up(&hub).await;
        let a = Srv::new(FakeClaude::exit(0));
        let s = a.env.sessions.start(a.env.request(10, "Velha")).unwrap();
        a.env.wait_state(&s.id, ct_core::SessionState::Exited);
        let ativo = a
            .env
            .sessions
            .ledger()
            .ensure_work_item(a.env.project_id, 11, WorkItemKind::Issue)
            .unwrap();
        a.env
            .sessions
            .ledger()
            .set_work_item_status(s.work_item_id, ct_core::WorkItemStatus::Concluido, "t")
            .unwrap();
        a.env.sessions.ledger().ensure_backfilled().unwrap();
        let mut sa = HubSender::new(
            a.env.sessions.clone(),
            remote(&url, &tok),
            Duration::from_secs(5),
        );
        sa.clock = daqui_a_31_dias;
        let sa = Arc::new(sa);

        // Hub fora: nada sai do outbox, nada é podado.
        let dead = DeadHub::new();
        let mut morto = HubSender::new(
            a.env.sessions.clone(),
            remote(&dead.url, &tok),
            Duration::from_secs(5),
        );
        morto.clock = daqui_a_31_dias;
        assert!(push(&Arc::new(morto)).await.is_err());
        assert!(a.env.sessions.ledger().get_session(&s.id).is_ok());

        // Laço de envio (sem pull: o projeto local legado segue atribuído).
        push(&sa).await.unwrap();
        {
            let l = a.env.sessions.ledger();
            assert!(l.get_work_item(s.work_item_id).is_err());
            assert!(l.get_session(&s.id).is_err());
            assert!(l.get_work_item(ativo.id).is_ok());
            assert_eq!(l.pruned_work_items().unwrap(), 1);
        }
        assert!(!a.env.cfg.dirs.session_dir(&s.id).exists());
        let st = sa.status();
        assert_eq!(
            (
                st.prune.retention_days,
                st.prune.pruned_work_items,
                st.prune.last_pruned
            ),
            (30, 1, 1)
        );
        assert!(st.prune.last_run_at.is_some() && st.prune.last_error.is_none());

        let antes = {
            let h = hub.lock();
            let w = h
                .find_work_item("deep-ion-ai", "demo", 10)
                .unwrap()
                .unwrap();
            assert_eq!(h.list_sessions(w.id).unwrap().len(), 1);
            h.list_events(w.id, 1000).unwrap().len()
        };
        assert!(antes > 0);

        // Reaberta: linha nova pela chave natural; o hub funde.
        a.env
            .sessions
            .ledger()
            .ensure_work_item(a.env.project_id, 10, WorkItemKind::Issue)
            .unwrap();
        push(&sa).await.unwrap();
        assert_eq!(sa.status().prune.pruned_work_items, 1);
        let h = hub.lock();
        let w = h
            .find_work_item("deep-ion-ai", "demo", 10)
            .unwrap()
            .unwrap();
        assert_eq!(h.list_events(w.id, 1000).unwrap().len(), antes + 1);
        assert_eq!(h.list_sessions(w.id).unwrap().len(), 1);
    }

    /// `retention_days = 0`: o envio segue e nada é podado.
    #[tokio::test(flavor = "multi_thread")]
    async fn retencao_zero_desliga_a_poda() {
        let hub = new_hub();
        let tok = hub.lock().add_legacy_env_token("a").unwrap();
        let url = hub_up(&hub).await;
        let a = env_with_changes();
        let wi = {
            let l = a.env.sessions.ledger();
            let wi = l.find_work_item(a.env.project_id, 10).unwrap().unwrap();
            l.set_work_item_status(wi.id, ct_core::WorkItemStatus::Concluido, "t")
                .unwrap();
            l.ensure_backfilled().unwrap();
            wi
        };
        let mut sa = HubSender::new(
            a.env.sessions.clone(),
            remote(&url, &tok),
            Duration::from_secs(5),
        )
        .with_retention_days(0);
        sa.clock = daqui_a_31_dias;
        let sa = Arc::new(sa);
        let r = sync_now(&sa).await;
        assert!(r.ok && r.pruned == 0, "{r:?}");
        assert!(a.env.sessions.ledger().get_work_item(wi.id).is_ok());
        let st = sa.status();
        assert_eq!(st.prune.retention_days, 0);
        assert!(st.prune.last_run_at.is_none());
    }

    #[test]
    fn backoff_dobra_ate_cinco_minutos() {
        let srv = Srv::new(FakeClaude::exit(0));
        let s = sender(&srv, remote("http://127.0.0.1:1", "t"));
        for _ in 0..10 {
            s.state.lock().unwrap().failures += 1;
        }
        assert_eq!(s.next_delay(), MAX_BACKOFF);
    }
}
