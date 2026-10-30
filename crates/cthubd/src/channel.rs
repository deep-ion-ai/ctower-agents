//! Canal reverso ambiente → hub (`GET /sync/channel`, spec da PWA §5, #198).
//!
//! O `ctowerd` abre e mantém um WebSocket com o hub, autenticado como o
//! `/sync/config`: token de sync preso ao `env_id` de `X-Ctower-Env-Id`
//! (outro `env_id` dá `403`). Por ele o hub chama métodos do `/ws` do
//! ambiente e recebe os pushes do ambiente em tempo real; o hub nunca abre
//! conexão com o ambiente, que pode estar atrás de NAT.
//!
//! - Um canal por ambiente: a conexão nova do mesmo `env_id` substitui a
//!   anterior, que fecha, e as chamadas em voo nela falham como offline.
//! - Hub → ambiente `{id, method, params}`; ambiente → hub `{id, result}` ou
//!   `{id, error}` e pushes `{event, data}`. Só frames de texto.
//! - [`EnvChannels::call`] aplica a lista [`ct_http::sync::CHANNEL_METHODS`]
//!   antes de mandar (o ambiente aplica de novo antes de despachar), espera
//!   [`ChannelOptions::call_timeout`] (30 s) e devolve
//!   [`CHANNEL_TIMEOUT`] "ambiente não respondeu" ou [`CHANNEL_ENV_OFFLINE`]
//!   "ambiente offline".
//! - O hub manda ping a cada 30 s; sem nenhum frame (o pong conta) em 60 s,
//!   o ambiente fica offline e a conexão fecha.
//! - Online/offline em tempo real: `HubEnv.online` em `hub.envs.list` e
//!   `hub.dashboard.get`, e o push `env.online { env_id, online }` no `/ws`
//!   e no `/web/ws`.
//! - Os pushes do ambiente da lista [`ct_http::sync::CHANNEL_PUSHES`] saem
//!   em [`EnvChannels::subscribe`] como [`EnvPush`] e vão às UIs (`/ws` e
//!   `/web/ws`) como `env.push { env_id, event, data }` (#343).
//! - `hub.env.call { env, method, params }` (`crate::ws`, #343) chama
//!   [`EnvChannels::call`]; a Web Push usa [`EnvChannels::states`] para o
//!   `env_stale` em tempo real (spec da PWA §7.1).

use crate::HubState;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use ct_http::rpc::RpcResult;
use ct_http::sync::{channel_method_allowed, channel_push_allowed, ENV_ID_HEADER};
pub use ct_http::sync::{CHANNEL_ENV_OFFLINE, CHANNEL_METHOD_UNAVAILABLE, CHANNEL_TIMEOUT};
use ct_http::{bearer_token, unauthorized};
use ct_ledger::{HubEnv, LedgerError};
use futures_util::{SinkExt, StreamExt};
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::{broadcast, mpsc, oneshot, Notify};

/// Prazos do canal (os testes encurtam).
#[derive(Debug, Clone, Copy)]
pub struct ChannelOptions {
    /// Prazo de cada chamada ao ambiente.
    pub call_timeout: Duration,
    /// Intervalo do ping do hub.
    pub ping_every: Duration,
    /// Sem nenhum frame do ambiente por este tempo, o canal fecha.
    pub pong_timeout: Duration,
}

impl Default for ChannelOptions {
    fn default() -> Self {
        ChannelOptions {
            call_timeout: Duration::from_secs(30),
            ping_every: Duration::from_secs(30),
            pong_timeout: Duration::from_secs(60),
        }
    }
}

/// Push de um ambiente recebido pelo canal; vai às UIs como `env.push`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct EnvPush {
    pub env_id: String,
    /// Um de `ct_http::sync::CHANNEL_PUSHES`.
    pub event: String,
    pub data: Value,
}

/// Dados do push `env.online`: o canal do ambiente abriu ou fechou.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct EnvOnline {
    pub env_id: String,
    pub online: bool,
}

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<RpcResult>>>>;

/// Canal aberto de um ambiente.
struct Link {
    /// Identifica a conexão (a nova substitui a velha).
    conn: u64,
    out: mpsc::UnboundedSender<Message>,
    pending: Pending,
    /// Encerra o laço da conexão (substituída ou derrubada pelo hub).
    stop: Arc<Notify>,
}

