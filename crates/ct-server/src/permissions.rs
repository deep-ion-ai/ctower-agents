//! Pedidos de permissão respondidos pela UI (#328).
//!
//! O hook `PermissionRequest` do Claude Code chama `ctower hook
//! permission-request`, que faz `POST /hooks/permission` (token de hook) e
//! espera. O daemon guarda o pedido, avisa a UI pelo push
//! `permission_request` e segura a resposta HTTP até a UI responder
//! (`session.permission.respond`) ou passar [`WAIT`]. Só segura se alguma
//! conexão da UI pediu os pedidos (`session.permission.watch`): uma UI que
//! não sabe respondê-los não atrasa o menu do terminal. A resposta vira a
//! decisão do hook; sem resposta (prazo, nenhuma UI acompanhando, `ask`) o
//! hook não decide e o Claude Code mostra o menu de sempre no terminal.
//! Push `permission_resolved` quando o pedido sai da lista.

use crate::{bearer_ok, unauthorized, AppState};
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use ct_sessions::HookEnvelope;
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::oneshot;

/// Quanto o daemon espera a resposta da UI. Fica abaixo do prazo do
/// `ctower hook` e do `timeout` do hook no Claude Code
/// (`ct_sessions::PERMISSION_HOOK_TIMEOUT_SECS`).
pub const WAIT: Duration = Duration::from_secs(120);

/// Respostas aceitas por `session.permission.respond`.
pub const DECISIONS: [&str; 4] = ["allow", "allow_session", "deny", "ask"];

/// Um pedido pendente, como a UI vê (`session.permission.list` e push
/// `permission_request`).
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct PermissionRequest {
    pub request_id: String,
    pub session_id: String,
    pub work_item_id: i64,
    pub tool_name: Option<String>,
    /// Argumentos da ferramenta, com os secrets da sessão mascarados.
    #[cfg_attr(feature = "ts", ts(type = "unknown"))]
    pub tool_input: Value,
    pub created_at: i64,
    /// Depois disso o pedido volta para o menu do terminal.
    pub expires_at: i64,
}

/// Resposta da UI repassada ao hook.
#[derive(Debug, Clone, PartialEq)]
pub struct Answer {
    pub decision: String,
    pub message: Option<String>,
}

struct Pending {
    view: PermissionRequest,
    tx: oneshot::Sender<Answer>,
}

#[derive(Default)]
pub struct Permissions {
    pending: Mutex<HashMap<String, Pending>>,
    /// Conexões da UI que acompanham os pedidos.
    watchers: AtomicUsize,
}

/// Uma conexão que acompanha os pedidos; sai da conta ao cair.
pub struct Watch(Shared);

impl Drop for Watch {
    fn drop(&mut self) {
        self.0.watchers.fetch_sub(1, Ordering::SeqCst);
    }
}

impl Permissions {
    pub fn watch(self: &Arc<Self>) -> Watch {
        self.watchers.fetch_add(1, Ordering::SeqCst);
        Watch(Arc::clone(self))
    }

    pub(crate) fn watched(&self) -> bool {
        self.watchers.load(Ordering::SeqCst) > 0
    }

    pub fn list(&self) -> Vec<PermissionRequest> {
        let mut v: Vec<_> = self
            .pending
            .lock()
            .unwrap()
            .values()
            .map(|p| p.view.clone())
            .collect();
        v.sort_by_key(|p| p.created_at);
        v
    }

    /// Entrega a resposta ao hook que espera. `false` = o pedido já não
    /// está pendente (respondido, expirado ou o agente desistiu).
    pub fn respond(&self, request_id: &str, answer: Answer) -> bool {
        match self.pending.lock().unwrap().remove(request_id) {
            Some(p) => p.tx.send(answer).is_ok(),
            None => false,
        }
    }

    fn insert(&self, view: PermissionRequest) -> oneshot::Receiver<Answer> {
        let (tx, rx) = oneshot::channel();
        self.pending
            .lock()
            .unwrap()
            .insert(view.request_id.clone(), Pending { view, tx });
        rx
    }

