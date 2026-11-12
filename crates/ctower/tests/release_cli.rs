//! `ctower release` (#204) com um `ctowerd` falso e o GitHub falso
//! (`acme/acme-web`).

use assert_cmd::Command;
use ct_gh::fake::FakeGitHub;
use predicates::str::contains;
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};

const R: &str = "/repos/acme/acme-web";
const SESSION_TOKEN: &str = "tok-da-sessao";
const RELEASE_TOKEN: &str = "tok-release-exclusivo";

/// `ctowerd` falso: contexto, secret e eventos. `release = false` responde
/// `403` no contexto, como para uma sessão que não é de release.
struct Daemon {
    url: String,
    events: Arc<Mutex<Vec<Value>>>,
}

fn daemon(api_url: String, release: bool) -> Daemon {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    let events = Arc::new(Mutex::new(Vec::new()));
    let ev = Arc::clone(&events);
    std::thread::spawn(move || {
        for c in l.incoming().flatten() {
            let (path, auth, body) = read_req(&c);
            let (status, out) = if auth != format!("Bearer {SESSION_TOKEN}") {
                ("401 X", "token".to_string())
            } else if path == "/session/release/context" {
                if release {
                    (
                        "200 OK",
                        json!({ "session_id": "s-rel", "owner": "acme", "repo": "acme-web",
                                "release": { "enabled": true }, "api_url": api_url })
                        .to_string(),
                    )
                } else {
                    (
                        "403 X",
                        "`ctower release` só roda na sessão de release do projeto".into(),
                    )
                }
            } else if path == "/session/secret" {
                let v: Value = serde_json::from_str(&body).unwrap();
                assert_eq!(v["name"], "CT_RELEASE_GH_TOKEN");
                ("200 OK", RELEASE_TOKEN.to_string())
            } else if path == "/session/release/event" {
                ev.lock()
                    .unwrap()
                    .push(serde_json::from_str(&body).unwrap());
                ("200 OK", "{\"id\":1}".to_string())
            } else {
                ("404 X", String::new())
            };
            let mut c = c;
            let _ = c.write_all(
                format!(
                    "HTTP/1.1 {status}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{out}",
                    out.len()
                )
                .as_bytes(),
            );
        }
    });
    Daemon { url, events }
}

fn read_req(mut c: &std::net::TcpStream) -> (String, String, String) {
    let mut req = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        let n = c.read(&mut buf).unwrap();
        req.extend_from_slice(&buf[..n]);
        let text = String::from_utf8_lossy(&req).to_string();
        if let Some(end) = text.find("\r\n\r\n") {
            let len = text
                .lines()
                .find_map(|l| {
                    l.to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .map(|v| v.trim().parse::<usize>().unwrap())
                })
                .unwrap_or(0);
            if req.len() >= end + 4 + len {
                let path = text.split_whitespace().nth(1).unwrap_or("").to_string();
                let auth = text
                    .lines()
                    .find_map(|l| {
                        l.split_once(':')
                            .filter(|(k, _)| k.eq_ignore_ascii_case("authorization"))
                            .map(|(_, v)| v.trim().to_string())
                    })
                    .unwrap_or_default();
                return (path, auth, text[end + 4..].to_string());
            }
        }
        if n == 0 {
            return (String::new(), String::new(), String::new());
        }
    }
}

fn ctower(d: &Daemon) -> Command {
    let mut c = Command::cargo_bin("ctower").unwrap();
    for (k, _) in std::env::vars() {
        if k.starts_with("CTOWER_") || k == "GH_TOKEN" {
            c.env_remove(k);
        }
    }
    c.env("CTOWER_URL", &d.url)
        .env("CTOWER_SESSION_TOKEN", SESSION_TOKEN)
        .env("CTOWER_RELEASE_POLL_MS", "5")
        .env("CTOWER_RELEASE_MINUTE_MS", "20");
    c
}