fn fail_all(pending: &Pending, why: &str) {
    let drained: Vec<_> = pending
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .drain()
        .collect();
    for (_, tx) in drained {
        let _ = tx.send(Err((CHANNEL_ENV_OFFLINE, why.to_string())));
    }
}

/// Os canais abertos, por `env_id`.
pub struct EnvChannels {
    opts: ChannelOptions,
    links: Mutex<HashMap<String, Link>>,
    next: AtomicU64,
    /// Push para as UIs do hub (`env.online`).
    events: broadcast::Sender<Value>,
    pushes: broadcast::Sender<EnvPush>,
    /// Ambientes que já tiveram canal desde que o hub subiu e estão sem ele
    /// agora: `env_id` → quando o canal fechou (ms).
    closed: Mutex<HashMap<String, i64>>,
}

impl EnvChannels {
    pub fn new(events: broadcast::Sender<Value>, opts: ChannelOptions) -> Self {
        let (pushes, _) = broadcast::channel(1024);
        EnvChannels {
            opts,
            links: Mutex::default(),
            next: AtomicU64::new(1),
            events,
            pushes,
            closed: Mutex::default(),
        }
    }

    pub fn options(&self) -> ChannelOptions {
        self.opts
    }

    fn links(&self) -> std::sync::MutexGuard<'_, HashMap<String, Link>> {
        self.links.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// O ambiente tem canal aberto agora.
    pub fn is_online(&self, env_id: &str) -> bool {
        self.links().contains_key(env_id)
    }

    /// `env_id` dos ambientes com canal aberto.
    pub fn online_envs(&self) -> HashSet<String> {
        self.links().keys().cloned().collect()
    }

    /// Preenche `HubEnv.online`.
    pub fn mark_online(&self, envs: &mut [HubEnv]) {
        let on = self.online_envs();
        for e in envs {
            e.online = on.contains(&e.env_id);
        }
    }

    /// Estado do canal por `env_id`, para o `env_stale` da Web Push: `None` =
    /// canal aberto; `Some(t)` = fechado desde `t` (ms). Ambiente que não
    /// abriu canal desde que o hub subiu (`ctowerd` antigo) fica de fora e
    /// segue pelo último sync.
    pub fn states(&self) -> ct_ledger::ChannelStates {
        let mut out: ct_ledger::ChannelStates = self
            .closed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .map(|(k, v)| (k.clone(), Some(*v)))
            .collect();
        for id in self.online_envs() {
            out.insert(id, None);
        }
        out
    }

    /// Testes: o canal de `env_id` fechou em `at` (ms).
    #[cfg(test)]
    pub(crate) fn set_closed_at(&self, env_id: &str, at: i64) {
        self.closed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(env_id.to_string(), at);
    }

    /// Pushes dos ambientes (só os de `CHANNEL_PUSHES`), em tempo real.
    pub fn subscribe(&self) -> broadcast::Receiver<EnvPush> {
        self.pushes.subscribe()
    }

