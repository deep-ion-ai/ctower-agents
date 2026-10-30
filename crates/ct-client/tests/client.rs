//! `ct-client` contra o `ct-server` real. Um proxy TCP entre os dois simula a
//! queda do daemon (corta as conexões e para de aceitar) e a volta.

use ct_client::{Backoff, Client, ClientError, ClientEvent, ClientOptions, EnvConfig};
use ct_server::hub_sync::{HubSender, HubTarget};
use ct_server::{notifier, router, AppState};
use ct_sessions::test_support::{FakeClaude, TestEnv};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{broadcast, mpsc};
use tokio::task::AbortHandle;

const T: Duration = Duration::from_secs(10);
const UI: &str = "ui-secreto";

struct Proxy {
    port: u16,
    target: SocketAddr,
    tasks: Arc<Mutex<Vec<AbortHandle>>>,
}

impl Proxy {
    async fn start(target: SocketAddr) -> Self {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        let p = Proxy {
            port,
            target,
            tasks: Arc::default(),
        };
        p.serve(l);
        p
    }

    fn serve(&self, l: TcpListener) {
        let (target, tasks) = (self.target, self.tasks.clone());
        let t2 = tasks.clone();
        let h = tokio::spawn(async move {
            while let Ok((mut a, _)) = l.accept().await {
                let h = tokio::spawn(async move {
                    if let Ok(mut b) = TcpStream::connect(target).await {
                        let _ = tokio::io::copy_bidirectional(&mut a, &mut b).await;
                    }
                });
                t2.lock().unwrap().push(h.abort_handle());
            }
        });
        tasks.lock().unwrap().push(h.abort_handle());
    }

