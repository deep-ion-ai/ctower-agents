//! `cthubd`: o hub, plano de controle do Control Tower (spec do hub como
//! plano de controle, épico #183). Guarda a visão consolidada em `hub.db`,
//! tira snapshots, sincroniza com o GitHub todos os projetos e serve:
//!
//! - `/sync/push`, `/sync/config` e `/sync/query/decisions` com o token de
//!   sync de cada ambiente (`cthubd add-env`), e com ele o canal reverso
//!   `/sync/channel` (`channel`, #198);
//! - `/ws` com o token de UI do hub (`hub.*`, `github.*`);
//! - com `[web] enabled`, a UI web embutida (`/`, `/assets/*`), o login
//!   web (`/web/login|logout|me`, `web_auth`, #200) e o `/web/ws` com a
//!   lista de permissões da web (`web`, #199);
//! - `/healthz` sem autenticação.
//!
//! Requisição vinda pela Cloudflare (`Cf-Connecting-IP`) só alcança as rotas
//! web: `/ws`, `/sync/*` e `/agent/v1/*` respondem `404` (spec da PWA §3.2).
//!
//! Não roda sessões, PTY, hooks nem MCP: esses são do `ctowerd`.

mod agent;
pub mod backup;
pub mod channel;
pub mod cli;
pub mod config;
pub mod docs_backup;
pub mod github;
pub mod migrate;
pub mod push;
pub mod remote;
pub mod secrets;
mod sync;
pub mod update;
pub mod web;
pub mod web_auth;
pub mod ws;

use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::Router;
use ct_ledger::SharedHub;
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::sync::broadcast;

/// Versão do protocolo UI ⇄ hub (`hub.info`), independente da do ambiente.
/// 1.2 (#328): `hub.projects.impact` e `hub.workitem.history {
/// include_sub_issues }`. 1.3 (fase 1 do épico #205): `HubSession.rc_session_id`
/// (#197), `hub.web.sessions.list|revoke` (#200) e
/// `hub.push.vapid_key|subscribe|list|update|remove|test` (#203). 1.4
/// (fase 2 do épico #205): `HubEnv.online` e o push `env.online` (canal
/// reverso, #198), `hub.env.call` e o push `env.push` (#343). 1.5 (épico
/// #353, Jira e GitLab): `tracker`, `forge`, `branch_pattern`, `attribution`
/// e `sync_env_id` em `HubProject`, `ref` (referência pronta do work item) no
/// ranking, na saúde e nas propostas, o status do rastreador, as regras do
/// agente e as sprints do espelho. 1.6 (#421): `hub.update.get|check|start|rollback`
/// e o push `hub.update.status` (atualizar o hub pela UI). 1.7 (#457): status
/// `aguardando_epico` do work item (sub-issue à espera do merge do épico).
/// 1.8 (#440): `docs` em `HubProject` e em `hub.projects.add|update`, e
/// `HubInfo.docs_dir`. 1.9 (#442): backup dos docs — `BackupConfigView.docs`,
/// `docs_*` no `hub.backup.set`, `what` no `hub.backup.run` e os avisos
/// `docs_backup_failed|stale|too_large` em `hub.status`. 1.10 (#434):
/// `hub.workitem.body` (descrição da issue lida no rastreador, sem gravar).
/// 1.11 (#90): `failures` e `failing_since` no `SyncState` (falhas seguidas
/// do sync do projeto).
pub const PROTOCOL_VERSION: &str = "1.11";

#[derive(Clone)]
pub struct HubState {
    pub hub: SharedHub,
    pub ui_token: Arc<str>,
    /// Push para as UIs conectadas: `{ "event": ..., "data": ... }`.
    pub events: broadcast::Sender<Value>,
    /// Sync com o GitHub (`github.*`); `None` = desativado.
    pub github: Option<Arc<github::HubGithub>>,
    /// Nome legível do hub (`name`).
    pub name: Arc<str>,
    /// Identidade do hub nos eventos que ele grava (`<config>/hub_id`).
    pub hub_id: Arc<str>,
    pub backups: Arc<backup::Backups>,
    /// Limiares de `hub.health.get` (`[health]`).
    pub health: Arc<ct_ledger::HealthThresholds>,
    /// Intervalo do sync do GitHub, para o sinal `github_sem_sync`; `0` = desligado.
    pub gh_sync_interval_secs: u64,
    /// Store de secrets e variáveis (#164); `None` = não aberto (testes).
    pub secrets: Option<Arc<secrets::HubSecrets>>,
    /// Rotas da UI web (`[web]`); desligadas por padrão.
    pub web: web::WebOptions,
    /// Web Push da PWA (`[web.push]`, #203); `None` = desligada.
    pub push: Option<Arc<push::WebPush>>,
    /// Canais reversos abertos pelos ambientes (`/sync/channel`, #198).
    pub channels: Arc<channel::EnvChannels>,
    /// Atualização do hub pela UI (`hub.update.*`, #421); `None` = desativada.
    pub updates: Option<Arc<update::HubUpdates>>,
    /// Pasta dos docs dos projetos (`docs_dir`, #440); `None` = desligada.
    pub docs_dir: Option<Arc<str>>,
}

