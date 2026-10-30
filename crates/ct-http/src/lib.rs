//! Base HTTP/WS comum aos daemons (`ctowerd` e `cthubd`): autenticação por
//! token, escuta no loopback e no `bind` explícito, JSON-RPC do `/ws` e o
//! contrato do sync ambiente → hub (spec do hub como plano de controle §2).

pub mod rpc;
pub mod sync;

use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Router;

/// `Authorization: Bearer <token>` confere com `expected` (comparação em tempo constante).
pub fn bearer_ok(headers: &HeaderMap, expected: &str) -> bool {
    let Some(got) = bearer_token(headers) else {
        return false;
    };
    let (a, b) = (got.as_bytes(), expected.as_bytes());
    !b.is_empty()
        && a.len() == b.len()
        && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Token de `Authorization: Bearer <token>`, se houver (a validação fica com quem chama).
pub fn bearer_token(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .filter(|t| !t.is_empty())
        .map(str::to_string)
}

pub fn unauthorized() -> Response {
    (StatusCode::UNAUTHORIZED, "token inválido").into_response()
}

/// Serve `app` em `127.0.0.1:<port>`, que fica sempre no ar (CLI, hooks e
/// o ambiente da mesma máquina falam com o loopback), e também em `bind`
/// quando ele é explícito (ex.: IP Tailscale). `0.0.0.0`/`::` escutam em
/// tudo. Termina com SIGTERM ou Ctrl-C. As rotas recebem o endereço de
/// quem conectou (`ConnectInfo<SocketAddr>`), que o hub usa para só confiar
/// em header de proxy vindo do loopback.
pub async fn serve(app: Router, bind: &str, port: u16, name: &str) -> std::io::Result<()> {
    let bind = bind.trim();
    let all = matches!(bind, "0.0.0.0" | "::" | "[::]");
    let primary = if all { bind } else { "127.0.0.1" };
    let listener = tokio::net::TcpListener::bind((primary.trim_matches(['[', ']']), port)).await?;
    tracing::info!("{name} escutando em {}", listener.local_addr()?);
    if !all && !bind.is_empty() && bind != "127.0.0.1" && bind != "localhost" {
        let extra = tokio::net::TcpListener::bind((bind, port)).await?;
        tracing::info!("{name} escutando também em {}", extra.local_addr()?);
        let app = app.clone();
        let bind = bind.to_string();
        tokio::spawn(async move {
            let svc = app.into_make_service_with_connect_info::<std::net::SocketAddr>();
            if let Err(e) = axum::serve(extra, svc).await {
                tracing::error!("listener {bind} encerrado: {e}");
            }
        });
    }
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await
}

async fn shutdown_signal() {
    use tokio::signal::unix::{signal, SignalKind};
    let mut term = signal(SignalKind::terminate()).expect("SIGTERM");
    tokio::select! {
        _ = term.recv() => {}
        _ = tokio::signal::ctrl_c() => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bearer_compara_exato() {
        let mut h = HeaderMap::new();
        assert!(!bearer_ok(&h, "abc"));
        assert!(bearer_token(&h).is_none());
        h.insert(header::AUTHORIZATION, "Bearer abc".parse().unwrap());
        assert!(bearer_ok(&h, "abc"));
        assert_eq!(bearer_token(&h).as_deref(), Some("abc"));
        assert!(!bearer_ok(&h, "abcd"));
        assert!(!bearer_ok(&h, "abd"));
        assert!(!bearer_ok(&h, ""));
        h.insert(header::AUTHORIZATION, "abc".parse().unwrap());
        assert!(!bearer_ok(&h, "abc"));
        assert!(bearer_token(&h).is_none());
    }
}
