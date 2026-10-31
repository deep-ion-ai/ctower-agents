//! `/ws` do hub (spec do hub como plano de controle §8): JSON-RPC em texto
//! com o token de UI do hub, protocolo próprio a partir de `1.0`.
//!
//! Métodos: `hub.info`, `hub.status`, `hub.projects.list|add|update|delete|impact`,
//! `hub.assignments.list|add|remove`, `hub.quarantine.list|discard`, `hub.tree.get`, `hub.workitem.history`,
//! `hub.workitem.set_status|set_priority|move`, `hub.workitem.body`, `hub.projects.move`,
//! `hub.ranking.get`, `hub.health.get`, `hub.proposals.list|decide`,
//! `hub.dashboard.get`, `hub.envs.list`,
//! `hub.rules.get|set|import|history` e `hub.rules.proposals.list|decide`
//! (regras do agente por projeto e por épico, #357),
//! `hub.secrets.list|set|delete` (secrets e variáveis, #165),
//! `hub.secrets.rotate_pending_dismiss` (lista a rotacionar depois de um
//! `cthubd secrets restore`, #170),
//! `hub.recovery.status|generate|verify` (chave de recuperação, #169) e
//! `hub.web.sessions.list|revoke` (sessões web, #200),
//! `hub.push.vapid_key|subscribe|list|update|remove|test` (Web Push, #203) e
//! `hub.env.call { env, method, params }` (método do ambiente pelo canal
//! reverso, #343) e
//! `github.status|set_token|clear_token` (token do GitHub do hub) e
//! `hub.update.get|check|start|rollback` (atualizar o hub, #421) e
//! `hub.logs.tail|follow|unfollow` (log do hub, #65). Push:
//! `hub_sync_status`, `hub.proposals.changed`, `hub.rules.changed`,
//! `hub.update.status`, `hub.logs.lines` (só na conexão que pediu),
//! `env.online` e `env.push` (canal reverso, `crate::channel`).
//! `HubEnv.online` em `hub.envs.list` e `hub.dashboard.get` vem do canal.
//!
//! O `/web/ws` (`crate::web`, #199) usa este mesmo despacho, com a lista de
//! permissões da web aplicada antes dele.
//!
//! Secrets: a UI só escreve. `hub.secrets.list` devolve nomes, escopo, tipo
//! e revisão, e o valor apenas das variáveis; nenhuma resposta nem erro
//! carrega o valor de um secret. A chave privada de recuperação só sai na
//! resposta de `hub.recovery.generate`; a colada em `hub.recovery.verify`
//! não volta em resposta nem erro.
//!
//! Projetos nascem e são editados só aqui (spec §3.1, issue #175): o
//! cadastro GitHub é pela URL do GitHub (`ct_core::github_repo_from_url`),
//! sem clone, validado com o token do GitHub do hub. O cadastro Jira +
//! GitLab (#348) leva `tracker` e `forge` e não passa pelo GitHub: o
//! ambiente de sync valida no primeiro ciclo. `hub.projects.update {
//! sync_env }` troca o ambiente de sync entre os atribuídos.

use crate::backup::{BackupStatus, Warning};
use crate::secrets::HubSecretsError;
use crate::HubState;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::Response;
use ct_http::rpc::{i64_param, invalid, str_param, RpcResult};
use ct_http::{bearer_ok, unauthorized};
use ct_ledger::{
    AssignmentFilter, HubAssignment, HubProjectPatch, HubSession, NewHubProject, Placement,
    ProjectProvider, QuarantineGroup, RankingFilter,
};
use ct_secrets::{Kind, Scope, SecretValue, Zeroizing};
use futures_util::{SinkExt, StreamExt};
use serde::Serialize;
use serde_json::{json, Value};
use tokio::sync::mpsc;

/// Resposta de `hub.info`.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct HubInfo {
    /// Versão do protocolo do hub (`major.minor`).
    pub protocol: String,
    pub version: String,
    /// Commit do build; vazio se compilado fora do git.
    pub commit: String,
    pub name: String,
    /// Revisão da configuração (projetos e atribuições).
    pub config_rev: i64,
    pub backup: BackupStatus,
    /// Pasta dos docs dos projetos (`docs_dir`, #440); `None` = desligada.
    pub docs_dir: Option<String>,
}

/// Resposta de `hub.status`: avisos para a faixa da UI.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct HubStatus {
    pub warnings: Vec<Warning>,
    pub backup: BackupStatus,
    /// Linhas de push em quarentena, por projeto e ambiente.
    pub quarantine: Vec<QuarantineGroup>,
}

/// Resposta de `hub.projects.delete`.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct ProjectDeleted {
    /// `config_rev` da lápide.
    pub rev: i64,
    /// Snapshot fora da rotação tirado antes de apagar.
    pub snapshot: String,
    pub assignments: Vec<HubAssignment>,
    pub live_sessions: Vec<HubSession>,
}

/// Resposta de `hub.assignments.remove`.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct AssignmentRemoved {
    pub rev: i64,
    pub assignment: HubAssignment,
    /// Sessões vivas relatadas, que o ambiente não encerra.
    pub live_sessions: Vec<HubSession>,
}

/// Erro de `hub.secrets.*`: o hub está sem o store de secrets aberto.
pub const SECRETS_UNAVAILABLE: i64 = -32020;
/// Erro de `hub.secrets.delete`: não há entrada com esse escopo e nome.
pub const SECRET_NOT_FOUND: i64 = -32021;
/// Erro de `hub.secrets.set`: o projeto do escopo não está cadastrado no hub.
pub const SECRET_UNKNOWN_PROJECT: i64 = -32022;

/// Erro de `hub.recovery.verify`: nenhuma chave de recuperação gerada.
pub const RECOVERY_NOT_STARTED: i64 = -32023;
/// Erro de `hub.recovery.verify`: a chave colada não confere.
pub const RECOVERY_KEY_MISMATCH: i64 = -32024;
/// Erro de `hub.update.rollback` sem `force`: o `hub.db` voltaria do
/// snapshot; a mensagem diz o que sai do banco em uso (#421).
pub const UPDATE_ROLLBACK_DB: i64 = -32025;

/// Resposta de `hub.secrets.set` e `hub.secrets.delete`.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct SecretChanged {
    /// `config_rev` da mudança (os ambientes recebem no próximo pull).
    pub rev: i64,
}

/// Resposta de `hub.quarantine.discard`.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct QuarantineDiscarded {
    pub discarded: usize,
}

/// Avisos da quarentena: um por projeto, com a contagem por ambiente.
fn quarantine_warnings(groups: &[QuarantineGroup]) -> Vec<Warning> {
    let mut out: Vec<Warning> = Vec::new();
    let mut i = 0;
    while i < groups.len() {
        let key = &groups[i].project_key;
        let same: Vec<&QuarantineGroup> = groups[i..]
            .iter()
            .take_while(|g| &g.project_key == key)
            .collect();
        let total: i64 = same.iter().map(|g| g.rows).sum();
        let envs = same
            .iter()
            .map(|g| format!("{} {}", g.env_name, g.rows))
            .collect::<Vec<_>>()
            .join(", ");
        out.push(Warning {
            kind: "push_quarantine",
            message: format!(
                "{total} linha(s) de push do projeto {key}, que não está cadastrado no hub, \
                 em quarentena ({envs}): cadastre o projeto ou descarte a quarentena"
            ),
        });
        i += same.len();
    }
    out
}

pub(crate) async fn upgrade(
    State(st): State<HubState>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    if !bearer_ok(&headers, &st.ui_token) {
        return unauthorized();
    }
    ws.on_upgrade(move |socket| serve(socket, st, Channel::Ui))
}

/// Por onde a conexão chegou: o `/ws` (token de UI, tudo) ou o `/web/ws`
/// (sessão web `id`, só `crate::web::WEB_METHODS` e `WEB_PUSHES`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Channel {
    Ui,
    Web(std::sync::Arc<str>),
}

impl Channel {
    fn web_session(&self) -> Option<&str> {
        match self {
            Channel::Ui => None,
            Channel::Web(id) => Some(id),
        }
    }
}

/// Com que frequência a conexão do `/web/ws` confere a sessão sem receber
/// mensagem (revogada ou vencida, a conexão fecha).
const WEB_RECHECK: std::time::Duration = std::time::Duration::from_secs(30);

/// A sessão web da conexão ainda vale? (`true` no `/ws`.)
async fn web_session_ok(st: &HubState, channel: &Channel) -> bool {
    let Some(id) = channel.web_session().map(str::to_string) else {
        return true;
    };
    let auth = st.web.auth.clone();
    tokio::task::spawn_blocking(move || auth.still_valid(&id))
        .await
        .unwrap_or(false)
}

pub(crate) async fn serve(socket: WebSocket, st: HubState, channel: Channel) {
    let (mut sink, mut stream) = socket.split();
    let (out, mut out_rx) = mpsc::unbounded_channel::<Message>();
    let writer = tokio::spawn(async move {
        while let Some(m) = out_rx.recv().await {
            if sink.send(m).await.is_err() {
                break;
            }
        }
    });
    let pusher = match channel {
        Channel::Ui => ct_http::rpc::spawn_pusher(st.events.subscribe(), out.clone()),
        Channel::Web(_) => ct_http::rpc::spawn_pusher_filtered(
            st.events.subscribe(),
            out.clone(),
            crate::web::web_push,
        ),
    };
    let web = channel.web_session().is_some();
    let mut log_follow: Option<tokio::task::JoinHandle<()>> = None;
    let mut recheck = tokio::time::interval(WEB_RECHECK);
    recheck.reset();
    loop {
        let msg = tokio::select! {
            m = stream.next() => m,
            _ = recheck.tick(), if web => {
                if web_session_ok(&st, &channel).await {
                    continue;
                }
                break;
            }
        };
        let Some(Ok(msg)) = msg else { break };
        match msg {
            Message::Text(t) => {
                // Sessão revogada ou vencida no meio da conexão: fecha.
                if web && !web_session_ok(&st, &channel).await {
                    let _ = out.send(Message::Close(Some(axum::extract::ws::CloseFrame {
                        code: 4401,
                        reason: "sessão web inválida".into(),
                    })));
                    break;
                }
                let reply = match ct_http::rpc::parse(&t) {
                    Ok(req) if web && !crate::web::web_allowed(&req.method) => ct_http::rpc::reply(
                        req.id,
                        Err((
                            crate::web::WEB_METHOD_UNAVAILABLE,
                            "método indisponível na web".into(),
                        )),
                    ),
                    Ok(req) => {
                        let r = match req.method.as_str() {
                            // Modo seguir do log (#65): por conexão.
                            "hub.logs.follow" | "hub.logs.unfollow" => {
                                logs_follow(&mut log_follow, &req.method, &req.params, &out)
                            }
                            _ => call(&st, &req.method, req.params, channel.web_session()).await,
                        };
                        ct_http::rpc::reply(req.id, r)
                    }
                    Err(reply) => reply,
                };
                let _ = out.send(Message::Text(reply.to_string().into()));
            }
            Message::Close(_) => break,
            _ => {}
        }
    }
    if let Some(f) = log_follow.take() {
        f.abort();
    }
    pusher.abort();
    drop(out);
    let _ = writer.await;
}

/// Push do modo seguir do log do hub (`hub.logs.follow`, #65).
pub const LOGS_PUSH: &str = "hub.logs.lines";

/// `hub.logs.follow { level?, q? }` e `hub.logs.unfollow`: liga ou desliga
/// o push [`LOGS_PUSH`] nesta conexão.
fn logs_follow(
    follow: &mut Option<tokio::task::JoinHandle<()>>,
    method: &str,
    p: &Value,
    out: &mpsc::UnboundedSender<Message>,
) -> RpcResult {
    let new = match method {
        "hub.logs.follow" => Some(ct_http::logs::spawn_follow(
            ct_http::logs::global(),
            p,
            out.clone(),
            LOGS_PUSH,
        )?),
        _ => None,
    };
    if let Some(old) = std::mem::replace(follow, new) {
        old.abort();
    }
    Ok(json!({}))
}

/// Parâmetros de `hub.env.call` (#343, spec da PWA §5.4).
#[derive(Debug, Clone, serde::Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct HubEnvCall {
    /// `env_id` do ambiente (`HubEnv.env_id`).
    pub env: String,
    /// Um de `ct_http::sync::CHANNEL_METHODS`.
    pub method: String,
    /// Parâmetros do método; ausente = `{}`.
    #[serde(default)]
    #[cfg_attr(
        feature = "ts",
        ts(type = "Record<string, unknown> | undefined", optional)
    )]
    pub params: Option<Value>,
}

/// `hub.env.call`: o método do ambiente pelo canal reverso, com a lista do
/// canal (`ct_http::sync::CHANNEL_METHODS`, spec §5.2) aplicada antes de
/// mandar. Ambiente sem canal: `-32030` com o nome dele e o motivo provável.
async fn env_call(st: &HubState, p: Value) -> RpcResult {
    let req: HubEnvCall = serde_json::from_value(p).map_err(|e| {
        invalid(format!(
            "hub.env.call espera {{ env, method, params }}: {e}"
        ))
    })?;
    let env = req.env.trim().to_string();
    if env.is_empty() {
        return Err(invalid("faltou env"));
    }
    let params = match req.params {
        None | Some(Value::Null) => json!({}),
        Some(v @ Value::Object(_)) => v,
        Some(_) => return Err(invalid("params precisa ser um objeto")),
    };
    if !ct_http::sync::channel_method_allowed(&req.method) {
        return Err((
            crate::channel::CHANNEL_METHOD_UNAVAILABLE,
            format!("método indisponível pelo hub: {}", req.method),
        ));
    }
    call_env(st, env, &req.method, params).await
}

/// Chamada ao ambiente `env` pelo canal reverso, com o erro de ambiente
/// desconhecido ou offline.
async fn call_env(st: &HubState, env: String, method: &str, params: Value) -> RpcResult {
    if !st.channels.is_online(&env) {
        let hub = st.hub.clone();
        let id = env.clone();
        let name = tokio::task::spawn_blocking(move || {
            hub.lock()
                .list_envs()
                .ok()
                .and_then(|v| v.into_iter().find(|e| e.env_id == id).map(|e| e.name))
        })
        .await
        .ok()
        .flatten();
        let Some(name) = name else {
            return Err(invalid(format!("ambiente desconhecido no hub: {env}")));
        };
        return Err((
            crate::channel::CHANNEL_ENV_OFFLINE,
            format!(
                "ambiente offline: {name} está sem canal com o hub agora (ctowerd parado, sem rede ou anterior ao canal reverso)"
            ),
        ));
    }
    st.channels.call(&env, method, params).await
}

/// Resposta de `hub.workitem.body` (#434): a descrição da issue lida agora
/// no rastreador, que o hub não grava.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct HubWorkItemBody {
    pub title: String,
    /// Markdown (GFM); `""` = issue sem descrição.
    pub body: String,
    /// Sempre `"markdown"`: a descrição do Jira já vem convertida.
    pub format: String,
    /// Link da issue no rastreador.
    pub url: String,
}

/// `hub.workitem.body { owner, repo, number }` (#434): passagem de leitura,
/// sem gravar nada. GitHub pela API, com o token do hub; Jira pelo ambiente
/// de sync do projeto (o hub não alcança o Jira), com a descrição convertida
/// para Markdown aqui, em memória.
async fn workitem_body(st: &HubState, p: Value) -> RpcResult {
    let (owner, repo) = (
        str_param(&p, "owner")?.to_string(),
        str_param(&p, "repo")?.to_string(),
    );
    let number = i64_param(&p, "number")?;
    if number <= 0 {
        return Err(invalid("number inválido"));
    }
    let hub = st.hub.clone();
    let (o, r) = (owner.clone(), repo.clone());
    let project = tokio::task::spawn_blocking(move || hub.lock().find_project(&o, &r))
        .await
        .map_err(|e| (-32603, e.to_string()))?
        .map_err(|e| (-32000, e.to_string()))?
        .ok_or_else(|| (-32000, format!("projeto {owner}/{repo} não está no hub")))?;
    let markdown = |title: &Value, body: String, url: String| {
        json!(HubWorkItemBody {
            title: title.as_str().unwrap_or("").to_string(),
            body,
            format: "markdown".into(),
            url,
        })
    };
    match project.tracker.kind() {
        ct_core::TrackerKind::Github => {
            let gh = st.github.clone().ok_or_else(|| {
                (
                    -32000,
                    "integração com o GitHub desativada no hub".to_string(),
                )
            })?;
            let path = format!(
                "/repos/{}/{}/issues/{number}",
                project.gh_owner, project.gh_repo
            );
            let v = tokio::task::spawn_blocking(move || gh.client().get_json(&path))
                .await
                .map_err(|e| (-32603, e.to_string()))?
                .map_err(|e| (-32000, format!("GitHub: {e}")))?
                .ok_or_else(|| (-32000, format!("issue #{number} não encontrada no GitHub")))?;
            let url = v
                .get("html_url")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| ct_core::item_url(&project.tracker_ref(), number));
            let body = v
                .get("body")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            Ok(markdown(&v["title"], body, url))
        }
        ct_core::TrackerKind::Jira => {
            let env = project.sync_env_id.clone().ok_or_else(|| {
                (
                    -32000,
                    "projeto Jira sem ambiente de sync: a descrição é lida por ele".to_string(),
                )
            })?;
            let params =
                json!({ "owner": project.gh_owner, "repo": project.gh_repo, "number": number });
            let v = call_env(st, env, "tracker.issue_body", params).await?;
            let body = ct_jira::description_markdown(v.get("description"));
            let url = v
                .get("url")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| ct_core::item_url(&project.tracker_ref(), number));
            Ok(markdown(&v["title"], body, url))
        }
    }
}

/// Resposta de `hub.web.sessions.revoke`.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct WebSessionsRevoked {
    /// Sessões revogadas agora (0 = já revogada ou inexistente).
    pub revoked: usize,
}

/// `hub.web.sessions.list|revoke` (#200), no `/ws` e no `/web/ws`.
/// `current` = a sessão web da própria conexão (só no `/web/ws`).
fn web_sessions_call(st: &HubState, method: &str, p: &Value, current: Option<&str>) -> RpcResult {
    let h = st.hub.lock();
    let e = |e: ct_ledger::LedgerError| (-32000, e.to_string());
    match method {
        "hub.web.sessions.list" => {
            let mut v = h.list_web_sessions(st.web.auth.idle_ms()).map_err(e)?;
            for s in &mut v {
                s.current = current == Some(s.id.as_str());
            }
            Ok(json!(v))
        }
        "hub.web.sessions.revoke" => {
            let revoked = if p.get("all").and_then(Value::as_bool) == Some(true) {
                h.revoke_all_web_sessions().map_err(e)?
            } else {
                let id = str_param(p, "id")?;
                usize::from(h.revoke_web_session(id.trim()).map_err(e)?)
            };
            tracing::info!(
                "{revoked} sessão(ões) web revogada(s) pelo {}",
                if current.is_some() { "/web/ws" } else { "/ws" }
            );
            Ok(json!(WebSessionsRevoked { revoked }))
        }
        _ => Err((-32601, format!("método desconhecido: {method}"))),
    }
}

/// Eventos de `hub.push.subscribe|update` (`events`), sem repetição.
fn push_events(p: &Value) -> Result<Option<Vec<ct_ledger::PushEvent>>, (i64, String)> {
    let Some(v) = p.get("events").filter(|v| !v.is_null()) else {
        return Ok(None);
    };
    let arr = v
        .as_array()
        .ok_or_else(|| invalid("events precisa ser uma lista"))?;
    let mut out = Vec::new();
    for e in arr {
        let name = e.as_str().unwrap_or("");
        let ev = ct_ledger::PushEvent::parse(name)
            .ok_or_else(|| invalid(format!("evento de push desconhecido: {name:?}")))?;
        if !out.contains(&ev) {
            out.push(ev);
        }
    }
    Ok(Some(out))
}

/// `hub.push.*` (#203, spec da PWA §4.2 e §7). `current` = a sessão web da
/// conexão: `hub.push.subscribe` só existe no `/web/ws`, porque a inscrição
/// fica presa à sessão web (revogá-la apaga a inscrição).
fn push_call(st: &HubState, method: &str, p: &Value, current: Option<&str>) -> RpcResult {
    let e = |e: ct_ledger::LedgerError| (-32000, e.to_string());
    let push = || {
        st.push.as_ref().ok_or_else(|| {
            (
                crate::push::PUSH_UNAVAILABLE,
                "Web Push desligada no hub ([web] enabled e [web.push] enabled)".to_string(),
            )
        })
    };
    match method {
        "hub.push.vapid_key" => Ok(json!(crate::push::PushVapidKey {
            key: push()?.vapid.public_key(),
        })),
        "hub.push.subscribe" => {
            let push = push()?;
            let Some(session) = current else {
                return Err(invalid(
                    "hub.push.subscribe só pelo /web/ws: a inscrição fica presa à sessão web",
                ));
            };
            let endpoint = str_param(p, "endpoint")?;
            let keys = p.get("keys").cloned().unwrap_or(Value::Null);
            let (p256dh, auth) = (str_param(&keys, "p256dh")?, str_param(&keys, "auth")?);
            push.validate(endpoint, p256dh, auth)
                .map_err(|m| invalid(&m))?;
            let sub = ct_ledger::NewPushSubscription {
                endpoint: endpoint.to_string(),
                p256dh: p256dh.to_string(),
                auth: auth.to_string(),
                label: p.get("label").and_then(Value::as_str).map(str::to_string),
                events: push_events(p)?,
            };
            let d = st.hub.lock().subscribe_push(session, &sub).map_err(e)?;
            tracing::info!("dispositivo de push {} inscrito ({})", d.id, d.service);
            Ok(json!(d))
        }
        "hub.push.list" => Ok(json!(st
            .hub
            .lock()
            .list_push_devices(current)
            .map_err(e)?)),
        "hub.push.update" => {
            let id = i64_param(p, "id")?;
            let events = push_events(p)?.ok_or_else(|| invalid("faltou events"))?;
            st.hub
                .lock()
                .update_push_events(id, &events, current)
                .map_err(e)?
                .map(|d| json!(d))
                .ok_or_else(|| (-32000, format!("dispositivo {id} não está inscrito")))
        }
        "hub.push.remove" => {
            let id = i64_param(p, "id")?;
            let removed = st.hub.lock().remove_push_subscription(id).map_err(e)?;
            Ok(json!(crate::push::PushRemoved { removed }))
        }
        "hub.push.test" => {
            let id = i64_param(p, "id")?;
            push()?
                .test_blocking(id)
                .map(|r| json!(r))
                .map_err(|m| (-32000, m))
        }
        _ => Err((-32601, format!("método desconhecido: {method}"))),
    }
}

