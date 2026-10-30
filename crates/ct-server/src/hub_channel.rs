//! Canal reverso com o hub (`/sync/channel`, spec da PWA §5, #198).
//!
//! O `ctowerd` abre e mantém um WebSocket com o hub (`ws://` a partir do
//! `hub_url`, como o `hub_sync` fala HTTP sem TLS), com o token de sync e o
//! `X-Ctower-Env-Id`. Por ele:
//!
//! - atende as chamadas `{id, method, params}` do hub com o mesmo despacho
//!   do `/ws` (`ws::channel_call`), só os métodos de
//!   [`ct_http::sync::CHANNEL_METHODS`] (o resto é recusado mesmo que o hub
//!   peça); cada chamada roda à parte, e uma lenta não segura as outras;
//! - repassa os pushes do ambiente de [`ct_http::sync::CHANNEL_PUSHES`] em
//!   tempo real (`permission_request`, `update.status` e `cleanup.status`
//!   ficam de fora).
//!
//! Queda, recusa ou hub fora do ar: tenta de novo com o backoff do
//! `hub_sync` (`PUSH_EVERY` dobrando até `MAX_BACKOFF`). O hub manda ping a
//! cada 30 s; sem nenhum frame dele em [`ChannelOptions::idle_timeout`], o
//! ambiente fecha e reconecta. O sync por `/sync/push` e `/sync/config`
//! continua como antes: o canal não o substitui.

use crate::hub_sync::{status_error, MAX_BACKOFF, PUSH_EVERY};
use crate::AppState;
use ct_http::sync::{channel_push_allowed, CHANNEL_PATH, ENV_ID_HEADER};
use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use std::time::Duration;
use tokio::sync::{broadcast, mpsc};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::{Error as WsError, Message};

/// Prazos do canal (os testes encurtam).
#[derive(Debug, Clone, Copy)]
pub struct ChannelOptions {
    /// Espera depois de uma queda sem falhas seguidas; dobra a cada falha.
    pub base_delay: Duration,
    /// Teto do backoff.
    pub max_backoff: Duration,
    /// Sem nenhum frame do hub (ele pinga a cada 30 s), reconecta.
    pub idle_timeout: Duration,
}

impl Default for ChannelOptions {
    fn default() -> Self {
        ChannelOptions {
            base_delay: PUSH_EVERY,
            max_backoff: MAX_BACKOFF,
            idle_timeout: Duration::from_secs(90),
        }
    }
}

impl ChannelOptions {
    /// Espera antes da próxima tentativa, como `HubSender::next_delay`.
    pub fn delay(&self, failures: u32) -> Duration {
        self.base_delay
            .checked_mul(1u32 << failures.min(16))
            .unwrap_or(self.max_backoff)
            .min(self.max_backoff)
    }
}

/// `ws://<hub>/sync/channel` a partir do `hub_url` (`http://` → `ws://`,
/// `https://` → `wss://`).
pub fn channel_url(hub_url: &str) -> String {
    let base = hub_url.trim().trim_end_matches('/');
    let base = if let Some(rest) = base.strip_prefix("http://") {
        format!("ws://{rest}")
    } else if let Some(rest) = base.strip_prefix("https://") {
        format!("wss://{rest}")
    } else {
        base.to_string()
    };
    format!("{base}{CHANNEL_PATH}")
}

