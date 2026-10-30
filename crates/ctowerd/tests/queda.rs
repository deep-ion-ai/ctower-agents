//! Cenário de queda (spec §14): daemon e sessão morrem juntos; no boot a
//! sessão vira `interrupted` e pode ser retomada com `claude --resume`.

use common::Daemon;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::cell::Cell;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;

mod common;

const T: Duration = Duration::from_secs(15);

struct Env {
    tmp: tempfile::TempDir,
    /// Porta do daemon; troca se outro teste a pegou antes (ver
    /// `common::start_ctowerd`).
    port: Cell<u16>,
    _hub: common::TestHub,
}

impl Env {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path();
        let repo = p.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        for args in [
            &["init", "-q", "-b", "develop"][..],
            &["config", "user.email", "t@t"],
            &["config", "user.name", "t"],
            &["commit", "-q", "--allow-empty", "-m", "init"],
        ] {
            assert!(Command::new("git")
                .arg("-C")
                .arg(&repo)
                .args(args)
                .status()
                .unwrap()
                .success());
        }
        std::fs::create_dir_all(p.join("out")).unwrap();
        let fake = p.join("claude");
        std::fs::write(
            &fake,
            format!(
                "#!/bin/bash\nprintf '%s\\n' \"$@\" > \"{}/out/$CTOWER_SESSION.txt\"\nexec sleep 300\n",
                p.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        let hub = common::hub_with_assignment(&p.join("config"), "o", "demo", &repo);
        std::fs::write(
            p.join("config/config.toml"),
            format!(
                "claude_bin = \"{}\"\nctower_bin = \"true\"\nworktrees_root = \"{}\"\ngh_sync_interval_secs = 0\nupdate_check_interval_hours = 0\nhub_url = \"{}\"\n",
                fake.display(),
                p.join("wt").display(),
                hub.url
            ),
        )
        .unwrap();
        Env {
            tmp,
            port: Cell::new(common::free_port()),
            _hub: hub,
        }
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.tmp.path().join(rel)
    }

    fn start_daemon(&self) -> Daemon {
        common::start_ctowerd(&self.port, &self.path("config"), || {
            let mut c = Command::new(env!("CARGO_BIN_EXE_ctowerd"));
            c.env("CTOWER_CONFIG_DIR", self.path("config"))
                .env("CTOWER_DATA_DIR", self.path("data"))
                .env("RUST_LOG", "warn");
            c
        })
    }

    fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{path}", self.port.get())
    }

    fn token(&self, name: &str) -> String {
        std::fs::read_to_string(self.path(&format!("config/{name}.token")))
            .unwrap()
            .trim()
            .to_string()
    }

    fn hook(&self, session: &str, event: &str, payload: Value) {
        let r = ureq::post(&self.url("/hooks"))
            .set("Authorization", &format!("Bearer {}", self.token("hook")))
            .send_json(json!({ "session": session, "event": event, "payload": payload, "ts": 0 }))
            .unwrap();
        assert_eq!(r.status(), 202);
    }

    fn status(&self) -> Value {
        ureq::get(&self.url("/status"))
            .set("Authorization", &format!("Bearer {}", self.token("ui")))
            .call()
            .unwrap()
            .into_json()
            .unwrap()
    }

    /// Uma chamada WS (conexão própria); ignora pushes e frames de terminal.
    fn ws(&self, method: &str, params: Value) -> Value {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let mut req = format!("ws://127.0.0.1:{}/ws", self.port.get())
                .into_client_request()
                .unwrap();
            req.headers_mut().insert(
                "authorization",
                format!("Bearer {}", self.token("ui")).parse().unwrap(),
            );
            let (mut ws, _) = tokio_tungstenite::connect_async(req).await.unwrap();
            let body = json!({ "id": 1, "method": method, "params": params });
            ws.send(Message::Text(body.to_string().into()))
                .await
                .unwrap();
            loop {
                let m = tokio::time::timeout(T, ws.next())
                    .await
                    .unwrap()
                    .unwrap()
                    .unwrap();
                if let Message::Text(t) = m {
                    let v: Value = serde_json::from_str(&t).unwrap();
                    if v["id"] == 1 {
                        assert!(v.get("error").is_none(), "{method}: {v}");
                        return v["result"].clone();
                    }
                }
            }
        })
    }

    fn wait_file(&self, p: &Path) -> String {
        let deadline = Instant::now() + T;
        loop {
            if let Ok(s) = std::fs::read_to_string(p) {
                if !s.is_empty() {
                    return s;
                }
            }
            assert!(Instant::now() < deadline, "{p:?} não apareceu");
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

fn sigkill(pid: i64) {
    // SAFETY: envio de sinal a um pid conhecido do teste.
    unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) };
}

#[test]
fn queda_do_daemon_e_retomada() {
    let env = Env::new();
    let mut daemon = env.start_daemon();

    // O projeto vem do hub (pull no boot).
    let project = common::wait_project(|m, p| env.ws(m, p), "demo");
    let s = env.ws(
        "session.start",
        json!({ "project_id": project["id"], "gh_number": 42, "title": "Queda",
                "prompt": "trabalhe" }),
    );
    let sid = s["id"].as_str().unwrap().to_string();
    let tool_id = s["tool_session_id"].as_str().unwrap().to_string();
    let child_pid = s["pid"].as_i64().unwrap();
    let args = env.wait_file(&env.path(&format!("out/{sid}.txt")));
    assert!(
        args.starts_with(&format!("--session-id\n{tool_id}\n")),
        "{args}"
    );

    let transcript = env.path("claude-projects/t.jsonl");
    std::fs::create_dir_all(transcript.parent().unwrap()).unwrap();
    std::fs::write(&transcript, "{\"conversa\":1}\n").unwrap();
    env.hook(
        &sid,
        "SessionStart",
        json!({ "session_id": tool_id, "transcript_path": transcript }),
    );
    env.hook(&sid, "Stop", json!({}));
    assert_eq!(env.status()["live_sessions"][0]["state"], "idle");

    // Queda: daemon e sessão morrem; o transcript original some junto.
    sigkill(i64::from(daemon.0.id()));
    let _ = daemon.0.wait();
    sigkill(child_pid);
    std::fs::remove_dir_all(transcript.parent().unwrap()).unwrap();

    let _daemon = env.start_daemon();
    let st = env.status();
    assert_eq!(st["interrupted"], 1, "{st}");
    assert_eq!(st["live_sessions"], json!([]));

    let rec = env.ws("recovery.list", json!({}));
    assert_eq!(rec[0]["session"]["id"], sid.as_str());
    assert_eq!(rec[0]["transcript"], "restauravel_do_backup");

    let new = env.ws("session.resume", json!({ "session_id": sid }));
    let new_id = new["id"].as_str().unwrap();
    let args = env.wait_file(&env.path(&format!("out/{new_id}.txt")));
    assert!(
        args.starts_with(&format!("--resume\n{tool_id}\n")),
        "{args}"
    );
    assert_eq!(
        std::fs::read_to_string(&transcript).unwrap(),
        "{\"conversa\":1}\n",
        "transcript restaurado do backup"
    );
    assert_eq!(env.status()["interrupted"], 0);
    env.ws("session.kill", json!({ "session_id": new_id }));
}