async fn call(st: &HubState, method: &str, p: Value, web_session: Option<&str>) -> RpcResult {
    match method {
        m if m.starts_with("hub.web.") => {
            let (st, method) = (st.clone(), method.to_string());
            let current = web_session.map(str::to_string);
            tokio::task::spawn_blocking(move || {
                web_sessions_call(&st, &method, &p, current.as_deref())
            })
            .await
            .map_err(|e| (-32603, e.to_string()))?
        }
        "github.status" | "github.set_token" | "github.clear_token" => github(st, method, &p).await,
        "hub.env.call" => env_call(st, p).await,
        // Log do hub (#65): as últimas linhas, mascaradas.
        "hub.logs.tail" => ct_http::logs::rpc_tail(ct_http::logs::global(), &p).await,
        "hub.workitem.body" => workitem_body(st, p).await,
        m if m.starts_with("hub.update.") => update_call(st, method, &p).await,
        "hub.projects.add" | "hub.projects.update" => {
            let (st, method) = (st.clone(), method.to_string());
            tokio::task::spawn_blocking(move || project_write(&st, &method, &p))
                .await
                .map_err(|e| (-32603, e.to_string()))?
        }
        m if m.starts_with("hub.recovery.") => {
            let (st, method) = (st.clone(), method.to_string());
            tokio::task::spawn_blocking(move || recovery_call(&st, &method, p))
                .await
                .map_err(|e| (-32603, e.to_string()))?
        }
        m if m.starts_with("hub.secrets.") => {
            // Fora de `hub_call`: o `HubSecrets` pega o lock do store e
            // depois o do hub, que `hub_call` já seguraria.
            let (st, method) = (st.clone(), method.to_string());
            tokio::task::spawn_blocking(move || secrets_call(&st, &method, &p))
                .await
                .map_err(|e| (-32603, e.to_string()))?
        }
        m if m.starts_with("hub.push.") => {
            // `hub.push.test` vai à rede: fora do `hub_call`, que segura o hub.
            let (st, method) = (st.clone(), method.to_string());
            let current = web_session.map(str::to_string);
            tokio::task::spawn_blocking(move || push_call(&st, &method, &p, current.as_deref()))
                .await
                .map_err(|e| (-32603, e.to_string()))?
        }
        m if m.starts_with("hub.backup.") => {
            // Rede e store de secrets: fora do `hub_call`, que segura o hub.
            let (st, method) = (st.clone(), method.to_string());
            tokio::task::spawn_blocking(move || backup_call(&st, &method, &p))
                .await
                .map_err(|e| (-32603, e.to_string()))?
        }
        m if m.starts_with("hub.") => {
            let (st, method) = (st.clone(), method.to_string());
            tokio::task::spawn_blocking(move || hub_call(&st, &method, &p))
                .await
                .map_err(|e| (-32603, e.to_string()))?
        }
        _ => Err((-32601, format!("método desconhecido: {method}"))),
    }
}

/// `hub.update.get|check|start|rollback` (#421): o atualizador roda fora
/// do serviço; o andamento chega pelo push `hub.update.status`.
async fn update_call(st: &HubState, method: &str, p: &Value) -> RpcResult {
    use crate::update::StartError;
    let Some(u) = st.updates.clone() else {
        return Err(invalid("atualização pela UI desativada neste hub"));
    };
    match method {
        "hub.update.get" => Ok(json!(u.status())),
        "hub.update.check" => Ok(json!(u.check().await)),
        "hub.update.start" | "hub.update.rollback" => {
            let rollback = method == "hub.update.rollback";
            let force = bool_param(p, "force")?.unwrap_or(false);
            u.start(rollback, force)
                .await
                .map(|s| json!(s))
                .map_err(|e| match e {
                    StartError::NeedsForce(m) => (
                        UPDATE_ROLLBACK_DB,
                        format!("{m}. Confirme com force para seguir"),
                    ),
                    StartError::Other(m) => (-32000, m),
                })
        }
        _ => Err((-32601, format!("método desconhecido: {method}"))),
    }
}

/// Métodos `hub.*` (síncronos: rodam fora do runtime). As mudanças em
/// work items podem tornar propostas obsoletas: se o número de pendentes
/// mudou, as UIs recebem `hub.proposals.changed`.
fn hub_call(st: &HubState, method: &str, p: &Value) -> RpcResult {
    let watch = matches!(
        method,
        "hub.workitem.set_status"
            | "hub.workitem.set_priority"
            | "hub.workitem.move"
            | "hub.proposals.decide"
    );
    let pending = || st.hub.lock().pending_proposals().ok();
    let before = watch.then(pending).flatten();
    let r = hub_call_locked(st, method, p);
    if watch && pending() != before {
        st.proposals_changed();
    }
    let rules_write = matches!(
        method,
        "hub.rules.set" | "hub.rules.import" | "hub.rules.proposals.decide"
    );
    if rules_write && r.is_ok() {
        st.rules_changed();
    }
    r
}

/// Escopo das regras do agente em `hub.rules.*`: `owner`, `repo` e
/// `epic_number` (ausente ou 0 = projeto).
fn rules_scope(p: &Value) -> Result<(&str, &str, i64), (i64, String)> {
    let epic = match p.get("epic_number") {
        None | Some(Value::Null) => 0,
        Some(v) => v
            .as_i64()
            .filter(|n| *n >= 0)
            .ok_or_else(|| invalid("epic_number deve ser 0 (projeto) ou o número do épico"))?,
    };
    Ok((str_param(p, "owner")?, str_param(p, "repo")?, epic))
}

fn hub_call_locked(st: &HubState, method: &str, p: &Value) -> RpcResult {
    let h = st.hub.lock();
    let e = |e: ct_ledger::LedgerError| (-32000, e.to_string());
    match method {
        "hub.info" => Ok(json!(HubInfo {
            protocol: crate::PROTOCOL_VERSION.into(),
            version: ct_core::VERSION.into(),
            commit: ct_core::BUILD_COMMIT.into(),
            name: st.name.to_string(),
            config_rev: h.config_rev().map_err(e)?,
            backup: st.backups.status(),
            docs_dir: st.docs_dir.as_deref().map(str::to_string),
        })),
        "hub.status" => {
            let quarantine = h.quarantine_summary().map_err(e)?;
            let mut warnings = st.backups.warnings();
            warnings.extend(quarantine_warnings(&quarantine));
            // Só os locks de `recovery` e do estado dos snapshots do
            // HubSecrets: pode com o hub pego.
            if let Some(s) = &st.secrets {
                warnings.extend(s.recovery_warnings());
                // O envio remoto em dia já leva o `secrets.age` para fora
                // da máquina: mesmo disco deixa de ser aviso, como no hub.db.
                let remote_fresh = st.backups.remote_fresh();
                warnings.extend(
                    s.snapshot_warnings()
                        .into_iter()
                        .filter(|w| !(remote_fresh && w.kind == "secrets_snapshot_same_device")),
                );
                warnings.extend(s.rotate_pending_warnings());
            }
            Ok(json!(HubStatus {
                warnings,
                backup: st.backups.status(),
                quarantine,
            }))
        }
        // O impacto da exclusão antes de confirmar (#328): o mesmo que
        // `hub.projects.delete` sem `confirm` usa para recusar.
        "hub.projects.impact" => {
            let (owner, repo) = (str_param(p, "owner")?, str_param(p, "repo")?);
            h.delete_impact(owner, repo)
                .map_err(e)?
                .map(|v| json!(v))
                .ok_or_else(|| (-32000, format!("projeto {owner}/{repo} não está no hub")))
        }
        "hub.projects.delete" => {
            let (owner, repo) = (str_param(p, "owner")?, str_param(p, "repo")?);
            let confirm = p.get("confirm").and_then(Value::as_bool).unwrap_or(false);
            let impact = h
                .delete_impact(owner, repo)
                .map_err(e)?
                .ok_or_else(|| (-32000, format!("projeto {owner}/{repo} não está no hub")))?;
            // Atribuições ativas ou sessões vivas relatadas (de qualquer
            // ambiente): sem `confirm`, recusa e lista o que seria atingido.
            if (!impact.assignments.is_empty() || !impact.live_sessions.is_empty()) && !confirm {
                let envs = if impact.assignments.is_empty() {
                    "não está atribuído a nenhum ambiente".to_string()
                } else {
                    format!(
                        "está atribuído a {} ambiente(s): {}",
                        impact.assignments.len(),
                        impact
                            .assignments
                            .iter()
                            .map(|a| a.env_name.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                };
                let names = h.env_names().map_err(e)?;
                return Err((
                    -32000,
                    format!(
                        "o projeto {owner}/{repo} {envs}; {}. A exclusão apaga o histórico do \
                         projeto no hub; repita com confirm: true para excluir",
                        sessions_text(&impact.live_sessions, &names)
                    ),
                ));
            }
            let dir = st.backups.dir();
            if dir.as_os_str().is_empty() {
                return Err((
                    -32000,
                    "sem backup_dir para o snapshot antes da exclusão".into(),
                ));
            }
            let snapshot = h
                .snapshot_pre_delete(&dir, ct_core::now_ms())
                .map_err(|err| (-32000, format!("snapshot antes da exclusão falhou: {err}")))?;
            let rev = h.delete_project(owner, repo).map_err(e)?;
            tracing::warn!(
                "projeto {owner}/{repo} excluído do hub (config_rev {rev}); snapshot prévio em {}",
                snapshot.display()
            );
            Ok(json!(ProjectDeleted {
                rev,
                snapshot: snapshot.display().to_string(),
                assignments: impact.assignments,
                live_sessions: impact.live_sessions,
            }))
        }
        "hub.assignments.list" => {
            let f = AssignmentFilter {
                env: opt_str(p, "env"),
                owner: opt_str(p, "owner"),
                repo: opt_str(p, "repo"),
            };
            Ok(json!(h.list_assignments(&f).map_err(e)?))
        }
        "hub.assignments.add" => {
            let (owner, repo) = (str_param(p, "owner")?, str_param(p, "repo")?);
            let env = str_param(p, "env")?;
            let repo_path = p.get("repo_path").and_then(Value::as_str);
            let added = h.add_assignment(owner, repo, env, repo_path).map_err(e)?;
            if added.changed {
                tracing::info!(
                    "{owner}/{repo} atribuído a {} (config_rev {})",
                    added.assignment.env_name,
                    added.assignment.rev
                );
            }
            Ok(json!(added))
        }
        "hub.assignments.remove" => {
            let (owner, repo) = (str_param(p, "owner")?, str_param(p, "repo")?);
            let env = str_param(p, "env")?;
            let confirm = p.get("confirm").and_then(Value::as_bool).unwrap_or(false);
            let (a, live) = h.assignment_impact(owner, repo, env).map_err(e)?;
            if !live.is_empty() && !confirm {
                let names = h.env_names().map_err(e)?;
                return Err((
                    -32000,
                    format!(
                        "{owner}/{repo} tem {}. O ambiente {} não encerra as sessões: elas \
                         seguem até terminarem; repita com confirm: true para remover a \
                         atribuição",
                        sessions_text(&live, &names),
                        a.env_name
                    ),
                ));
            }
            let rev = h.remove_assignment(owner, repo, env).map_err(e)?;
            tracing::info!(
                "atribuição de {owner}/{repo} a {} removida (config_rev {rev})",
                a.env_name
            );
            Ok(json!(AssignmentRemoved {
                rev,
                assignment: a,
                live_sessions: live,
            }))
        }
        "hub.quarantine.list" => Ok(json!(h.quarantine_summary().map_err(e)?)),
        "hub.quarantine.discard" => {
            let (owner, repo) = (str_param(p, "owner")?, str_param(p, "repo")?);
            let confirm = p.get("confirm").and_then(Value::as_bool).unwrap_or(false);
            let n = h.quarantine_count(owner, repo).map_err(e)?;
            if n > 0 && !confirm {
                return Err((
                    -32000,
                    format!(
                        "{n} linha(s) de push de {owner}/{repo} em quarentena; repita com \
                         confirm: true para descartá-las (não há como recuperá-las depois, \
                         exceto pelo ctower hub resync do ambiente)"
                    ),
                ));
            }
            let discarded = h.discard_quarantine(owner, repo).map_err(e)?;
            if discarded > 0 {
                tracing::warn!("quarentena de {owner}/{repo} descartada: {discarded} linha(s)");
            }
            Ok(json!(QuarantineDiscarded { discarded }))
        }
        "hub.projects.list" => Ok(json!(h.project_views().map_err(e)?)),
        "hub.tree.get" => {
            let (owner, repo) = (str_param(p, "owner")?, str_param(p, "repo")?);
            h.tree(owner, repo)
                .map_err(e)?
                .map(|t| json!(t))
                .ok_or_else(|| (-32000, format!("projeto {owner}/{repo} não está no hub")))
        }
        "hub.workitem.history" => {
            let (owner, repo) = (str_param(p, "owner")?, str_param(p, "repo")?);
            let number = i64_param(p, "number")?;
            let limit = p.get("limit").and_then(Value::as_u64).unwrap_or(100) as u32;
            let before = p.get("before_hlc").and_then(Value::as_i64);
            // Épico: decisões e PRs das sub-issues junto (#328).
            let subs = p.get("include_sub_issues").and_then(Value::as_bool) == Some(true);
            h.history(owner, repo, number, limit, before, subs)
                .map_err(e)?
                .map(|v| json!(v))
                .ok_or_else(|| (-32000, format!("{owner}/{repo}#{number} não está no hub")))
        }
        "hub.workitem.set_status" => {
            let (owner, repo) = (str_param(p, "owner")?, str_param(p, "repo")?);
            let number = i64_param(p, "number")?;
            let status: ct_core::WorkItemStatus = str_param(p, "status")?
                .parse()
                .map_err(|_| invalid("status inválido"))?;
            let reason = p.get("reason").and_then(Value::as_str);
            h.set_status_manual(owner, repo, number, status, reason, &st.hub_id)
                .map(|w| json!(w))
                .map_err(e)
        }
        "hub.workitem.set_priority" => {
            let (owner, repo) = (str_param(p, "owner")?, str_param(p, "repo")?);
            let number = i64_param(p, "number")?;
            let priority = match p.get("priority") {
                Some(Value::Null) => None,
                Some(Value::String(s)) => Some(
                    s.parse::<ct_core::Priority>()
                        .map_err(|_| invalid("priority deve ser P0, P1, P2, P3 ou null"))?,
                ),
                _ => {
                    return Err(invalid(
                        "parâmetro obrigatório: priority (P0, P1, P2, P3 ou null)",
                    ))
                }
            };
            let reason = p.get("reason").and_then(Value::as_str);
            h.set_priority(owner, repo, number, priority, reason, None, &st.hub_id)
                .map(|w| json!(w))
                .map_err(e)
        }
        "hub.workitem.move" => {
            let (owner, repo) = (str_param(p, "owner")?, str_param(p, "repo")?);
            let number = i64_param(p, "number")?;
            let num = |k: &str| match p.get(k) {
                None | Some(Value::Null) => Ok(None),
                Some(v) => v
                    .as_i64()
                    .map(Some)
                    .ok_or_else(|| invalid(format!("{k} deve ser o número de um item"))),
            };
            let to = placement(num("before")?, num("after")?)?;
            h.move_work_item(owner, repo, number, to, None, &st.hub_id)
                .map(|w| json!(w))
                .map_err(e)
        }
        "hub.projects.move" => {
            let (owner, repo) = (str_param(p, "owner")?, str_param(p, "repo")?);
            let proj = |k: &str| match p.get(k) {
                None | Some(Value::Null) => Ok(None),
                Some(v) => match (str_param(v, "owner"), str_param(v, "repo")) {
                    (Ok(o), Ok(r)) => Ok(Some((o, r))),
                    _ => Err(invalid(format!("{k} deve ser {{ owner, repo }}"))),
                },
            };
            let to = placement(proj("before")?, proj("after")?)?;
            h.move_project(owner, repo, to, &st.hub_id)
                .map(|p| json!(p))
                .map_err(e)
        }
        "hub.ranking.get" => {
            let project = match (opt_str(p, "owner"), opt_str(p, "repo")) {
                (Some(o), Some(r)) => Some((o, r)),
                (None, None) => None,
                _ => return Err(invalid("owner e repo vão juntos")),
            };
            let priority = opt_str(p, "priority")
                .map(|s| s.parse::<ct_core::Priority>())
                .transpose()
                .map_err(|_| invalid("priority deve ser P0, P1, P2 ou P3"))?;
            let limit = p
                .get("limit")
                .and_then(Value::as_u64)
                .map_or(ct_ledger::RANKING_LIMIT, |l| l.min(u32::MAX as u64) as u32);
            let f = RankingFilter { project, priority };
            Ok(json!(h.ranking(&f, limit).map_err(e)?))
        }
        "hub.health.get" => {
            let project = match (opt_str(p, "owner"), opt_str(p, "repo")) {
                (Some(o), Some(r)) => Some((o, r)),
                (None, None) => None,
                _ => return Err(invalid("owner e repo vão juntos")),
            };
            let min_severity = opt_str(p, "min_severity")
                .map(|s| s.parse::<ct_ledger::Severity>())
                .transpose()
                .map_err(invalid)?
                .unwrap_or(ct_ledger::Severity::Info);
            let f = ct_ledger::HealthFilter {
                project,
                min_severity,
            };
            Ok(json!(h
                .health(&st.health, st.gh_sync_interval_secs, &f, ct_core::now_ms())
                .map_err(e)?))
        }
        "hub.proposals.list" => {
            let state = crate::agent::proposal_state(p.get("state").and_then(Value::as_str))
                .map_err(invalid)?;
            let project = match (opt_str(p, "owner"), opt_str(p, "repo")) {
                (Some(o), Some(r)) => Some((o, r)),
                (None, None) => None,
                _ => return Err(invalid("owner e repo vão juntos")),
            };
            let f = ct_ledger::ProposalFilter {
                state,
                project,
                number: p.get("number").and_then(Value::as_i64),
            };
            let limit = p
                .get("limit")
                .and_then(Value::as_u64)
                .map_or(ct_ledger::RANKING_LIMIT, |l| l.min(u32::MAX as u64) as u32);
            Ok(json!(h
                .list_proposals(&f, limit, ct_core::now_ms())
                .map_err(e)?))
        }
        "hub.proposals.decide" => {
            let approve = p
                .get("approve")
                .and_then(Value::as_bool)
                .ok_or_else(|| invalid("parâmetro obrigatório: approve (true ou false)"))?;
            let note = p.get("note").and_then(Value::as_str);
            let decide =
                |id: i64| h.decide_proposal(id, approve, note, &st.hub_id, ct_core::now_ms());
            match (p.get("id").and_then(Value::as_i64), p.get("ids")) {
                (Some(id), None) => decide(id).map(|v| json!(v)).map_err(e),
                // Em lote: cada uma na sua transação; uma recusada não
                // impede as outras.
                (None, Some(ids)) => {
                    let ids: Vec<i64> = serde_json::from_value(ids.clone())
                        .map_err(|_| invalid("ids deve ser uma lista de números"))?;
                    if ids.is_empty() || ids.len() > ct_ledger::PROPOSAL_PENDING_MAX as usize {
                        return Err(invalid(format!(
                            "ids deve ter de 1 a {} propostas",
                            ct_ledger::PROPOSAL_PENDING_MAX
                        )));
                    }
                    let out: Vec<ct_ledger::ProposalDecided> = ids
                        .into_iter()
                        .map(|id| match decide(id) {
                            Ok(v) => ct_ledger::ProposalDecided {
                                id,
                                proposal: Some(v),
                                error: None,
                            },
                            Err(err) => ct_ledger::ProposalDecided {
                                id,
                                proposal: None,
                                error: Some(err.to_string()),
                            },
                        })
                        .collect();
                    Ok(json!(out))
                }
                _ => Err(invalid("use id ou ids (não os dois)")),
            }
        }
        "hub.dashboard.get" => {
            let mut d = h.dashboard().map_err(e)?;
            st.channels.mark_online(&mut d.envs);
            Ok(json!(d))
        }
        "hub.envs.list" => {
            let mut envs = h.list_envs().map_err(e)?;
            st.channels.mark_online(&mut envs);
            Ok(json!(envs))
        }
        "hub.rules.get" => {
            let (owner, repo, epic) = rules_scope(p)?;
            Ok(json!(h.rules(owner, repo, epic).map_err(e)?))
        }
        "hub.rules.set" | "hub.rules.import" => {
            let (owner, repo, epic) = rules_scope(p)?;
            let content = p
                .get("content")
                .and_then(Value::as_str)
                .ok_or_else(|| invalid("parâmetro obrigatório: content"))?;
            let source = if method == "hub.rules.import" {
                ct_core::RuleSource::Importacao
            } else {
                ct_core::RuleSource::Usuario
            };
            let base = p.get("base_version").and_then(Value::as_i64);
            let note = p.get("note").and_then(Value::as_str);
            h.set_rules(
                owner,
                repo,
                epic,
                content,
                source,
                base,
                note,
                ct_core::now_ms(),
            )
            .map(|v| json!(v))
            .map_err(e)
        }
        "hub.rules.history" => {
            let (owner, repo, epic) = rules_scope(p)?;
            let limit = p.get("limit").and_then(Value::as_u64).unwrap_or(50) as u32;
            Ok(json!(h
                .rules_history(owner, repo, epic, limit)
                .map_err(e)?))
        }
        "hub.rules.proposals.list" => {
            let state = crate::agent::proposal_state(p.get("state").and_then(Value::as_str))
                .map_err(invalid)?;
            let project = match (opt_str(p, "owner"), opt_str(p, "repo")) {
                (Some(o), Some(r)) => Some((o, r)),
                (None, None) => None,
                _ => return Err(invalid("owner e repo vão juntos")),
            };
            let f = ct_ledger::RulesProposalFilter {
                state,
                project,
                epic_number: p.get("epic_number").and_then(Value::as_i64),
            };
            let limit = p.get("limit").and_then(Value::as_u64).unwrap_or(100) as u32;
            Ok(json!(h.list_rules_proposals(&f, limit).map_err(e)?))
        }
        "hub.rules.proposals.decide" => {
            let id = i64_param(p, "id")?;
            let approve = p
                .get("approve")
                .and_then(Value::as_bool)
                .ok_or_else(|| invalid("parâmetro obrigatório: approve (true ou false)"))?;
            let note = p.get("note").and_then(Value::as_str);
            h.decide_rules_proposal(id, approve, note, ct_core::now_ms())
                .map(|v| json!(v))
                .map_err(e)
        }
        _ => Err((-32601, format!("método desconhecido: {method}"))),
    }
}

/// `before`/`after` de `hub.workitem.move` e `hub.projects.move`: no máximo
/// um; nenhum = fim.
fn placement<T>(before: Option<T>, after: Option<T>) -> Result<Placement<T>, (i64, String)> {
    match (before, after) {
        (Some(_), Some(_)) => Err(invalid("use before ou after, não os dois")),
        (Some(b), None) => Ok(Placement::Before(b)),
        (None, Some(a)) => Ok(Placement::After(a)),
        (None, None) => Ok(Placement::End),
    }
}

/// "sessões vivas relatadas: ..." para as mensagens de confirmação.
fn sessions_text(
    live: &[ct_ledger::HubSession],
    names: &std::collections::HashMap<String, String>,
) -> String {
    if live.is_empty() {
        return "nenhuma sessão viva relatada".into();
    }
    let list = live
        .iter()
        .map(|s| {
            let env = names.get(&s.env_id).unwrap_or(&s.env_id);
            format!("{} em {env} ({})", s.id, s.state.as_str())
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!("sessões vivas relatadas: {list}")
}

/// Texto não vazio, aparado.
fn opt_str(p: &Value, k: &str) -> Option<String> {
    p.get(k)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
}

fn protected_param(p: &Value) -> Result<Option<Vec<String>>, (i64, String)> {
    match p.get("protected_branches") {
        None | Some(Value::Null) => Ok(None),
        Some(v) => serde_json::from_value(v.clone())
            .map(Some)
            .map_err(|_| invalid("protected_branches deve ser lista de strings")),
    }
}

/// `tracker` e `forge` do cadastro (#348), com as normalizações: URL do Jira
/// sem barra final, chave em maiúsculas, mapa de tipos padrão quando vazio,
/// e o repositório do GitLab pela URL de clone (`clone_url`, ou `url` do
/// cadastro) ou por `host` + `path`.
fn provider_param(
    p: &Value,
) -> Result<(ct_core::TrackerConfig, ct_core::ForgeConfig), (i64, String)> {
    use ct_core::{ForgeConfig, TrackerConfig};
    let tracker: TrackerConfig = match p.get("tracker") {
        None | Some(Value::Null) => TrackerConfig::Github,
        Some(v) => serde_json::from_value(v.clone())
            .map_err(|e| invalid(format!("tracker inválido: {e}")))?,
    };
    let forge: ForgeConfig = match p.get("forge") {
        None | Some(Value::Null) => ForgeConfig::Github,
        Some(v) => serde_json::from_value(v.clone())
            .map_err(|e| invalid(format!("forge inválido: {e}")))?,
    };
    let tracker = match tracker {
        TrackerConfig::Jira(mut j) => {
            j.url = j.url.trim().trim_end_matches('/').to_string();
            j.project_key = j.project_key.trim().to_ascii_uppercase();
            j.jql = j.jql.trim().to_string();
            j.credential_cmd = j.credential_cmd.trim().to_string();
            j.config_file = j.config_file.trim().to_string();
            j.epic_link_field = j.epic_link_field.trim().to_string();
            j.epic_name_field = j.epic_name_field.trim().to_string();
            if j.type_map.is_empty() {
                j.type_map = ct_core::default_type_map();
            }
            TrackerConfig::Jira(j)
        }
        t => t,
    };
    let forge = match forge {
        ForgeConfig::Gitlab(mut g) => {
            let url = Some(g.clone_url.trim())
                .filter(|u| !u.is_empty())
                .or_else(|| p.get("url").and_then(Value::as_str).map(str::trim))
                .filter(|u| !u.is_empty())
                .map(String::from);
            match url {
                Some(url) => {
                    let r = ct_core::repo_from_url(&url)
                        .ok_or_else(|| invalid(format!("URL do GitLab não reconhecida: {url}")))?;
                    let given =
                        |a: &str, b: &str| a.trim().is_empty() || a.trim().eq_ignore_ascii_case(b);
                    if !given(&g.host, &r.host) || !given(g.path.trim_matches('/'), &r.path) {
                        return Err(invalid(format!(
                            "host e caminho do GitLab ({}/{}) não batem com a URL {url}",
                            g.host, g.path
                        )));
                    }
                    g.host = r.host;
                    g.path = r.path;
                    g.clone_url = url;
                }
                None => {
                    let host = g.host.trim().to_ascii_lowercase();
                    let path = g.path.trim().trim_matches('/').trim_end_matches(".git");
                    let r = ct_core::repo_from_url(&format!("https://{host}/{path}")).ok_or_else(
                        || {
                            invalid(format!(
                                "informe a URL de clone do GitLab, ou host e caminho válidos \
                                 (host {host:?}, caminho {path:?})"
                            ))
                        },
                    )?;
                    g.clone_url = format!("https://{}/{}.git", r.host, r.path);
                    g.host = r.host;
                    g.path = r.path;
                }
            }
            g.credential_cmd = g.credential_cmd.trim().to_string();
            ForgeConfig::Gitlab(g)
        }
        f => f,
    };
    Ok((tracker, forge))
}

/// `branch_pattern`: texto aparado; vazio = o padrão do rastreador.
fn pattern_param(p: &Value) -> Result<Option<String>, (i64, String)> {
    match p.get("branch_pattern") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.trim().to_string())),
        Some(_) => Err(invalid("branch_pattern deve ser texto")),
    }
}

/// `docs` (#440): `{ enabled, project_dirs, epic_dirs }`, campos ausentes =
/// padrão; o objeto inteiro substitui o anterior.
fn docs_param(p: &Value) -> Result<Option<ct_core::DocsConfig>, (i64, String)> {
    match p.get("docs") {
        None | Some(Value::Null) => Ok(None),
        Some(v @ Value::Object(_)) => serde_json::from_value(v.clone())
            .map(Some)
            .map_err(|e| invalid(format!("docs inválido: {e}"))),
        Some(_) => Err(invalid(
            "docs deve ser um objeto { enabled, project_dirs, epic_dirs }",
        )),
    }
}

fn bool_param(p: &Value, k: &str) -> Result<Option<bool>, (i64, String)> {
    match p.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(b)) => Ok(Some(*b)),
        Some(_) => Err(invalid(format!("{k} deve ser true ou false"))),
    }
}