    /// Chama `method` no ambiente pelo canal, com a lista do §5.2 aplicada
    /// antes. Erros: [`CHANNEL_METHOD_UNAVAILABLE`], [`CHANNEL_ENV_OFFLINE`]
    /// (sem canal, ou ele caiu antes da resposta), [`CHANNEL_TIMEOUT`] e o
    /// erro que o ambiente devolveu.
    pub async fn call(&self, env_id: &str, method: &str, params: Value) -> RpcResult {
        if !channel_method_allowed(method) {
            return Err((
                CHANNEL_METHOD_UNAVAILABLE,
                format!("método indisponível pelo canal: {method}"),
            ));
        }
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        let pending = {
            let links = self.links();
            let Some(link) = links.get(env_id) else {
                return Err((CHANNEL_ENV_OFFLINE, "ambiente offline".into()));
            };
            link.pending
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(id, tx);
            let req = json!({ "id": id, "method": method, "params": params });
            if link
                .out
                .send(Message::Text(req.to_string().into()))
                .is_err()
            {
                link.pending
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&id);
                return Err((CHANNEL_ENV_OFFLINE, "ambiente offline".into()));
            }
            Arc::clone(&link.pending)
        };
        match tokio::time::timeout(self.opts.call_timeout, rx).await {
            Ok(Ok(r)) => r,
            Ok(Err(_)) => Err((CHANNEL_ENV_OFFLINE, "ambiente offline".into())),
            Err(_) => {
                pending
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&id);
                Err((CHANNEL_TIMEOUT, "ambiente não respondeu".into()))
            }
        }
    }

    /// Derruba o canal do ambiente (ele reconecta com backoff). `false` =
    /// não havia canal.
    pub fn disconnect(&self, env_id: &str) -> bool {
        let Some(link) = self.links().remove(env_id) else {
            return false;
        };
        link.stop.notify_one();
        fail_all(&link.pending, "ambiente offline: canal fechado pelo hub");
        self.online_changed(env_id, false);
        true
    }

    fn online_changed(&self, env_id: &str, online: bool) {
        {
            let mut closed = self.closed.lock().unwrap_or_else(|e| e.into_inner());
            if online {
                closed.remove(env_id);
            } else {
                closed.insert(env_id.to_string(), ct_core::now_ms());
            }
        }
        let data = EnvOnline {
            env_id: env_id.to_string(),
            online,
        };
        let _ = self
            .events
            .send(json!({ "event": "env.online", "data": data }));
    }

    /// Conexão nova do ambiente; a anterior, se houver, fecha.
    fn register(&self, env_id: &str, out: mpsc::UnboundedSender<Message>) -> Link {
        let conn = self.next.fetch_add(1, Ordering::Relaxed);
        let link = Link {
            conn,
            out,
            pending: Pending::default(),
            stop: Arc::new(Notify::new()),
        };
        let mine = Link {
            conn,
            out: link.out.clone(),
            pending: Arc::clone(&link.pending),
            stop: Arc::clone(&link.stop),
        };
        let old = self.links().insert(env_id.to_string(), link);
        match old {
            Some(old) => {
                tracing::info!("canal de {env_id}: conexão nova substitui a anterior");
                old.stop.notify_one();
                fail_all(&old.pending, "ambiente offline: canal substituído");
            }
            None => self.online_changed(env_id, true),
        }
        mine
    }

    /// A conexão `conn` terminou; se ainda era a do ambiente, ele fica offline.
    fn unregister(&self, env_id: &str, conn: u64) {
        let removed = {
            let mut links = self.links();
            match links.get(env_id) {
                Some(l) if l.conn == conn => links.remove(env_id),
                _ => None,
            }
        };
        if let Some(link) = removed {
            fail_all(&link.pending, "ambiente offline: o canal caiu");
            self.online_changed(env_id, false);
        }
    }

    /// Frame de texto do ambiente: resposta de uma chamada ou push.
    fn incoming(&self, env_id: &str, pending: &Pending, text: &str) {
        let Ok(v) = serde_json::from_str::<Value>(text) else {
            tracing::warn!("canal de {env_id}: frame que não é JSON, ignorado");
            return;
        };
        if let Some(event) = v.get("event").and_then(Value::as_str) {
            if channel_push_allowed(event) {
                let push = EnvPush {
                    env_id: env_id.to_string(),
                    event: event.to_string(),
                    data: v.get("data").cloned().unwrap_or(Value::Null),
                };
                // Às UIs do hub (`/ws` e `/web/ws`) como `env.push`.
                let _ = self
                    .events
                    .send(json!({ "event": "env.push", "data": push }));
                let _ = self.pushes.send(push);
            } else {
                tracing::debug!("canal de {env_id}: push {event} fora da lista, ignorado");
            }
            return;
        }
        let Some(id) = v.get("id").and_then(Value::as_u64) else {
            return;
        };
        let Some(tx) = pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&id)
        else {
            return; // prazo estourado: a resposta chegou tarde
        };
        let r = match v.get("error") {
            Some(err) => Err((
                err.get("code").and_then(Value::as_i64).unwrap_or(-32000),
                err.get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("erro do ambiente")
                    .to_string(),
            )),
            None => Ok(v.get("result").cloned().unwrap_or(Value::Null)),
        };
        let _ = tx.send(r);
    }
}