impl HubState {
    /// Estado mínimo: sem GitHub e sem snapshots.
    pub fn new(hub: SharedHub, ui_token: &str, name: &str, hub_id: &str) -> Self {
        let (events, _) = broadcast::channel(1024);
        let hub_clone = hub.clone();
        let events_clone = events.clone();
        HubState {
            hub,
            ui_token: ui_token.into(),
            events,
            github: None,
            name: name.into(),
            hub_id: hub_id.into(),
            backups: Arc::new(backup::Backups::disabled()),
            health: Arc::default(),
            gh_sync_interval_secs: 0,
            secrets: None,
            web: web::WebOptions::disabled(hub_clone, ui_token),
            push: None,
            channels: Arc::new(channel::EnvChannels::new(
                events_clone,
                channel::ChannelOptions::default(),
            )),
            updates: None,
            docs_dir: None,
        }
    }
}

impl HubState {
    /// Avisa as UIs que a lista de propostas mudou (`hub.proposals.changed`).
    /// Sem UI conectada não há receptor: tudo bem.
    pub(crate) fn proposals_changed(&self) {
        let _ = self
            .events
            .send(json!({ "event": "hub.proposals.changed", "data": {} }));
        self.push_kick();
    }

    /// Pede ao motor de Web Push uma varredura já (proposta nova, sync).
    pub(crate) fn push_kick(&self) {
        if let Some(p) = &self.push {
            p.kick();
        }
    }

    /// Avisa as UIs que regras do agente ou propostas de regras mudaram
    /// (`hub.rules.changed`, #357).
    pub(crate) fn rules_changed(&self) {
        let _ = self
            .events
            .send(json!({ "event": "hub.rules.changed", "data": {} }));
    }
}

pub fn router(state: HubState) -> Router {
    let web = state.web.enabled;
    let r = Router::new()
        .route("/healthz", get(healthz))
        .route("/ws", get(ws::upgrade))
        .route(
            "/sync/push",
            post(sync::push).layer(axum::extract::DefaultBodyLimit::max(
                ct_http::sync::SYNC_BODY_LIMIT,
            )),
        )
        .route("/sync/config", get(sync::config))
        .route("/sync/done", get(sync::done))
        .route(ct_http::sync::CHANNEL_PATH, get(channel::upgrade))
        .route("/sync/query/decisions", post(sync::query_decisions))
        .route("/agent/v1/overview", get(agent::overview))
        .route(
            "/agent/v1/projects/{owner}/{repo}",
            get(agent::project_tree),
        )
        .route(
            "/agent/v1/work-items/{owner}/{repo}/{number}",
            get(agent::work_item),
        )
        .route("/agent/v1/ranking", get(agent::ranking))
        .route("/agent/v1/health", get(agent::health))
        .route("/agent/v1/decisions", get(agent::decisions))
        .route(
            "/agent/v1/proposals",
            get(agent::list_proposals).post(agent::create_proposal),
        );
    // Desligada, `/`, `/assets/*` e `/web/*` caem no 404 padrão.
    let r = if web {
        r.route("/web/ws", get(web::upgrade))
            .route(
                "/web/login",
                post(web::login).layer(axum::extract::DefaultBodyLimit::max(web::LOGIN_BODY_LIMIT)),
            )
            .route("/web/logout", post(web::logout))
            .route("/web/me", get(web::me))
            .fallback(web::asset)
    } else {
        r
    };
    r.layer(axum::middleware::from_fn(cloudflare_only_web))
        .with_state(state)
}

/// Rotas que a Cloudflare não alcança: o app desktop e os ambientes usam o
/// Tailscale ou a LAN (spec da PWA §3.2, #252).
fn closed_to_cloudflare(path: &str) -> bool {
    path == "/ws"
        || ["/sync", "/agent/v1"]
            .iter()
            .any(|p| path == *p || path.starts_with(&format!("{p}/")))
}

/// Com `Cf-Connecting-IP` (veio pelo `cloudflared`), `/ws`, `/sync/*` e
/// `/agent/v1/*` respondem `404`, com ou sem `[web.cloudflare]`.
async fn cloudflare_only_web(
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    if req.headers().contains_key(web_auth::CF_CONNECTING_IP)
        && closed_to_cloudflare(req.uri().path())
    {
        tracing::warn!(
            "{} recusado: requisição vinda pela Cloudflare",
            req.uri().path()
        );
        return axum::http::StatusCode::NOT_FOUND.into_response();
    }
    next.run(req).await
}

async fn healthz() -> impl IntoResponse {
    axum::Json(json!({
        "ok": true,
        "role": "hub",
        "version": ct_core::VERSION,
        "commit": ct_core::BUILD_COMMIT,
        "protocol": PROTOCOL_VERSION,
    }))
}
