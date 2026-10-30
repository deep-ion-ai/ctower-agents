//! Ponte entre os `ct_client::Client` (um por ambiente) e a janela: repassa
//! status e pushes como eventos (`daemon://status`, `daemon://push`) e a
//! saída de terminal pelo canal de cada attach. O token só existe aqui e no
//! cliente; nenhum payload que vai para o JavaScript o carrega.

use crate::envs::{EnvEntry, NotifyPrefs};
use crate::notify;
use ct_client::{Client, ClientEvent, ClientOptions, EnvConfig, Handshake};
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Destino dos eventos (a janela, ou um coletor nos testes).
pub trait Sink: Send + Sync + 'static {
    fn emit(&self, event: &str, payload: Value);
    /// Notificação nativa do sistema.
    fn notify(&self, title: &str, body: &str);
}

/// Recebe a saída de um terminal: 1º byte = 1 se snapshot, 0 se ao vivo.
pub type TermSink = Box<dyn Fn(Vec<u8>) + Send + Sync>;

/// Obtém o token do ambiente (lê da distro no tipo `wsl`).
pub type Resolver = Arc<dyn Fn(&EnvEntry) -> Result<String, String> + Send + Sync>;

pub const STATUS_EVENT: &str = "daemon://status";
pub const PUSH_EVENT: &str = "daemon://push";

/// Espera antes de tentar de novo ler o token de um ambiente.
const TOKEN_RETRY: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EnvStatus {
    pub env: String,
    pub connected: bool,
    pub error: Option<String>,
    pub since: i64,
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or_default()
}

struct Conn {
    client: Option<Client>,
    entry: EnvEntry,
    /// Muda a cada reconfiguração; tarefas de uma geração antiga param.
    gen: u64,
}

pub struct Bridge {
    sink: Arc<dyn Sink>,
    resolve: Resolver,
    conns: Mutex<HashMap<String, Conn>>,
    statuses: Mutex<HashMap<String, EnvStatus>>,
    terms: Mutex<HashMap<(String, String), TermSink>>,
    next_gen: AtomicU64,
    prefs: Mutex<NotifyPrefs>,
    /// `env.info` (ambientes) ou `hub.info` (conexão com o hub).
    handshake: Handshake,
}

impl Bridge {
    pub fn new(sink: Arc<dyn Sink>, resolve: Resolver, prefs: NotifyPrefs) -> Arc<Self> {
        Self::with_handshake(sink, resolve, prefs, Handshake::ENV)
    }

    /// Ponte com outro handshake (a do hub usa [`Handshake::HUB`]).
    pub fn with_handshake(
        sink: Arc<dyn Sink>,
        resolve: Resolver,
        prefs: NotifyPrefs,
        handshake: Handshake,
    ) -> Arc<Self> {
        Arc::new(Bridge {
            sink,
            resolve,
            conns: Mutex::default(),
            statuses: Mutex::default(),
            terms: Mutex::default(),
            next_gen: AtomicU64::new(1),
            prefs: Mutex::new(prefs),
            handshake,
        })
    }

    pub fn set_prefs(&self, prefs: NotifyPrefs) {
        *self.prefs.lock().unwrap() = prefs;
    }

    /// Conecta os ambientes (precisa de um runtime tokio ativo).
    pub fn start(self: &Arc<Self>, envs: Vec<EnvEntry>) {
        for e in envs {
            self.connect(e);
        }
    }

    /// Reconecta só os ambientes que mudaram; remove os que saíram.
    pub fn reconfigure(self: &Arc<Self>, envs: Vec<EnvEntry>) {
        let stale: Vec<String> = {
            let conns = self.conns.lock().unwrap();
            conns
                .iter()
                .filter(|(id, c)| !envs.iter().any(|e| &e.id == *id && *e == c.entry))
                .map(|(id, _)| id.clone())
                .collect()
        };
        for id in &stale {
            self.disconnect(id);
        }
        let current: Vec<String> = self.conns.lock().unwrap().keys().cloned().collect();
        for e in envs {
            if !current.contains(&e.id) {
                self.connect(e);
            }
        }
    }

