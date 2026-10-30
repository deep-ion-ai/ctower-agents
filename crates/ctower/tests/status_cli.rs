//! `ctower status` / `ctower restart` contra um `/status` falso.

use assert_cmd::Command;
use predicates::prelude::*;
use predicates::str::contains;
use std::io::{Read, Write};
use std::net::TcpListener;

/// Servidor HTTP mínimo que responde `body` a toda requisição.
fn fake_daemon(body: &'static str) -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for mut s in l.incoming().flatten() {
            let mut buf = [0u8; 4096];
            let _ = s.read(&mut buf);
            let _ = write!(
                s,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });
    port
}

fn ctower(cfg: &std::path::Path, port: u16) -> Command {
    let mut c = Command::cargo_bin("ctower").unwrap();
    c.env("CTOWER_CONFIG_DIR", cfg)
        .env("CTOWER_DATA_DIR", cfg.join("data"))
        .env("CTOWER_PORT", port.to_string())
        .env("CTOWER_SYSTEMCTL", "true");
    c
}

const COM_SESSAO: &str = r#"{"version":"0.1.0","interrupted":0,"live_sessions":[{"id":"s-1","state":"running","cwd":"/w/feat__1-a"}]}"#;
const SEM_SESSAO: &str = r#"{"version":"0.1.0","interrupted":2,"live_sessions":[]}"#;

#[test]
fn status_mostra_sessoes() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("ui.token"), "t").unwrap();
    let port = fake_daemon(COM_SESSAO);
    ctower(d.path(), port)
        .arg("status")
        .assert()
        .success()
        .stdout(contains("1 sessão(ões) viva(s)"))
        .stdout(contains("s-1"));
}

#[test]
fn status_com_daemon_fora_do_ar_falha() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("ui.token"), "t").unwrap();
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    ctower(d.path(), port)
        .arg("status")
        .assert()
        .code(1)
        .stderr(contains("fora do ar"));
}

#[test]
fn status_aponta_executavel_ausente_do_launchagent() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("ui.token"), "t").unwrap();
    let agents = d.path().join("Library/LaunchAgents");
    std::fs::create_dir_all(&agents).unwrap();
    let home = d.path().to_str().unwrap();
    let plist = include_str!("../../../packaging/launchd/ai.deep-ion.ctowerd.plist")
        .replace("@HOME@", home);
    std::fs::write(agents.join("ai.deep-ion.ctowerd.plist"), plist).unwrap();
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    ctower(d.path(), port)
        .env("HOME", home)
        .env("CTOWER_SERVICE_MANAGER", "launchd")
        .arg("status")
        .assert()
        .code(1)
        .stderr(contains("fora do ar"))
        .stderr(contains(format!(
            "executável do serviço ausente: {home}/.cargo/bin/ctowerd"
        )));
}

#[test]
fn status_nao_acusa_ausencia_com_o_executavel_presente() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("ui.token"), "t").unwrap();
    let unit_dir = d.path().join(".config/systemd/user");
    std::fs::create_dir_all(&unit_dir).unwrap();
    std::fs::write(
        unit_dir.join("ctowerd.service"),
        "[Service]\nExecStart=/bin/sh\n",
    )
    .unwrap();
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    ctower(d.path(), port)
        .env("HOME", d.path())
        .env("CTOWER_SERVICE_MANAGER", "systemd")
        .arg("status")
        .assert()
        .code(1)
        .stderr(contains("fora do ar"))
        .stderr(contains("ausente").not());
}

#[test]
fn restart_recusa_com_sessoes_vivas_sem_force() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("ui.token"), "t").unwrap();
    let port = fake_daemon(COM_SESSAO);
    ctower(d.path(), port)
        .arg("restart")
        .assert()
        .code(1)
        .stderr(contains("Recusado"))
        .stderr(contains("s-1"));
    ctower(d.path(), port)
        .args(["restart", "--force"])
        .assert()
        .success()
        .stdout(contains("reiniciado"));
}

#[test]
fn restart_sem_sessoes_reinicia() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("ui.token"), "t").unwrap();
    let port = fake_daemon(SEM_SESSAO);
    ctower(d.path(), port).arg("restart").assert().success();
}

/// Sem token de UI (ex.: HOME temporário de um teste, #283) e com um ctowerd
/// escutando: não dá para ver as sessões, então recusa sem `--force`.
#[test]
fn restart_sem_token_com_daemon_no_ar_recusa() {
    let d = tempfile::tempdir().unwrap();
    let port = fake_daemon(COM_SESSAO);
    ctower(d.path(), port)
        .arg("restart")
        .assert()
        .code(1)
        .stderr(contains("não consegui ver as sessões vivas"));
    ctower(d.path(), port)
        .args(["restart", "--force"])
        .assert()
        .success();
}

#[test]
fn restart_sem_token_com_daemon_fora_do_ar_reinicia() {
    let d = tempfile::tempdir().unwrap();
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    ctower(d.path(), port).arg("restart").assert().success();
}