/// GitHub falso com develop e main de mesmo conteúdo e nada aberto.
fn github() -> FakeGitHub {
    let gh = FakeGitHub::start(RELEASE_TOKEN);
    let branch = |name: &str, sha: &str, tree: &str| {
        gh.set(
            &format!("{R}/branches/{name}"),
            json!({ "commit": { "sha": sha, "commit": { "tree": { "sha": tree } } } }),
        );
    };
    branch("develop", "d1", "tree-a");
    branch("main", "m1", "tree-a");
    gh.set(
        &format!("{R}/compare/develop...main"),
        json!({ "ahead_by": 0, "behind_by": 1 }),
    );
    for (head, base) in [
        ("develop", "main"),
        ("release-please--branches--main", "main"),
        ("main", "develop"),
    ] {
        gh.set(
            &ct_gh::sync::open_pulls_path("acme", "acme-web", head, base),
            json!([]),
        );
    }
    gh.set(&format!("{R}/releases?per_page=20"), json!([]));
    gh.set(&format!("{R}/rules/branches/main"), json!([]));
    gh
}

#[test]
fn fora_da_sessao_de_release_recusa() {
    let gh = github();
    let d = daemon(gh.url(), false);
    ctower(&d)
        .args(["release", "status"])
        .assert()
        .code(1)
        .stderr(contains("só roda na sessão de release"));
    // Sem token de sessão nenhum.
    let mut c = ctower(&d);
    c.env_remove("CTOWER_SESSION_TOKEN");
    c.args(["release", "promote"])
        .assert()
        .code(1)
        .stderr(contains("CTOWER_SESSION_TOKEN"));
    assert!(gh.calls().is_empty());
}

#[test]
fn sem_diferenca_status_e_promote_relatam_nada_a_lancar() {
    let gh = github();
    let d = daemon(gh.url(), true);
    let out = ctower(&d)
        .args(["release", "status"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let st: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(st["content_diff"], false);
    assert_eq!(st["next"], "nothing");

    std::fs::write(std::env::temp_dir().join("ct-release-corpo.md"), "Corpo").unwrap();
    ctower(&d)
        .args(["release", "promote", "--body-file"])
        .arg(std::env::temp_dir().join("ct-release-corpo.md"))
        .assert()
        .code(3)
        .stderr(contains("nada a lançar"));
    assert!(gh.posted().is_empty(), "nenhum PR aberto");
    let evs = d.events.lock().unwrap().clone();
    assert_eq!(evs.len(), 2);
    assert_eq!(evs[0]["step"], "status");
    assert_eq!(evs[1]["step"], "promote");
    assert_eq!(evs[1]["result"], "refused");
}

#[test]
fn merge_promotion_com_merge_commit_e_evento_no_ledger() {
    let gh = github();
    let pr = json!({
        "number": 10, "title": "release: develop -> main", "state": "open",
        "merged": false, "mergeable": true, "mergeable_state": "clean",
        "html_url": "https://github.com/acme/acme-web/pull/10",
        "head": { "ref": "develop", "sha": "d2d2d2d2" }, "base": { "ref": "main" },
    });
    gh.set(
        &ct_gh::sync::open_pulls_path("acme", "acme-web", "develop", "main"),
        json!([pr.clone()]),
    );
    gh.set(&format!("{R}/pulls/10"), pr);
    gh.set(
        &format!("{R}/pulls/10/merge"),
        json!({ "merged": true, "sha": "mc1" }),
    );
    let d = daemon(gh.url(), true);
    // Head mudou desde a conferência: recusa, nada mesclado.
    ctower(&d)
        .args(["release", "merge-promotion", "--sha", "d1d1d1d1"])
        .assert()
        .code(3)
        .stderr(contains("mudou"));
    assert!(gh.posted().is_empty());

    ctower(&d)
        .args(["release", "merge-promotion", "--sha", "d2d2d2d2"])
        .assert()
        .success();
    let posted = gh.posted();
    assert_eq!(posted.len(), 1);
    assert_eq!(posted[0].0, format!("{R}/pulls/10/merge"));
    assert_eq!(
        posted[0].1,
        json!({ "merge_method": "merge", "sha": "d2d2d2d2" })
    );
    let evs = d.events.lock().unwrap().clone();
    let last = evs.last().unwrap();
    assert_eq!(last["step"], "merge-promotion");
    assert_eq!(last["result"], "ok");
    assert_eq!(last["pr"], 10);
    assert_eq!(last["sha"], "d2d2d2d2");
}
