//! Cliente do protocolo `/ws` do daemon (spec §9, §12), usado pelo app de UI.
//!
//! Um supervisor por ambiente mantém a conexão: envia o token no handshake,
//! confere a versão do protocolo (`env.info`), reconecta com backoff e, ao
//! reconectar, refaz o attach das sessões anexadas (o snapshot novo chega
//! como `Output { snapshot: true }`, para a UI limpar o terminal antes).

mod reconnect;

pub use reconnect::Backoff;

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;

/// Versão major do protocolo do ambiente que este cliente entende.
pub const PROTOCOL_MAJOR: &str = "2";
/// Versão major do protocolo do `/ws` do hub (`cthubd`, `hub.info`).
pub const HUB_PROTOCOL_MAJOR: &str = "1";
/// Métodos com nome e forma estáveis entre versões major: funcionam mesmo
/// com protocolo incompatível (cada chamada abre uma conexão avulsa), para
/// a UI oferecer a atualização do lado defasado.
pub const STABLE_METHODS: &[&str] = &["env.info", "env.check_update", "env.update"];
pub const CHANNEL_OUTPUT: u8 = 0;
pub const CHANNEL_INPUT: u8 = 1;

/// Um daemon. O `Debug` não mostra o token.
#[derive(Clone)]
pub struct EnvConfig {
    pub name: String,
    /// `ws://host:porta/ws`.
    pub url: String,
    pub token: String,
}

impl fmt::Debug for EnvConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EnvConfig")
            .field("name", &self.name)
            .field("url", &self.url)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ClientEvent {
    /// Mudou o estado da conexão (só é emitido quando muda).
    Status {
        connected: bool,
        error: Option<String>,
    },
    /// Push do daemon (`{event, data}`).
    Push { event: String, data: Value },
    /// Saída de terminal. `snapshot` = scrollback enviado no attach: a UI
    /// limpa o terminal antes de escrever.
    Output {
        session: String,
        bytes: Vec<u8>,
        snapshot: bool,
    },
    /// O re-attach depois de reconectar falhou (ex.: a sessão terminou).
    Detached { session: String, reason: String },
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ClientError {
    #[error("ambiente desconectado")]
    Disconnected,
    #[error("{message}")]
    Rpc { code: i64, message: String },
    #[error("daemon com protocolo incompatível ({daemon})")]
    Incompatible { daemon: String },
    #[error("o daemon não respondeu a tempo")]
    Timeout,
}

/// Verificação de versão logo após conectar: o método que devolve
/// `protocol` e o major aceito.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Handshake {
    pub method: &'static str,
    pub major: &'static str,
}

impl Handshake {
    /// `/ws` do ambiente (`ctowerd`): `env.info`, protocolo 2.x.
    pub const ENV: Handshake = Handshake {
        method: "env.info",
        major: PROTOCOL_MAJOR,
    };
    /// `/ws` do hub (`cthubd`): `hub.info`, protocolo 1.x.
    pub const HUB: Handshake = Handshake {
        method: "hub.info",
        major: HUB_PROTOCOL_MAJOR,
    };
}

#[derive(Debug, Clone)]
pub struct ClientOptions {
    pub backoff: Backoff,
    pub call_timeout: Duration,
    pub handshake: Handshake,
}

impl Default for ClientOptions {
    fn default() -> Self {
        ClientOptions {
            backoff: Backoff::default(),
            call_timeout: Duration::from_secs(30),
            handshake: Handshake::ENV,
        }
    }
}

enum Cmd {
    Call {
        method: String,
        params: Value,
        reply: oneshot::Sender<Result<Value, ClientError>>,
    },
    Input {
        session: String,
        bytes: Vec<u8>,
    },
    Shutdown,
}

#[derive(Default)]
struct Shared {
    connected: AtomicBool,
    /// Versão do daemon, se incompatível.
    incompatible: Mutex<Option<String>>,
    /// Sessões anexadas (refeitas a cada reconexão).
    attached: Mutex<HashSet<String>>,
    attempts: AtomicU64,
}