/// Mantém o canal com o hub enquanto o daemon roda. Sem hub ligado
/// (`hub_sender`), não faz nada.
pub fn spawn(st: AppState, opts: ChannelOptions) -> Option<tokio::task::JoinHandle<()>> {
    let sender = st.hub_sender.clone()?;
    let url = channel_url(&sender.target.url);
    let token = sender.target.token.clone();
    let env_id = st.sessions.ledger().env_id().to_string();
    Some(tokio::spawn(async move {
        let mut failures = 0u32;
        loop {
            match connect(&url, &token, &env_id).await {
                Ok(ws) => {
                    failures = 0;
                    tracing::info!("canal com o hub aberto ({url})");
                    let why = serve(ws, st.clone(), opts).await;
                    tracing::warn!("canal com o hub fechado: {why}");
                }
                Err(e) => {
                    failures = failures.saturating_add(1);
                    tracing::warn!("canal com o hub: {e}");
                }
            }
            tokio::time::sleep(opts.delay(failures)).await;
        }
    }))
}

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn connect(url: &str, token: &str, env_id: &str) -> Result<Ws, String> {
    let mut req = url
        .into_client_request()
        .map_err(|e| format!("hub_url inválido para o canal: {e}"))?;
    let h = req.headers_mut();
    h.insert(
        "authorization",
        HeaderValue::from_str(&format!("Bearer {token}")).map_err(|e| e.to_string())?,
    );
    h.insert(
        ENV_ID_HEADER,
        HeaderValue::from_str(env_id).map_err(|e| e.to_string())?,
    );
    match tokio_tungstenite::connect_async(req).await {
        Ok((ws, _)) => Ok(ws),
        Err(WsError::Http(resp)) => {
            let body = resp
                .body()
                .as_deref()
                .map(String::from_utf8_lossy)
                .unwrap_or_default()
                .to_string();
            Err(status_error(resp.status().as_u16(), &body))
        }
        Err(e) => Err(format!("hub inacessível: {e}")),
    }
}

/// Atende o canal até ele cair; devolve o motivo.
async fn serve(ws: Ws, st: AppState, opts: ChannelOptions) -> String {
    let (mut sink, mut stream) = ws.split();
    let (out, mut out_rx) = mpsc::unbounded_channel::<Message>();
    let writer = tokio::spawn(async move {
        while let Some(m) = out_rx.recv().await {
            if sink.send(m).await.is_err() {
                break;
            }
        }
        let _ = sink.close().await;
    });
    let pusher = tokio::spawn(forward_pushes(st.events.subscribe(), out.clone()));
    // O laço pode ser abortado (daemon encerrando, testes): as tarefas da
    // conexão saem junto, e a conexão fecha.
    let _guard = AbortOnDrop([writer.abort_handle(), pusher.abort_handle()]);
    let why = loop {
        let msg = match tokio::time::timeout(opts.idle_timeout, stream.next()).await {
            Err(_) => break format!("hub sem sinal há {}s", opts.idle_timeout.as_secs()),
            Ok(None) => break "conexão encerrada".to_string(),
            Ok(Some(Err(e))) => break format!("conexão caiu: {e}"),
            Ok(Some(Ok(m))) => m,
        };
        match msg {
            Message::Text(t) => match ct_http::rpc::parse(&t) {
                Ok(req) => {
                    let (st, out) = (st.clone(), out.clone());
                    tokio::spawn(async move {
                        let r = crate::ws::channel_call(st, &req.method, &req.params).await;
                        let reply = ct_http::rpc::reply(req.id, r);
                        let _ = out.send(Message::Text(reply.to_string().into()));
                    });
                }
                Err(reply) => {
                    let _ = out.send(Message::Text(reply.to_string().into()));
                }
            },
            Message::Close(_) => break "fechado pelo hub".to_string(),
            // Ping: o tungstenite responde o pong sozinho. Binário não
            // existe no canal.
            _ => {}
        }
    };
    pusher.abort();
    drop(out);
    let _ = tokio::time::timeout(Duration::from_secs(5), writer).await;
    why
}

struct AbortOnDrop([tokio::task::AbortHandle; 2]);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        for h in &self.0 {
            h.abort();
        }
    }
}

