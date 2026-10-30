//! `POST /hooks`: eventos do `ctower hook` (spec §7.2).

use crate::{bearer_ok, unauthorized, AppState};
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use ct_sessions::HookEnvelope;

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
    let sessions = st.sessions.clone();
    match tokio::task::spawn_blocking(move || sessions.apply_hook(&h)).await {
        Ok(Ok(outcome)) => {
            tracing::debug!(?outcome, "hook aplicado");
            StatusCode::ACCEPTED.into_response()
        }
        Ok(Err(e)) => {
            tracing::warn!(error = %e, "falha ao aplicar hook");
            (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

#[cfg(test)]
mod tests {
    use crate::test_util::*;
    use axum::http::StatusCode;
    use ct_core::SessionState;
    use ct_sessions::test_support::FakeClaude;
    use serde_json::json;

    #[tokio::test]
    async fn autenticacao_do_endpoint() {
        let srv = Srv::new(FakeClaude::exit(0));
        let body = json!({ "session": "x", "event": "Stop", "payload": {}, "ts": 1 });
        for token in [None, Some("errado"), Some(UI)] {
            let (st, _) = srv.post("/hooks", token, &[], body.clone()).await;
            assert_eq!(st, StatusCode::UNAUTHORIZED, "{token:?}");
            let (st, _) = srv
                .post("/hooks/permission", token, &[], body.clone())
                .await;
            assert_eq!(st, StatusCode::UNAUTHORIZED, "{token:?}");
        }
        // Sessão desconhecida: aceito sem efeito.
        let (st, _) = srv.post("/hooks", Some(HOOK), &[], body).await;
        assert_eq!(st, StatusCode::ACCEPTED);
        let (st, _) = srv.post("/hooks", Some(HOOK), &[], json!("lixo")).await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn hook_muda_o_ledger() {
        let srv = Srv::new(FakeClaude::sleep(30));
        let s = srv.env.sessions.start(srv.env.request(1, "A")).unwrap();
        let (st, _) = srv
            .post(
                "/hooks",
                Some(HOOK),
                &[],
                json!({ "session": s.id, "event": "SessionStart", "payload": { "session_id": "u", "transcript_path": "/home/u/t.jsonl" }, "ts": 1 }),
            )
            .await;
        assert_eq!(st, StatusCode::ACCEPTED);
        let got = srv.env.sessions.ledger().get_session(&s.id).unwrap();
        assert_eq!(got.state, SessionState::Running);
        assert_eq!(got.transcript_path.as_deref(), Some("/home/u/t.jsonl"));
        srv.env.sessions.kill(&s.id).unwrap();
    }
}