/// Conexão supervisionada com um daemon. Clonar compartilha a mesma conexão;
/// o supervisor termina com `shutdown` ou quando o último clone some.
#[derive(Clone)]
pub struct Client {
    cmd: mpsc::UnboundedSender<Cmd>,
    shared: Arc<Shared>,
    call_timeout: Duration,
    cfg: Arc<EnvConfig>,
}

impl Client {
    pub fn spawn(cfg: EnvConfig) -> (Self, mpsc::UnboundedReceiver<ClientEvent>) {
        Self::spawn_with(cfg, ClientOptions::default())
    }

    pub fn spawn_with(
        cfg: EnvConfig,
        opts: ClientOptions,
    ) -> (Self, mpsc::UnboundedReceiver<ClientEvent>) {
        let (cmd, cmd_rx) = mpsc::unbounded_channel();
        let (ev, ev_rx) = mpsc::unbounded_channel();
        let shared = Arc::new(Shared::default());
        let cfg = Arc::new(cfg);
        tokio::spawn(supervise(
            (*cfg).clone(),
            opts.handshake,
            opts.backoff,
            shared.clone(),
            cmd_rx,
            ev,
        ));
        let client = Client {
            cmd,
            shared,
            call_timeout: opts.call_timeout,
            cfg,
        };
        (client, ev_rx)
    }

    pub fn is_connected(&self) -> bool {
        self.shared.connected.load(Ordering::SeqCst)
    }

    /// Tentativas de conexão feitas até agora.
    pub fn attempts(&self) -> u64 {
        self.shared.attempts.load(Ordering::SeqCst)
    }

    pub async fn call(&self, method: &str, params: Value) -> Result<Value, ClientError> {
        let incompatible = self.shared.incompatible.lock().unwrap().clone();
        if let Some(daemon) = incompatible {
            if STABLE_METHODS.contains(&method) {
                return call_once(&self.cfg, method, params, self.call_timeout).await;
            }
            return Err(ClientError::Incompatible { daemon });
        }
        if !self.is_connected() {
            return Err(ClientError::Disconnected);
        }
        let (reply, rx) = oneshot::channel();
        self.cmd
            .send(Cmd::Call {
                method: method.into(),
                params,
                reply,
            })
            .map_err(|_| ClientError::Disconnected)?;
        match tokio::time::timeout(self.call_timeout, rx).await {
            Err(_) => Err(ClientError::Timeout),
            // Supervisor descartou a chamada: a conexão caiu antes da resposta.
            Ok(Err(_)) => Err(ClientError::Disconnected),
            Ok(Ok(r)) => r,
        }
    }

    /// Anexa ao terminal da sessão; o snapshot chega como `Output`. A sessão
    /// fica anexada entre reconexões até `detach`.
    pub async fn attach(&self, session: &str) -> Result<(), ClientError> {
        self.shared
            .attached
            .lock()
            .unwrap()
            .insert(session.to_string());
        match self
            .call("session.attach", json!({ "session_id": session }))
            .await
        {
            Ok(_) => Ok(()),
            Err(e) => {
                if !matches!(e, ClientError::Disconnected | ClientError::Timeout) {
                    self.shared.attached.lock().unwrap().remove(session);
                }
                Err(e)
            }
        }
    }

    pub async fn detach(&self, session: &str) -> Result<(), ClientError> {
        self.shared.attached.lock().unwrap().remove(session);
        match self
            .call("session.detach", json!({ "session_id": session }))
            .await
        {
            // Sem conexão não há o que desanexar no daemon.
            Ok(_) | Err(ClientError::Disconnected) => Ok(()),
            Err(e) => Err(e),
        }
    }

    pub fn input(&self, session: &str, bytes: &[u8]) -> Result<(), ClientError> {
        if !self.is_connected() {
            return Err(ClientError::Disconnected);
        }
        self.cmd
            .send(Cmd::Input {
                session: session.into(),
                bytes: bytes.to_vec(),
            })
            .map_err(|_| ClientError::Disconnected)
    }