    fn disconnect(&self, env: &str) {
        if let Some(c) = self.conns.lock().unwrap().remove(env) {
            if let Some(client) = c.client {
                client.shutdown();
            }
        }
        self.statuses.lock().unwrap().remove(env);
        let lost: Vec<String> = {
            let mut terms = self.terms.lock().unwrap();
            let keys: Vec<_> = terms.keys().filter(|(e, _)| e == env).cloned().collect();
            keys.into_iter()
                .map(|k| {
                    terms.remove(&k);
                    k.1
                })
                .collect()
        };
        for s in lost {
            self.push(
                env,
                "attach_lost",
                json!({ "session_id": s, "reason": "ambiente reconfigurado" }),
            );
        }
    }

    fn connect(self: &Arc<Self>, entry: EnvEntry) {
        let gen = self.next_gen.fetch_add(1, Ordering::SeqCst);
        let env = entry.id.clone();
        self.conns.lock().unwrap().insert(
            env.clone(),
            Conn {
                client: None,
                entry: entry.clone(),
                gen,
            },
        );
        let me = self.clone();
        tokio::spawn(async move {
            let token = loop {
                let resolve = me.resolve.clone();
                let e = entry.clone();
                let r = tokio::task::spawn_blocking(move || resolve(&e))
                    .await
                    .unwrap_or_else(|e| Err(e.to_string()));
                if !me.current(&env, gen) {
                    return;
                }
                match r {
                    Ok(t) => break t,
                    Err(err) => {
                        me.handle(
                            &env,
                            ClientEvent::Status {
                                connected: false,
                                error: Some(format!("token de UI indisponível: {err}")),
                            },
                        );
                        tokio::time::sleep(TOKEN_RETRY).await;
                    }
                }
            };
            let (client, mut rx) = Client::spawn_with(
                EnvConfig {
                    name: entry.name.clone(),
                    url: entry.url.clone(),
                    token,
                },
                ClientOptions {
                    handshake: me.handshake,
                    ..ClientOptions::default()
                },
            );
            {
                let mut conns = me.conns.lock().unwrap();
                match conns.get_mut(&env) {
                    Some(c) if c.gen == gen => c.client = Some(client),
                    _ => {
                        client.shutdown();
                        return;
                    }
                }
            }
            while let Some(ev) = rx.recv().await {
                if !me.current(&env, gen) {
                    return;
                }
                me.handle(&env, ev);
            }
        });
    }

    fn current(&self, env: &str, gen: u64) -> bool {
        self.conns
            .lock()
            .unwrap()
            .get(env)
            .is_some_and(|c| c.gen == gen)
    }

    fn push(&self, env: &str, event: &str, data: Value) {
        self.sink.emit(
            PUSH_EVENT,
            json!({ "env": env, "event": event, "data": data }),
        );
    }

    /// Traduz um evento do cliente para a janela.
    pub fn handle(&self, env: &str, ev: ClientEvent) {
        let was = self.statuses.lock().unwrap().get(env).map(|s| s.connected);
        let name = self
            .entry(env)
            .map(|e| e.name)
            .unwrap_or_else(|| env.to_string());
        let prefs = self.prefs.lock().unwrap().clone();
        // Ambiente silenciado nas Configurações (#315): nenhuma notificação dele.
        let muted = prefs.muted_envs.iter().any(|m| m == env);
        if let Some(n) = notify::rule(&prefs, &name, &ev, was).filter(|_| !muted) {
            self.sink.notify(&n.title, &n.body);
        }
        match ev {
            ClientEvent::Status { connected, error } => {
                let st = EnvStatus {
                    env: env.into(),
                    connected,
                    error,
                    since: now_ms(),
                };
                self.statuses.lock().unwrap().insert(env.into(), st.clone());
                self.sink.emit(STATUS_EVENT, json!(st));
            }
            ClientEvent::Push { event, data } => self.push(env, &event, data),
            ClientEvent::Output {
                session,
                bytes,
                snapshot,
            } => {
                let terms = self.terms.lock().unwrap();
                if let Some(t) = terms.get(&(env.to_string(), session)) {
                    let mut b = Vec::with_capacity(bytes.len() + 1);
                    b.push(u8::from(snapshot));
                    b.extend_from_slice(&bytes);
                    t(b);
                }
            }
            ClientEvent::Detached { session, reason } => {
                self.terms
                    .lock()
                    .unwrap()
                    .remove(&(env.to_string(), session.clone()));
                self.push(
                    env,
                    "attach_lost",
                    json!({ "session_id": session, "reason": reason }),
                );
            }
        }
    }