/// Repassa ao hub os pushes da lista do canal.
async fn forward_pushes(
    mut events: broadcast::Receiver<Value>,
    out: mpsc::UnboundedSender<Message>,
) {
    loop {
        match events.recv().await {
            Ok(v) => {
                let keep = v
                    .get("event")
                    .and_then(Value::as_str)
                    .is_some_and(channel_push_allowed);
                if keep && out.send(Message::Text(v.to_string().into())).is_err() {
                    break;
                }
            }
            Err(broadcast::error::RecvError::Lagged(n)) => {
                tracing::warn!("canal com o hub: {n} push(es) perdido(s) por atraso");
            }
            Err(broadcast::error::RecvError::Closed) => break,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hub_sync::{HubSender, HubTarget};
    use crate::test_util::{serve_hub_state, DeadHub, Srv};
    use axum::extract::ws::{Message as AxMsg, WebSocketUpgrade};
    use ct_http::sync::{CHANNEL_ENV_OFFLINE, CHANNEL_METHOD_UNAVAILABLE};
    use ct_ledger::{Hub, SharedHub};
    use ct_sessions::test_support::FakeClaude;
    use serde_json::json;
    use std::sync::Arc;

    const T: Duration = Duration::from_secs(10);

    fn fast() -> ChannelOptions {
        ChannelOptions {
            base_delay: Duration::from_millis(20),
            max_backoff: Duration::from_millis(200),
            idle_timeout: Duration::from_secs(5),
        }
    }

    /// Ambiente ligado ao hub em `url`, com um token de sync preso ao
    /// `env_id` dele (emitido em `hub`, se houver). Devolve o `env_id`.
    fn env(url: &str, hub: Option<&cthubd::HubState>, token: Option<&str>) -> (Srv, String) {
        let mut srv = Srv::new(FakeClaude::sleep(30));
        let id = srv.env.sessions.ledger().env_id().to_string();
        let token = match (hub, token) {
            (_, Some(t)) => t.to_string(),
            (Some(h), None) => {
                h.hub
                    .lock()
                    .issue_env_token_with_env_id("wsl", &id)
                    .unwrap()
                    .token
            }
            (None, None) => "sync".into(),
        };
        srv.state.hub_sender = Some(Arc::new(HubSender::new(
            srv.env.sessions.clone(),
            HubTarget {
                url: url.to_string(),
                token,
            },
            Duration::from_secs(5),
        )));
        (srv, id)
    }

    fn hub_state() -> cthubd::HubState {
        cthubd::HubState::new(
            SharedHub::new(Hub::open_in_memory().unwrap()),
            "ui-do-hub",
            "hub",
            "hub-id",
        )
    }

    async fn hub_up(st: &cthubd::HubState) -> String {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", l.local_addr().unwrap());
        serve_hub_state(st.clone(), l).await;
        url
    }

    async fn wait(mut cond: impl FnMut() -> bool) {
        tokio::time::timeout(T, async {
            while !cond() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("prazo");
    }

    #[test]
    fn url_do_canal_e_backoff() {
        assert_eq!(
            channel_url("http://10.0.0.2:7879/"),
            "ws://10.0.0.2:7879/sync/channel"
        );
        assert_eq!(
            channel_url("https://hub.ts.net"),
            "wss://hub.ts.net/sync/channel"
        );
        let o = ChannelOptions::default();
        assert_eq!(o.delay(0), PUSH_EVERY);
        assert_eq!(o.delay(2), PUSH_EVERY * 4);
        assert_eq!(o.delay(30), MAX_BACKOFF);
    }

    /// Chamada de ida e volta pelo `cthubd` de verdade, e só os pushes da
    /// lista chegam ao hub.
    #[tokio::test(flavor = "multi_thread")]
    async fn chamada_de_ida_e_volta_e_pushes_da_lista() {
        let hub = hub_state();
        let url = hub_up(&hub).await;
        let (srv, id) = env(&url, Some(&hub), None);
        let task = spawn(srv.state.clone(), fast()).unwrap();
        let ch = Arc::clone(&hub.channels);
        wait(|| ch.is_online(&id)).await;

        let info = ch.call(&id, "env.info", json!({})).await.unwrap();
        assert_eq!(info["env_id"], id.as_str());
        assert_eq!(info["protocol"], crate::PROTOCOL_VERSION);
        let d = ch.call(&id, "dashboard.get", json!({})).await.unwrap();
        assert!(d.get("projects").is_some(), "{d}");
        let e = ch.call(&id, "session.get", json!({})).await.unwrap_err();
        assert_eq!(e.0, -32602, "erro do ambiente volta ao hub: {e:?}");

        let mut pushes = ch.subscribe();
        srv.state
            .events_send("permission_request", json!({ "tool_input": "segredo" }));
        srv.state.events_send("update.status", json!({}));
        srv.state
            .events_send("needs_human", json!({ "session_id": "s1" }));
        let p = tokio::time::timeout(T, pushes.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            (p.env_id.as_str(), p.event.as_str()),
            (id.as_str(), "needs_human")
        );
        assert_eq!(p.data, json!({ "session_id": "s1" }));

        task.abort();
        wait(|| !ch.is_online(&id)).await;
        let e = ch.call(&id, "env.info", json!({})).await.unwrap_err();
        assert_eq!(e.0, CHANNEL_ENV_OFFLINE);
    }

    /// Mesmo que o hub peça (aqui um hub falso, sem a lista dele), o
    /// ambiente recusa o que está fora da lista e atende o que está nela.
    #[tokio::test(flavor = "multi_thread")]
    async fn metodo_fora_da_lista_recusado_no_ambiente() {
        let refused = [
            "session.attach",
            "session.detach",
            "session.input",
            "session.resize",
            "session.peek",
            "session.permission.watch",
            "session.permission.respond",
            "env.update",
            "env.config.set",
            "cleanup.run",
            "github.set_token",
            "github.pr.comment",
            "sync.now",
            "inventado",
        ];
        let (tx, rx) = tokio::sync::oneshot::channel::<Vec<Value>>();
        let tx = Arc::new(std::sync::Mutex::new(Some(tx)));
        let app = axum::Router::new().route(
            CHANNEL_PATH,
            axum::routing::get(move |ws: WebSocketUpgrade| {
                let tx = Arc::clone(&tx);
                async move {
                    ws.on_upgrade(move |mut socket| async move {
                        let mut methods: Vec<&str> = refused.to_vec();
                        methods.push("env.info");
                        for (i, m) in methods.iter().enumerate() {
                            let req =
                                json!({ "id": i, "method": m, "params": { "session_id": "x" } });
                            socket
                                .send(AxMsg::Text(req.to_string().into()))
                                .await
                                .unwrap();
                        }
                        let mut got = Vec::new();
                        while got.len() < methods.len() {
                            match socket.next().await {
                                Some(Ok(AxMsg::Text(t))) => {
                                    got.push(serde_json::from_str::<Value>(&t).unwrap())
                                }
                                Some(Ok(_)) => {}
                                _ => break,
                            }
                        }
                        got.sort_by_key(|v| v["id"].as_u64());
                        if let Some(tx) = tx.lock().unwrap().take() {
                            let _ = tx.send(got);
                        }
                    })
                }
            }),
        );
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", l.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(l, app).await.unwrap() });

        let (srv, _) = env(&url, None, None);
        let task = spawn(srv.state.clone(), fast()).unwrap();
        let got = tokio::time::timeout(T, rx).await.unwrap().unwrap();
        task.abort();
        assert_eq!(got.len(), refused.len() + 1);
        for (m, v) in refused.iter().zip(&got) {
            assert_eq!(v["error"]["code"], CHANNEL_METHOD_UNAVAILABLE, "{m}: {v}");
        }
        assert!(got.last().unwrap()["result"]["env_id"].is_string());
    }

    /// Hub fora do ar no começo: o ambiente tenta com backoff e conecta
    /// quando ele sobe; depois de uma queda, reconecta sozinho.
    #[tokio::test(flavor = "multi_thread")]
    async fn reconexao_depois_de_queda() {
        let hub = hub_state();
        let dead = DeadHub::new();
        let (srv, id) = env(&dead.url, Some(&hub), None);
        let task = spawn(srv.state.clone(), fast()).unwrap();
        tokio::time::sleep(Duration::from_millis(150)).await;
        serve_hub_state(hub.clone(), dead.revive()).await;
        let ch = Arc::clone(&hub.channels);
        wait(|| ch.is_online(&id)).await;

        assert!(ch.disconnect(&id));
        assert!(!ch.is_online(&id));
        wait(|| ch.is_online(&id)).await;
        let info = ch.call(&id, "env.info", json!({})).await.unwrap();
        assert_eq!(info["env_id"], id.as_str());
        task.abort();
    }

    /// Token preso a outro ambiente: o hub recusa (403) e o canal não abre.
    #[tokio::test(flavor = "multi_thread")]
    async fn token_de_outro_ambiente_nao_abre_o_canal() {
        let hub = hub_state();
        let url = hub_up(&hub).await;
        let outro = hub
            .hub
            .lock()
            .issue_env_token_with_env_id("outro", "env-do-outro")
            .unwrap()
            .token;
        let (srv, id) = env(&url, None, Some(&outro));
        let e = connect(&channel_url(&url), &outro, &id).await.unwrap_err();
        assert!(e.contains("403"), "{e}");
        let task = spawn(srv.state.clone(), fast()).unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(hub.channels.online_envs().is_empty());
        task.abort();
    }
}