/// `GET /sync/channel`: autentica como o `/sync/config` e abre o canal.
pub(crate) async fn upgrade(
    State(st): State<HubState>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    let Some(token) = bearer_token(&headers) else {
        return unauthorized();
    };
    if let Some(r) = crate::sync::agent_token_here(&token) {
        return r;
    }
    let Some(env_id) = headers
        .get(ENV_ID_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string)
    else {
        return (StatusCode::BAD_REQUEST, "falta o cabeçalho X-Ctower-Env-Id").into_response();
    };
    let hub = st.hub.clone();
    let id = env_id.clone();
    let r = tokio::task::spawn_blocking(move || -> Result<Option<String>, LedgerError> {
        let hub = hub.lock();
        let Some(env) = hub.authenticate(&token)? else {
            return Ok(None);
        };
        hub.bind_env_token(&env.name, &id)?;
        Ok(Some(env.name))
    })
    .await;
    let name = match r {
        Ok(Ok(Some(name))) => name,
        Ok(Ok(None)) => return unauthorized(),
        Ok(Err(LedgerError::Conflict(m))) => return (StatusCode::FORBIDDEN, m).into_response(),
        Ok(Err(e)) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };
    ws.on_upgrade(move |socket| serve(socket, st, env_id, name))
}

async fn serve(socket: WebSocket, st: HubState, env_id: String, name: String) {
    let channels = Arc::clone(&st.channels);
    let opts = channels.options();
    let (mut sink, mut stream) = socket.split();
    let (out, mut out_rx) = mpsc::unbounded_channel::<Message>();
    let writer = tokio::spawn(async move {
        while let Some(m) = out_rx.recv().await {
            let close = matches!(m, Message::Close(_));
            if sink.send(m).await.is_err() || close {
                break;
            }
        }
    });
    let link = channels.register(&env_id, out);
    tracing::info!("canal de {name} ({env_id}) aberto");
    let mut ping = tokio::time::interval(opts.ping_every);
    ping.reset();
    let mut last = Instant::now();
    let why = loop {
        tokio::select! {
            m = stream.next() => match m {
                Some(Ok(msg)) => {
                    last = Instant::now();
                    match msg {
                        Message::Text(t) => channels.incoming(&env_id, &link.pending, &t),
                        Message::Close(_) => break "fechado pelo ambiente",
                        _ => {}
                    }
                }
                Some(Err(_)) | None => break "conexão caiu",
            },
            _ = ping.tick() => {
                if last.elapsed() > opts.pong_timeout {
                    break "sem pong";
                }
                let _ = link.out.send(Message::Ping(Vec::new().into()));
            }
            _ = link.stop.notified() => break "substituído ou derrubado pelo hub",
        }
    };
    tracing::info!("canal de {name} ({env_id}) fechado: {why}");
    channels.unregister(&env_id, link.conn);
    let _ = link.out.send(Message::Close(None));
    drop(link);
    let _ = tokio::time::timeout(Duration::from_secs(5), writer).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::router;
    use ct_ledger::{Hub, SharedHub};
    use tokio::net::TcpListener;
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    use tokio_tungstenite::tungstenite::Message as WsMsg;
    use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};

    type Client = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;
    const T: Duration = Duration::from_secs(10);

    fn state(opts: ChannelOptions) -> HubState {
        let mut st = HubState::new(
            SharedHub::new(Hub::open_in_memory().unwrap()),
            "ui-secreto",
            "hub",
            "hub-id",
        );
        st.channels = Arc::new(EnvChannels::new(st.events.clone(), opts));
        st
    }

    async fn serve_hub(st: &HubState) -> String {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        let app = router(st.clone());
        tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
        format!("ws://{addr}{}", ct_http::sync::CHANNEL_PATH)
    }

    async fn open(
        url: &str,
        token: Option<&str>,
        env_id: Option<&str>,
        extra: &[(&'static str, &str)],
    ) -> Result<Client, u16> {
        let mut req = url.into_client_request().unwrap();
        let h = req.headers_mut();
        if let Some(t) = token {
            h.insert("authorization", format!("Bearer {t}").parse().unwrap());
        }
        if let Some(e) = env_id {
            h.insert(ENV_ID_HEADER, e.parse().unwrap());
        }
        for (k, v) in extra {
            h.insert(*k, v.parse().unwrap());
        }
        match connect_async(req).await {
            Ok((ws, _)) => Ok(ws),
            Err(tokio_tungstenite::tungstenite::Error::Http(r)) => Err(r.status().as_u16()),
            Err(e) => panic!("{e}"),
        }
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

    /// Ambiente falso: responde cada chamada com o método e os parâmetros,
    /// `lento` sem responder.
    fn fake_env(ws: Client) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let (mut tx, mut rx) = ws.split();
            while let Some(Ok(m)) = rx.next().await {
                let WsMsg::Text(t) = m else { continue };
                let v: Value = serde_json::from_str(&t).unwrap();
                if v["params"]["lento"] == true {
                    continue;
                }
                let r = json!({ "id": v["id"], "result": { "method": v["method"], "params": v["params"] } });
                if tx.send(WsMsg::Text(r.to_string().into())).await.is_err() {
                    break;
                }
            }
        })
    }

    #[tokio::test]
    async fn token_de_outro_ambiente_e_recusado() {
        let st = state(ChannelOptions::default());
        let (tok_a, tok_b) = {
            let h = st.hub.lock();
            (
                h.issue_env_token_with_env_id("a", "env-a").unwrap().token,
                h.issue_env_token_with_env_id("b", "env-b").unwrap().token,
            )
        };
        let url = serve_hub(&st).await;
        assert_eq!(open(&url, None, Some("env-a"), &[]).await.err(), Some(401));
        assert_eq!(
            open(&url, Some("errado"), Some("env-a"), &[]).await.err(),
            Some(401)
        );
        assert_eq!(open(&url, Some(&tok_a), None, &[]).await.err(), Some(400));
        let a = open(&url, Some(&tok_a), Some("env-a"), &[]).await.unwrap();
        let b = open(&url, Some(&tok_b), Some("env-b"), &[]).await.unwrap();
        // Token de b com o env_id de a, e o de a com outro env_id: 403.
        assert_eq!(
            open(&url, Some(&tok_b), Some("env-a"), &[]).await.err(),
            Some(403)
        );
        assert_eq!(
            open(&url, Some(&tok_a), Some("env-x"), &[]).await.err(),
            Some(403)
        );
        assert!(st.channels.is_online("env-a") && st.channels.is_online("env-b"));
        drop((a, b));
    }

    #[tokio::test]
    async fn cloudflare_nao_alcanca_o_canal() {
        let st = state(ChannelOptions::default());
        let tok = st
            .hub
            .lock()
            .issue_env_token_with_env_id("a", "env-a")
            .unwrap()
            .token;
        let url = serve_hub(&st).await;
        let r = open(
            &url,
            Some(&tok),
            Some("env-a"),
            &[("cf-connecting-ip", "203.0.113.9")],
        )
        .await;
        assert_eq!(r.err(), Some(404));
        assert!(!st.channels.is_online("env-a"));
    }

    #[tokio::test]
    async fn chamada_ida_e_volta_lista_prazo_e_offline() {
        let st = state(ChannelOptions {
            call_timeout: Duration::from_millis(300),
            ..ChannelOptions::default()
        });
        let tok = st
            .hub
            .lock()
            .issue_env_token_with_env_id("a", "env-a")
            .unwrap()
            .token;
        let url = serve_hub(&st).await;
        let ch = Arc::clone(&st.channels);

        let r = ch.call("env-a", "env.info", json!({})).await;
        assert_eq!(r.unwrap_err().0, CHANNEL_ENV_OFFLINE);

        let env = fake_env(open(&url, Some(&tok), Some("env-a"), &[]).await.unwrap());
        wait(|| ch.is_online("env-a")).await;
        let r = ch
            .call("env-a", "session.start", json!({ "number": 7 }))
            .await
            .unwrap();
        assert_eq!(
            r,
            json!({ "method": "session.start", "params": { "number": 7 } })
        );

        // Chamadas concorrentes, cada uma com a sua resposta.
        let (x, y) = tokio::join!(
            ch.call("env-a", "session.get", json!({ "n": 1 })),
            ch.call("env-a", "session.get", json!({ "n": 2 })),
        );
        assert_eq!(x.unwrap()["params"]["n"], 1);
        assert_eq!(y.unwrap()["params"]["n"], 2);

        // Fora da lista: o hub recusa antes de mandar.
        for m in [
            "session.input",
            "session.attach",
            "cleanup.run",
            "env.update",
        ] {
            let e = ch.call("env-a", m, json!({})).await.unwrap_err();
            assert_eq!(e.0, CHANNEL_METHOD_UNAVAILABLE, "{m}");
        }

        let e = ch
            .call("env-a", "env.info", json!({ "lento": true }))
            .await
            .unwrap_err();
        assert_eq!(e, (CHANNEL_TIMEOUT, "ambiente não respondeu".into()));

        env.abort();
        wait(|| !ch.is_online("env-a")).await;
        let e = ch.call("env-a", "env.info", json!({})).await.unwrap_err();
        assert_eq!(e.0, CHANNEL_ENV_OFFLINE);
    }

    #[tokio::test]
    async fn online_em_tempo_real_e_conexao_nova_substitui() {
        let st = state(ChannelOptions::default());
        let tok = st
            .hub
            .lock()
            .issue_env_token_with_env_id("a", "env-a")
            .unwrap()
            .token;
        st.hub.lock().ingest("env-a", "a", &[]).unwrap();
        let url = serve_hub(&st).await;
        let mut events = st.events.subscribe();
        let envs = |st: &HubState| {
            let mut v = st.hub.lock().list_envs().unwrap();
            st.channels.mark_online(&mut v);
            v
        };
        assert!(!envs(&st)[0].online);
        let v = crate::ws::tests::tests_call(&st, "hub.envs.list").await;
        assert_eq!(v[0]["online"], false);
        // Sem canal desde que o hub subiu: fora do estado (o `env_stale`
        // segue pelo último sync).
        assert!(st.channels.states().is_empty());

        let first = fake_env(open(&url, Some(&tok), Some("env-a"), &[]).await.unwrap());
        wait(|| st.channels.is_online("env-a")).await;
        let v = crate::ws::tests::tests_call(&st, "hub.envs.list").await;
        assert_eq!(v[0]["online"], true);
        assert_eq!(
            st.channels.states(),
            ct_ledger::ChannelStates::from([("env-a".into(), None)])
        );
        let v = crate::ws::tests::tests_call(&st, "hub.dashboard.get").await;
        assert_eq!(v["envs"][0]["online"], true);
        let ev = events.recv().await.unwrap();
        assert_eq!(
            ev,
            json!({ "event": "env.online", "data": { "env_id": "env-a", "online": true } })
        );

        // Conexão nova do mesmo ambiente: a velha fecha, sem passar por offline.
        let second = fake_env(open(&url, Some(&tok), Some("env-a"), &[]).await.unwrap());
        tokio::time::timeout(T, first).await.unwrap().unwrap();
        assert!(st.channels.is_online("env-a"));
        let r = st.channels.call("env-a", "env.info", json!({})).await;
        assert_eq!(r.unwrap()["method"], "env.info");

        second.abort();
        let before = ct_core::now_ms();
        wait(|| !st.channels.is_online("env-a")).await;
        assert!(!envs(&st)[0].online);
        // Fechado: o `env_stale` da Web Push conta a partir daqui.
        let closed = st.channels.states()["env-a"].expect("fechado");
        assert!(closed >= before && closed <= ct_core::now_ms());
        let ev = events.recv().await.unwrap();
        assert_eq!(ev["data"], json!({ "env_id": "env-a", "online": false }));
        assert!(events.try_recv().is_err(), "a troca não gerou offline");
    }

    #[tokio::test]
    async fn pushes_da_lista_e_ambiente_mudo_fica_offline() {
        let st = state(ChannelOptions {
            ping_every: Duration::from_millis(50),
            pong_timeout: Duration::from_millis(200),
            ..ChannelOptions::default()
        });
        let tok = st
            .hub
            .lock()
            .issue_env_token_with_env_id("a", "env-a")
            .unwrap()
            .token;
        let url = serve_hub(&st).await;
        let mut pushes = st.channels.subscribe();
        let mut ws = open(&url, Some(&tok), Some("env-a"), &[]).await.unwrap();
        for e in ["permission_request", "update.status", "needs_human"] {
            let p = json!({ "event": e, "data": { "session_id": "s1" } });
            ws.send(WsMsg::Text(p.to_string().into())).await.unwrap();
        }
        let p = tokio::time::timeout(T, pushes.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            p,
            EnvPush {
                env_id: "env-a".into(),
                event: "needs_human".into(),
                data: json!({ "session_id": "s1" }),
            }
        );
        // Lendo, o cliente responde os pings e o canal fica de pé.
        let reader = tokio::spawn(async move {
            let deadline = Instant::now() + Duration::from_millis(500);
            while Instant::now() < deadline {
                let _ = tokio::time::timeout(Duration::from_millis(20), ws.next()).await;
            }
            ws
        });
        let ws = reader.await.unwrap();
        assert!(st.channels.is_online("env-a"));
        // Parado (sem ler, sem pong), cai depois do prazo.
        wait(|| !st.channels.is_online("env-a")).await;
        drop(ws);
    }
}
