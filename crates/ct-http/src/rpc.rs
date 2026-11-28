//! JSON-RPC do `/ws`: requisição `{id, method, params}` → `{id, result}` ou
//! `{id, error: {code, message}}`; push do servidor `{event, data}`.

use axum::extract::ws::Message;
use serde_json::{json, Value};
use tokio::sync::{broadcast, mpsc};

/// Resultado de um método: valor ou (código, mensagem).
pub type RpcResult = Result<Value, (i64, String)>;

/// Parâmetro inválido ou ausente (-32602).
pub fn invalid(msg: impl Into<String>) -> (i64, String) {
    (-32602, msg.into())
}

pub fn str_param<'a>(p: &'a Value, k: &str) -> Result<&'a str, (i64, String)> {
    p.get(k)
        .and_then(Value::as_str)
        .ok_or_else(|| invalid(format!("parâmetro obrigatório: {k}")))
}

pub fn i64_param(p: &Value, k: &str) -> Result<i64, (i64, String)> {
    p.get(k)
        .and_then(Value::as_i64)
        .ok_or_else(|| invalid(format!("parâmetro obrigatório: {k}")))
}

/// Requisição decodificada de um frame de texto.
#[derive(Debug)]
pub struct Request {
    pub id: Value,
    pub method: String,
    pub params: Value,
}

impl Request {
    /// Span `rpc{method, req_id, session}` do despacho (#243): tudo o que o
    /// método loga leva a requisição junto. `req_id` é o `id` do frame
    /// (número ou texto) e `session` o `session_id` dos parâmetros, quando
    /// há, para achar também por aqui o que se passou com uma sessão.
    pub fn span(&self) -> tracing::Span {
        let req_id = match &self.id {
            Value::Null => String::new(),
            Value::String(s) => s.clone(),
            v => v.to_string(),
        };
        let span = tracing::info_span!(
            "rpc",
            method = %self.method,
            req_id = %req_id,
            session = tracing::field::Empty
        );
        if let Some(s) = self.params.get("session_id").and_then(Value::as_str) {
            span.record("session", s);
        }
        span
    }
}

/// Decodifica o frame; JSON inválido já vira a resposta de erro.
pub fn parse(text: &str) -> Result<Request, Value> {
    let Ok(req) = serde_json::from_str::<Value>(text) else {
        return Err(json!({ "id": null, "error": { "code": -32700, "message": "JSON inválido" } }));
    };
    Ok(Request {
        id: req.get("id").cloned().unwrap_or(Value::Null),
        method: req
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        params: req.get("params").cloned().unwrap_or(json!({})),
    })
}

/// Resposta da requisição `id`.
pub fn reply(id: Value, r: RpcResult) -> Value {
    match r {
        Ok(result) => json!({ "id": id, "result": result }),
        Err((code, message)) => json!({ "id": id, "error": { "code": code, "message": message } }),
    }
}

/// Encaminha o canal de push para a conexão até ela fechar. Atraso do
/// receptor vira um push `lagged { missed }`.
pub fn spawn_pusher(
    events: broadcast::Receiver<Value>,
    out: mpsc::UnboundedSender<Message>,
) -> tokio::task::JoinHandle<()> {
    spawn_pusher_filtered(events, out, |_| true)
}

/// Como [`spawn_pusher`], mas só repassa os pushes em que `keep` dá
/// verdadeiro (ex.: os do `/web/ws` do hub). O `lagged` sempre passa.
pub fn spawn_pusher_filtered(
    mut events: broadcast::Receiver<Value>,
    out: mpsc::UnboundedSender<Message>,
    keep: fn(&Value) -> bool,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            match events.recv().await {
                Ok(v) if !keep(&v) => {}
                Ok(v) => {
                    if out.send(Message::Text(v.to_string().into())).is_err() {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    let lag = json!({ "event": "lagged", "data": { "missed": n } });
                    let _ = out.send(Message::Text(lag.to_string().into()));
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_e_reply() {
        let r = parse(r#"{"id":3,"method":"hub.info"}"#).unwrap();
        assert_eq!((r.id.clone(), r.method.as_str()), (json!(3), "hub.info"));
        assert_eq!(r.params, json!({}));
        assert_eq!(reply(r.id, Ok(json!(1))), json!({ "id": 3, "result": 1 }));
        let e = parse("{").unwrap_err();
        assert_eq!(e["error"]["code"], -32700);
        let v = reply(json!(1), Err(invalid("x")));
        assert_eq!(v["error"]["code"], -32602);
        assert!(str_param(&json!({}), "owner").is_err());
        assert_eq!(i64_param(&json!({ "n": 2 }), "n").unwrap(), 2);
    }

    #[test]
    fn span_do_despacho() {
        let mut out = Vec::new();
        let buf = std::sync::Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));
        struct W(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);
        impl std::io::Write for W {
            fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(b);
                Ok(b.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let w = buf.clone();
        let sub = tracing_subscriber::fmt()
            .with_writer(move || W(w.clone()))
            .with_ansi(false)
            .finish();
        tracing::subscriber::with_default(sub, || {
            let r =
                parse(r#"{"id":3,"method":"session.get","params":{"session_id":"s-1"}}"#).unwrap();
            r.span().in_scope(|| tracing::info!("x"));
            let r = parse(r#"{"id":"abc","method":"hub.info"}"#).unwrap();
            r.span().in_scope(|| tracing::info!("y"));
        });
        out.extend(buf.lock().unwrap().iter());
        let text = String::from_utf8(out).unwrap();
        assert!(
            text.contains(r#"rpc{method=session.get req_id=3 session="s-1"}"#),
            "{text}"
        );
        assert!(text.contains("rpc{method=hub.info req_id=abc}"), "{text}");
    }
}
