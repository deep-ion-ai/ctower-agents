//! `ctower hub sync` contra um `/hub/sync` falso.

use assert_cmd::Command;
use predicates::str::contains;
use std::io::{Read, Write};
use std::net::TcpListener;

/// Servidor HTTP mínimo que responde `status` e `body` a toda requisição.
fn fake_daemon(status: &'static str, body: &'static str) -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for mut s in l.incoming().flatten() {
            let mut buf = [0u8; 4096];
            let _ = s.read(&mut buf);
            let _ = write!(
                s,
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });
    port
}

fn ctower(cfg: &std::path::Path, port: u16) -> Command {
    std::fs::write(cfg.join("ui.token"), "t").unwrap();
    let mut c = Command::cargo_bin("ctower").unwrap();
    c.env("CTOWER_CONFIG_DIR", cfg)
        .env("CTOWER_DATA_DIR", cfg.join("data"))
        .env("CTOWER_PORT", port.to_string())
        .args(["hub", "sync"]);
    c
}

#[test]
fn sync_ok_imprime_o_resumo() {
    let d = tempfile::tempdir().unwrap();
    let port = fake_daemon(
        "200 OK",
        r#"{"ok":true,"batches":2,"sent":250,"acked_seq":250,"pending":0,"error":null,"joined":false}"#,
    );
    ctower(d.path(), port).assert().success().stdout(contains(
        "hub confirmou 250 mudança(s) em 2 lote(s) (até seq 250)",
    ));
}

#[test]
fn sync_mostra_o_pull_da_configuracao() {
    let d = tempfile::tempdir().unwrap();
    let port = fake_daemon(
        "200 OK",
        r#"{"ok":true,"batches":1,"sent":1,"acked_seq":9,"pending":0,"error":null,"joined":false,
            "config":{"ok":true,"changed":true,"config_rev":42,"added":["o/a"],"updated":[],
                      "unassigned":["o/b","o/c"],"pending_applies":2,"error":null,"legacy_hub":false}}"#,
    );
    ctower(d.path(), port).assert().success().stdout(contains(
        "configuração do hub 42: 1 projeto(s) novo(s), 0 alterado(s), 2 desatribuído(s)",
    ));
    let d2 = tempfile::tempdir().unwrap();
    let port2 = fake_daemon(
        "200 OK",
        r#"{"ok":true,"batches":0,"sent":0,"acked_seq":null,"pending":0,"error":null,"joined":false,
            "config":{"ok":true,"changed":false,"config_rev":42,"added":[],"updated":[],
                      "unassigned":[],"pending_applies":2,"error":null,"legacy_hub":false}}"#,
    );
    ctower(d2.path(), port2).assert().success().stdout(contains(
        "2 atribuição(ões) sendo aplicada(s) em segundo plano",
    ));
    let d = tempfile::tempdir().unwrap();
    let port = fake_daemon(
        "200 OK",
        r#"{"ok":true,"batches":0,"sent":0,"acked_seq":null,"pending":0,"error":null,"joined":false,
            "config":{"ok":true,"changed":false,"config_rev":null,"added":[],"updated":[],
                      "unassigned":[],"pending_applies":0,"error":null,"legacy_hub":true}}"#,
    );
    ctower(d.path(), port)
        .assert()
        .success()
        .stdout(contains("projetos locais mantidos"));
}

#[test]
fn sync_com_hub_fora_falha_com_o_motivo() {
    let d = tempfile::tempdir().unwrap();
    let port = fake_daemon(
        "200 OK",
        r#"{"ok":false,"batches":0,"sent":0,"acked_seq":null,"pending":7,"error":"hub inacessível: Connection refused","joined":false}"#,
    );
    ctower(d.path(), port)
        .assert()
        .code(1)
        .stderr(contains("hub inacessível"))
        .stderr(contains("7 mudança(s) continuam no outbox"));
}

#[test]
fn sync_sem_hub_configurado_falha() {
    let d = tempfile::tempdir().unwrap();
    let port = fake_daemon("404 Not Found", "ambiente não ligado a um hub");
    ctower(d.path(), port)
        .assert()
        .code(1)
        .stderr(contains("ambiente não ligado a um hub"));
}

#[test]
fn sync_com_daemon_fora_do_ar_falha() {
    let d = tempfile::tempdir().unwrap();
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    ctower(d.path(), port)
        .assert()
        .code(1)
        .stderr(contains("fora do ar"));
}