/// `hub.projects.add` e `hub.projects.update`. A validação no GitHub roda
/// sem o lock do `hub.db`. Síncrono: roda fora do runtime.
///
/// Projeto Jira + GitLab (#348): o hub não alcança esses servidores e não
/// usa o token do GitHub. O cadastro confere só a forma (combinação,
/// campos e padrão de branch); o ambiente de sync confere o resto no
/// primeiro ciclo e relata pelo `tracker_status`.
fn project_write(st: &HubState, method: &str, p: &Value) -> RpcResult {
    let github = || {
        st.github.clone().ok_or_else(|| {
            (
                -32000,
                "integração com o GitHub desativada: o hub não consegue validar o repositório"
                    .to_string(),
            )
        })
    };
    let e = |e: ct_ledger::LedgerError| (-32000, e.to_string());
    let protected = protected_param(p)?;
    if method == "hub.projects.add" {
        let (tracker, forge) = provider_param(p)?;
        let provider = ProjectProvider {
            tracker,
            forge,
            branch_pattern: pattern_param(p)?.filter(|s| !s.is_empty()),
            attribution: bool_param(p, "attribution")?.unwrap_or(true),
            docs: docs_param(p)?.unwrap_or_default().normalized(),
        };
        provider.validate().map_err(invalid)?;
        let base_branch = opt_str(p, "base_branch").unwrap_or_else(|| "develop".into());
        let (owner, repo) = match provider.forge.gitlab() {
            Some(g) => {
                let (o, r) = g.path.rsplit_once('/').expect("caminho validado");
                (o.to_string(), r.to_string())
            }
            None => {
                let url = str_param(p, "url")?;
                ct_core::github_repo_from_url(url)
                    .ok_or_else(|| invalid(format!("URL do GitHub não reconhecida: {url}")))?
            }
        };
        if let Some(prev) = st.hub.lock().find_project(&owner, &repo).map_err(e)? {
            return Err(invalid(format!(
                "{owner}/{repo} já está cadastrado no hub como {}",
                prev.name
            )));
        }
        let (owner, repo) = if provider.forge.gitlab().is_some() {
            (owner, repo)
        } else {
            let gh = github()?;
            let (owner, repo) = gh.validate_repo(&owner, &repo).map_err(|m| (-32000, m))?;
            gh.validate_branch(&owner, &repo, &base_branch)
                .map_err(|m| (-32000, m))?;
            (owner, repo)
        };
        let np = NewHubProject {
            name: opt_str(p, "name").unwrap_or_else(|| repo.clone()),
            owner,
            repo,
            base_branch,
            protected_branches: protected
                .unwrap_or_else(|| vec!["develop".into(), "main".into(), "master".into()]),
        };
        let added = st.hub.lock().add_project_with(&np, &provider).map_err(e)?;
        tracing::info!(
            "projeto {}/{} ({} + {}) cadastrado no hub (config_rev {}, {} linha(s) da quarentena reaplicada(s))",
            np.owner,
            np.repo,
            provider.tracker.kind(),
            provider.forge.kind(),
            added.rev,
            added.reapplied
        );
        return Ok(json!(added));
    }
    let (owner, repo) = (str_param(p, "owner")?, str_param(p, "repo")?);
    let (tracker, forge) = (p.get("tracker").is_some(), p.get("forge").is_some());
    let provider = (tracker || forge).then(|| provider_param(p)).transpose()?;
    let patch = HubProjectPatch {
        name: opt_str(p, "name"),
        base_branch: opt_str(p, "base_branch"),
        protected_branches: protected,
        tracker: provider
            .as_ref()
            .filter(|_| tracker)
            .map(|(t, _)| t.clone()),
        forge: provider.as_ref().filter(|_| forge).map(|(_, f)| f.clone()),
        branch_pattern: pattern_param(p)?,
        attribution: bool_param(p, "attribution")?,
        sync_env: opt_str(p, "sync_env"),
        docs: docs_param(p)?,
    };
    if patch.is_empty() {
        return Err(invalid(
            "nada a mudar: informe name, base_branch, protected_branches, tracker, forge, \
             branch_pattern, attribution, sync_env ou docs",
        ));
    }
    let cur = st
        .hub
        .lock()
        .find_project(owner, repo)
        .map_err(e)?
        .ok_or_else(|| (-32000, format!("projeto {owner}/{repo} não está no hub")))?;
    // Só o GitHub o hub alcança: a base do projeto Jira + GitLab o ambiente
    // de sync confere.
    if let Some(b) = patch
        .base_branch
        .as_deref()
        .filter(|b| *b != cur.base_branch && cur.forge.gitlab().is_none())
    {
        github()?
            .validate_branch(&cur.gh_owner, &cur.gh_repo, b)
            .map_err(|m| (-32000, m))?;
    }
    let updated = st
        .hub
        .lock()
        .update_project(owner, repo, &patch)
        .map_err(e)?;
    if let Some(env) = &patch.sync_env {
        tracing::info!(
            "ambiente de sync de {owner}/{repo}: {env} (config_rev {})",
            st.hub.lock().config_rev().map_err(e)?
        );
    }
    Ok(json!(updated))
}

/// `scope`: `"global"` ou `{"project": "owner/repo"}` (projeto em
/// minúsculas).
fn scope_param(p: &Value) -> Result<Scope, (i64, String)> {
    let bad = || invalid(r#"scope deve ser "global" ou {"project": "owner/repo"}"#);
    match p.get("scope") {
        Some(Value::String(s)) if s == "global" => Ok(Scope::Global),
        Some(Value::Object(o)) if o.len() == 1 => {
            let key = o.get("project").and_then(Value::as_str).ok_or_else(bad)?;
            Scope::parse_project(key).map_err(|e| invalid(e.to_string()))
        }
        None => Err(invalid("parâmetro obrigatório: scope")),
        _ => Err(bad()),
    }
}

fn scope_text(s: &Scope) -> String {
    match s {
        Scope::Global => "global".into(),
        Scope::Project(k) => k.clone(),
    }
}

/// Erro do store para o `/ws`: nome inválido é parâmetro inválido; o resto,
/// erro do hub. As mensagens do `ct-secrets` nunca levam valores.
fn secrets_err(e: HubSecretsError) -> (i64, String) {
    match e {
        HubSecretsError::Secrets(
            e @ (ct_secrets::SecretsError::InvalidName(_)
            | ct_secrets::SecretsError::InvalidProject(_)),
        ) => invalid(e.to_string()),
        e @ HubSecretsError::InvalidRecoveryKey(_) => invalid(e.to_string()),
        e @ HubSecretsError::RecoveryNotStarted => (RECOVERY_NOT_STARTED, e.to_string()),
        e @ HubSecretsError::RecoveryKeyMismatch(_) => (RECOVERY_KEY_MISMATCH, e.to_string()),
        e => (-32000, e.to_string()),
    }
}

/// Resposta de `hub.backup.get`: a configuração (sem credenciais) e o estado.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct BackupConfigView {
    pub remote: crate::remote::RemoteSettings,
    pub status: BackupStatus,
    /// Campos de credencial que o destino atual pede.
    pub credential_fields: Vec<String>,
    /// Backup dos docs (#442), separado do `hub.db`: agenda, retenção,
    /// limite e o resultado da última rodada e do último envio.
    pub docs: crate::backup::DocsBackupStatus,
}

/// `hub.backup.*` (síncronos). Credenciais entram em `credentials`
/// (`campo → valor`), vão direto ao store e nunca voltam na resposta.
fn backup_call(st: &HubState, method: &str, p: &Value) -> RpcResult {
    use crate::backup::BackupUpdate;
    use crate::remote::RemoteKind;
    let b = &st.backups;
    let creds = || -> std::collections::BTreeMap<String, String> {
        p.get("credentials")
            .and_then(Value::as_object)
            .map(|o| {
                o.iter()
                    .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string())))
                    .collect()
            })
            .unwrap_or_default()
    };
    let remote_param = || -> Result<Option<crate::remote::RemoteSettings>, (i64, String)> {
        match p.get("remote") {
            None | Some(Value::Null) => Ok(None),
            Some(v) => serde_json::from_value(v.clone())
                .map(Some)
                .map_err(|e| invalid(format!("remote inválido: {e}"))),
        }
    };
    let view = || {
        let remote = b.settings_view();
        json!(BackupConfigView {
            credential_fields: crate::remote::credential_fields(remote.kind)
                .iter()
                .map(|s| s.to_string())
                .collect(),
            remote,
            status: b.status(),
            docs: b.docs_status(),
        })
    };
    match method {
        "hub.backup.get" => Ok(view()),
        "hub.backup.set" => {
            let remote = remote_param()?;
            let kind = remote
                .as_ref()
                .map_or_else(|| b.settings_view().kind, |r| r.kind);
            // Credenciais antes: a validação do destino as exige gravadas.
            if kind != RemoteKind::None {
                b.set_credentials(kind, &creds()).map_err(|e| (-32000, e))?;
            }
            let u = BackupUpdate {
                dir: opt_str(p, "dir"),
                interval_hours: p.get("interval_hours").and_then(Value::as_u64),
                keep: p.get("keep").and_then(Value::as_u64).map(|k| k as usize),
                remote,
                docs_interval_hours: p.get("docs_interval_hours").and_then(Value::as_u64),
                docs_keep: p
                    .get("docs_keep")
                    .and_then(Value::as_u64)
                    .map(|k| k as usize),
                docs_max_mb: p.get("docs_max_mb").and_then(Value::as_u64),
            };
            b.update(&u).map_err(invalid)?;
            tracing::info!("configuração do backup atualizada pela UI");
            Ok(view())
        }
        "hub.backup.test" => {
            let r = remote_param()?.unwrap_or_else(|| b.settings_view());
            if r.kind == RemoteKind::None {
                return Err(invalid("escolha um destino remoto para testar"));
            }
            let c = b.merged_credentials(r.kind, &creds());
            match crate::backup::Backups::test_remote(&r, &c) {
                Ok(()) => Ok(json!({ "ok": true })),
                Err(e) => Ok(json!({ "ok": false, "error": e })),
            }
        }
        // `what`: `all` (padrão), `hub` (só o hub.db) ou `docs` (só o pacote
        // dos docs, #442). A falha dos docs fica no estado (`docs`), não no
        // erro: o snapshot do hub.db não depende dela.
        "hub.backup.run" => {
            let what = p.get("what").and_then(Value::as_str).unwrap_or("all");
            if !matches!(what, "all" | "hub" | "docs") {
                return Err(invalid(format!("what inválido: {what} (all, hub ou docs)")));
            }
            if what != "hub" {
                if let Err(e) = b.run_docs_once() {
                    tracing::warn!("hub: backup dos docs falhou: {e}");
                }
            }
            if what != "docs" {
                b.run_once(&st.hub).map_err(|e| (-32000, e))?;
            }
            Ok(view())
        }
        "hub.backup.remote.list" => {
            let r = b.settings_view();
            if r.kind == RemoteKind::None {
                return Err(invalid("sem destino remoto configurado"));
            }
            let store = crate::remote::open(&r, &b.credentials(r.kind)).map_err(|e| (-32000, e))?;
            let objs = store.list().map_err(|e| (-32000, e))?;
            Ok(json!(crate::remote::hub_snapshots(objs)))
        }
        _ => Err((-32601, format!("método desconhecido: {method}"))),
    }
}

/// `hub.recovery.*` (síncronos: rodam fora do runtime). A chave colada em
/// `verify` sai de `p` para um [`Zeroizing`], sem cópia, e não entra em
/// log nem erro.
fn recovery_call(st: &HubState, method: &str, mut p: Value) -> RpcResult {
    let secrets = st.secrets.clone().ok_or_else(|| {
        (
            SECRETS_UNAVAILABLE,
            "store de secrets indisponível neste hub".to_string(),
        )
    })?;
    match method {
        "hub.recovery.status" => Ok(json!(secrets.recovery_status())),
        "hub.recovery.generate" => secrets
            .recovery_generate()
            .map(|g| json!(g))
            .map_err(secrets_err),
        "hub.recovery.verify" => {
            let key = match p.get_mut("key").map(Value::take) {
                Some(Value::String(k)) => Zeroizing::new(k),
                _ => return Err(invalid("parâmetro obrigatório: key")),
            };
            secrets
                .recovery_verify(&key)
                .map(|s| json!(s))
                .map_err(secrets_err)
        }
        _ => Err((-32601, format!("método desconhecido: {method}"))),
    }
}