    pub fn statuses(&self) -> Vec<EnvStatus> {
        let mut v: Vec<_> = self.statuses.lock().unwrap().values().cloned().collect();
        v.sort_by(|a, b| a.env.cmp(&b.env));
        v
    }

    pub fn entry(&self, env: &str) -> Option<EnvEntry> {
        self.conns.lock().unwrap().get(env).map(|c| c.entry.clone())
    }

    fn client(&self, env: &str) -> Result<Client, String> {
        let conns = self.conns.lock().unwrap();
        let c = conns
            .get(env)
            .ok_or_else(|| format!("ambiente desconhecido: {env}"))?;
        c.client
            .clone()
            .ok_or_else(|| "ambiente desconectado".to_string())
    }

    pub async fn call(&self, env: &str, method: &str, params: Value) -> Result<Value, String> {
        self.client(env)?
            .call(method, params)
            .await
            .map_err(|e| e.to_string())
    }

    pub async fn attach(&self, env: &str, session: &str, sink: TermSink) -> Result<(), String> {
        let client = self.client(env)?;
        self.terms
            .lock()
            .unwrap()
            .insert((env.into(), session.into()), sink);
        let r = client.attach(session).await.map_err(|e| e.to_string());
        if r.is_err() {
            self.terms
                .lock()
                .unwrap()
                .remove(&(env.to_string(), session.to_string()));
        }
        r
    }

    pub async fn detach(&self, env: &str, session: &str) -> Result<(), String> {
        self.terms
            .lock()
            .unwrap()
            .remove(&(env.to_string(), session.to_string()));
        self.client(env)?
            .detach(session)
            .await
            .map_err(|e| e.to_string())
    }

    pub fn input(&self, env: &str, session: &str, data: &str) -> Result<(), String> {
        self.client(env)?
            .input(session, data.as_bytes())
            .map_err(|e| e.to_string())
    }

