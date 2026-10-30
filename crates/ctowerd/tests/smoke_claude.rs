//! Smoke com o Claude Code real (spec §16): confere hooks, `--session-id`,
//! transcript e backup na versão instalada. Usa a conta logada na distro.
//!
//! Rodar (dentro da distro, com `claude` logado):
//!   cargo build -p ctower && cargo test -p ctowerd --test smoke_claude -- --ignored --nocapture

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;

mod common;

const PROMPT: &str = "Responda apenas com a palavra OK e nada mais.";

fn which(bin: &str) -> PathBuf {
    let out = Command::new("bash")
        .args(["-lc", &format!("command -v {bin}")])
        .output()
        .unwrap();
    let p = String::from_utf8_lossy(&out.stdout).trim().to_string();
    assert!(!p.is_empty(), "{bin} não encontrado no PATH");
    PathBuf::from(p)
}

fn git(repo: &std::path::Path, args: &[&str]) {
    assert!(Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .status()
        .unwrap()
        .success());
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "usa o Claude Code real e a conta logada"]
async fn smoke_com_claude_real() {
    let tmp = tempfile::tempdir().unwrap();
    let p = tmp.path();
    let repo = p.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "develop"]);
    git(&repo, &["config", "user.email", "t@t"]);
    git(&repo, &["config", "user.name", "t"]);
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "init"]);

    let ctower = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/ctower");
    assert!(ctower.is_file(), "rode `cargo build -p ctower` antes");
    // Sem hub o ambiente não tem projetos: um hub de verdade atribui o smoke.
    let hub = common::hub_with_assignment(&p.join("config"), "o", "smoke", &repo);
    std::fs::write(
        p.join("config/config.toml"),
        format!(
            "claude_bin = \"{}\"\nctower_bin = \"{}\"\nworktrees_root = \"{}\"\ngh_sync_interval_secs = 0\nupdate_check_interval_hours = 0\nhub_url = \"{}\"\n",
            which("claude").display(),
            ctower.canonicalize().unwrap().display(),
            p.join("wt").display(),
            hub.url
        ),
    )
    .unwrap();
    let port = std::cell::Cell::new(common::free_port());
    let _daemon = common::start_ctowerd(&port, &p.join("config"), || {
        let mut c = Command::new(env!("CARGO_BIN_EXE_ctowerd"));
        c.env("CTOWER_CONFIG_DIR", p.join("config"))
            .env("CTOWER_DATA_DIR", p.join("data"))
            .env("RUST_LOG", "info");
        c
    });
    let port = port.get();
    let token = std::fs::read_to_string(p.join("config/ui.token")).unwrap();

    let mut req = format!("ws://127.0.0.1:{port}/ws")
        .into_client_request()
        .unwrap();
    req.headers_mut().insert(
        "authorization",
        format!("Bearer {}", token.trim()).parse().unwrap(),
    );
    let (mut ws, _) = tokio_tungstenite::connect_async(req).await.unwrap();
    let mut next_id = 0;
    let mut term = Vec::<u8>::new();
    let mut pushes = Vec::<Value>::new();

    macro_rules! call {
        ($m:expr, $p:expr) => {{
            next_id += 1;
            let id = next_id;
            let body = json!({ "id": id, "method": $m, "params": $p });
            ws.send(Message::Text(body.to_string().into())).await.unwrap();
            loop {
                match ws.next().await.unwrap().unwrap() {
                    Message::Text(t) => {
                        let v: Value = serde_json::from_str(&t).unwrap();
                        if v["id"] == id {
                            break v;
                        }
                        pushes.push(v);
                    }
                    Message::Binary(b) => term.extend_from_slice(&b[17..]),
                    _ => {}
                }
            }
        }};
    }

    // O projeto vem do hub (pull no boot); a chave natural basta.
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let list = call!("projects.list", json!({}));
        if list["result"][0]["gh_repo"] == "smoke" {
            break;
        }
        assert!(Instant::now() < deadline, "smoke não chegou do hub: {list}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let s = call!(
        "session.start",
        json!({ "owner": "o", "repo": "smoke", "gh_number": 1, "title": "Smoke",
                "prompt": PROMPT })
    );
    let s = s["result"].clone();
    let sid = s["id"].as_str().expect("session.start falhou").to_string();
    let our_uuid = s["tool_session_id"].as_str().unwrap().to_string();
    call!("session.attach", json!({ "session_id": sid }));

    // Observa até a sessão ficar idle (hook Stop), sem tocar no terminal: o
    // daemon pré-aceita a confiança na pasta (issue #16), então o diálogo
    // "Is this a project you trust?" não pode aparecer.
    let mut trust_dialog = false;
    let deadline = Instant::now() + Duration::from_secs(180);
    let mut idle = false;
    while Instant::now() < deadline && !idle {
        let msg = tokio::time::timeout(Duration::from_secs(2), ws.next()).await;
        if let Ok(Some(Ok(m))) = msg {
            match m {
                Message::Binary(b) => term.extend_from_slice(&b[17..]),
                Message::Text(t) => {
                    let v: Value = serde_json::from_str(&t).unwrap();
                    if v["event"] == "session_state" && v["data"]["state"] == "idle" {
                        idle = true;
                    }
                    pushes.push(v);
                }
                _ => {}
            }
        }
        let screen: String = strip_ansi(&String::from_utf8_lossy(&term))
            .to_lowercase()
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        if screen.contains("itrustthisfolder") {
            trust_dialog = true;
            break;
        }
    }
    std::fs::write(p.join("terminal.log"), &term).unwrap();
    let screen = strip_ansi(&String::from_utf8_lossy(&term));
    println!("--- terminal (fim) ---\n{}", tail(&screen, 40));
    println!("--- pushes ---");
    for v in &pushes {
        println!("{v}");
    }

    // Diagnóstico: eventos do work item direto do ledger.
    let db = p.join("data/ledger.db");
    let evs = Command::new("python3")
        .args([
            "-c",
            "import sqlite3,sys;c=sqlite3.connect(sys.argv[1]);\
             [print(r) for r in c.execute('select ts,type,payload from event order by id')]",
            db.to_str().unwrap(),
        ])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default();
    println!("--- eventos ---\n{evs}");

    // O push `idle` sai antes do backup do Stop terminar: espera o backup_path.
    let mut dash;
    let backup_deadline = Instant::now() + Duration::from_secs(10);
    loop {
        dash = call!("dashboard.get", json!({}));
        let has_backup = dash["result"]["live_sessions"]
            .as_array()
            .is_some_and(|a| a.iter().any(|x| x["backup_path"].is_string()));
        if has_backup || Instant::now() > backup_deadline {
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    let live = &dash["result"]["live_sessions"];
    println!(
        "--- sessão ---\n{}",
        serde_json::to_string_pretty(live).unwrap()
    );
    call!("session.kill", json!({ "session_id": sid }));

    assert!(
        !trust_dialog,
        "diálogo de confiança apareceu apesar do pré-aceite"
    );
    assert!(idle, "sessão não chegou a idle (hook Stop) em 180 s");
    let sess = live
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["id"] == sid.as_str())
        .unwrap();
    assert_eq!(
        sess["tool_session_id"],
        our_uuid.as_str(),
        "SessionStart confirmou o --session-id"
    );
    let transcript = sess["transcript_path"]
        .as_str()
        .expect("transcript_path do SessionStart");
    assert!(std::path::Path::new(transcript).is_file(), "{transcript}");
    let backup = sess["backup_path"].as_str().expect("backup no Stop");
    assert!(std::path::Path::new(backup).is_file(), "{backup}");
}

fn strip_ansi(s: &str) -> String {
    let mut out = String::new();
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c == '\x1b' {
            if it.peek() == Some(&'[') {
                it.next();
                for c in it.by_ref() {
                    if c.is_ascii_alphabetic() || c == '~' {
                        break;
                    }
                }
            } else {
                it.next();
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn tail(s: &str, n: usize) -> String {
    let lines: Vec<&str> = s.lines().filter(|l| !l.trim().is_empty()).collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}
