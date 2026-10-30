//! Sync com o GitHub ponta a ponta (spec §7.5): `ctowerd` real contra um
//! GitHub falso; o laço periódico leva o work item a `em_revisao` quando o
//! PR da branch abre e a `concluido` quando é mesclado.

use ct_gh::fake::{fake_gh, FakeGitHub};
use ct_gh::sync::{closed_since, issues_path, pulls_path};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::cell::Cell;
use std::os::unix::fs::PermissionsExt;
use std::process::Command;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;

mod common;

const T: Duration = Duration::from_secs(20);

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

struct Env {
    tmp: tempfile::TempDir,
    port: Cell<u16>,
}

impl Env {
    fn ws(&self, method: &str, params: Value) -> Value {
        let token = std::fs::read_to_string(self.tmp.path().join("config/ui.token")).unwrap();
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let mut req = format!("ws://127.0.0.1:{}/ws", self.port.get())
                .into_client_request()
                .unwrap();
            req.headers_mut().insert(
                "authorization",
                format!("Bearer {}", token.trim()).parse().unwrap(),
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

    /// Espera até `f(dashboard)` valer, consultando o daemon.
    fn wait_dashboard(&self, what: &str, f: impl Fn(&Value) -> bool) -> Value {
        let deadline = Instant::now() + T;
        loop {
            let d = self.ws("dashboard.get", json!({}));
            if f(&d) {
                return d;
            }
            assert!(Instant::now() < deadline, "{what}: {d}");
            std::thread::sleep(Duration::from_millis(200));
        }
    }
}

fn status_of(d: &Value, gh_number: i64) -> Option<String> {
    d["projects"][0]["work_items"]
        .as_array()?
        .iter()
        .find(|w| w["gh_number"] == gh_number)?["status"]
        .as_str()
        .map(String::from)
}

fn pr(state: &str, merged: bool) -> Value {
    json!([{
        "number": 30, "state": state,
        "merged_at": if merged { json!("2026-10-15T11:00:00Z") } else { Value::Null },
        "head": { "ref": "feat/1-sincronizar" }, "base": { "ref": "develop" },
        "html_url": "https://github.com/o/demo/pull/30",
        "updated_at": "2026-10-15T11:00:00Z",
    }])
}

#[test]
fn pr_aberto_e_mesclado_pelo_laco_periodico() {
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
    let fake_claude = p.join("claude");
    std::fs::write(&fake_claude, "#!/bin/sh\nexec sleep 300\n").unwrap();
    std::fs::set_permissions(&fake_claude, std::fs::Permissions::from_mode(0o755)).unwrap();

    let gh = FakeGitHub::start("tok");
    let gh_bin = fake_gh(p, "tok");
    gh.set(
        &issues_path("o", "demo", "open", None),
        json!([{
            "number": 1, "title": "Sincronizar", "state": "open", "labels": [],
            "html_url": "https://github.com/o/demo/issues/1",
            "updated_at": "2026-10-14T10:00:00Z",
            "sub_issues_summary": { "total": 0 },
        }]),
    );
    gh.set(
        &issues_path("o", "demo", "closed", Some(&closed_since(now_ms()))),
        json!([]),
    );
    let pulls = pulls_path("o", "demo", "feat/1-sincronizar");
    gh.set(&pulls, json!([]));

    let hub = common::hub_with_assignment(&p.join("config"), "o", "demo", &repo);
    std::fs::write(
        p.join("config/config.toml"),
        format!(
            "claude_bin = \"{}\"\nctower_bin = \"true\"\nworktrees_root = \"{}\"\n\
             pre_trust_worktrees = false\ngh_sync_interval_secs = 1\nupdate_check_interval_hours = 0\n\
             gh_api_url = \"{}\"\ngh_bin = \"{}\"\nhub_url = \"{}\"\n",
            fake_claude.display(),
            p.join("wt").display(),
            gh.url(),
            gh_bin.display(),
            hub.url
        ),
    )
    .unwrap();
    let port = Cell::new(common::free_port());
    let _daemon = common::start_ctowerd(&port, &p.join("config"), || {
        let mut c = Command::new(env!("CARGO_BIN_EXE_ctowerd"));
        c.env("CTOWER_CONFIG_DIR", p.join("config"))
            .env("CTOWER_DATA_DIR", p.join("data"))
            .env("RUST_LOG", "warn");
        c
    });
    let env = Env { tmp, port };

    // O projeto vem do hub (pull no boot).
    let project = common::wait_project(|m, p| env.ws(m, p), "demo");
    let pid = project["id"].as_i64().unwrap();
    // O laço espelha a issue sem ninguém pedir.
    let deadline = Instant::now() + T;
    loop {
        let t = env.ws("tree.get", json!({ "project_id": pid }));
        if t["roots"][0]["issue"]["gh_number"] == 1 {
            break;
        }
        assert!(Instant::now() < deadline, "issue não sincronizou: {t}");
        std::thread::sleep(Duration::from_millis(200));
    }

    // Título vem do cache; a branch sai dele.
    let s = env.ws(
        "session.start",
        json!({ "project_id": pid, "gh_number": 1, "prompt": "trabalhe" }),
    );
    let sid = s["id"].as_str().unwrap().to_string();
    env.wait_dashboard("em execução", |d| {
        status_of(d, 1).as_deref() == Some("em_execucao")
    });

    gh.set(&pulls, pr("open", false));
    env.wait_dashboard("PR aberto", |d| {
        status_of(d, 1).as_deref() == Some("em_revisao")
    });
    gh.set(&pulls, pr("closed", true));
    let d = env.wait_dashboard("PR mesclado", |d| {
        status_of(d, 1).as_deref() == Some("concluido")
    });
    assert!(d["sync"][0]["last_ok_at"].is_i64(), "{d}");
    assert!(d["sync"][0]["last_error"].is_null(), "{d}");

    let t = env.ws("tree.get", json!({ "project_id": pid }));
    assert_eq!(t["roots"][0]["work_item"]["status"], "concluido");
    let r = env.ws(
        "review.get",
        json!({ "work_item_id": t["roots"][0]["work_item"]["id"] }),
    );
    assert_eq!(r["prs"][0]["state"], "merged");
    env.ws("session.kill", json!({ "session_id": sid }));
}