    /// Derruba as conexões e para de aceitar.
    async fn cut(&self) {
        for h in self.tasks.lock().unwrap().drain(..) {
            h.abort();
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    /// Volta a aceitar na mesma porta.
    async fn restore(&self) {
        for _ in 0..100 {
            if let Ok(l) = TcpListener::bind(("127.0.0.1", self.port)).await {
                self.serve(l);
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("não religou a porta {}", self.port);
    }

    fn url(&self) -> String {
        format!("ws://127.0.0.1:{}/ws", self.port)
    }
}

struct Daemon {
    env: TestEnv,
    proxy: Proxy,
}

impl Daemon {
    async fn start(fake: FakeClaude) -> Self {
        let (events, _) = broadcast::channel(256);
        let env = TestEnv::with_notifier(fake, notifier(events.clone()));
        let state = AppState {
            sessions: env.sessions.clone(),
            ui_token: UI.into(),
            hook_token: "hook".into(),
            events,
            github: None,
            tracker: None,
            env_name: "teste".into(),
            // Ligado a um hub (que o teste nunca chama): sem hub o ambiente
            // não inicia sessões.
            hub_sender: Some(Arc::new(HubSender::new(
                env.sessions.clone(),
                HubTarget {
                    url: "http://127.0.0.1:9".into(),
                    token: "sync".into(),
                },
                Duration::from_secs(1),
            ))),
            updates: None,
            cleanup: None,
            env_config: None,
            permissions: Default::default(),
        };
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(l, router(state)).await.unwrap() });
        let proxy = Proxy::start(addr).await;
        Daemon { env, proxy }
    }

    fn client(&self, token: &str) -> (Client, mpsc::UnboundedReceiver<ClientEvent>) {
        spawn(&self.proxy.url(), token)
    }
}

fn spawn(url: &str, token: &str) -> (Client, mpsc::UnboundedReceiver<ClientEvent>) {
    Client::spawn_with(
        EnvConfig {
            name: "teste".into(),
            url: url.into(),
            token: token.into(),
        },
        ClientOptions {
            backoff: Backoff {
                min: Duration::from_millis(50),
                max: Duration::from_millis(400),
            },
            call_timeout: T,
            ..ClientOptions::default()
        },
    )
}

/// Próximo evento que satisfaz `pred` (os demais são descartados).
async fn wait_for(
    rx: &mut mpsc::UnboundedReceiver<ClientEvent>,
    mut pred: impl FnMut(&ClientEvent) -> bool,
) -> ClientEvent {
    tokio::time::timeout(T, async {
        loop {
            let e = rx.recv().await.expect("canal de eventos fechou");
            if pred(&e) {
                return e;
            }
        }
    })
    .await
    .expect("prazo esgotado esperando evento")
}

async fn wait_status(rx: &mut mpsc::UnboundedReceiver<ClientEvent>, want: bool) -> ClientEvent {
    wait_for(
        rx,
        |e| matches!(e, ClientEvent::Status { connected, .. } if *connected == want),
    )
    .await
}

/// Acumula a saída da sessão até conter `needle`; devolve se algum trecho
/// veio marcado como snapshot.
async fn wait_output(
    rx: &mut mpsc::UnboundedReceiver<ClientEvent>,
    sid: &str,
    needle: &str,
) -> (String, bool) {
    let mut acc = Vec::new();
    let mut saw_snapshot = false;
    while !String::from_utf8_lossy(&acc).contains(needle) {
        if let ClientEvent::Output {
            bytes, snapshot, ..
        } = wait_for(
            rx,
            |e| matches!(e, ClientEvent::Output { session, .. } if session == sid),
        )
        .await
        {
            if snapshot {
                acc.clear();
                saw_snapshot = true;
            }
            acc.extend(bytes);
        }
    }
    (String::from_utf8_lossy(&acc).into_owned(), saw_snapshot)
}

fn eco() -> FakeClaude {
    FakeClaude::script("echo pronto; while read -r l; do echo \"eco:$l\"; done")
}

async fn start_session(d: &Daemon, c: &Client) -> String {
    let v = c
        .call(
            "session.start",
            json!({ "project_id": d.env.project_id, "gh_number": 1, "title": "A", "prompt": "oi" }),
        )
        .await
        .unwrap();
    v["id"].as_str().unwrap().to_string()
}

#[tokio::test(flavor = "multi_thread")]
async fn token_errado_nao_entra_em_laco_apertado() {
    let d = Daemon::start(FakeClaude::exit(0)).await;
    let (c, mut rx) = d.client("errado");
    let e = wait_status(&mut rx, false).await;
    let ClientEvent::Status { error, .. } = e else {
        unreachable!()
    };
    assert!(error.unwrap().contains("401"));
    tokio::time::sleep(Duration::from_millis(1000)).await;
    // Backoff de 50 ms dobrando até 400 ms: poucas tentativas por segundo.
    let n = c.attempts();
    assert!((2..=7).contains(&n), "{n} tentativas");
    // O mesmo erro não é reemitido a cada tentativa.
    while let Ok(e) = rx.try_recv() {
        assert!(!matches!(e, ClientEvent::Status { .. }), "{e:?}");
    }
    assert_eq!(
        c.call("env.info", json!({})).await,
        Err(ClientError::Disconnected)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn chamada_com_resposta_erro_e_push() {
    let d = Daemon::start(FakeClaude::sleep(30)).await;
    let (c, mut rx) = d.client(UI);
    wait_status(&mut rx, true).await;
    let v = c.call("env.info", json!({})).await.unwrap();
    assert_eq!(v["protocol"], ct_server::PROTOCOL_VERSION);
    match c.call("nada", json!({})).await {
        Err(ClientError::Rpc { code, .. }) => assert_eq!(code, -32601),
        r => panic!("{r:?}"),
    }
    let sid = start_session(&d, &c).await;
    let e = wait_for(
        &mut rx,
        |e| matches!(e, ClientEvent::Push { event, .. } if event == "session_started"),
    )
    .await;
    let ClientEvent::Push { data, .. } = e else {
        unreachable!()
    };
    assert_eq!(data["id"], Value::String(sid.clone()));
    c.call("session.kill", json!({ "session_id": sid }))
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn attach_com_snapshot_e_entrada() {
    let d = Daemon::start(eco()).await;
    let (c, mut rx) = d.client(UI);
    wait_status(&mut rx, true).await;
    let sid = start_session(&d, &c).await;
    c.attach(&sid).await.unwrap();
    let (_, snap) = wait_output(&mut rx, &sid, "pronto").await;
    assert!(snap, "o primeiro trecho do attach é o snapshot");
    c.input(&sid, b"um\n").unwrap();
    let (out, snap) = wait_output(&mut rx, &sid, "eco:um").await;
    assert!(!snap && out.contains("eco:um"));
    c.resize(&sid, 120, 40).await.unwrap();
    c.detach(&sid).await.unwrap();
    c.call("session.kill", json!({ "session_id": sid }))
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn queda_e_volta_refaz_o_attach_sem_duplicar() {
    let d = Daemon::start(eco()).await;
    let (c, mut rx) = d.client(UI);
    wait_status(&mut rx, true).await;
    let sid = start_session(&d, &c).await;
    c.attach(&sid).await.unwrap();
    c.input(&sid, b"um\n").unwrap();
    wait_output(&mut rx, &sid, "eco:um").await;

    d.proxy.cut().await;
    wait_status(&mut rx, false).await;
    assert_eq!(
        c.call("env.info", json!({})).await,
        Err(ClientError::Disconnected)
    );
    assert_eq!(c.input(&sid, b"x\n"), Err(ClientError::Disconnected));

    d.proxy.restore().await;
    wait_status(&mut rx, true).await;
    // Re-attach automático: snapshot novo com o histórico, uma vez só.
    let (out, snap) = wait_output(&mut rx, &sid, "eco:um").await;
    assert!(snap, "reconexão entrega snapshot marcado");
    assert_eq!(out.matches("eco:um").count(), 1, "{out:?}");
    c.input(&sid, b"dois\n").unwrap();
    wait_output(&mut rx, &sid, "eco:dois").await;
    c.call("session.kill", json!({ "session_id": sid }))
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn sessao_encerrada_durante_a_queda_vira_detached() {
    let d = Daemon::start(eco()).await;
    let (c, mut rx) = d.client(UI);
    wait_status(&mut rx, true).await;
    let sid = start_session(&d, &c).await;
    c.attach(&sid).await.unwrap();
    wait_output(&mut rx, &sid, "pronto").await;

    d.proxy.cut().await;
    wait_status(&mut rx, false).await;
    d.env.sessions.kill(&sid).unwrap();
    d.env.wait_state(&sid, ct_core::SessionState::Exited);
    d.proxy.restore().await;
    let e = wait_for(&mut rx, |e| matches!(e, ClientEvent::Detached { .. })).await;
    let ClientEvent::Detached { session, .. } = e else {
        unreachable!()
    };
    assert_eq!(session, sid);
}

#[tokio::test(flavor = "multi_thread")]
async fn daemon_com_protocolo_incompativel() {
    // Daemon falso que responde env.info com protocolo 2.0.
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((s, _)) = l.accept().await {
            tokio::spawn(async move {
                let mut ws = tokio_tungstenite::accept_async(s).await.unwrap();
                while let Some(Ok(m)) = ws.next().await {
                    if let Ok(t) = m.to_text() {
                        let v: Value = serde_json::from_str(t).unwrap_or_default();
                        let r = match v["method"].as_str() {
                            Some("env.update") => {
                                json!({ "id": v["id"], "result": { "ok": v["params"]["force"] } })
                            }
                            _ => json!({ "id": v["id"], "result": { "protocol": "3.0" } }),
                        };
                        let _ = ws
                            .send(tokio_tungstenite::tungstenite::Message::Text(
                                r.to_string().into(),
                            ))
                            .await;
                    }
                }
            });
        }
    });
    let (c, mut rx) = spawn(&format!("ws://{addr}/ws"), UI);
    let e = wait_status(&mut rx, false).await;
    let ClientEvent::Status { error, .. } = e else {
        unreachable!()
    };
    assert!(error.unwrap().contains("incompatível"));
    assert_eq!(
        c.call("dashboard.get", json!({})).await,
        Err(ClientError::Incompatible {
            daemon: "3.0".into()
        })
    );
    // Os métodos estáveis seguem funcionando, por conexão avulsa.
    assert_eq!(
        c.call("env.info", json!({})).await.unwrap()["protocol"],
        "3.0"
    );
    assert_eq!(
        c.call("env.update", json!({ "force": true })).await,
        Ok(json!({ "ok": true }))
    );
    assert!(!c.is_connected());
}

/// O mesmo cliente fala com o `/ws` do hub (`cthubd`), com o handshake por
/// `hub.info` e o protocolo próprio dele (1.x).
#[tokio::test]
async fn conecta_ao_hub_com_o_handshake_do_hub() {
    use ct_client::Handshake;
    use ct_ledger::{Hub, SharedHub};
    let hub = SharedHub::new(Hub::open_in_memory().unwrap());
    let state = cthubd::HubState::new(hub, UI, "mac-mini", "hub-id");
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(l, cthubd::router(state)).await.unwrap() });
    let url = format!("ws://{addr}/ws");
    let cfg = || EnvConfig {
        name: "hub".into(),
        url: url.clone(),
        token: UI.into(),
    };
    let opts = |handshake| ClientOptions {
        backoff: Backoff {
            min: Duration::from_millis(50),
            max: Duration::from_millis(400),
        },
        call_timeout: T,
        handshake,
    };

    let (c, mut rx) = Client::spawn_with(cfg(), opts(Handshake::HUB));
    wait_status(&mut rx, true).await;
    let info = c.call("hub.info", json!({})).await.unwrap();
    assert_eq!(info["protocol"], cthubd::PROTOCOL_VERSION);
    assert_eq!(info["name"], "mac-mini");
    c.shutdown();

    // Com o handshake do ambiente, o hub não serve `env.info`: incompatível.
    let (c, mut rx) = Client::spawn_with(cfg(), opts(Handshake::ENV));
    let ev = wait_for(&mut rx, |e| {
        matches!(e, ClientEvent::Status { error: Some(_), .. })
    })
    .await;
    assert!(
        matches!(&ev, ClientEvent::Status { error: Some(e), .. } if e.contains("incompatível")),
        "{ev:?}"
    );
    c.shutdown();
}