    pub async fn resize(
        &self,
        env: &str,
        session: &str,
        cols: u16,
        rows: u16,
    ) -> Result<(), String> {
        self.client(env)?
            .resize(session, cols, rows)
            .await
            .map_err(|e| e.to_string())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::envs::EnvKind;

    #[derive(Default)]
    pub struct Collect(pub Mutex<Vec<(String, Value)>>, pub Mutex<Vec<String>>);

    impl Sink for Collect {
        fn emit(&self, event: &str, payload: Value) {
            self.0.lock().unwrap().push((event.into(), payload));
        }
        fn notify(&self, title: &str, body: &str) {
            self.1.lock().unwrap().push(format!("{title} | {body}"));
        }
    }

    const SEGREDO: &str = "segredo-do-token";

    fn remote(id: &str, url: &str) -> EnvEntry {
        EnvEntry {
            id: id.into(),
            name: id.into(),
            url: url.into(),
            kind: EnvKind::Remote,
            distro: None,
            token: Some(SEGREDO.into()),
        }
    }

    /// Daemon falso que aceita e derruba toda conexão: o cliente falha na
    /// hora. Uma porta fechada qualquer (ex.: `127.0.0.1:9`) não serve: no
    /// WSL e no runner a conexão fica pendurada em vez de ser recusada.
    async fn dead_daemon() -> String {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((s, _)) = l.accept().await {
                drop(s);
            }
        });
        format!("ws://{addr}/ws")
    }

    fn bridge() -> (Arc<Bridge>, Arc<Collect>) {
        let sink = Arc::new(Collect::default());
        let resolve: Resolver =
            Arc::new(|e: &EnvEntry| e.token.clone().ok_or_else(|| "sem token".into()));
        let prefs = crate::envs::UiConfig::default().notify;
        (Bridge::new(sink.clone(), resolve, prefs), sink)
    }

    #[test]
    fn notifica_queda_so_depois_de_conectar_e_respeita_preferencias() {
        let (b, sink) = bridge();
        let st = |connected| ClientEvent::Status {
            connected,
            error: (!connected).then(|| "conexão perdida".to_string()),
        };
        b.handle("mac", st(false));
        assert!(
            sink.1.lock().unwrap().is_empty(),
            "falha inicial não notifica"
        );
        b.handle("mac", st(true));
        b.handle("mac", st(false));
        assert_eq!(
            *sink.1.lock().unwrap(),
            ["Ambiente desconectado | mac: conexão perdida. Reconectando…"]
        );
        let mut prefs = crate::envs::UiConfig::default().notify;
        prefs.needs_human = false;
        b.set_prefs(prefs);
        b.handle(
            "mac",
            ClientEvent::Push {
                event: "needs_human".into(),
                data: json!({ "message": "x" }),
            },
        );
        assert_eq!(sink.1.lock().unwrap().len(), 1);
        // Ambiente silenciado (#315): nem o desconectar notifica.
        let mut prefs = crate::envs::UiConfig::default().notify;
        prefs.muted_envs = vec!["mac".into()];
        b.set_prefs(prefs);
        b.handle("mac", st(true));
        b.handle("mac", st(false));
        assert_eq!(sink.1.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn payloads_nunca_levam_o_token() {
        let (b, sink) = bridge();
        // Daemon que derruba a conexão: o cliente falha e emite status com o erro.
        b.start(vec![remote("mac", &dead_daemon().await)]);
        for _ in 0..100 {
            if !b.statuses().is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        b.handle(
            "mac",
            ClientEvent::Push {
                event: "needs_human".into(),
                data: json!({ "session_id": "s" }),
            },
        );
        b.handle(
            "mac",
            ClientEvent::Detached {
                session: "s".into(),
                reason: "acabou".into(),
            },
        );
        let emitted = serde_json::to_string(&*sink.0.lock().unwrap()).unwrap();
        assert!(emitted.contains("daemon://status"), "{emitted}");
        assert!(!emitted.contains(SEGREDO), "{emitted}");
        assert!(!serde_json::to_string(&b.statuses())
            .unwrap()
            .contains(SEGREDO));
        let st = &b.statuses()[0];
        assert!(!st.connected && st.error.is_some());
    }

    #[test]
    fn saida_vai_para_o_terminal_certo_com_marcador_de_snapshot() {
        let (b, _) = bridge();
        let got = Arc::new(Mutex::new(Vec::new()));
        let g = got.clone();
        b.terms.lock().unwrap().insert(
            ("mac".into(), "s1".into()),
            Box::new(move |v| g.lock().unwrap().push(v)),
        );
        let out = |session: &str, snapshot| ClientEvent::Output {
            session: session.into(),
            bytes: b"oi".to_vec(),
            snapshot,
        };
        b.handle("mac", out("s1", true));
        b.handle("mac", out("s1", false));
        b.handle("mac", out("outra", false));
        b.handle("wsl", out("s1", false));
        assert_eq!(
            *got.lock().unwrap(),
            vec![b"\x01oi".to_vec(), b"\x00oi".to_vec()]
        );
    }

    #[test]
    fn detached_avisa_e_solta_o_terminal() {
        let (b, sink) = bridge();
        b.terms
            .lock()
            .unwrap()
            .insert(("mac".into(), "s1".into()), Box::new(|_| {}));
        b.handle(
            "mac",
            ClientEvent::Detached {
                session: "s1".into(),
                reason: "sessão não está rodando".into(),
            },
        );
        assert!(b.terms.lock().unwrap().is_empty());
        let (ev, p) = sink.0.lock().unwrap()[0].clone();
        assert_eq!(ev, PUSH_EVENT);
        assert_eq!(p["event"], "attach_lost");
        assert_eq!(p["data"]["session_id"], "s1");
    }

    #[tokio::test]
    async fn reconfigurar_so_reconecta_o_que_mudou() {
        let (b, _) = bridge();
        let url = dead_daemon().await;
        b.start(vec![remote("a", &url), remote("b", &url)]);
        let gen = |id: &str| b.conns.lock().unwrap().get(id).map(|c| c.gen);
        let (ga, gb) = (gen("a"), gen("b"));
        b.reconfigure(vec![remote("a", &url), remote("c", &url)]);
        assert_eq!(gen("a"), ga, "inalterado não reconecta");
        assert_eq!(gen("b"), None, "removido sai");
        assert!(gen("c").is_some() && gen("c") != gb);
        assert!(b.entry("c").is_some());
    }
}