/// `hub.secrets.*` (síncronos: rodam fora do runtime). Nunca segura o lock
/// do hub ao chamar o `HubSecrets`.
fn secrets_call(st: &HubState, method: &str, p: &Value) -> RpcResult {
    let secrets = st.secrets.clone().ok_or_else(|| {
        (
            SECRETS_UNAVAILABLE,
            "store de secrets indisponível neste hub".to_string(),
        )
    })?;
    match method {
        "hub.secrets.list" => secrets.list().map(|l| json!(l)).map_err(secrets_err),
        "hub.secrets.rotate_pending_dismiss" => {
            secrets.rotate_pending_dismiss().map_err(secrets_err)?;
            tracing::info!("lista de secrets a rotacionar na origem dispensada");
            Ok(json!({}))
        }
        "hub.secrets.set" => {
            let scope = scope_param(p)?;
            let name = str_param(p, "name")?;
            ct_secrets::validate_name(name).map_err(|e| invalid(e.to_string()))?;
            if ct_secrets::is_reserved_name(name) {
                return Err(invalid(format!(
                    "nome reservado: {name} (o ambiente já define essa variável para a \
                     sessão; CTOWER_*, PATH, HOME, LC_*, LD_* e afins não podem ser \
                     sobrescritos)"
                )));
            }
            let kind = match str_param(p, "kind")? {
                "secret" => Kind::Secret,
                "variable" => Kind::Variable,
                _ => return Err(invalid(r#"kind deve ser "secret" ou "variable""#)),
            };
            let value = p
                .get("value")
                .and_then(Value::as_str)
                .ok_or_else(|| invalid("parâmetro obrigatório: value"))?;
            if kind == Kind::Secret && value.is_empty() {
                return Err(invalid(format!("valor vazio para o secret {name}")));
            }
            if let Scope::Project(key) = &scope {
                // Na última barra: no GitLab, `owner` tem barras.
                let (owner, repo) = key.rsplit_once('/').unwrap_or_default();
                let found = st
                    .hub
                    .lock()
                    .find_project(owner, repo)
                    .map_err(|e| (-32000, e.to_string()))?;
                if found.is_none() {
                    return Err((
                        SECRET_UNKNOWN_PROJECT,
                        format!("projeto {key} não está cadastrado no hub"),
                    ));
                }
            }
            let what = scope_text(&scope);
            let rev = secrets
                .set(scope, name, kind, SecretValue::new(value))
                .map_err(secrets_err)?;
            tracing::info!("{name} ({what}) gravado no store de secrets (config_rev {rev})");
            Ok(json!(SecretChanged { rev }))
        }
        "hub.secrets.delete" => {
            let scope = scope_param(p)?;
            let name = str_param(p, "name")?;
            let what = scope_text(&scope);
            match secrets.delete(&scope, name).map_err(secrets_err)? {
                Some(rev) => {
                    tracing::info!(
                        "{name} ({what}) removido do store de secrets (config_rev {rev})"
                    );
                    Ok(json!(SecretChanged { rev }))
                }
                None => Err((SECRET_NOT_FOUND, format!("{name} não existe em {what}"))),
            }
        }
        _ => Err((-32601, format!("método desconhecido: {method}"))),
    }
}

/// Token do GitHub do hub. O token nunca volta na resposta: só conta e origem.
async fn github(st: &HubState, method: &str, p: &Value) -> RpcResult {
    let gh = st
        .github
        .clone()
        .ok_or_else(|| (-32000, "integração com o GitHub desativada".into()))?;
    let token = match method {
        "github.set_token" => Some(str_param(p, "token")?.to_string()),
        _ => None,
    };
    let method = method.to_string();
    tokio::task::spawn_blocking(move || {
        let c = gh.client();
        match method.as_str() {
            "github.set_token" => {
                c.set_token(&token.unwrap_or_default())?;
            }
            "github.clear_token" => c.clear_token()?,
            _ => {}
        }
        Ok::<_, ct_gh::GhError>(c.status())
    })
    .await
    .map_err(|e| (-32603, e.to_string()))?
    .map(|st| json!(st))
    .map_err(|e| (-32000, e.to_string()))
}

#[cfg(all(test, feature = "ts"))]
mod export {
    use super::*;
    use std::fs;
    use std::path::Path;
    use ts_rs::TS;

    /// Regenera `ui/src/protocol/hub-gen` com os tipos do `/ws` do hub. O CI
    /// roda este teste e falha se o resultado diferir do que está
    /// commitado. Fica à parte de `ui/src/protocol/gen` (do ambiente,
    /// `cargo test -p ct-server --features ts export_ts`): os dois protocolos
    /// têm versões próprias, e o `cthubd` não depende do `ct-server`.
    ///
    /// Inteiros de 64 bits viram `number`: o JSON chega à UI por
    /// `JSON.parse`, e ids e timestamps em ms cabem em 2^53.
    #[test]
    fn export_ts() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ui/src/protocol/hub-gen");
        let _ = fs::remove_dir_all(&dir);
        HubInfo::export_all_to(&dir).unwrap();
        HubStatus::export_all_to(&dir).unwrap();
        BackupConfigView::export_all_to(&dir).unwrap();
        ProjectDeleted::export_all_to(&dir).unwrap();
        AssignmentRemoved::export_all_to(&dir).unwrap();
        QuarantineDiscarded::export_all_to(&dir).unwrap();
        SecretChanged::export_all_to(&dir).unwrap();
        WebSessionsRevoked::export_all_to(&dir).unwrap();
        HubWorkItemBody::export_all_to(&dir).unwrap();
        crate::push::PushVapidKey::export_all_to(&dir).unwrap();
        crate::channel::EnvPush::export_all_to(&dir).unwrap();
        crate::channel::EnvOnline::export_all_to(&dir).unwrap();
        HubEnvCall::export_all_to(&dir).unwrap();
        crate::push::PushRemoved::export_all_to(&dir).unwrap();
        crate::push::PushTestResult::export_all_to(&dir).unwrap();
        ct_ledger::HubPushDevice::export_all_to(&dir).unwrap();
        ct_ledger::HubWebSession::export_all_to(&dir).unwrap();
        crate::secrets::SecretsList::export_all_to(&dir).unwrap();
        crate::secrets::RecoveryStatus::export_all_to(&dir).unwrap();
        crate::secrets::RecoveryGenerated::export_all_to(&dir).unwrap();
        ct_ledger::ProjectAdded::export_all_to(&dir).unwrap();
        ct_ledger::DeleteImpact::export_all_to(&dir).unwrap();
        ct_ledger::AssignmentAdded::export_all_to(&dir).unwrap();
        ct_ledger::HubProjectView::export_all_to(&dir).unwrap();
        ct_ledger::HubEnv::export_all_to(&dir).unwrap();
        ct_ledger::HubTree::export_all_to(&dir).unwrap();
        ct_ledger::HubHistory::export_all_to(&dir).unwrap();
        ct_ledger::HubDashboard::export_all_to(&dir).unwrap();
        ct_ledger::HubRanking::export_all_to(&dir).unwrap();
        ct_ledger::HubHealth::export_all_to(&dir).unwrap();
        ct_ledger::HubProposals::export_all_to(&dir).unwrap();
        ct_ledger::ProposalDecided::export_all_to(&dir).unwrap();
        ct_ledger::HubRules::export_all_to(&dir).unwrap();
        ct_ledger::HubRulesHistory::export_all_to(&dir).unwrap();
        ct_ledger::HubRulesProposal::export_all_to(&dir).unwrap();
        ct_ledger::HubRulesProposals::export_all_to(&dir).unwrap();
        ct_gh::TokenStatus::export_all_to(&dir).unwrap();
        crate::update::HubUpdateStatus::export_all_to(&dir).unwrap();
        // Rastreador e forja (#347): os tipos soltos, além dos de `HubProject`.
        ct_core::TrackerKind::export_all_to(&dir).unwrap();
        ct_core::ForgeKind::export_all_to(&dir).unwrap();

        // (nome do tipo, caminho relativo sem extensão), ex.: serde_json/JsonValue.
        fn walk(dir: &Path, rel: &str, out: &mut Vec<(String, String)>) {
            for entry in fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                let name = path.file_stem().unwrap().to_string_lossy().to_string();
                if path.is_dir() {
                    walk(&path, &format!("{rel}{name}/"), out);
                    continue;
                }
                let ts = fs::read_to_string(&path).unwrap();
                fs::write(&path, ts.replace("bigint", "number")).unwrap();
                out.push((name.clone(), format!("{rel}{name}")));
            }
        }
        let mut names = Vec::new();
        walk(&dir, "", &mut names);
        names.sort();
        let index: String = names
            .iter()
            .map(|(n, p)| format!("export type {{ {n} }} from \"./{p}\";\n"))
            .collect();
        fs::write(
            dir.join("index.ts"),
            format!("// Gerado por `cargo test -p cthubd --features ts export_ts`.\n{index}"),
        )
        .unwrap();
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Chamada ao despacho do `/ws`, sem conexão (testes de outros módulos).
    pub(crate) async fn tests_call(st: &HubState, method: &str) -> Value {
        call(st, method, json!({}), None).await.unwrap()
    }
    use crate::router;
    use ct_core::WorkItemKind;
    use ct_ledger::{Hub, Ledger, NewProject, SharedHub};
    use std::time::Duration;
    use tokio::net::TcpListener;
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    use tokio_tungstenite::tungstenite::Message as WsMsg;
    use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};

    type Client = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;
    const UI: &str = "ui-secreto";
    const T: Duration = Duration::from_secs(10);

    async fn spawn_server(st: &HubState) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = router(st.clone());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        format!("ws://{addr}/ws")
    }

    async fn connect(url: &str, token: &str) -> Result<Client, String> {
        let mut req = url.into_client_request().unwrap();
        req.headers_mut()
            .insert("authorization", format!("Bearer {token}").parse().unwrap());
        connect_async(req)
            .await
            .map(|(ws, _)| ws)
            .map_err(|e| e.to_string())
    }

    struct Ui {
        ws: Client,
        next: i64,
        pushes: Vec<Value>,
    }

    impl Ui {
        async fn new(url: &str) -> Self {
            Ui {
                ws: connect(url, UI).await.unwrap(),
                next: 0,
                pushes: Vec::new(),
            }
        }

        async fn call(&mut self, method: &str, params: Value) -> Value {
            self.next += 1;
            let id = self.next;
            let req = json!({ "id": id, "method": method, "params": params });
            self.ws
                .send(WsMsg::Text(req.to_string().into()))
                .await
                .unwrap();
            loop {
                let m = tokio::time::timeout(T, self.ws.next())
                    .await
                    .expect("prazo")
                    .unwrap()
                    .unwrap();
                if let WsMsg::Text(t) = m {
                    let v: Value = serde_json::from_str(&t).unwrap();
                    if v.get("event").is_some() {
                        self.pushes.push(v);
                    } else if v["id"] == id {
                        return v;
                    }
                }
            }
        }
    }

    /// Hub com o work item #5 do projeto demo, enviado pelo ambiente "wsl".
    fn hub_com_dados() -> HubState {
        let hub = SharedHub::new(Hub::open_in_memory().unwrap());
        hub.lock()
            .add_project(&ct_ledger::NewHubProject {
                owner: "deep-ion-ai".into(),
                repo: "demo".into(),
                name: "demo".into(),
                base_branch: "develop".into(),
                protected_branches: vec![],
            })
            .unwrap();
        let l = Ledger::open_in_memory().unwrap();
        let p = l
            .add_project(&NewProject {
                name: "demo".into(),
                gh_owner: "deep-ion-ai".into(),
                gh_repo: "demo".into(),
                repo_path: "/x".into(),
                base_branch: "develop".into(),
                protected_branches: vec![],
            })
            .unwrap();
        l.ensure_work_item(p.id, 5, WorkItemKind::Issue).unwrap();
        hub.lock()
            .ingest(l.env_id(), "wsl", &l.outbox_pending(1000).unwrap())
            .unwrap();
        HubState::new(hub, UI, "mac-mini", "hub-id")
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn handshake_exige_token_de_ui_do_hub() {
        let st = hub_com_dados();
        let tok = st.hub.lock().add_legacy_env_token("a").unwrap();
        // O token de agente também não abre o /ws (não aprova propostas).
        let agent = st.hub.lock().add_agent_token("a").unwrap();
        let url = spawn_server(&st).await;
        for t in ["", "errado", tok.as_str(), agent.as_str()] {
            let err = connect(&url, t).await.unwrap_err();
            assert!(err.contains("401"), "{t}: {err}");
        }
        assert!(connect(&url, UI).await.is_ok());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn info_e_status() {
        let st = hub_com_dados();
        let url = spawn_server(&st).await;
        let mut ui = Ui::new(&url).await;
        let v = ui.call("hub.info", json!({})).await;
        let r = &v["result"];
        assert_eq!(r["protocol"], crate::PROTOCOL_VERSION);
        assert_eq!(r["name"], "mac-mini");
        assert_eq!(r["config_rev"], 1);
        assert_eq!(r["backup"]["interval_hours"], 0);
        let v = ui.call("hub.status", json!({})).await;
        assert_eq!(v["result"]["warnings"][0]["kind"], "backup_disabled");
        // O hub não é ambiente: nada de sessões nem env.info.
        for m in ["env.info", "session.start", "hub.sync.now"] {
            let v = ui.call(m, json!({})).await;
            assert_eq!(v["error"]["code"], -32601, "{m}");
        }
        let v = ui.call("github.status", json!({})).await;
        assert_eq!(v["error"]["code"], -32000);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn metodos_do_hub() {
        let st = hub_com_dados();
        let url = spawn_server(&st).await;
        let mut ui = Ui::new(&url).await;
        let v = ui.call("hub.projects.list", json!({})).await;
        assert_eq!(v["result"][0]["project"]["gh_repo"], "demo");
        assert_eq!(v["result"][0]["locations"][0]["env_name"], "wsl");
        let v = ui
            .call(
                "hub.tree.get",
                json!({ "owner": "deep-ion-ai", "repo": "demo" }),
            )
            .await;
        assert_eq!(v["result"]["untracked"][0]["work_item"]["gh_number"], 5);
        let v = ui
            .call("hub.tree.get", json!({ "owner": "x", "repo": "y" }))
            .await;
        assert_eq!(v["error"]["code"], -32000);
        let v = ui
            .call(
                "hub.workitem.history",
                json!({ "owner": "deep-ion-ai", "repo": "demo", "number": 5, "limit": 10 }),
            )
            .await;
        assert_eq!(v["result"]["work_item"]["gh_number"], 5);
        assert!(!v["result"]["events"].as_array().unwrap().is_empty());
        let set = |n: i64, st: &str| json!({ "owner": "deep-ion-ai", "repo": "demo", "number": n, "status": st, "reason": "teste" });
        let v = ui
            .call("hub.workitem.set_status", set(5, "bloqueado"))
            .await;
        assert_eq!(v["result"]["gh_number"], 5);
        assert_eq!(v["result"]["status"], "bloqueado");
        let v = ui
            .call(
                "hub.workitem.history",
                json!({ "owner": "deep-ion-ai", "repo": "demo", "number": 5 }),
            )
            .await;
        assert_eq!(v["result"]["work_item"]["status"], "bloqueado");
        let evs = v["result"]["events"].as_array().unwrap();
        assert!(evs.iter().any(|e| e["kind"] == "status_set_manual"
            && e["payload"]["reason"] == "teste"
            && e["env_id"] == "hub-id"));
        let v = ui.call("hub.workitem.set_status", set(5, "voando")).await;
        assert_eq!(v["error"]["code"], -32602);
        let v = ui.call("hub.workitem.set_status", set(99, "pausado")).await;
        assert_eq!(v["error"]["code"], -32000);
        assert!(v["error"]["message"].as_str().unwrap().contains("#99"));
        let v = ui
            .call(
                "hub.workitem.set_status",
                json!({ "owner": "x", "repo": "y", "number": 1, "status": "pausado" }),
            )
            .await;
        assert_eq!(v["error"]["code"], -32000);
        let v = ui.call("hub.dashboard.get", json!({})).await;
        assert_eq!(v["result"]["envs"][0]["name"], "wsl");
        let v = ui.call("hub.envs.list", json!({})).await;
        assert_eq!(v["result"].as_array().unwrap().len(), 1);
        let v = ui.call("hub.inventado", json!({})).await;
        assert_eq!(v["error"]["code"], -32601);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn prioridade_ordem_e_ranking() {
        let st = hub_com_dados();
        let url = spawn_server(&st).await;
        let mut ui = Ui::new(&url).await;
        let d = |n: i64, extra: Value| {
            let mut v = json!({ "owner": "deep-ion-ai", "repo": "demo", "number": n });
            v.as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            v
        };
        let v = ui
            .call(
                "hub.workitem.set_priority",
                d(5, json!({ "priority": "P1", "reason": "teste" })),
            )
            .await;
        assert_eq!(v["result"]["priority"], "P1");
        assert_eq!(v["result"]["rank"], 1);
        let v = ui
            .call(
                "hub.workitem.history",
                json!({ "owner": "deep-ion-ai", "repo": "demo", "number": 5 }),
            )
            .await;
        let evs = v["result"]["events"].as_array().unwrap();
        assert!(evs.iter().any(|e| e["kind"] == "priority_set"
            && e["payload"]["to"] == "P1"
            && e["payload"]["reason"] == "teste"
            && e["env_id"] == "hub-id"));
        let v = ui.call("hub.workitem.set_priority", d(5, json!({}))).await;
        assert_eq!(v["error"]["code"], -32602);
        let v = ui
            .call(
                "hub.workitem.set_priority",
                d(5, json!({ "priority": "P9" })),
            )
            .await;
        assert_eq!(v["error"]["code"], -32602);
        let v = ui
            .call(
                "hub.workitem.move",
                d(5, json!({ "before": 1, "after": 2 })),
            )
            .await;
        assert_eq!(v["error"]["code"], -32602);
        let v = ui
            .call("hub.workitem.move", d(5, json!({ "before": 99 })))
            .await;
        assert_eq!(v["error"]["code"], -32000);
        let v = ui.call("hub.workitem.move", d(5, json!({}))).await;
        assert_eq!(v["result"]["rank"], 1);

        let v = ui
            .call("hub.ranking.get", json!({ "priority": "P1", "limit": 10 }))
            .await;
        assert_eq!(v["result"]["items"][0]["number"], 5);
        assert_eq!(v["result"]["items"][0]["position"], 1);
        assert_eq!(v["result"]["truncated"], false);
        let v = ui.call("hub.ranking.get", json!({ "owner": "x" })).await;
        assert_eq!(v["error"]["code"], -32602);
        let v = ui
            .call("hub.health.get", json!({ "min_severity": "info" }))
            .await;
        assert!(v["result"]["signals"].is_array());
        assert!(v["result"]["counts"]["critico"].is_number());
        let v = ui
            .call("hub.health.get", json!({ "min_severity": "grave" }))
            .await;
        assert_eq!(v["error"]["code"], -32602);
        let v = ui.call("hub.health.get", json!({ "repo": "demo" })).await;
        assert_eq!(v["error"]["code"], -32602);
        let v = ui.call("hub.dashboard.get", json!({})).await;
        assert_eq!(
            v["result"]["projects"][0]["work_items"][0]["priority"],
            "P1"
        );

        let v = ui
            .call(
                "hub.projects.move",
                json!({ "owner": "deep-ion-ai", "repo": "demo" }),
            )
            .await;
        assert_eq!(v["result"]["rank"], 1);
        let v = ui
            .call(
                "hub.projects.move",
                json!({ "owner": "deep-ion-ai", "repo": "demo", "after": { "owner": "x" } }),
            )
            .await;
        assert_eq!(v["error"]["code"], -32602);
        let v = ui
            .call(
                "hub.workitem.set_priority",
                d(5, json!({ "priority": null })),
            )
            .await;
        assert_eq!(v["result"]["priority"], Value::Null);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn propostas_listar_decidir_e_push() {
        let st = hub_com_dados();
        let env_id = st.hub.lock().list_envs().unwrap()[0].env_id.clone();
        let propose = |kind: ct_core::ProposalKind, proposed: Value| {
            st.hub
                .lock()
                .create_proposal(
                    &ct_ledger::NewProposal {
                        kind,
                        owner: "deep-ion-ai".into(),
                        repo: "demo".into(),
                        number: 5,
                        proposed,
                        justification: "o gestor acha".into(),
                    },
                    &env_id,
                    Some("s-gestor"),
                    ct_core::now_ms(),
                )
                .unwrap()
                .id
        };
        let a = propose(ct_core::ProposalKind::Priority, json!({ "priority": "P1" }));
        let b = propose(
            ct_core::ProposalKind::Status,
            json!({ "status": "pausado" }),
        );
        let url = spawn_server(&st).await;
        let mut ui = Ui::new(&url).await;
        let v = ui.call("hub.proposals.list", json!({})).await;
        let list = v["result"]["proposals"].as_array().unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0]["id"], b);
        assert_eq!(list[0]["env_name"], "wsl");
        let v = ui
            .call("hub.proposals.list", json!({ "state": "voando" }))
            .await;
        assert_eq!(v["error"]["code"], -32602);

        // Uma por vez.
        let v = ui
            .call(
                "hub.proposals.decide",
                json!({ "id": a, "approve": true, "note": "fechado" }),
            )
            .await;
        assert_eq!(v["result"]["state"], "aprovada", "{v}");
        let v = ui
            .call(
                "hub.workitem.history",
                json!({ "owner": "deep-ion-ai", "repo": "demo", "number": 5 }),
            )
            .await;
        assert_eq!(v["result"]["work_item"]["priority"], "P1");
        assert!(v["result"]["decisions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["origin"] == "orientada" && d["env_id"] == "hub-id"));
        // Já decidida: erro com o estado.
        let v = ui
            .call("hub.proposals.decide", json!({ "id": a, "approve": false }))
            .await;
        assert!(v["error"]["message"].as_str().unwrap().contains("aprovada"));

        // Em lote: a decidida falha, a pendente é rejeitada.
        let v = ui
            .call(
                "hub.proposals.decide",
                json!({ "ids": [a, b, 999], "approve": false }),
            )
            .await;
        let out = v["result"].as_array().unwrap();
        assert!(out[0]["error"].is_string());
        assert_eq!(out[1]["proposal"]["state"], "rejeitada");
        assert!(out[2]["error"].as_str().unwrap().contains("#999"));
        for bad in [
            json!({ "id": b }),
            json!({ "id": b, "ids": [b], "approve": true }),
            json!({ "ids": [], "approve": true }),
        ] {
            let v = ui.call("hub.proposals.decide", bad.clone()).await;
            assert_eq!(v["error"]["code"], -32602, "{bad}");
        }
        let v = ui
            .call("hub.proposals.list", json!({ "state": "todos" }))
            .await;
        assert_eq!(v["result"]["proposals"].as_array().unwrap().len(), 2);
        // As decisões avisaram as UIs.
        for _ in 0..20 {
            if ui
                .pushes
                .iter()
                .any(|p| p["event"] == "hub.proposals.changed")
            {
                break;
            }
            ui.call("hub.info", json!({})).await;
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(ui
            .pushes
            .iter()
            .any(|p| p["event"] == "hub.proposals.changed"));
    }

    /// Regras do agente (#357): editar e importar gravam versões com a
    /// origem; a proposta vinda do outbox fica pendente, a lista avisa que a
    /// base mudou e aprovar grava a versão `proposta`; as UIs recebem
    /// `hub.rules.changed`.
    #[tokio::test(flavor = "multi_thread")]
    async fn regras_editar_importar_propor_e_aprovar() {
        let st = hub_com_dados();
        let env_id = st.hub.lock().list_envs().unwrap()[0].env_id.clone();
        let url = spawn_server(&st).await;
        let mut ui = Ui::new(&url).await;
        let scope = |extra: Value| {
            let mut v = json!({ "owner": "deep-ion-ai", "repo": "demo" });
            v.as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            v
        };
        let v = ui.call("hub.rules.get", scope(json!({}))).await;
        assert_eq!(v["result"]["current"], Value::Null, "{v}");
        assert_eq!(v["result"]["max_bytes"], 16384);
        let v = ui
            .call(
                "hub.rules.import",
                scope(json!({ "epic_number": 5, "content": "# CLAUDE.md do épico", "base_version": 0 })),
            )
            .await;
        assert_eq!(v["result"]["source"], "importacao", "{v}");
        let v = ui
            .call(
                "hub.rules.set",
                scope(json!({ "epic_number": 5, "content": "editado", "base_version": 0 })),
            )
            .await;
        assert!(
            v["error"]["message"]
                .as_str()
                .unwrap()
                .contains("recarregue"),
            "{v}"
        );
        let big = "x".repeat(ct_ledger::RULES_MAX_BYTES + 1);
        let v = ui
            .call("hub.rules.set", scope(json!({ "content": big })))
            .await;
        assert!(
            v["error"]["message"].as_str().unwrap().contains("16 KiB"),
            "{v}"
        );
        let v = ui
            .call(
                "hub.rules.set",
                scope(json!({ "epic_number": -1, "content": "x" })),
            )
            .await;
        assert_eq!(v["error"]["code"], -32602);

        let entry = ct_ledger::OutboxEntry {
            seq: 1,
            created_at: 0,
            change: ct_ledger::SyncChange::RulesProposal(ct_ledger::RulesProposalChange {
                uid: "r-1".into(),
                owner: "deep-ion-ai".into(),
                repo: "demo".into(),
                epic_number: 5,
                base_version: 0,
                content: "do agente".into(),
                reason: "o bot fechou o MR".into(),
                session_id: None,
            }),
        };
        let r = st.hub.lock().ingest(&env_id, "wsl", &[entry]).unwrap();
        assert_eq!(r.rules_proposals, 1);
        let v = ui
            .call(
                "hub.rules.proposals.list",
                json!({ "owner": "deep-ion-ai", "repo": "demo" }),
            )
            .await;
        let p = &v["result"]["proposals"][0];
        assert_eq!(
            (p["state"].clone(), p["base_changed"].clone()),
            (json!("pendente"), json!(true))
        );
        assert_eq!(p["current_content"], "# CLAUDE.md do épico");
        assert_eq!(p["label"], "deep-ion-ai/demo#5");
        let id = p["id"].as_i64().unwrap();
        let v = ui
            .call(
                "hub.rules.proposals.decide",
                json!({ "id": id, "approve": true }),
            )
            .await;
        assert_eq!(v["result"]["applied_version"], 2, "{v}");
        let v = ui
            .call("hub.rules.history", scope(json!({ "epic_number": 5 })))
            .await;
        let vs = v["result"]["versions"].as_array().unwrap();
        assert_eq!(
            vs.iter().map(|v| v["source"].clone()).collect::<Vec<_>>(),
            [json!("proposta"), json!("importacao")]
        );
        assert_eq!(vs[0]["proposal_id"], id);
        assert_eq!(vs[0]["author_env_name"], "wsl");
        for _ in 0..20 {
            if ui.pushes.iter().any(|p| p["event"] == "hub.rules.changed") {
                break;
            }
            ui.call("hub.info", json!({})).await;
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(ui.pushes.iter().any(|p| p["event"] == "hub.rules.changed"));
    }

    /// Hub com GitHub falso (repositório `deep-ion-ai/novo` com `develop` e
    /// `main`) e snapshots em `tmp/backups`.
    fn hub_com_github(tmp: &std::path::Path) -> (HubState, ct_gh::fake::FakeGitHub) {
        use ct_gh::fake::{fake_gh, FakeGitHub};
        use ct_gh::{GhClient, GhConfig};
        let gh = FakeGitHub::start("tok");
        gh.set(
            "/repos/deep-ion-ai/novo",
            json!({ "name": "novo", "owner": { "login": "deep-ion-ai" } }),
        );
        for b in ["develop", "main"] {
            gh.set(&format!("/repos/deep-ion-ai/novo/branches/{b}"), json!({}));
        }
        let bin = fake_gh(tmp, "tok");
        let client = GhClient::new(
            GhConfig::new(&gh.url(), bin.to_str().unwrap())
                .with_token_timeout(ct_gh::fake::FAKE_GH_TOKEN_TIMEOUT),
        );
        let mut st = hub_com_dados();
        st.github = Some(std::sync::Arc::new(crate::github::HubGithub::new(
            st.hub.clone(),
            client,
            "hub-id",
            st.events.clone(),
        )));
        st.backups = std::sync::Arc::new(crate::backup::Backups::new(
            tmp.join("backups"),
            Duration::from_secs(3600),
            2,
            None,
        ));
        (st, gh)
    }

    /// Um ambiente "wsl" envia o work item `n` de `deep-ion-ai/novo`, com uma
    /// sessão viva.
    fn push_novo(st: &HubState, l: &Ledger, n: i64) -> ct_ledger::IngestResult {
        if l.list_projects().unwrap().is_empty() {
            l.add_project(&NewProject {
                name: "novo-local".into(),
                gh_owner: "deep-ion-ai".into(),
                gh_repo: "novo".into(),
                repo_path: "/wsl/novo".into(),
                base_branch: "trunk".into(),
                protected_branches: vec![],
            })
            .unwrap();
        }
        let p = l.list_projects().unwrap()[0].id;
        let wi = l.ensure_work_item(p, n, WorkItemKind::Issue).unwrap();
        let s = l
            .create_session(&ct_ledger::NewSession {
                work_item_id: wi.id,
                tool: ct_core::Tool::Claude,
                cwd: "/wsl/wt".into(),
                launch_cmd: vec![],
                model: None,
                mode: ct_core::SessionMode::Native,
            })
            .unwrap();
        l.transition_session(&s.id, ct_core::SessionState::Running, None)
            .unwrap();
        let batch = l.outbox_pending(1000).unwrap();
        let r = st.hub.lock().ingest(l.env_id(), "wsl", &batch).unwrap();
        l.outbox_ack(r.last_seq).unwrap();
        r
    }

    /// Docs do projeto (#440): `hub.projects.update { docs }` grava o modelo
    /// normalizado e muda o `config_rev`; modelo inválido é recusado.
    #[tokio::test(flavor = "multi_thread")]
    async fn docs_do_projeto_pela_edicao() {
        let st = hub_com_dados();
        let url = spawn_server(&st).await;
        let mut ui = Ui::new(&url).await;
        let rev = ui.call("hub.info", json!({})).await["result"]["config_rev"].clone();
        assert_eq!(
            ui.call("hub.info", json!({})).await["result"]["docs_dir"],
            Value::Null
        );
        let v = ui
            .call(
                "hub.projects.update",
                json!({ "owner": "deep-ion-ai", "repo": "demo",
                        "docs": { "enabled": true, "project_dirs": ["analises/", "analises"],
                                  "epic_dirs": ["prompts"] } }),
            )
            .await;
        let d = &v["result"]["docs"];
        assert_eq!(d["enabled"], true, "{v}");
        assert_eq!(d["project_dirs"], json!(["analises"]), "{v}");
        assert_eq!(d["epic_dirs"], json!(["prompts"]), "{v}");
        let novo = ui.call("hub.info", json!({})).await["result"]["config_rev"].clone();
        assert!(novo.as_i64() > rev.as_i64(), "{rev} → {novo}");
        let v = ui
            .call(
                "hub.projects.update",
                json!({ "owner": "deep-ion-ai", "repo": "demo",
                        "docs": { "enabled": true, "project_dirs": ["../fora"] } }),
            )
            .await;
        assert!(
            v["error"]["message"]
                .as_str()
                .unwrap()
                .contains("docs.project_dirs"),
            "{v}"
        );
        let v = ui
            .call(
                "hub.projects.update",
                json!({ "owner": "deep-ion-ai", "repo": "demo", "docs": true }),
            )
            .await;
        assert_eq!(v["error"]["code"], -32602, "{v}");
    }

    /// Cadastro Jira + GitLab (#348): sem a integração com o GitHub (que o
    /// projeto GitHub exige), sem secret do hub, com as recusas de
    /// combinação e de padrão de branch; e o ambiente de sync pelo
    /// `hub.projects.update`.
    #[tokio::test(flavor = "multi_thread")]
    async fn cadastro_jira_gitlab_sem_github_e_ambiente_de_sync() {
        let st = hub_com_dados();
        assert!(st.github.is_none());
        let url = spawn_server(&st).await;
        let mut ui = Ui::new(&url).await;
        let jira = json!({
            "kind": "jira", "url": "https://jira.example.com/", "project_key": "proj",
            "jql": "", "active_sprint_only": true,
            "credential_cmd": "pass show jira | head -1"
        });
        let gitlab = json!({
            "kind": "gitlab", "clone_url": "git@gitlab.example.com:grupo/projeto/app.git",
            "credential_cmd": "pass show gitlab | head -1"
        });

        // O projeto GitHub ainda precisa do token do hub.
        let v = ui
            .call(
                "hub.projects.add",
                json!({ "url": "https://github.com/o/r" }),
            )
            .await;
        assert!(
            v["error"]["message"]
                .as_str()
                .unwrap()
                .contains("GitHub desativada"),
            "{v}"
        );
        // Recusas: combinação, padrão de branch e GitLab sem repositório.
        let v = ui
            .call(
                "hub.projects.add",
                json!({ "url": "https://github.com/o/r", "tracker": jira }),
            )
            .await;
        assert_eq!(v["error"]["code"], -32602, "{v}");
        assert!(
            v["error"]["message"]
                .as_str()
                .unwrap()
                .contains("combinação"),
            "{v}"
        );
        let v = ui
            .call(
                "hub.projects.add",
                json!({ "tracker": jira, "forge": gitlab, "branch_pattern": "{prefix}/{slug}" }),
            )
            .await;
        assert_eq!(v["error"]["code"], -32602, "{v}");
        assert!(
            v["error"]["message"].as_str().unwrap().contains("{n}"),
            "{v}"
        );
        let v = ui
            .call(
                "hub.projects.add",
                json!({ "tracker": jira, "forge": { "kind": "gitlab", "credential_cmd": "x" } }),
            )
            .await;
        assert_eq!(v["error"]["code"], -32602, "{v}");

        // Cadastro: normaliza URL e chave, preenche o mapa de tipos, deriva
        // host e caminho da URL de clone.
        let v = ui
            .call(
                "hub.projects.add",
                json!({ "tracker": jira, "forge": gitlab, "branch_pattern": "{key}",
                        "attribution": false, "base_branch": "develop",
                        "protected_branches": ["develop", "master"] }),
            )
            .await;
        let p = &v["result"]["project"];
        assert_eq!(
            (p["gh_owner"].as_str(), p["gh_repo"].as_str()),
            (Some("grupo/projeto"), Some("app")),
            "{v}"
        );
        assert_eq!(p["tracker"]["url"], "https://jira.example.com");
        assert_eq!(p["tracker"]["project_key"], "PROJ");
        assert_eq!(p["tracker"]["type_map"]["Épico"], "epic");
        assert_eq!(p["forge"]["host"], "gitlab.example.com");
        assert_eq!(p["forge"]["path"], "grupo/projeto/app");
        assert_eq!(p["branch_pattern"], "{key}");
        assert_eq!(p["attribution"], false);
        assert_eq!(p["sync_env_id"], Value::Null);
        // Credenciais nunca viram secret do hub: só o comando, no cadastro.
        assert_eq!(p["tracker"]["credential_cmd"], "pass show jira | head -1");
        assert_eq!(p["forge"]["credential_cmd"], "pass show gitlab | head -1");
        let v = ui
            .call(
                "hub.projects.add",
                json!({ "tracker": jira, "forge": gitlab }),
            )
            .await;
        assert!(
            v["error"]["message"]
                .as_str()
                .unwrap()
                .contains("já está cadastrado"),
            "{v}"
        );

        // A primeira atribuição vira o ambiente de sync; a troca é só entre
        // os atribuídos.
        let env = st.hub.lock().list_envs().unwrap()[0].env_id.clone();
        let (o, r) = ("grupo/projeto", "app");
        let v = ui
            .call(
                "hub.assignments.add",
                json!({ "owner": o, "repo": r, "env": env }),
            )
            .await;
        assert_eq!(v["result"]["sync_env"], true, "{v}");
        st.hub.lock().ingest("env-b", "b", &[]).unwrap();
        let v = ui
            .call(
                "hub.projects.update",
                json!({ "owner": o, "repo": r, "sync_env": "b" }),
            )
            .await;
        assert!(
            v["error"]["message"]
                .as_str()
                .unwrap()
                .contains("atribuído"),
            "{v}"
        );
        ui.call(
            "hub.assignments.add",
            json!({ "owner": o, "repo": r, "env": "b" }),
        )
        .await;
        let v = ui
            .call(
                "hub.projects.update",
                json!({ "owner": o, "repo": r, "sync_env": "b" }),
            )
            .await;
        assert_eq!(v["result"]["sync_env_id"], "env-b", "{v}");
        // A base do projeto GitLab muda sem passar pelo GitHub.
        let v = ui
            .call(
                "hub.projects.update",
                json!({ "owner": o, "repo": r, "base_branch": "main" }),
            )
            .await;
        assert_eq!(v["result"]["base_branch"], "main", "{v}");
        let v = ui.call("hub.projects.list", json!({})).await;
        let jira = v["result"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["project"]["gh_repo"] == "app")
            .unwrap();
        assert_eq!(jira["tracker_status"], Value::Null);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn projetos_nascem_no_hub_com_quarentena_e_lapide() {
        let tmp = tempfile::tempdir().unwrap();
        let (st, gh) = hub_com_github(tmp.path());
        let url = spawn_server(&st).await;
        let mut ui = Ui::new(&url).await;
        let l = Ledger::open_in_memory().unwrap();

        // Push de projeto desconhecido: ack, quarentena e aviso no status.
        let r = push_novo(&st, &l, 3);
        assert!(r.quarantined > 0 && r.applied == 0, "{r:?}");
        let v = ui.call("hub.status", json!({})).await;
        let ws = v["result"]["warnings"].as_array().unwrap();
        let w = ws.iter().find(|w| w["kind"] == "push_quarantine").unwrap();
        assert!(
            w["message"].as_str().unwrap().contains("deep-ion-ai/novo"),
            "{w}"
        );
        assert_eq!(v["result"]["quarantine"][0]["env_name"], "wsl");
        let v = ui.call("hub.quarantine.list", json!({})).await;
        assert_eq!(v["result"][0]["project_key"], "deep-ion-ai/novo");
        let v = ui.call("hub.projects.list", json!({})).await;
        assert_eq!(v["result"].as_array().unwrap().len(), 1);

        // Validação no GitHub.
        let add = |u: &str, extra: Value| {
            let mut p = json!({ "url": u });
            p.as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            p
        };
        let v = ui
            .call("hub.projects.add", add("https://gitlab.com/a/b", json!({})))
            .await;
        assert_eq!(v["error"]["code"], -32602, "{v}");
        let v = ui
            .call(
                "hub.projects.add",
                add("https://github.com/deep-ion-ai/sumiu", json!({})),
            )
            .await;
        assert!(
            v["error"]["message"].as_str().unwrap().contains("token"),
            "{v}"
        );
        let v = ui
            .call(
                "hub.projects.add",
                add(
                    "https://github.com/deep-ion-ai/novo",
                    json!({ "base_branch": "trunk" }),
                ),
            )
            .await;
        assert!(
            v["error"]["message"].as_str().unwrap().contains("trunk"),
            "{v}"
        );
        let v = ui
            .call(
                "hub.projects.add",
                add("git@github.com:deep-ion-ai/demo.git", json!({})),
            )
            .await;
        assert!(
            v["error"]["message"]
                .as_str()
                .unwrap()
                .contains("já está cadastrado"),
            "{v}"
        );

        // Cadastro: reaplica a quarentena; atributos do hub, não do ambiente.
        let v = ui
            .call(
                "hub.projects.add",
                add(
                    "https://github.com/deep-ion-ai/novo/issues/1",
                    json!({ "name": "Novo" }),
                ),
            )
            .await;
        let r = &v["result"];
        assert_eq!(r["project"]["name"], "Novo", "{v}");
        assert_eq!(r["project"]["base_branch"], "develop");
        assert_eq!(
            r["project"]["protected_branches"],
            json!(["develop", "main", "master"])
        );
        assert_eq!(r["rev"], 2);
        assert!(r["reapplied"].as_i64().unwrap() > 0);
        let v = ui.call("hub.quarantine.list", json!({})).await;
        assert_eq!(v["result"], json!([]));
        let v = ui
            .call(
                "hub.tree.get",
                json!({ "owner": "deep-ion-ai", "repo": "novo" }),
            )
            .await;
        assert_eq!(v["result"]["untracked"][0]["work_item"]["gh_number"], 3);
        // Sem a quarentena, sem o aviso.
        let v = ui.call("hub.status", json!({})).await;
        assert!(!v.to_string().contains("push_quarantine"), "{v}");

        // Edição, com a branch validada.
        let v = ui
            .call(
                "hub.projects.update",
                json!({ "owner": "deep-ion-ai", "repo": "novo", "base_branch": "main",
                        "protected_branches": ["main"] }),
            )
            .await;
        assert_eq!(v["result"]["base_branch"], "main", "{v}");
        assert_eq!(v["result"]["protected_branches"], json!(["main"]));
        let v = ui
            .call(
                "hub.projects.update",
                json!({ "owner": "deep-ion-ai", "repo": "novo", "base_branch": "nada" }),
            )
            .await;
        assert_eq!(v["error"]["code"], -32000, "{v}");
        let v = ui
            .call(
                "hub.projects.update",
                json!({ "owner": "deep-ion-ai", "repo": "novo" }),
            )
            .await;
        assert_eq!(v["error"]["code"], -32602, "{v}");
        let v = ui.call("hub.info", json!({})).await;
        assert_eq!(v["result"]["config_rev"], 3);

        // Impacto antes de excluir: o mesmo da recusa sem confirm.
        let v = ui
            .call(
                "hub.projects.impact",
                json!({ "owner": "deep-ion-ai", "repo": "novo" }),
            )
            .await;
        assert_eq!(v["result"]["project"]["gh_repo"], "novo", "{v}");
        assert_eq!(v["result"]["live_sessions"][0]["cwd"], "/wsl/wt", "{v}");
        let v = ui
            .call(
                "hub.projects.impact",
                json!({ "owner": "deep-ion-ai", "repo": "nada" }),
            )
            .await;
        assert_eq!(v["error"]["code"], -32000, "{v}");

        // Exclusão: com o ambiente usando o projeto, exige confirm.
        let v = ui
            .call(
                "hub.projects.delete",
                json!({ "owner": "deep-ion-ai", "repo": "novo" }),
            )
            .await;
        let m = v["error"]["message"].as_str().unwrap();
        assert!(m.contains("wsl") && m.contains("confirm"), "{m}");
        assert!(m.contains("sessões vivas"), "{m}");
        let v = ui
            .call(
                "hub.projects.delete",
                json!({ "owner": "deep-ion-ai", "repo": "novo", "confirm": true }),
            )
            .await;
        assert_eq!(v["result"]["rev"], 4, "{v}");
        let snap = std::path::PathBuf::from(v["result"]["snapshot"].as_str().unwrap());
        assert!(snap.exists());
        assert!(snap
            .file_name()
            .unwrap()
            .to_string_lossy()
            .ends_with("-pre-delete.db"));
        assert!(ct_ledger::list_snapshots(&tmp.path().join("backups"))
            .unwrap()
            .is_empty());
        let v = ui.call("hub.projects.list", json!({})).await;
        assert_eq!(v["result"].as_array().unwrap().len(), 1);
        let v = ui
            .call(
                "hub.tree.get",
                json!({ "owner": "deep-ion-ai", "repo": "novo" }),
            )
            .await;
        assert_eq!(v["error"]["code"], -32000);

        // Push atrasado do projeto excluído: descartado, sem quarentena.
        let r = push_novo(&st, &l, 4);
        assert!(r.discarded > 0 && r.quarantined == 0, "{r:?}");
        let v = ui.call("hub.quarantine.list", json!({})).await;
        assert_eq!(v["result"], json!([]));

        // Recadastrar limpa a lápide.
        let v = ui
            .call(
                "hub.projects.add",
                add("https://github.com/deep-ion-ai/novo", json!({})),
            )
            .await;
        assert_eq!(v["result"]["restored"], true, "{v}");
        assert_eq!(v["result"]["rev"], 5);
        // Sem ambientes ligados ao projeto, a exclusão não pede confirm.
        let v = ui
            .call(
                "hub.projects.delete",
                json!({ "owner": "deep-ion-ai", "repo": "novo" }),
            )
            .await;
        assert_eq!(v["result"]["rev"], 6, "{v}");
        let v = ui
            .call(
                "hub.projects.delete",
                json!({ "owner": "deep-ion-ai", "repo": "novo" }),
            )
            .await;
        assert_eq!(v["error"]["code"], -32000);
        drop(gh);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn atribuicoes_com_config_rev_e_confirmacao() {
        let st = hub_com_dados();
        let url = spawn_server(&st).await;
        let mut ui = Ui::new(&url).await;
        let demo = |extra: Value| {
            let mut p = json!({ "owner": "deep-ion-ai", "repo": "demo" });
            p.as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            p
        };
        let v = ui.call("hub.assignments.list", json!({})).await;
        assert_eq!(v["result"], json!([]), "{v}");

        // Atribuir: pendente até o ambiente relatar; `config_rev` sobe.
        let v = ui
            .call("hub.assignments.add", demo(json!({ "env": "wsl" })))
            .await;
        let r = &v["result"];
        assert_eq!(r["changed"], true, "{v}");
        assert_eq!(r["assignment"]["rev"], 2);
        assert_eq!(r["assignment"]["state"], "pending");
        assert_eq!(r["assignment"]["repo_path"], "");
        let v = ui.call("hub.info", json!({})).await;
        assert_eq!(v["result"]["config_rev"], 2);
        let v = ui.call("hub.projects.list", json!({})).await;
        let a = &v["result"][0]["assignments"][0];
        assert_eq!(
            (a["env_name"].clone(), a["state"].clone()),
            (json!("wsl"), json!("pending"))
        );
        assert_eq!(a["reported_path"], "/x");
        let v = ui
            .call(
                "hub.assignments.list",
                json!({ "env": "wsl", "repo": "DEMO" }),
            )
            .await;
        assert_eq!(v["result"].as_array().unwrap().len(), 1, "{v}");

        // Recusas.
        let v = ui.call("hub.assignments.add", demo(json!({}))).await;
        assert_eq!(v["error"]["code"], -32602, "{v}");
        let v = ui
            .call("hub.assignments.add", demo(json!({ "env": "sumiu" })))
            .await;
        assert_eq!(v["error"]["code"], -32000, "{v}");
        let v = ui
            .call(
                "hub.assignments.add",
                demo(json!({ "env": "wsl", "repo_path": "rel/demo" })),
            )
            .await;
        assert_eq!(v["error"]["code"], -32000, "{v}");
        let v = ui.call("hub.info", json!({})).await;
        assert_eq!(v["result"]["config_rev"], 2);

        // Sem sessões vivas no ambiente, remove sem confirm; reatribuir volta.
        let v = ui
            .call("hub.assignments.remove", demo(json!({ "env": "wsl" })))
            .await;
        assert_eq!(v["result"]["rev"], 3, "{v}");
        let v = ui
            .call("hub.assignments.remove", demo(json!({ "env": "wsl" })))
            .await;
        assert_eq!(v["error"]["code"], -32000, "{v}");
        let v = ui
            .call("hub.assignments.add", demo(json!({ "env": "wsl" })))
            .await;
        assert_eq!(v["result"]["reassigned"], true, "{v}");
        assert_eq!(v["result"]["assignment"]["rev"], 4);

        // Outro ambiente com sessão viva no projeto.
        let nb = Ledger::open_in_memory().unwrap();
        let p = nb
            .add_project(&NewProject {
                name: "demo".into(),
                gh_owner: "deep-ion-ai".into(),
                gh_repo: "demo".into(),
                repo_path: "/nb/demo".into(),
                base_branch: "develop".into(),
                protected_branches: vec![],
            })
            .unwrap();
        let wi = nb.ensure_work_item(p.id, 9, WorkItemKind::Issue).unwrap();
        let s = nb
            .create_session(&ct_ledger::NewSession {
                work_item_id: wi.id,
                tool: ct_core::Tool::Claude,
                cwd: "/nb/wt".into(),
                launch_cmd: vec![],
                model: None,
                mode: ct_core::SessionMode::Native,
            })
            .unwrap();
        nb.transition_session(&s.id, ct_core::SessionState::Running, None)
            .unwrap();
        st.hub
            .lock()
            .ingest(nb.env_id(), "notebook", &nb.outbox_pending(1000).unwrap())
            .unwrap();
        let v = ui
            .call(
                "hub.assignments.add",
                demo(json!({ "env": "notebook", "repo_path": "/nb/demo" })),
            )
            .await;
        assert_eq!(v["result"]["assignment"]["rev"], 5, "{v}");
        // Remover com sessão viva: avisa e exige confirm.
        let v = ui
            .call("hub.assignments.remove", demo(json!({ "env": "notebook" })))
            .await;
        let m = v["error"]["message"].as_str().unwrap();
        assert!(m.contains("confirm") && m.contains(&s.id), "{m}");
        assert!(m.contains("não encerra"), "{m}");
        let v = ui.call("hub.info", json!({})).await;
        assert_eq!(v["result"]["config_rev"], 5);
        let v = ui
            .call(
                "hub.assignments.remove",
                demo(json!({ "env": "notebook", "confirm": true })),
            )
            .await;
        assert_eq!(v["result"]["rev"], 6, "{v}");
        assert_eq!(v["result"]["live_sessions"][0]["id"], s.id.as_str());
        // A sessão segue viva no hub: só o ambiente a encerra.
        {
            let h = st.hub.lock();
            let pid = h.find_project("deep-ion-ai", "demo").unwrap().unwrap().id;
            assert_eq!(h.live_sessions(pid, None).unwrap().len(), 1);
        }

        // Excluir o projeto: sem atribuição em "notebook", mas a sessão viva
        // relatada basta para exigir confirm.
        let v = ui
            .call("hub.assignments.remove", demo(json!({ "env": "wsl" })))
            .await;
        assert_eq!(v["result"]["rev"], 7, "{v}");
        let v = ui.call("hub.projects.delete", demo(json!({}))).await;
        let m = v["error"]["message"].as_str().unwrap();
        assert!(
            m.contains("nenhum ambiente") && m.contains(&s.id) && m.contains("confirm"),
            "{m}"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn descartar_quarentena_exige_confirmacao() {
        let tmp = tempfile::tempdir().unwrap();
        let (st, _gh) = hub_com_github(tmp.path());
        let url = spawn_server(&st).await;
        let mut ui = Ui::new(&url).await;
        push_novo(&st, &Ledger::open_in_memory().unwrap(), 1);
        let q = json!({ "owner": "deep-ion-ai", "repo": "novo" });
        let v = ui.call("hub.quarantine.discard", q.clone()).await;
        assert!(
            v["error"]["message"].as_str().unwrap().contains("confirm"),
            "{v}"
        );
        let mut c = q.clone();
        c["confirm"] = json!(true);
        let v = ui.call("hub.quarantine.discard", c).await;
        assert!(v["result"]["discarded"].as_i64().unwrap() > 0, "{v}");
        let v = ui.call("hub.quarantine.discard", q).await;
        assert_eq!(v["result"]["discarded"], 0);
        let v = ui.call("hub.quarantine.list", json!({})).await;
        assert_eq!(v["result"], json!([]));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn cadastro_sem_github_e_recusado() {
        let st = hub_com_dados();
        let url = spawn_server(&st).await;
        let mut ui = Ui::new(&url).await;
        let v = ui
            .call(
                "hub.projects.add",
                json!({ "url": "https://github.com/deep-ion-ai/x" }),
            )
            .await;
        assert_eq!(v["error"]["code"], -32000);
        // Sem backup_dir, a exclusão não roda sem o snapshot prévio.
        let v = ui
            .call(
                "hub.projects.delete",
                json!({ "owner": "deep-ion-ai", "repo": "demo", "confirm": true }),
            )
            .await;
        assert!(
            v["error"]["message"].as_str().unwrap().contains("snapshot"),
            "{v}"
        );
        assert_eq!(st.hub.lock().list_projects().unwrap().len(), 1);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn token_do_github_do_hub_sem_expor_o_token() {
        use ct_gh::fake::{fake_gh, FakeGitHub};
        use ct_gh::{GhClient, GhConfig};
        let tmp = tempfile::tempdir().unwrap();
        let gh = FakeGitHub::start("tok");
        gh.set("/user", json!({ "login": "octocat" }));
        let bin = fake_gh(tmp.path(), "tok");
        let file = tmp.path().join("github.token");
        let client = GhClient::new(
            GhConfig::new(&gh.url(), bin.to_str().unwrap())
                .with_token_timeout(ct_gh::fake::FAKE_GH_TOKEN_TIMEOUT)
                .with_token_file(file.clone()),
        );
        let mut st = hub_com_dados();
        st.github = Some(std::sync::Arc::new(crate::github::HubGithub::new(
            st.hub.clone(),
            client,
            "hub-id",
            st.events.clone(),
        )));
        let url = spawn_server(&st).await;
        let mut ui = Ui::new(&url).await;
        let v = ui.call("github.status", json!({})).await;
        assert_eq!(v["result"]["source"], "gh", "{v}");
        assert_eq!(v["result"]["login"], "octocat");
        let v = ui
            .call("github.set_token", json!({ "token": "invalido" }))
            .await;
        assert_eq!(v["error"]["code"], -32000, "{v}");
        let v = ui.call("github.set_token", json!({ "token": "tok" })).await;
        assert_eq!(v["result"]["source"], "token", "{v}");
        assert!(!v.to_string().contains("\"tok\""), "{v}");
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "tok");
        let v = ui.call("github.clear_token", json!({})).await;
        assert_eq!(v["result"]["source"], "gh", "{v}");
        assert!(!file.exists());
        let v = ui.call("github.set_token", json!({})).await;
        assert_eq!(v["error"]["code"], -32602);
    }

    /// Hub de dados com o store de secrets aberto em `tmp`.
    fn hub_com_secrets(tmp: &std::path::Path) -> HubState {
        let dirs = crate::config::Dirs {
            config: tmp.join("config"),
            data: tmp.join("data"),
        };
        dirs.ensure().unwrap();
        let mut st = hub_com_dados();
        st.secrets = Some(std::sync::Arc::new(
            crate::secrets::HubSecrets::open(&dirs, st.hub.clone()).unwrap(),
        ));
        st
    }

    const SEGREDO: &str = "valor-que-nunca-sai-987";

    #[tokio::test(flavor = "multi_thread")]
    async fn secrets_list_nunca_devolve_o_valor_de_um_secret() {
        let tmp = tempfile::tempdir().unwrap();
        let st = hub_com_secrets(tmp.path());
        let url = spawn_server(&st).await;
        let mut ui = Ui::new(&url).await;
        let demo = json!({ "project": "Deep-Ion-AI/demo" });
        let set = |scope: Value, name: &str, kind: &str, value: &str| json!({ "scope": scope, "name": name, "kind": kind, "value": value });

        // Criar nos dois escopos.
        let v = ui
            .call(
                "hub.secrets.set",
                set(json!("global"), "API_TOKEN", "secret", SEGREDO),
            )
            .await;
        assert_eq!(v["result"]["rev"], 2, "{v}");
        let v = ui
            .call(
                "hub.secrets.set",
                set(json!("global"), "RUST_LOG", "variable", "info"),
            )
            .await;
        assert_eq!(v["result"]["rev"], 3, "{v}");
        let v = ui
            .call(
                "hub.secrets.set",
                set(demo.clone(), "API_TOKEN", "secret", &format!("{SEGREDO}-p")),
            )
            .await;
        assert_eq!(v["result"]["rev"], 4, "{v}");
        let v = ui
            .call(
                "hub.secrets.set",
                set(demo.clone(), "MODO", "variable", "dev"),
            )
            .await;
        assert_eq!(v["result"]["rev"], 5, "{v}");
        let v = ui.call("hub.info", json!({})).await;
        assert_eq!(v["result"]["config_rev"], 5);

        let v = ui.call("hub.secrets.list", json!({})).await;
        let texto = v.to_string();
        assert!(!texto.contains(SEGREDO), "{texto}");
        let r = &v["result"];
        assert_eq!(r["status"]["entries"], 4);
        assert_eq!(r["status"]["recovery_configured"], false);
        let items = r["items"].as_array().unwrap();
        let achar = |scope: Value, name: &str| {
            items
                .iter()
                .find(|i| i["scope"] == scope && i["name"] == name)
                .unwrap_or_else(|| panic!("{name}: {texto}"))
                .clone()
        };
        let g = achar(json!("global"), "API_TOKEN");
        assert_eq!(
            (g["kind"].clone(), g["rev"].clone()),
            (json!("secret"), json!(2))
        );
        assert!(g.get("value").is_none(), "{g}");
        assert!(g["updated_at"].as_i64().unwrap() > 0);
        let p = achar(json!({ "project": "deep-ion-ai/demo" }), "API_TOKEN");
        assert!(p.get("value").is_none(), "{p}");
        assert_eq!(achar(json!("global"), "RUST_LOG")["value"], "info");
        assert_eq!(
            achar(json!({ "project": "deep-ion-ai/demo" }), "MODO")["value"],
            "dev"
        );
        // O ambiente "wsl" falou com o hub, mas nunca mandou a chave.
        assert_eq!(r["delivery"][0]["env_name"], "wsl");
        assert_eq!(r["delivery"][0]["state"], "no_key");

        // Editar: trocar o tipo é permitido; virar secret esconde o valor.
        let v = ui
            .call(
                "hub.secrets.set",
                set(demo.clone(), "MODO", "secret", SEGREDO),
            )
            .await;
        assert_eq!(v["result"]["rev"], 6, "{v}");
        let v = ui
            .call(
                "hub.secrets.set",
                set(json!("global"), "RUST_LOG", "variable", "debug"),
            )
            .await;
        assert_eq!(v["result"]["rev"], 7, "{v}");
        let v = ui.call("hub.secrets.list", json!({})).await;
        assert!(!v.to_string().contains(SEGREDO), "{v}");
        let items = v["result"]["items"].as_array().unwrap().clone();
        let modo = items.iter().find(|i| i["name"] == "MODO").unwrap();
        assert_eq!(modo["kind"], "secret");
        assert!(modo.get("value").is_none(), "{modo}");
        let log = items.iter().find(|i| i["name"] == "RUST_LOG").unwrap();
        assert_eq!(
            (log["value"].clone(), log["rev"].clone()),
            (json!("debug"), json!(7))
        );

        // Remover nos dois escopos.
        let del = |scope: Value, name: &str| json!({ "scope": scope, "name": name });
        let v = ui
            .call("hub.secrets.delete", del(demo.clone(), "API_TOKEN"))
            .await;
        assert_eq!(v["result"]["rev"], 8, "{v}");
        let v = ui
            .call("hub.secrets.delete", del(json!("global"), "RUST_LOG"))
            .await;
        assert_eq!(v["result"]["rev"], 9, "{v}");
        let v = ui
            .call("hub.secrets.delete", del(json!("global"), "RUST_LOG"))
            .await;
        assert_eq!(v["error"]["code"], SECRET_NOT_FOUND, "{v}");
        let v = ui.call("hub.secrets.list", json!({})).await;
        let nomes: Vec<(Value, Value)> = v["result"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| (i["scope"].clone(), i["name"].clone()))
            .collect();
        assert_eq!(
            nomes,
            [
                (json!("global"), json!("API_TOKEN")),
                (json!({ "project": "deep-ion-ai/demo" }), json!("MODO")),
            ]
        );
        assert_eq!(v["result"]["status"]["store_rev"], 9);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn secrets_recusas_sem_o_valor_na_mensagem() {
        let tmp = tempfile::tempdir().unwrap();
        let st = hub_com_secrets(tmp.path());
        let url = spawn_server(&st).await;
        let mut ui = Ui::new(&url).await;
        let casos = [
            (
                json!({ "scope": "global", "name": "minusculo", "kind": "secret", "value": SEGREDO }),
                -32602,
            ),
            (
                json!({ "scope": "global", "name": "A", "kind": "outro", "value": SEGREDO }),
                -32602,
            ),
            (
                json!({ "scope": "todos", "name": "A", "kind": "secret", "value": SEGREDO }),
                -32602,
            ),
            (
                json!({ "scope": { "project": "sem-barra" }, "name": "A", "kind": "secret", "value": SEGREDO }),
                -32602,
            ),
            (
                json!({ "scope": "global", "name": "A", "kind": "secret", "value": "" }),
                -32602,
            ),
            (
                json!({ "scope": "global", "name": "A", "kind": "secret" }),
                -32602,
            ),
            (
                json!({ "scope": { "project": "x/nao-cadastrado" }, "name": "A", "kind": "secret", "value": SEGREDO }),
                SECRET_UNKNOWN_PROJECT,
            ),
            // Nomes reservados (#166): o ambiente já define para a sessão.
            (
                json!({ "scope": "global", "name": "CTOWER_TOKEN", "kind": "secret", "value": SEGREDO }),
                -32602,
            ),
            (
                json!({ "scope": "global", "name": "PATH", "kind": "variable", "value": SEGREDO }),
                -32602,
            ),
            (
                json!({ "scope": "global", "name": "LD_PRELOAD", "kind": "variable", "value": SEGREDO }),
                -32602,
            ),
        ];
        for (p, code) in casos {
            let v = ui.call("hub.secrets.set", p.clone()).await;
            assert_eq!(v["error"]["code"], code, "{p}: {v}");
            assert!(!v.to_string().contains(SEGREDO), "{v}");
            if ct_secrets::is_reserved_name(p["name"].as_str().unwrap()) {
                let msg = v["error"]["message"].as_str().unwrap();
                assert!(msg.contains("nome reservado"), "{msg}");
            }
        }
        let v = ui.call("hub.info", json!({})).await;
        assert_eq!(
            v["result"]["config_rev"], 1,
            "recusas não mexem no config_rev"
        );

        // Hub sem store: erro próprio.
        let st = hub_com_dados();
        let url = spawn_server(&st).await;
        let mut ui = Ui::new(&url).await;
        for m in ["hub.secrets.list", "hub.secrets.set", "hub.secrets.delete"] {
            let v = ui.call(m, json!({})).await;
            assert_eq!(v["error"]["code"], SECRETS_UNAVAILABLE, "{m}: {v}");
        }
    }

    /// Estado de entrega: sem chave, pendente até o ambiente confirmar a
    /// revisão (globais, projetos atribuídos e remoções), entregue depois.
    #[tokio::test(flavor = "multi_thread")]
    async fn secrets_estado_de_entrega_por_ambiente() {
        let tmp = tempfile::tempdir().unwrap();
        let st = hub_com_secrets(tmp.path());
        let url = spawn_server(&st).await;
        let mut ui = Ui::new(&url).await;
        let env_id = st.hub.lock().list_envs().unwrap()[0].env_id.clone();
        let epoch = st.hub.lock().config_epoch().unwrap();
        let estado = |v: &Value| {
            let d = &v["result"]["delivery"][0];
            (
                d["state"].as_str().unwrap().to_string(),
                d["required_rev"].as_i64().unwrap(),
            )
        };
        // O ambiente manda a chave, recebe a entrega e confirma a revisão.
        let confirma = |rev: i64| {
            let h = st.hub.lock();
            h.set_env_pubkey(&env_id, "age1chave").unwrap();
            h.mark_secrets_delivered(&env_id, "age1chave").unwrap();
            h.record_env_applied(&env_id, Some(rev), Some(epoch))
                .unwrap();
        };
        let set = |scope: Value, name: &str| json!({ "scope": scope, "name": name, "kind": "secret", "value": SEGREDO });
        let v = ui.call("hub.secrets.list", json!({})).await;
        assert_eq!(estado(&v), ("no_key".into(), 0));
        confirma(1);
        let v = ui.call("hub.secrets.list", json!({})).await;
        assert_eq!(estado(&v), ("delivered".into(), 0));

        // Projeto não atribuído: não afeta o ambiente.
        let demo = json!({ "project": "deep-ion-ai/demo" });
        ui.call("hub.secrets.set", set(demo.clone(), "D")).await;
        let v = ui.call("hub.secrets.list", json!({})).await;
        assert_eq!(estado(&v), ("delivered".into(), 0));
        // Global: pendente até confirmar.
        let r = ui.call("hub.secrets.set", set(json!("global"), "G")).await["result"]["rev"]
            .as_i64()
            .unwrap();
        let v = ui.call("hub.secrets.list", json!({})).await;
        assert_eq!(estado(&v), ("pending".into(), r));
        confirma(r);
        let v = ui.call("hub.secrets.list", json!({})).await;
        assert_eq!(estado(&v), ("delivered".into(), r));
        // Atribuir o projeto com secrets: a atribuição entra na conta.
        let a = st
            .hub
            .lock()
            .add_assignment("deep-ion-ai", "demo", &env_id, None)
            .unwrap()
            .assignment
            .rev;
        let v = ui.call("hub.secrets.list", json!({})).await;
        assert_eq!(estado(&v), ("pending".into(), a));
        assert_eq!(
            v["result"]["delivery"][0]["projects"],
            json!(["deep-ion-ai/demo"])
        );
        confirma(a);
        // Remover um secret do projeto atribuído também deixa pendente.
        let r = ui
            .call("hub.secrets.delete", json!({ "scope": demo, "name": "D" }))
            .await["result"]["rev"]
            .as_i64()
            .unwrap();
        let v = ui.call("hub.secrets.list", json!({})).await;
        assert_eq!(estado(&v), ("pending".into(), r));
        confirma(r);
        let v = ui.call("hub.secrets.list", json!({})).await;
        assert_eq!(estado(&v), ("delivered".into(), r));
        // Chave nova ainda sem entrega: pendente.
        st.hub.lock().set_env_pubkey(&env_id, "age1outra").unwrap();
        let v = ui.call("hub.secrets.list", json!({})).await;
        assert_eq!(estado(&v).0, "pending");
    }

    /// Lista "a rotacionar na origem" (#170): vem no `hub.secrets.list`, vira
    /// aviso no `hub.status` e some com o dismiss (que apaga o arquivo).
    #[tokio::test(flavor = "multi_thread")]
    async fn secrets_a_rotacionar_pelo_ws() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("config/secrets.rotate-pending.json");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(
            &file,
            r#"{"restored_at":5,"items":[{"scope":"global","name":"API_TOKEN"},{"scope":{"project":"deep-ion-ai/demo"},"name":"D"}]}"#,
        )
        .unwrap();
        let st = hub_com_secrets(tmp.path());
        let url = spawn_server(&st).await;
        let mut ui = Ui::new(&url).await;
        let v = ui.call("hub.secrets.list", json!({})).await;
        let p = &v["result"]["rotate_pending"];
        assert_eq!(p["restored_at"], 5, "{v}");
        assert_eq!(p["items"][0]["name"], "API_TOKEN");
        assert_eq!(p["items"][1]["scope"]["project"], "deep-ion-ai/demo");
        let tem_aviso = |v: &Value| {
            v["result"]["warnings"]
                .as_array()
                .unwrap()
                .iter()
                .any(|w| w["kind"] == "secrets_rotate_pending")
        };
        let v = ui.call("hub.status", json!({})).await;
        assert!(tem_aviso(&v), "{v}");

        let v = ui
            .call("hub.secrets.rotate_pending_dismiss", json!({}))
            .await;
        assert!(v["error"].is_null(), "{v}");
        assert!(!file.exists());
        let v = ui.call("hub.secrets.list", json!({})).await;
        assert!(v["result"]["rotate_pending"].is_null(), "{v}");
        let v = ui.call("hub.status", json!({})).await;
        assert!(!tem_aviso(&v), "{v}");
        // De novo, sem arquivo: não é erro.
        let v = ui
            .call("hub.secrets.rotate_pending_dismiss", json!({}))
            .await;
        assert!(v["error"].is_null(), "{v}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn chave_de_recuperacao_pelo_ws() {
        let tmp = tempfile::tempdir().unwrap();
        let st = hub_com_secrets(tmp.path());
        let url = spawn_server(&st).await;
        let mut ui = Ui::new(&url).await;
        let kinds = |v: &Value| -> Vec<String> {
            v["result"]["warnings"]
                .as_array()
                .unwrap()
                .iter()
                .map(|w| w["kind"].as_str().unwrap().to_string())
                .collect()
        };

        let v = ui.call("hub.recovery.status", json!({})).await;
        assert_eq!(v["result"]["configured"], false, "{v}");
        assert_eq!(v["result"]["reverify_days"], 90);
        let v = ui.call("hub.status", json!({})).await;
        assert!(kinds(&v).contains(&"recovery_not_configured".into()), "{v}");
        let outra = ct_secrets::identity_text(&ct_secrets::Identity::generate());
        let v = ui
            .call("hub.recovery.verify", json!({ "key": outra.as_str() }))
            .await;
        assert_eq!(v["error"]["code"], RECOVERY_NOT_STARTED, "{v}");

        let v = ui.call("hub.recovery.generate", json!({})).await;
        let chave = v["result"]["private_key"].as_str().unwrap().to_string();
        assert!(chave.starts_with("AGE-SECRET-KEY-1"), "{v}");
        assert_eq!(v["result"]["status"]["pending"], true);

        // Chave errada, inválida ou ausente: erro sem eco do que veio.
        let v = ui
            .call("hub.recovery.verify", json!({ "key": outra.as_str() }))
            .await;
        assert_eq!(v["error"]["code"], RECOVERY_KEY_MISMATCH, "{v}");
        assert!(!v.to_string().contains(outra.as_str()));
        let v = ui
            .call(
                "hub.recovery.verify",
                json!({ "key": "AGE-SECRET-KEY-1RUIM" }),
            )
            .await;
        assert_eq!(v["error"]["code"], -32602, "{v}");
        assert!(!v.to_string().contains("RUIM"), "{v}");
        let v = ui.call("hub.recovery.verify", json!({})).await;
        assert_eq!(v["error"]["code"], -32602, "{v}");
        let v = ui.call("hub.recovery.status", json!({})).await;
        assert_eq!(
            (
                v["result"]["configured"].clone(),
                v["result"]["pending"].clone()
            ),
            (json!(false), json!(true))
        );

        let v = ui
            .call("hub.recovery.verify", json!({ "key": chave }))
            .await;
        assert_eq!(v["result"]["configured"], true, "{v}");
        assert_eq!(v["result"]["pending"], false);
        assert!(v["result"]["last_verified_at"].as_i64().is_some());
        assert!(!v.to_string().contains(&chave));
        let v = ui.call("hub.status", json!({})).await;
        assert!(!kinds(&v).iter().any(|k| k.starts_with("recovery_")), "{v}");
        let v = ui.call("hub.secrets.list", json!({})).await;
        assert_eq!(v["result"]["status"]["recovery_configured"], true);
        let rec = ct_secrets::parse_identity(&chave).ok().unwrap();
        let dirs_config = tmp.path().join("config");
        ct_secrets::SecretsStore::open(dirs_config.join("secrets.age"), rec, &[]).unwrap();

        // Sem store: indisponível.
        let sem = hub_com_dados();
        let url = spawn_server(&sem).await;
        let mut ui = Ui::new(&url).await;
        let v = ui.call("hub.recovery.generate", json!({})).await;
        assert_eq!(v["error"]["code"], SECRETS_UNAVAILABLE, "{v}");
    }

    /// `hub.backup.*`: salva com credenciais, testa, faz backup e lista o
    /// remoto; a credencial não volta em nenhuma resposta (#263).
    #[tokio::test(flavor = "multi_thread")]
    async fn backup_remoto_pela_ui() {
        use crate::remote::fake;
        let d = tempfile::tempdir().unwrap();
        let dirs = crate::config::Dirs {
            config: d.path().join("config"),
            data: d.path().join("data"),
        };
        dirs.ensure().unwrap();
        let (endpoint, objs) = fake::serve();
        let mut st = hub_com_dados();
        let secrets =
            std::sync::Arc::new(crate::secrets::HubSecrets::open(&dirs, st.hub.clone()).unwrap());
        st.backups = std::sync::Arc::new(
            crate::backup::Backups::new(
                d.path().join("snaps"),
                std::time::Duration::from_secs(3600),
                3,
                None,
            )
            .with_docs(
                Some(d.path().join("docs")),
                std::time::Duration::from_secs(86400),
                7,
                1024,
            )
            .with_remote(dirs.backup_settings_file(), std::sync::Arc::clone(&secrets)),
        );
        std::fs::create_dir_all(d.path().join("docs/acme__app")).unwrap();
        std::fs::write(d.path().join("docs/acme__app/nota.md"), "nota").unwrap();
        st.secrets = Some(secrets);
        let url = spawn_server(&st).await;
        let mut ui = Ui::new(&url).await;
        let remote = json!({ "kind": "s3", "bucket": "bk", "endpoint": endpoint,
                             "encrypt": true });

        // Sem credenciais o destino não passa na validação.
        let v = ui.call("hub.backup.set", json!({ "remote": remote })).await;
        assert_eq!(v["error"]["code"], -32602, "{v}");

        let creds = json!({ "access_key": "AK-UI", "secret_key": "SK-UI-SECRETA" });
        let t = ui
            .call(
                "hub.backup.test",
                json!({ "remote": remote, "credentials": creds }),
            )
            .await;
        assert_eq!(t["result"]["ok"], true, "{t}");
        assert!(
            objs.lock().unwrap().objects.is_empty(),
            "arquivo de prova apagado"
        );

        let v = ui
            .call(
                "hub.backup.set",
                json!({ "remote": remote, "credentials": creds, "keep": 3 }),
            )
            .await;
        assert_eq!(
            v["result"]["status"]["remote"]["credentials_configured"], true,
            "{v}"
        );
        assert_eq!(v["result"]["remote"]["kind"], "s3");
        assert!(!v.to_string().contains("SECRETA"));

        // Os docs (#442) vêm num campo separado e com a agenda deles.
        let v = ui
            .call(
                "hub.backup.set",
                json!({ "docs_keep": 5, "docs_interval_hours": 12 }),
            )
            .await;
        assert_eq!(v["result"]["docs"]["keep"], 5, "{v}");
        assert_eq!(v["result"]["docs"]["interval_hours"], 12);
        assert!(v["result"]["docs"]["last_ok_at"].is_null());
        let v = ui.call("hub.backup.run", json!({ "what": "x" })).await;
        assert_eq!(v["error"]["code"], -32602, "{v}");

        let v = ui.call("hub.backup.run", json!({})).await;
        assert!(
            v["result"]["status"]["remote"]["last_name"]
                .as_str()
                .unwrap()
                .ends_with(".db.age"),
            "{v}"
        );
        assert!(
            v["result"]["docs"]["remote"]["last_name"]
                .as_str()
                .unwrap()
                .ends_with(".tar.zst.age"),
            "{v}"
        );
        assert!(v["result"]["docs"]["newest_name"]
            .as_str()
            .unwrap()
            .starts_with("docs-"));
        // A lista de snapshots do hub.db não mistura os pacotes dos docs.
        let v = ui.call("hub.backup.remote.list", json!({})).await;
        assert_eq!(v["result"].as_array().unwrap().len(), 1);
        // hub.status mostra o estado remoto sem credenciais.
        let v = ui.call("hub.status", json!({})).await;
        assert_eq!(v["result"]["backup"]["remote"]["kind"], "s3");
        assert!(!v.to_string().contains("SECRETA"));
        // E os secrets visíveis na UI não incluem a credencial.
        let v = ui.call("hub.secrets.list", json!({})).await;
        assert!(
            v["result"]["status"]["entries"].as_u64().unwrap() > 0,
            "{v}"
        );
        assert!(!v.to_string().contains("CTHUB_BACKUP"), "{v}");
        let v = ui
            .call(
                "hub.secrets.set",
                json!({ "scope": "global", "name": "CTHUB_X", "kind": "secret", "value": "v" }),
            )
            .await;
        assert_eq!(v["error"]["code"], -32602, "nome reservado: {v}");
    }

    // ---- /web/ws (#199) ----

    const ORIGIN: &str = "http://127.0.0.1:7879";

    /// Hub com dados e a web ligada, e o cookie de uma sessão web válida.
    fn hub_web() -> (HubState, String) {
        let mut st = hub_com_dados();
        st.web = hub_web_options(&st);
        let cookie = web_cookie(&st);
        (st, cookie)
    }

    fn hub_web_options(st: &HubState) -> crate::web::WebOptions {
        let cfg = crate::config::WebConfig {
            enabled: true,
            ..Default::default()
        };
        crate::web::WebOptions::new(&cfg, 7879, st.hub.clone(), UI)
    }

    fn web_cookie(st: &HubState) -> String {
        let s = st
            .hub
            .lock()
            .create_web_session(None, Some("teste"), 3_600_000)
            .unwrap();
        format!("ct_web={}", s.cookie)
    }

    async fn connect_web(url: &str, headers: &[(&str, &str)]) -> Result<Client, String> {
        let mut req = url.replace("/ws", "/web/ws").into_client_request().unwrap();
        for (k, v) in headers {
            let name: axum::http::HeaderName = k.parse().unwrap();
            req.headers_mut().insert(name, v.parse().unwrap());
        }
        connect_async(req)
            .await
            .map(|(ws, _)| ws)
            .map_err(|e| e.to_string())
    }

    async fn web_ui(url: &str, cookie: &str) -> Ui {
        Ui {
            ws: connect_web(
                url,
                &[("origin", ORIGIN), ("cookie", &format!("a=1; {cookie}"))],
            )
            .await
            .unwrap(),
            next: 0,
            pushes: Vec::new(),
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn web_ws_atende_a_lista_de_permissoes() {
        let (st, cookie) = hub_web();
        let url = spawn_server(&st).await;
        let mut web = web_ui(&url, &cookie).await;
        let v = web.call("hub.dashboard.get", json!({})).await;
        assert!(v["result"]["envs"].is_array(), "{v}");
        let v = web.call("hub.info", json!({})).await;
        assert_eq!(v["result"]["protocol"], crate::PROTOCOL_VERSION);
        let v = web.call("hub.projects.list", json!({})).await;
        assert_eq!(v["result"][0]["project"]["gh_repo"], "demo");
        // Ação da fase 1 que escreve: mudar status.
        let v = web
            .call(
                "hub.workitem.set_status",
                json!({ "owner": "deep-ion-ai", "repo": "demo", "number": 5,
                        "status": "bloqueado", "reason": "pelo celular" }),
            )
            .await;
        assert_eq!(v["result"]["status"], "bloqueado", "{v}");
        // Método desconhecido também cai na recusa da web.
        let v = web.call("hub.inventado", json!({})).await;
        assert_eq!(v["error"]["code"], -32601);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn sensiveis_recusados_na_web_e_aceitos_no_ws() {
        let (st, cookie) = hub_web();
        let url = spawn_server(&st).await;
        let mut web = web_ui(&url, &cookie).await;
        let mut ui = Ui::new(&url).await;
        let params = json!({ "owner": "deep-ion-ai", "repo": "demo", "token": "t",
                             "scope": "global", "name": "X", "value": "v", "key": "k" });
        for m in crate::web::WEB_SENSITIVE
            .iter()
            .chain(crate::web::WEB_REFUSED)
        {
            let v = web.call(m, params.clone()).await;
            assert_eq!(v["error"]["code"], -32601, "{m}: {v}");
            assert_eq!(v["error"]["message"], "método indisponível na web", "{m}");
        }
        // Nada mudou: o projeto segue no hub.
        let v = web.call("hub.projects.list", json!({})).await;
        assert_eq!(v["result"].as_array().unwrap().len(), 1);
        // No /ws, com o token de UI, os sensíveis chegam ao despacho (podem
        // falhar por outro motivo, como o store fechado, mas não pela lista).
        for m in crate::web::WEB_SENSITIVE {
            let v = ui.call(m, params.clone()).await;
            assert_ne!(v["error"]["code"], -32601, "{m}: {v}");
        }
        let v = ui
            .call("hub.projects.delete", json!({ "owner": "x", "repo": "y" }))
            .await;
        assert!(
            v["error"]["message"]
                .as_str()
                .unwrap()
                .contains("não está no hub"),
            "{v}"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn web_ws_repassa_so_os_pushes_da_web() {
        let (st, cookie) = hub_web();
        let url = spawn_server(&st).await;
        let mut web = web_ui(&url, &cookie).await;
        web.call("hub.info", json!({})).await; // conexão inscrita no canal
        let _ = st.events.send(json!({ "event": "interno", "data": {} }));
        let _ = st
            .events
            .send(json!({ "event": "hub_sync_status", "data": { "ok": true } }));
        st.proposals_changed();
        web.call("hub.info", json!({})).await;
        let events: Vec<_> = web.pushes.iter().map(|p| p["event"].clone()).collect();
        assert_eq!(
            events,
            [json!("hub_sync_status"), json!("hub.proposals.changed")],
            "{events:?}"
        );
    }

    /// Log do hub (#65) pela web: `hub.logs.tail` mascarado e o modo
    /// seguir só na conexão que pediu, mesmo sendo a do `/web/ws`.
    #[tokio::test(flavor = "multi_thread")]
    async fn logs_do_hub_na_web() {
        let dir = tempfile::tempdir().unwrap();
        let logs = ct_http::logs::install(
            ct_http::logs::Logs::open(dir.path(), "cthubd", 1 << 20, 2).unwrap(),
        );
        let tag = uuid::Uuid::new_v4().simple().to_string();
        let line = |message: String| ct_http::logs::LogLine {
            id: 0,
            ts: ct_core::now_ms(),
            level: "warn".into(),
            target: "cthubd::teste".into(),
            message,
            fields: Default::default(),
        };
        logs.write(line(format!("{tag} Bearer abc123")));
        let (st, cookie) = hub_web();
        let url = spawn_server(&st).await;
        let mut web = web_ui(&url, &cookie).await;
        let mut ui = Ui::new(&url).await;
        let v = web.call("hub.logs.tail", json!({ "q": tag })).await;
        assert_eq!(
            v["result"]["lines"][0]["message"],
            format!("{tag} Bearer ***"),
            "{v}"
        );
        let v = web.call("hub.logs.follow", json!({ "q": tag })).await;
        assert_eq!(v["result"], json!({}), "{v}");
        logs.write(line(format!("{tag} nova")));
        let push = loop {
            if let Some(p) = web.pushes.iter().find(|p| p["event"] == LOGS_PUSH) {
                break p.clone();
            }
            web.call("hub.info", json!({})).await;
        };
        assert_eq!(push["data"]["lines"][0]["message"], format!("{tag} nova"));
        ui.call("hub.info", json!({})).await;
        assert!(ui.pushes.iter().all(|p| p["event"] != LOGS_PUSH));
        let v = web.call("hub.logs.unfollow", json!({})).await;
        assert_eq!(v["result"], json!({}));
    }

    // ---- hub.env.call e env.push (#343) ----

    /// Ambiente falso no canal reverso de `env_id` ("x"): responde cada
    /// chamada com o método e os parâmetros e, em `session.start`, manda os
    /// pushes `permission_request` (fora da lista) e `session_started`.
    async fn fake_env(st: &HubState, url: &str, env_id: &str) -> tokio::task::JoinHandle<()> {
        let tok = {
            let h = st.hub.lock();
            let t = h.issue_env_token_with_env_id("x", env_id).unwrap().token;
            h.ingest(env_id, "x", &[]).unwrap();
            t
        };
        let mut req = url
            .replace("/ws", ct_http::sync::CHANNEL_PATH)
            .into_client_request()
            .unwrap();
        let h = req.headers_mut();
        h.insert("authorization", format!("Bearer {tok}").parse().unwrap());
        h.insert(ct_http::sync::ENV_ID_HEADER, env_id.parse().unwrap());
        let (ws, _) = connect_async(req).await.unwrap();
        let task = tokio::spawn(async move {
            let (mut tx, mut rx) = ws.split();
            while let Some(Ok(m)) = rx.next().await {
                let WsMsg::Text(t) = m else { continue };
                let v: Value = serde_json::from_str(&t).unwrap();
                if v["method"] == "session.start" {
                    for e in ["permission_request", "session_started"] {
                        let p = json!({ "event": e, "data": { "session_id": "s-9" } });
                        tx.send(WsMsg::Text(p.to_string().into())).await.unwrap();
                    }
                }
                let r = if v["method"] == "session.kill" {
                    json!({ "id": v["id"], "error": { "code": -32000, "message": "sessão não está rodando" } })
                } else if v["method"] == "tracker.issue_body" {
                    // "Ver issue" de projeto Jira (#434): 7 com descrição em
                    // wiki markup, 8 sem descrição, o resto com a senha recusada.
                    let n = v["params"]["number"].as_i64().unwrap_or(0);
                    let d = match n {
                        7 => json!("h2. Contexto\n*a* <script>x</script>"),
                        8 => Value::Null,
                        _ => {
                            let e = format!("Jira, PROJ-{n}: o Jira recusou a senha (401)");
                            let r =
                                json!({ "id": v["id"], "error": { "code": -32000, "message": e } });
                            let _ = tx.send(WsMsg::Text(r.to_string().into())).await;
                            continue;
                        }
                    };
                    json!({ "id": v["id"], "result": { "title": "Corrigir login", "description": d,
                            "url": format!("https://jira.x/browse/PROJ-{n}") } })
                } else {
                    json!({ "id": v["id"], "result": { "method": v["method"], "params": v["params"] } })
                };
                if tx.send(WsMsg::Text(r.to_string().into())).await.is_err() {
                    break;
                }
            }
        });
        let ch = Arc::clone(&st.channels);
        let id = env_id.to_string();
        tokio::time::timeout(T, async move {
            while !ch.is_online(&id) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("canal de pé");
        task
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn env_call_roteia_ao_ambiente_pelo_canal_na_web_e_no_ws() {
        let (st, cookie) = hub_web();
        let url = spawn_server(&st).await;
        let mut web = web_ui(&url, &cookie).await;
        let mut ui = Ui::new(&url).await;
        web.call("hub.info", json!({})).await; // inscrita nos pushes
        let env = fake_env(&st, &url, "env-x").await;

        let params = json!({ "owner": "deep-ion-ai", "repo": "demo", "gh_number": 5,
                             "prompt": "vai", "remote_control": true });
        let v = web
            .call(
                "hub.env.call",
                json!({ "env": "env-x", "method": "session.start", "params": params }),
            )
            .await;
        assert_eq!(
            v["result"],
            json!({ "method": "session.start", "params": params }),
            "{v}"
        );
        // O mesmo pelo /ws (desktop e testes), e sem params.
        let v = ui
            .call(
                "hub.env.call",
                json!({ "env": "env-x", "method": "env.info" }),
            )
            .await;
        assert_eq!(
            v["result"],
            json!({ "method": "env.info", "params": {} }),
            "{v}"
        );
        // Erro do ambiente volta como veio.
        let v = web
            .call(
                "hub.env.call",
                json!({ "env": "env-x", "method": "session.kill", "params": { "session_id": "s" } }),
            )
            .await;
        assert_eq!(
            v["error"],
            json!({ "code": -32000, "message": "sessão não está rodando" }),
            "{v}"
        );

        // Pushes: `env.online` e `env.push` chegam à web; o fora da lista do
        // canal (`permission_request`) nem sai do hub.
        web.call("hub.info", json!({})).await;
        let events: Vec<_> = web
            .pushes
            .iter()
            .map(|p| (p["event"].clone(), p["data"]["event"].clone()))
            .collect();
        assert_eq!(
            events,
            [
                (json!("env.online"), Value::Null),
                (json!("env.push"), json!("session_started")),
            ],
            "{:?}",
            web.pushes
        );
        assert_eq!(
            web.pushes[0]["data"],
            json!({ "env_id": "env-x", "online": true })
        );
        assert_eq!(
            web.pushes[1]["data"],
            json!({ "env_id": "env-x", "event": "session_started", "data": { "session_id": "s-9" } })
        );
        ui.call("hub.info", json!({})).await;
        assert!(
            ui.pushes.iter().any(|p| p["event"] == "env.push"),
            "{:?}",
            ui.pushes
        );
        let v = web.call("hub.envs.list", json!({})).await;
        let x = v["result"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["env_id"] == "env-x")
            .cloned()
            .unwrap();
        assert_eq!(x["online"], true, "{v}");

        // Caiu: offline em tempo real, com erro claro.
        env.abort();
        let ch = Arc::clone(&st.channels);
        tokio::time::timeout(T, async move {
            while ch.is_online("env-x") {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        let v = web
            .call(
                "hub.env.call",
                json!({ "env": "env-x", "method": "session.resume", "params": { "session_id": "s" } }),
            )
            .await;
        assert_eq!(
            v["error"]["code"],
            crate::channel::CHANNEL_ENV_OFFLINE,
            "{v}"
        );
        let msg = v["error"]["message"].as_str().unwrap();
        assert!(msg.starts_with("ambiente offline: x "), "{msg}");
        web.call("hub.info", json!({})).await;
        assert_eq!(
            web.pushes.last().unwrap(),
            &json!({ "event": "env.online", "data": { "env_id": "env-x", "online": false } })
        );
    }

    /// "Ver issue" de projeto GitHub (#434): a descrição vem da API agora, a
    /// cada chamada, pelo /ws e pela web; nada dela fica no hub.
    #[tokio::test(flavor = "multi_thread")]
    async fn workitem_body_do_github_de_passagem() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut st, gh) = hub_com_github(tmp.path());
        st.web = hub_web_options(&st);
        let cookie = web_cookie(&st);
        let url = spawn_server(&st).await;
        let mut ui = Ui::new(&url).await;
        let mut web = web_ui(&url, &cookie).await;
        let issue = |body: Value| {
            json!({ "number": 5, "title": "Corrigir login", "body": body,
                    "html_url": "https://github.com/deep-ion-ai/demo/issues/5" })
        };
        gh.set(
            "/repos/deep-ion-ai/demo/issues/5",
            issue(json!("## Contexto\n- [ ] critério")),
        );
        let p = json!({ "owner": "deep-ion-ai", "repo": "demo", "number": 5 });
        for c in [&mut ui, &mut web] {
            let v = c.call("hub.workitem.body", p.clone()).await;
            assert_eq!(
                v["result"],
                json!({ "title": "Corrigir login", "body": "## Contexto\n- [ ] critério",
                        "format": "markdown",
                        "url": "https://github.com/deep-ion-ai/demo/issues/5" }),
                "{v}"
            );
        }
        // Mudou no GitHub: a próxima leitura já vem nova (sem cache).
        gh.set("/repos/deep-ion-ai/demo/issues/5", issue(Value::Null));
        let v = ui.call("hub.workitem.body", p.clone()).await;
        assert_eq!(v["result"]["body"], "", "sem descrição: {v}");
        // Nada da descrição ficou no hub.
        let tree = ui
            .call(
                "hub.tree.get",
                json!({ "owner": "deep-ion-ai", "repo": "demo" }),
            )
            .await;
        assert!(!tree.to_string().contains("critério"), "{tree}");

        // Erros: issue inexistente, projeto fora do hub e parâmetros.
        let v = ui
            .call(
                "hub.workitem.body",
                json!({ "owner": "deep-ion-ai", "repo": "demo", "number": 99 }),
            )
            .await;
        assert!(
            v["error"]["message"]
                .as_str()
                .unwrap()
                .contains("não encontrada"),
            "{v}"
        );
        let v = ui
            .call(
                "hub.workitem.body",
                json!({ "owner": "x", "repo": "y", "number": 1 }),
            )
            .await;
        assert!(
            v["error"]["message"]
                .as_str()
                .unwrap()
                .contains("não está no hub"),
            "{v}"
        );
        let v = ui
            .call(
                "hub.workitem.body",
                json!({ "owner": "deep-ion-ai", "repo": "demo" }),
            )
            .await;
        assert_eq!(v["error"]["code"], -32602, "{v}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn workitem_body_sem_github_no_hub() {
        let st = hub_com_dados();
        let url = spawn_server(&st).await;
        let mut ui = Ui::new(&url).await;
        let v = ui
            .call(
                "hub.workitem.body",
                json!({ "owner": "deep-ion-ai", "repo": "demo", "number": 5 }),
            )
            .await;
        assert!(
            v["error"]["message"]
                .as_str()
                .unwrap()
                .contains("GitHub desativada"),
            "{v}"
        );
    }

    /// "Ver issue" de projeto Jira (#434): o hub pede ao ambiente de sync pelo
    /// canal e converte a descrição para Markdown; sem ambiente de sync,
    /// offline ou com erro do Jira, a falha volta clara.
    #[tokio::test(flavor = "multi_thread")]
    async fn workitem_body_do_jira_pelo_ambiente_de_sync() {
        let (st, cookie) = hub_web();
        let url = spawn_server(&st).await;
        let mut ui = Ui::new(&url).await;
        let mut web = web_ui(&url, &cookie).await;
        let v = ui
            .call(
                "hub.projects.add",
                json!({
                    "tracker": { "kind": "jira", "url": "https://jira.x", "project_key": "PROJ",
                                 "jql": "", "active_sprint_only": true,
                                 "credential_cmd": "pass show jira" },
                    "forge": { "kind": "gitlab", "clone_url": "git@git.x:pje/app.git",
                               "credential_cmd": "pass show gitlab" }
                }),
            )
            .await;
        assert!(v["result"].is_object(), "{v}");
        let p = |n: i64| json!({ "owner": "pje", "repo": "app", "number": n });

        let v = ui.call("hub.workitem.body", p(7)).await;
        assert!(
            v["error"]["message"]
                .as_str()
                .unwrap()
                .contains("sem ambiente de sync"),
            "{v}"
        );
        st.hub.lock().ingest("env-x", "x", &[]).unwrap();
        let v = ui
            .call(
                "hub.assignments.add",
                json!({ "owner": "pje", "repo": "app", "env": "env-x" }),
            )
            .await;
        assert_eq!(v["result"]["sync_env"], true, "{v}");
        let v = ui.call("hub.workitem.body", p(7)).await;
        assert_eq!(
            v["error"]["code"],
            crate::channel::CHANNEL_ENV_OFFLINE,
            "{v}"
        );

        let _env = fake_env(&st, &url, "env-x").await;
        for c in [&mut ui, &mut web] {
            let v = c.call("hub.workitem.body", p(7)).await;
            assert_eq!(
                v["result"],
                json!({ "title": "Corrigir login",
                        "body": "## Contexto\n**a** &lt;script>x&lt;/script>",
                        "format": "markdown", "url": "https://jira.x/browse/PROJ-7" }),
                "{v}"
            );
        }
        let v = ui.call("hub.workitem.body", p(8)).await;
        assert_eq!(v["result"]["body"], "", "{v}");
        let v = ui.call("hub.workitem.body", p(9)).await;
        assert_eq!(
            v["error"]["message"], "Jira, PROJ-9: o Jira recusou a senha (401)",
            "{v}"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn env_call_recusa_metodo_fora_da_lista_e_ambiente_desconhecido() {
        let (st, cookie) = hub_web();
        let url = spawn_server(&st).await;
        let mut web = web_ui(&url, &cookie).await;
        let mut ui = Ui::new(&url).await;
        let _env = fake_env(&st, &url, "env-x").await;
        for m in [
            "session.input",
            "session.attach",
            "session.permission.respond",
            "cleanup.run",
            "env.update",
            "env.config.set",
            "github.set_token",
            "sync.now",
            "hub.secrets.list",
        ] {
            for c in [&mut web, &mut ui] {
                let v = c
                    .call(
                        "hub.env.call",
                        json!({ "env": "env-x", "method": m, "params": {} }),
                    )
                    .await;
                assert_eq!(v["error"]["code"], -32601, "{m}: {v}");
                assert_eq!(
                    v["error"]["message"],
                    format!("método indisponível pelo hub: {m}"),
                    "{m}"
                );
            }
        }
        let v = web
            .call(
                "hub.env.call",
                json!({ "env": "nada", "method": "env.info" }),
            )
            .await;
        assert_eq!(v["error"]["code"], -32602, "{v}");
        let v = web
            .call(
                "hub.env.call",
                json!({ "env": "env-x", "method": "env.info", "params": [1] }),
            )
            .await;
        assert_eq!(v["error"]["code"], -32602, "{v}");
        let v = web
            .call("hub.env.call", json!({ "method": "env.info" }))
            .await;
        assert_eq!(v["error"]["code"], -32602, "{v}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn upgrade_do_web_ws_confere_origin_e_sessao() {
        let (st, cookie) = hub_web();
        let url = spawn_server(&st).await;
        let ok = ("cookie", cookie.as_str());
        for (h, want) in [
            (vec![ok], "403"),                                     // sem Origin
            (vec![("origin", "https://evil.example"), ok], "403"), // Origin errado
            (vec![("origin", "http://localhost:1420"), ok], "403"),
            (vec![("origin", ORIGIN)], "401"), // sem cookie
            (vec![("origin", ORIGIN), ("cookie", "ct_web=errado")], "401"),
            (
                vec![
                    ("origin", ORIGIN),
                    ok,
                    ("authorization", "Bearer ui-secreto"),
                ],
                "400",
            ),
        ] {
            let err = connect_web(&url, &h).await.unwrap_err();
            assert!(err.contains(want), "{h:?}: {err}");
        }
        assert!(
            connect_web(&url, &[("origin", "http://localhost:7879"), ok])
                .await
                .is_ok()
        );
        // O token de UI não abre o /web/ws sem sessão web.
        let err = connect_web(
            &url,
            &[("origin", ORIGIN), ("authorization", "Bearer ui-secreto")],
        )
        .await
        .unwrap_err();
        assert!(err.contains("400"), "{err}");

        // Web desligada: o /web/ws nem existe; o /ws segue igual.
        let st = hub_com_dados();
        let url = spawn_server(&st).await;
        let err = connect_web(&url, &[("origin", ORIGIN), ok])
            .await
            .unwrap_err();
        assert!(err.contains("404"), "{err}");
        assert!(connect(&url, UI).await.is_ok());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn sessoes_web_listadas_e_revogadas_pelos_dois_canais() {
        let (st, cookie) = hub_web();
        let url = spawn_server(&st).await;
        let other = st
            .hub
            .lock()
            .create_web_session(Some("eu@github"), None, 3_600_000)
            .unwrap();
        let mut web = web_ui(&url, &cookie).await;
        let mut ui = Ui::new(&url).await;
        let v = web.call("hub.web.sessions.list", json!({})).await;
        let list = v["result"].as_array().unwrap();
        assert_eq!(list.len(), 2, "{v}");
        let me = cookie
            .trim_start_matches("ct_web=")
            .split('.')
            .next()
            .unwrap();
        for s in list {
            assert_eq!(s["current"], s["id"] == me, "{s}");
            assert!(s.get("secret_hash").is_none(), "{s}");
        }
        assert!(!v.to_string().contains(cookie.split('.').nth(1).unwrap()));
        // No /ws (desktop) nenhuma é a atual.
        let v = ui.call("hub.web.sessions.list", json!({})).await;
        assert!(v["result"]
            .as_array()
            .unwrap()
            .iter()
            .all(|s| s["current"] == false));
        let v = ui
            .call("hub.web.sessions.revoke", json!({ "id": other.id }))
            .await;
        assert_eq!(v["result"]["revoked"], 1, "{v}");
        let v = ui.call("hub.web.sessions.revoke", json!({})).await;
        assert_eq!(v["error"]["code"], -32602, "{v}");
        let v = web.call("hub.web.sessions.list", json!({})).await;
        assert_eq!(v["result"].as_array().unwrap().len(), 1, "{v}");

        // Revogar todas pelo /web/ws derruba a própria conexão.
        let v = web
            .call("hub.web.sessions.revoke", json!({ "all": true }))
            .await;
        assert_eq!(v["result"]["revoked"], 1, "{v}");
        web.ws
            .send(WsMsg::Text(
                json!({ "id": 99, "method": "hub.info" }).to_string().into(),
            ))
            .await
            .unwrap();
        let m = tokio::time::timeout(T, web.ws.next()).await.expect("prazo");
        match m {
            Some(Ok(WsMsg::Close(Some(f)))) => assert_eq!(u16::from(f.code), 4401),
            other => panic!("esperava Close: {other:?}"),
        }
        // E o cookie não abre outra.
        let err = connect_web(&url, &[("origin", ORIGIN), ("cookie", &cookie)])
            .await
            .unwrap_err();
        assert!(err.contains("401"), "{err}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn sessao_vencida_por_inatividade_recusada() {
        let (st, cookie) = hub_web();
        let url = spawn_server(&st).await;
        let idle = st.web.auth.idle_ms();
        st.hub
            .lock()
            .conn()
            .execute(
                "UPDATE web_session SET last_used_at = last_used_at - ?1",
                [idle],
            )
            .unwrap();
        let err = connect_web(&url, &[("origin", ORIGIN), ("cookie", &cookie)])
            .await
            .unwrap_err();
        assert!(err.contains("401"), "{err}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn vindo_pela_cloudflare_so_as_rotas_web() {
        let (st, cookie) = hub_web();
        let url = spawn_server(&st).await;
        let tok = st.hub.lock().add_legacy_env_token("a").unwrap();
        let mut req = url.as_str().into_client_request().unwrap();
        req.headers_mut()
            .insert("authorization", format!("Bearer {UI}").parse().unwrap());
        req.headers_mut()
            .insert("cf-connecting-ip", "203.0.113.9".parse().unwrap());
        let err = connect_async(req).await.unwrap_err().to_string();
        assert!(err.contains("404"), "{err}");
        let base = url.replace("ws://", "http://").replace("/ws", "");
        let agent = st.hub.lock().add_agent_token("a").unwrap();
        for (path, bearer) in [
            ("/sync/config", tok.as_str()),
            ("/sync/done", tok.as_str()),
            ("/agent/v1/overview", agent.as_str()),
        ] {
            // Sem o header, a rota responde (o que for, menos 404).
            let ok = ureq::get(&format!("{base}{path}"))
                .set("authorization", &format!("Bearer {bearer}"))
                .call();
            assert!(
                !matches!(ok, Err(ureq::Error::Status(404, _))),
                "{path}: {ok:?}"
            );
            let r = ureq::get(&format!("{base}{path}"))
                .set("authorization", &format!("Bearer {bearer}"))
                .set("cf-connecting-ip", "203.0.113.9")
                .call();
            match r {
                Err(ureq::Error::Status(404, _)) => {}
                r => panic!("{path}: {r:?}"),
            }
        }
        let r = ureq::post(&format!("{base}/sync/push"))
            .set("authorization", &format!("Bearer {tok}"))
            .set("cf-connecting-ip", "203.0.113.9")
            .send_string("{}");
        assert!(matches!(r, Err(ureq::Error::Status(404, _))), "{r:?}");
        // As rotas web seguem acessíveis.
        assert!(connect_web(
            &url,
            &[
                ("origin", ORIGIN),
                ("cookie", &cookie),
                ("cf-connecting-ip", "203.0.113.9")
            ]
        )
        .await
        .is_ok());
        let me = ureq::get(&format!("{base}/web/me"))
            .set("cf-connecting-ip", "203.0.113.9")
            .call()
            .unwrap();
        assert_eq!(me.status(), 200);
    }

    // ---- Web Push (#203) ----

    /// Serviço de push local: guarda cada `POST /push/<id>` (cabeçalhos e
    /// corpo) e responde 201, ou 410 em `/push/gone`.
    /// Requisição recebida: cabeçalhos, corpo e o `<id>` do caminho.
    type Delivered = (axum::http::HeaderMap, Vec<u8>, String);

    #[derive(Clone, Default)]
    struct FakePushService {
        got: std::sync::Arc<std::sync::Mutex<Vec<Delivered>>>,
    }

    impl FakePushService {
        async fn spawn(&self) -> String {
            use axum::extract::{Path as AxPath, State as AxState};
            async fn handle(
                AxState(f): AxState<FakePushService>,
                AxPath(id): AxPath<String>,
                headers: axum::http::HeaderMap,
                body: axum::body::Bytes,
            ) -> axum::http::StatusCode {
                f.got
                    .lock()
                    .unwrap()
                    .push((headers, body.to_vec(), id.clone()));
                if id == "gone" {
                    axum::http::StatusCode::GONE
                } else {
                    axum::http::StatusCode::CREATED
                }
            }
            let app = axum::Router::new()
                .route("/push/{id}", axum::routing::post(handle))
                .with_state(self.clone());
            let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = l.local_addr().unwrap();
            tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
            format!("http://{addr}/push")
        }

        fn take(&self) -> Vec<Delivered> {
            std::mem::take(&mut *self.got.lock().unwrap())
        }
    }

    /// Hub com a web e a Web Push ligadas (endpoint http do serviço local).
    fn hub_push(tmp: &std::path::Path) -> (HubState, String, Arc<crate::push::WebPush>) {
        let (mut st, cookie) = hub_web();
        let vapid =
            crate::push::Vapid::load_or_create(&tmp.join("vapid.key"), "mailto:eu@exemplo.com")
                .unwrap();
        let push = Arc::new(crate::push::WebPush::new(
            vapid,
            st.hub.clone(),
            crate::push::PushOptions {
                stale_ms: 0,
                allow_http: true,
            },
        ));
        st.push = Some(Arc::clone(&push));
        (st, cookie, push)
    }

    use crate::push::tests::{check_vapid, FakeBrowser};
    use std::sync::Arc;

    async fn subscribe(web: &mut Ui, endpoint: &str, b: &FakeBrowser) -> Value {
        let (p256dh, auth) = b.keys();
        web.call(
            "hub.push.subscribe",
            json!({ "endpoint": endpoint, "keys": { "p256dh": p256dh, "auth": auth },
                    "label": "Chrome · Android" }),
        )
        .await
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn push_de_teste_cifrado_com_vapid_para_inscricao_falsa() {
        let tmp = tempfile::tempdir().unwrap();
        let (st, cookie, push) = hub_push(tmp.path());
        let url = spawn_server(&st).await;
        let svc = FakePushService::default();
        let base = svc.spawn().await;
        let mut web = web_ui(&url, &cookie).await;

        let v = web.call("hub.push.vapid_key", json!({})).await;
        assert_eq!(v["result"]["key"], push.vapid.public_key(), "{v}");
        let b = FakeBrowser::new();
        let v = subscribe(&mut web, &format!("{base}/abc"), &b).await;
        let dev = &v["result"];
        assert_eq!(dev["current"], true, "{v}");
        assert_eq!(dev["events"].as_array().unwrap().len(), 4, "{v}");
        assert!(dev.get("endpoint").is_none() && dev.get("p256dh").is_none());
        let id = dev["id"].as_i64().unwrap();

        // Recusas: chave inválida, evento desconhecido, endpoint não https.
        let v = web
            .call(
                "hub.push.subscribe",
                json!({ "endpoint": format!("{base}/x"), "keys": { "p256dh": "AAAA", "auth": "BBBB" } }),
            )
            .await;
        assert_eq!(v["error"]["code"], -32602, "{v}");
        let v = web
            .call("hub.push.update", json!({ "id": id, "events": ["tudo"] }))
            .await;
        assert_eq!(v["error"]["code"], -32602, "{v}");

        let v = web.call("hub.push.test", json!({ "id": id })).await;
        assert_eq!(v["result"]["delivered"], true, "{v}");
        assert_eq!(v["result"]["status"], 201, "{v}");
        let got = svc.take();
        assert_eq!(got.len(), 1);
        let (h, body, path) = &got[0];
        assert_eq!(path, "abc");
        let hv = |k: &str| h.get(k).unwrap().to_str().unwrap().to_string();
        assert_eq!(hv("content-encoding"), "aes128gcm");
        assert_eq!(hv("ttl"), "3600");
        let claims = check_vapid(&hv("authorization"), &push.vapid.public_key());
        assert_eq!(claims["aud"], base.trim_end_matches("/push"));
        assert_eq!(claims["sub"], "mailto:eu@exemplo.com");
        let plain: Value = serde_json::from_slice(&b.decrypt(body)).unwrap();
        assert_eq!(plain["title"], "Control Tower", "{plain}");
        assert_eq!(plain["url"], "/?go=hub:devices");
        assert!(plain.get("key").is_none(), "{plain}");

        // O desktop vê a lista, mas não inscreve.
        let mut ui = Ui::new(&url).await;
        let v = ui.call("hub.push.list", json!({})).await;
        assert_eq!(v["result"][0]["current"], false, "{v}");
        assert_eq!(v["result"][0]["label"], "Chrome · Android", "{v}");
        let b2 = FakeBrowser::new();
        let v = subscribe(&mut ui, &format!("{base}/d"), &b2).await;
        assert_eq!(v["error"]["code"], -32602, "{v}");
        let v = ui
            .call(
                "hub.push.update",
                json!({ "id": id, "events": ["env_stale"] }),
            )
            .await;
        assert_eq!(v["result"]["events"], json!(["env_stale"]), "{v}");
        let v = ui.call("hub.push.remove", json!({ "id": id })).await;
        assert_eq!(v["result"]["removed"], true, "{v}");
        let v = ui.call("hub.push.list", json!({})).await;
        assert_eq!(v["result"], json!([]), "{v}");

        // Sem a Web Push ligada.
        let st = hub_com_dados();
        let url = spawn_server(&st).await;
        let mut ui = Ui::new(&url).await;
        let v = ui.call("hub.push.vapid_key", json!({})).await;
        assert_eq!(v["error"]["code"], crate::push::PUSH_UNAVAILABLE, "{v}");
    }

    /// Ingere no hub o que o ledger tiver pendente.
    fn ingest_all(st: &HubState, l: &Ledger) {
        let h = st.hub.lock();
        if h.bind_env_token("wsl", l.env_id()).is_err() {
            h.add_legacy_env_token("wsl").unwrap();
            h.bind_env_token("wsl", l.env_id()).unwrap();
        }
        h.ingest(l.env_id(), "wsl", &l.outbox_pending(1000).unwrap())
            .unwrap();
    }

    /// Sessão viva no item #5 do demo (ambiente novo), com ou sem Remote Control.
    fn sessao_viva(st: &HubState, rc: Option<&str>) -> (Ledger, String) {
        let l = Ledger::open_in_memory().unwrap();
        let p = l
            .add_project(&NewProject {
                name: "demo".into(),
                gh_owner: "deep-ion-ai".into(),
                gh_repo: "demo".into(),
                repo_path: "/x".into(),
                base_branch: "develop".into(),
                protected_branches: vec![],
            })
            .unwrap();
        let w = l.ensure_work_item(p.id, 5, WorkItemKind::Issue).unwrap();
        let s = l
            .create_session(&ct_ledger::NewSession {
                work_item_id: w.id,
                tool: ct_core::Tool::Claude,
                cwd: "/x".into(),
                launch_cmd: vec![],
                model: None,
                mode: Default::default(),
            })
            .unwrap();
        if let Some(rc) = rc {
            l.set_session_rc_id(&s.id, rc).unwrap();
        }
        l.transition_session(&s.id, ct_core::SessionState::Running, None)
            .unwrap();
        ingest_all(st, &l);
        (l, s.id)
    }

    fn aguardando(l: &Ledger, sid: &str) {
        let s = l.get_session(sid).unwrap();
        let w = l.get_work_item(s.work_item_id).unwrap();
        l.append_event(ct_ledger::NewEvent {
            project_id: w.project_id,
            work_item_id: Some(w.id),
            session_id: Some(sid),
            kind: "needs_human",
            payload: json!({ "message": "rm -rf segredo" }),
        })
        .unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn aguardando_voce_vira_push_so_sem_remote_control() {
        let tmp = tempfile::tempdir().unwrap();
        let (st, cookie, push) = hub_push(tmp.path());
        let url = spawn_server(&st).await;
        let svc = FakePushService::default();
        let base = svc.spawn().await;
        let mut web = web_ui(&url, &cookie).await;
        let b = FakeBrowser::new();
        let v = subscribe(&mut web, &format!("{base}/cel"), &b).await;
        assert!(v["result"]["id"].is_i64(), "{v}");
        // Primeira varredura: só marca o que já existe.
        assert_eq!(push.scan().await, 0);

        // Com Remote Control: o app Claude já avisa, a Web Push não.
        let (l_rc, sid_rc) = sessao_viva(&st, Some("session_01rc"));
        aguardando(&l_rc, &sid_rc);
        ingest_all(&st, &l_rc);
        assert_eq!(push.scan().await, 0);
        assert!(svc.take().is_empty());

        // Sem Remote Control: um push, com o deep link do item e sem a mensagem.
        let (l, sid) = sessao_viva(&st, None);
        aguardando(&l, &sid);
        ingest_all(&st, &l);
        assert_eq!(push.scan().await, 1);
        let got = svc.take();
        assert_eq!(got.len(), 1);
        let plain: Value = serde_json::from_slice(&b.decrypt(&got[0].1)).unwrap();
        assert_eq!(plain["title"], "Aguardando você", "{plain}");
        assert_eq!(plain["event"], "needs_human");
        assert_eq!(plain["url"], "/?go=item&project=deep-ion-ai/demo&n=5");
        assert!(plain["body"]
            .as_str()
            .unwrap()
            .starts_with("deep-ion-ai/demo #5"));
        assert!(!plain.to_string().contains("segredo"), "{plain}");
        assert_eq!(got[0].0.get("urgency").unwrap(), "high");
        // Deduplicado: a mesma situação não repete.
        assert_eq!(push.scan().await, 0);

        // Dispositivo sem o evento marcado não recebe.
        let id = st.hub.lock().list_push_devices(None).unwrap()[0].id;
        let v = web
            .call(
                "hub.push.update",
                json!({ "id": id, "events": ["proposal_new"] }),
            )
            .await;
        assert_eq!(v["result"]["events"], json!(["proposal_new"]), "{v}");
        aguardando(&l, &sid);
        ingest_all(&st, &l);
        assert_eq!(push.scan().await, 0);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn inscricao_410_apagada_e_revogar_sessao_apaga_inscricoes() {
        let tmp = tempfile::tempdir().unwrap();
        let (st, cookie, push) = hub_push(tmp.path());
        let url = spawn_server(&st).await;
        let svc = FakePushService::default();
        let base = svc.spawn().await;
        let mut web = web_ui(&url, &cookie).await;
        let (b1, b2) = (FakeBrowser::new(), FakeBrowser::new());
        subscribe(&mut web, &format!("{base}/gone"), &b1).await;
        subscribe(&mut web, &format!("{base}/ok"), &b2).await;
        push.scan().await;
        let (l, sid) = sessao_viva(&st, None);
        aguardando(&l, &sid);
        ingest_all(&st, &l);
        assert_eq!(push.scan().await, 2);
        let mut paths: Vec<_> = svc.take().into_iter().map(|g| g.2).collect();
        paths.sort();
        assert_eq!(paths, ["gone", "ok"]);
        // A de 410 sumiu; a outra ficou, com a entrega registrada.
        let devs = st.hub.lock().list_push_devices(None).unwrap();
        assert_eq!(devs.len(), 1, "{devs:?}");
        assert!(devs[0].last_ok_at.is_some());
        let v = web.call("hub.push.test", json!({ "id": 999 })).await;
        assert_eq!(v["error"]["code"], -32000, "{v}");

        // Revogar a sessão web (pelo desktop) apaga as inscrições dela.
        let me = cookie
            .trim_start_matches("ct_web=")
            .split('.')
            .next()
            .unwrap()
            .to_string();
        let mut ui = Ui::new(&url).await;
        let v = ui
            .call("hub.web.sessions.revoke", json!({ "id": me }))
            .await;
        assert_eq!(v["result"]["revoked"], 1, "{v}");
        assert!(st.hub.lock().list_push_devices(None).unwrap().is_empty());
    }

    /// `env_stale` em tempo real (spec da PWA §7.1, fase 2): ambiente com
    /// canal avisa quando o canal fecha há mais de 1 min, mesmo com o
    /// último sync recente; enquanto o canal está aberto, nunca.
    #[tokio::test(flavor = "multi_thread")]
    async fn env_stale_pelo_canal_em_tempo_real() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut st, cookie) = hub_web();
        let vapid = crate::push::Vapid::load_or_create(
            &tmp.path().join("vapid.key"),
            "mailto:eu@exemplo.com",
        )
        .unwrap();
        let push = Arc::new(
            crate::push::WebPush::new(
                vapid,
                st.hub.clone(),
                crate::push::PushOptions {
                    stale_ms: 15 * 60_000,
                    allow_http: true,
                },
            )
            .with_channels(Arc::clone(&st.channels)),
        );
        st.push = Some(Arc::clone(&push));
        let url = spawn_server(&st).await;
        let svc = FakePushService::default();
        let base = svc.spawn().await;
        let mut web = web_ui(&url, &cookie).await;
        let b = FakeBrowser::new();
        subscribe(&mut web, &format!("{base}/cel"), &b).await;
        let env = fake_env(&st, &url, "env-x").await;
        assert_eq!(push.scan().await, 0); // primeira: só marca

        // Canal aberto: nada. Caiu agora: ainda nada (menos de 1 min).
        assert_eq!(push.scan().await, 0);
        env.abort();
        let ch = Arc::clone(&st.channels);
        tokio::time::timeout(T, async move {
            while ch.is_online("env-x") {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(push.scan().await, 0);
        // Fechado há 2 min (o último sync é de agora): um push, uma vez.
        let closed = ct_core::now_ms() - 2 * 60_000;
        st.channels.set_closed_at("env-x", closed);
        assert_eq!(push.scan().await, 1);
        let got = svc.take();
        let plain: Value = serde_json::from_slice(&b.decrypt(&got[0].1)).unwrap();
        assert_eq!(plain["event"], "env_stale", "{plain}");
        assert_eq!(plain["url"], "/?go=hub:envs");
        assert!(
            plain["body"]
                .as_str()
                .unwrap()
                .starts_with("x: sem canal com o hub há 2 min"),
            "{plain}"
        );
        assert_eq!(push.scan().await, 0);
    }

    // --- atualização do hub pela UI (#421) -----------------------------------

    /// Executável de teste em `dir`.
    fn exe(dir: &std::path::Path, name: &str, body: &str) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let p = dir.join(name);
        std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p
    }

    /// Hub com atualização pela UI em `tmp`: `systemd-run` falso (descarta
    /// as opções e roda o comando) e `cthubd` falso (grava `args` e as fases
    /// `build` e `done` no `state.json`, como o atualizador).
    fn hub_com_updates(tmp: &std::path::Path, repo: &str) -> HubState {
        let dirs = crate::config::Dirs {
            config: tmp.join("config"),
            data: tmp.join("data"),
        };
        dirs.ensure().unwrap();
        let dir = dirs.update_dir();
        let run = exe(
            tmp,
            "fake-systemd-run",
            "echo \"$@\" > \"$(dirname \"$0\")/run-args\"\n\
             while [ \"${1#--}\" != \"$1\" ]; do shift; done; \"$@\"",
        );
        let state = |phase: &str| {
            format!(
                "now=$(($(date +%s)*1000)); mkdir -p {d}; printf '{{\"kind\":\"update\",\"phase\":\"{phase}\",\"ref\":\"main\",\"from\":{{\"version\":\"0.1.0\",\"commit\":\"a\"}},\"to\":null,\"started_at\":%s,\"updated_at\":%s,\"finished_at\":null,\"message\":null,\"error\":null,\"log_tail\":[]}}' $((now+1000)) $((now+1000)) > {d}/state.json",
                d = dir.display()
            )
        };
        let cthubd = exe(
            tmp,
            "fake-cthubd",
            &format!(
                "echo \"$@\" > {t}/args\n{}\nsleep 1\n{}",
                state("build"),
                state("done"),
                t = tmp.display()
            ),
        );
        let cfg = crate::config::HubConfig {
            update_repo: repo.into(),
            update_ref: "main".into(),
            ..Default::default()
        };
        let mut st = hub_com_dados();
        st.updates = Some(Arc::new(
            crate::update::HubUpdates::new(&cfg, &dirs, st.events.clone())
                .with_cthubd(cthubd)
                .with_systemd_run(run.to_str().unwrap())
                .with_installed(ct_update::Build {
                    version: "0.1.0".into(),
                    commit: String::new(),
                }),
        ));
        st
    }

    impl Ui {
        /// Espera um push `event` com `pred` (os outros ficam para depois).
        async fn wait_push(&mut self, event: &str, pred: impl Fn(&Value) -> bool) -> Value {
            let deadline = tokio::time::Instant::now() + T;
            loop {
                if let Some(i) = self
                    .pushes
                    .iter()
                    .position(|p| p["event"] == event && pred(p))
                {
                    return self.pushes.remove(i);
                }
                assert!(tokio::time::Instant::now() < deadline, "sem push {event}");
                let m = tokio::time::timeout(T, self.ws.next())
                    .await
                    .expect("prazo")
                    .unwrap()
                    .unwrap();
                if let WsMsg::Text(t) = m {
                    let v: Value = serde_json::from_str(&t).unwrap();
                    if v.get("event").is_some() {
                        self.pushes.push(v);
                    }
                }
            }
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn update_desativado_sem_atualizador() {
        let url = spawn_server(&hub_com_dados()).await;
        let mut ui = Ui::new(&url).await;
        for m in ["hub.update.get", "hub.update.check", "hub.update.start"] {
            let v = ui.call(m, json!({})).await;
            assert_eq!(v["error"]["code"], -32602, "{m}: {v}");
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn update_check_compara_com_o_ref() {
        use ct_update::{git, run};
        let tmp = tempfile::tempdir().unwrap();
        let origin = tmp.path().join("origin");
        std::fs::create_dir(&origin).unwrap();
        let g = |args: &[&str]| run(git(&origin).args(args)).unwrap();
        g(&["init", "-q", "-b", "main"]);
        std::fs::write(
            origin.join("Cargo.toml"),
            "[workspace.package]\nversion = \"9.9.9\"\n",
        )
        .unwrap();
        g(&["add", "."]);
        g(&[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-qm",
            "v9",
        ]);
        let head = g(&["rev-parse", "HEAD"]);
        let st = hub_com_updates(tmp.path(), origin.to_str().unwrap());
        let url = spawn_server(&st).await;
        let mut ui = Ui::new(&url).await;

        let v = ui.call("hub.update.get", json!({})).await;
        let r = &v["result"];
        assert_eq!(r["ref"], "main", "{v}");
        assert_eq!(r["installed"]["version"], "0.1.0");
        assert_eq!(r["available"], Value::Null);
        assert_eq!(r["state"], Value::Null);
        assert_eq!(r["previous"], Value::Null);

        let v = ui.call("hub.update.check", json!({})).await;
        let a = &v["result"]["available"];
        assert_eq!(a["build"]["version"], "9.9.9", "{v}");
        assert_eq!(a["build"]["commit"], head.as_str());
        assert_eq!(a["update_available"], true);
        let p = ui.wait_push("hub.update.status", |_| true).await;
        assert_eq!(p["data"]["available"]["build"]["version"], "9.9.9");

        // Falha na verificação não apaga o último resultado.
        std::fs::remove_dir_all(&origin).unwrap();
        let v = ui.call("hub.update.check", json!({})).await;
        assert!(v["result"]["check_error"].is_string(), "{v}");
        assert_eq!(v["result"]["available"]["build"]["version"], "9.9.9");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn update_start_dispara_fora_do_servico_e_acompanha() {
        let tmp = tempfile::tempdir().unwrap();
        let st = hub_com_updates(tmp.path(), "/nao-usado");
        let url = spawn_server(&st).await;
        let mut ui = Ui::new(&url).await;

        let v = ui.call("hub.update.start", json!({})).await;
        assert!(v.get("error").is_none(), "{v}");
        let args = std::fs::read_to_string(tmp.path().join("args")).unwrap();
        assert_eq!(args.trim(), "update");
        // Na unidade transitória, com as variáveis repassadas.
        let run = std::fs::read_to_string(tmp.path().join("run-args")).unwrap();
        assert!(
            run.starts_with("--user --unit=cthubd-update --collect --quiet"),
            "{run}"
        );
        assert!(run.contains("--setenv=PATH="), "{run}");
        assert!(run.trim_end().ends_with("fake-cthubd update"), "{run}");

        let p = ui
            .wait_push("hub.update.status", |p| {
                p["data"]["state"]["phase"] == "done"
            })
            .await;
        assert_eq!(p["data"]["state"]["kind"], "update");
        let v = ui.call("hub.update.get", json!({})).await;
        assert_eq!(v["result"]["state"]["phase"], "done");

        // Uma em andamento recusa a próxima.
        let dir = tmp.path().join("data/update");
        let mut s = ct_update::read_state(&dir).unwrap();
        s.phase = ct_update::Phase::Build;
        s.updated_at = ct_core::now_ms();
        ct_update::write_state(&dir, &s).unwrap();
        let v = ui.call("hub.update.start", json!({})).await;
        assert!(
            v["error"]["message"]
                .as_str()
                .unwrap()
                .contains("em andamento"),
            "{v}"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn update_rollback_pede_force_quando_o_banco_volta() {
        let tmp = tempfile::tempdir().unwrap();
        let st = hub_com_updates(tmp.path(), "/nao-usado");
        let url = spawn_server(&st).await;
        let mut ui = Ui::new(&url).await;

        // Sem versão guardada: nada a voltar.
        let v = ui.call("hub.update.rollback", json!({})).await;
        assert!(
            v["error"]["message"]
                .as_str()
                .unwrap()
                .contains("nenhuma versão anterior"),
            "{v}"
        );
        assert!(!tmp.path().join("args").exists());

        // hub.db em uso com esquema à frente do snapshot (prev.json).
        let db = tmp.path().join("data/hub.db");
        drop(Hub::open(&db).unwrap());
        let dir = tmp.path().join("data/update");
        std::fs::create_dir_all(dir.join("prev")).unwrap();
        std::fs::create_dir_all(dir.join("db")).unwrap();
        std::fs::copy(&db, dir.join("db/hub.db")).unwrap();
        std::fs::write(
            dir.join("prev/prev.json"),
            json!({ "build": { "version": "0.0.9", "commit": "abc" },
                    "dbs": [["hub.db", 1]], "taken_at": 0 })
            .to_string(),
        )
        .unwrap();
        let v = ui.call("hub.update.get", json!({})).await;
        assert_eq!(v["result"]["previous"]["version"], "0.0.9", "{v}");

        let v = ui.call("hub.update.rollback", json!({})).await;
        assert_eq!(v["error"]["code"], UPDATE_ROLLBACK_DB, "{v}");
        let msg = v["error"]["message"].as_str().unwrap();
        assert!(msg.contains("hub.db volta ao snapshot"), "{msg}");
        assert!(msg.contains("force"), "{msg}");
        assert!(!tmp.path().join("args").exists());

        let v = ui
            .call("hub.update.rollback", json!({ "force": true }))
            .await;
        assert!(v.get("error").is_none(), "{v}");
        let args = std::fs::read_to_string(tmp.path().join("args")).unwrap();
        assert_eq!(args.trim(), "update --rollback --force");
    }
}