    fn remove(&self, request_id: &str) -> bool {
        self.pending.lock().unwrap().remove(request_id).is_some()
    }
}

/// Tira o pedido da lista se a espera acabar sem resposta (prazo ou
/// conexão do hook caída) e avisa a UI.
struct Expire<'a> {
    st: &'a AppState,
    view: &'a PermissionRequest,
}

impl Drop for Expire<'_> {
    fn drop(&mut self) {
        if self.st.permissions.remove(&self.view.request_id) {
            resolved(self.st, self.view, "expired");
        }
    }
}

fn resolved(st: &AppState, view: &PermissionRequest, decision: &str) {
    st.events_send(
        "permission_resolved",
        json!({
            "request_id": view.request_id,
            "session_id": view.session_id,
            "work_item_id": view.work_item_id,
            "decision": decision,
        }),
    );
    let sessions = st.sessions.clone();
    let (id, decision) = (view.session_id.clone(), decision.to_string());
    tokio::task::spawn_blocking(move || {
        if let Err(e) = sessions.permission_resolved(&id, &decision) {
            tracing::warn!(error = %e, "falha ao registrar a resposta da permissão");
        }
    });
}

/// Sem decisão: o hook deixa o Claude Code perguntar no terminal.
fn no_decision() -> Response {
    axum::Json(json!({ "decision": null })).into_response()
}

pub(crate) async fn handle(
    State(st): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !bearer_ok(&headers, &st.hook_token) {
        return unauthorized();
    }
    let Ok(h) = serde_json::from_slice::<HookEnvelope>(&body) else {
        return (StatusCode::BAD_REQUEST, "corpo de hook inválido").into_response();
    };
    // Sem UI acompanhando ninguém responderia: não segura o agente.
    if !st.permissions.watched() {
        return no_decision();
    }
    let sessions = st.sessions.clone();
    let req = h.clone();
    let found = match tokio::task::spawn_blocking(move || sessions.permission_requested(&req)).await
    {
        Ok(Ok(found)) => found,
        Ok(Err(e)) => {
            tracing::warn!(error = %e, "falha ao registrar o pedido de permissão");
            return no_decision();
        }
        Err(_) => return no_decision(),
    };
    let Some((session, tool_input)) = found else {
        return no_decision();
    };
    let now = ct_core::now_ms();
    let view = PermissionRequest {
        request_id: uuid::Uuid::new_v4().to_string(),
        session_id: session.id,
        work_item_id: session.work_item_id,
        tool_name: h
            .payload
            .get("tool_name")
            .and_then(Value::as_str)
            .map(String::from),
        tool_input,
        created_at: now,
        expires_at: now + WAIT.as_millis() as i64,
    };
    let rx = st.permissions.insert(view.clone());
    let guard = Expire {
        st: &st,
        view: &view,
    };
    st.events_send("permission_request", json!(view));
    let answer = tokio::time::timeout(WAIT, rx).await;
    drop(guard);
    match answer {
        Ok(Ok(a)) => {
            resolved(&st, &view, &a.decision);
            if a.decision == "ask" {
                return no_decision();
            }
            axum::Json(json!({ "decision": a.decision, "message": a.message })).into_response()
        }
        // Prazo: o guard já tirou o pedido e avisou a UI.
        _ => no_decision(),
    }
}

/// `session.permission.respond`.
pub(crate) fn respond(st: &AppState, p: &Value) -> ct_http::rpc::RpcResult {
    use ct_http::rpc::{invalid, str_param};
    let id = str_param(p, "request_id")?;
    let decision = str_param(p, "decision")?;
    if !DECISIONS.contains(&decision) {
        return Err(invalid(format!(
            "decision inválida: {decision} (aceitas: {})",
            DECISIONS.join(", ")
        )));
    }
    let message = p.get("message").and_then(Value::as_str).map(String::from);
    let answer = Answer {
        decision: decision.into(),
        message,
    };
    if st.permissions.respond(id, answer) {
        Ok(json!({}))
    } else {
        Err((-32000, "pedido de permissão não está mais pendente".into()))
    }
}

pub type Shared = Arc<Permissions>;