    pub async fn resize(&self, session: &str, cols: u16, rows: u16) -> Result<(), ClientError> {
        self.call(
            "session.resize",
            json!({ "session_id": session, "cols": cols, "rows": rows }),
        )
        .await
        .map(|_| ())
    }

    pub fn shutdown(&self) {
        let _ = self.cmd.send(Cmd::Shutdown);
    }
}

/// Como terminou uma conexão.
enum End {
    Shutdown,
    /// Caiu depois de funcionar.
    Dropped,
    /// Não chegou a funcionar.
    Failed(String),
}

struct Supervisor {
    shared: Arc<Shared>,
    ev: mpsc::UnboundedSender<ClientEvent>,
    last_status: Option<(bool, Option<String>)>,
    failures: u32,
}

impl Supervisor {
    fn status(&mut self, connected: bool, error: Option<String>) {
        self.shared.connected.store(connected, Ordering::SeqCst);
        let s = (connected, error);
        if self.last_status.as_ref() != Some(&s) {
            let _ = self.ev.send(ClientEvent::Status {
                connected: s.0,
                error: s.1.clone(),
            });
            self.last_status = Some(s);
        }
    }

    fn emit(&self, e: ClientEvent) {
        let _ = self.ev.send(e);
    }
}

async fn supervise(
    cfg: EnvConfig,
    handshake: Handshake,
    backoff: Backoff,
    shared: Arc<Shared>,
    mut cmd_rx: mpsc::UnboundedReceiver<Cmd>,
    ev: mpsc::UnboundedSender<ClientEvent>,
) {
    let mut sup = Supervisor {
        shared,
        ev,
        last_status: None,
        failures: 0,
    };
    loop {
        sup.shared.attempts.fetch_add(1, Ordering::SeqCst);
        let end = match connect(&cfg).await {
            Ok(ws) => run(ws, handshake, &mut sup, &mut cmd_rx).await,
            Err(e) => End::Failed(e),
        };
        match end {
            End::Shutdown => {
                sup.status(false, None);
                return;
            }
            End::Dropped => sup.status(false, Some("conexão perdida".into())),
            End::Failed(e) => sup.status(false, Some(e)),
        }
        let sleep = tokio::time::sleep(backoff.delay(sup.failures));
        sup.failures = sup.failures.saturating_add(1);
        tokio::pin!(sleep);
        loop {
            tokio::select! {
                _ = &mut sleep => break,
                cmd = cmd_rx.recv() => match cmd {
                    None | Some(Cmd::Shutdown) => {
                        sup.status(false, None);
                        return;
                    }
                    Some(Cmd::Call { reply, .. }) => {
                        let _ = reply.send(Err(ClientError::Disconnected));
                    }
                    Some(Cmd::Input { .. }) => {}
                },
            }
        }
    }
}

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn connect(cfg: &EnvConfig) -> Result<Ws, String> {
    let mut req = cfg
        .url
        .as_str()
        .into_client_request()
        .map_err(|e| format!("URL inválida: {e}"))?;
    let auth = format!("Bearer {}", cfg.token)
        .parse()
        .map_err(|_| "token com caracteres inválidos".to_string())?;
    req.headers_mut().insert("authorization", auth);
    let connecting = tokio_tungstenite::connect_async(req);
    match tokio::time::timeout(Duration::from_secs(10), connecting).await {
        Err(_) => Err("daemon não respondeu ao conectar".into()),
        Ok(Err(tokio_tungstenite::tungstenite::Error::Http(r))) if r.status() == 401 => {
            Err("token recusado pelo daemon (401)".into())
        }
        Ok(Err(e)) => Err(format!("daemon inacessível: {e}")),
        Ok(Ok((ws, _))) => Ok(ws),
    }
}

enum Pending {
    /// Verificação de versão logo após conectar.
    EnvInfo,
    /// Chamada do usuário; `attach` = sessão cujo snapshot é esperado.
    User {
        reply: oneshot::Sender<Result<Value, ClientError>>,
        attach: Option<String>,
    },
    /// Re-attach automático depois de reconectar.
    ReAttach(String),
}

/// Uma chamada numa conexão avulsa, sem conferir o protocolo.
async fn call_once(
    cfg: &EnvConfig,
    method: &str,
    params: Value,
    timeout: Duration,
) -> Result<Value, ClientError> {
    let once = async {
        let ws = connect(cfg).await.map_err(|_| ClientError::Disconnected)?;
        let (mut sink, mut stream) = ws.split();
        let req = json!({ "id": 1, "method": method, "params": params });
        sink.send(Message::Text(req.to_string().into()))
            .await
            .map_err(|_| ClientError::Disconnected)?;
        while let Some(Ok(m)) = stream.next().await {
            let Message::Text(t) = m else { continue };
            let Ok(v) = serde_json::from_str::<Value>(&t) else {
                continue;
            };
            if v.get("id").and_then(Value::as_u64) == Some(1) {
                let _ = sink.close().await;
                return rpc_result(&v);
            }
        }
        Err(ClientError::Disconnected)
    };
    tokio::time::timeout(timeout, once)
        .await
        .unwrap_or(Err(ClientError::Timeout))
}

fn rpc_result(v: &Value) -> Result<Value, ClientError> {
    match v.get("error") {
        Some(e) => Err(ClientError::Rpc {
            code: e.get("code").and_then(Value::as_i64).unwrap_or(-32603),
            message: e
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("erro sem mensagem")
                .into(),
        }),
        None => Ok(v.get("result").cloned().unwrap_or(Value::Null)),
    }
}

/// `"2.0"` → compatível se o major for `major`.
fn compatible(protocol: &str, major: &str) -> bool {
    protocol.split('.').next() == Some(major)
}

async fn run(
    ws: Ws,
    handshake: Handshake,
    sup: &mut Supervisor,
    cmd_rx: &mut mpsc::UnboundedReceiver<Cmd>,
) -> End {
    let (mut sink, mut stream) = ws.split();
    let mut next_id: u64 = 0;
    let mut pending: HashMap<u64, Pending> = HashMap::new();
    let mut awaiting_snapshot: HashSet<String> = HashSet::new();
    let mut ready = false;

    macro_rules! send_req {
        ($method:expr, $params:expr, $pending:expr) => {{
            next_id += 1;
            let req = json!({ "id": next_id, "method": $method, "params": $params });
            pending.insert(next_id, $pending);
            if sink.send(Message::Text(req.to_string().into())).await.is_err() {
                return if ready {
                    End::Dropped
                } else {
                    End::Failed("conexão perdida".into())
                };
            }
        }};
    }

    send_req!(handshake.method, json!({}), Pending::EnvInfo);
    loop {
        tokio::select! {
            msg = stream.next() => {
                let msg = match msg {
                    Some(Ok(m)) => m,
                    _ if ready => return End::Dropped,
                    _ => return End::Failed("conexão perdida".into()),
                };
                match msg {
                    Message::Text(t) => {
                        let Ok(v) = serde_json::from_str::<Value>(&t) else { continue };
                        if let Some(event) = v.get("event").and_then(Value::as_str) {
                            sup.emit(ClientEvent::Push {
                                event: event.into(),
                                data: v.get("data").cloned().unwrap_or(Value::Null),
                            });
                            continue;
                        }
                        let Some(p) = v.get("id").and_then(Value::as_u64).and_then(|id| pending.remove(&id)) else {
                            continue;
                        };
                        let result = rpc_result(&v);
                        match p {
                            Pending::EnvInfo => {
                                let protocol = result
                                    .as_ref()
                                    .ok()
                                    .and_then(|r| r.get("protocol"))
                                    .and_then(Value::as_str)
                                    .unwrap_or("?")
                                    .to_string();
                                if !compatible(&protocol, handshake.major) {
                                    let msg = format!("daemon com protocolo incompatível ({protocol})");
                                    *sup.shared.incompatible.lock().unwrap() = Some(protocol);
                                    let _ = sink.close().await;
                                    return End::Failed(msg);
                                }
                                *sup.shared.incompatible.lock().unwrap() = None;
                                ready = true;
                                sup.failures = 0;
                                sup.status(true, None);
                                let sessions: Vec<String> =
                                    sup.shared.attached.lock().unwrap().iter().cloned().collect();
                                for s in sessions {
                                    awaiting_snapshot.insert(s.clone());
                                    send_req!("session.attach", json!({ "session_id": s }), Pending::ReAttach(s));
                                }
                            }
                            Pending::User { reply, attach } => {
                                if let (Some(s), Err(_)) = (&attach, &result) {
                                    awaiting_snapshot.remove(s);
                                }
                                let _ = reply.send(result);
                            }
                            Pending::ReAttach(s) => {
                                if let Err(e) = result {
                                    awaiting_snapshot.remove(&s);
                                    sup.shared.attached.lock().unwrap().remove(&s);
                                    sup.emit(ClientEvent::Detached { session: s, reason: e.to_string() });
                                }
                            }
                        }
                    }
                    Message::Binary(b) => {
                        if b.len() < 17 || b[0] != CHANNEL_OUTPUT {
                            continue;
                        }
                        let Ok(id) = uuid::Uuid::from_slice(&b[1..17]) else { continue };
                        let session = id.to_string();
                        let snapshot = awaiting_snapshot.remove(&session);
                        sup.emit(ClientEvent::Output { session, bytes: b[17..].to_vec(), snapshot });
                    }
                    Message::Close(_) if ready => return End::Dropped,
                    Message::Close(_) => return End::Failed("conexão encerrada pelo daemon".into()),
                    _ => {}
                }
            }
            cmd = cmd_rx.recv() => match cmd {
                None | Some(Cmd::Shutdown) => {
                    let _ = sink.close().await;
                    return End::Shutdown;
                }
                Some(Cmd::Call { method, params, reply }) => {
                    let attach = (method == "session.attach")
                        .then(|| params.get("session_id").and_then(Value::as_str).map(String::from))
                        .flatten();
                    if let Some(s) = &attach {
                        awaiting_snapshot.insert(s.clone());
                    }
                    send_req!(method, params, Pending::User { reply, attach });
                }
                Some(Cmd::Input { session, bytes }) => {
                    let Ok(id) = uuid::Uuid::parse_str(&session) else { continue };
                    let mut f = Vec::with_capacity(17 + bytes.len());
                    f.push(CHANNEL_INPUT);
                    f.extend_from_slice(id.as_bytes());
                    f.extend_from_slice(&bytes);
                    if sink.send(Message::Binary(f.into())).await.is_err() {
                        return End::Dropped;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compatibilidade_pelo_major() {
        let m = PROTOCOL_MAJOR;
        assert!(compatible("2.0", m));
        assert!(compatible("2.7", m));
        // 1.x: daemon de antes do cthubd (papel de hub no ambiente).
        assert!(!compatible("1.2", m));
        assert!(!compatible("3.0", m));
        assert!(!compatible("20.0", m));
        assert!(!compatible("?", m));
        // O hub tem protocolo próprio, a partir de 1.0.
        assert!(compatible("1.0", HUB_PROTOCOL_MAJOR));
        assert!(!compatible("2.0", HUB_PROTOCOL_MAJOR));
    }

    #[test]
    fn debug_nao_mostra_o_token() {
        let cfg = EnvConfig {
            name: "wsl".into(),
            url: "ws://127.0.0.1:7878/ws".into(),
            token: "segredo-123".into(),
        };
        assert!(!format!("{cfg:?}").contains("segredo"));
    }

    #[test]
    fn resultado_e_erro_rpc() {
        assert_eq!(rpc_result(&json!({ "id": 1, "result": 5 })), Ok(json!(5)));
        assert_eq!(
            rpc_result(&json!({ "id": 1, "error": { "code": -32601, "message": "x" } })),
            Err(ClientError::Rpc {
                code: -32601,
                message: "x".into()
            })
        );
    }
}
