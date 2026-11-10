//! `ctower secrets rotate-env-key` com o ctowerd fora do ar (troca local).

use assert_cmd::Command;
use predicates::str::contains;

#[test]
fn rotacao_offline_troca_a_chave_e_mantem_o_cache() {
    let d = tempfile::tempdir().unwrap();
    let cfg = d.path().join("config");
    let data = d.path().join("data");
    std::fs::create_dir_all(&cfg).unwrap();
    std::fs::write(cfg.join("ui.token"), "t").unwrap();
    let cache = ct_secrets::EnvCache::new(cfg.join("env.key"), data.join("secrets-cache.age"));
    let antiga = cache.public_key().unwrap();
    let entrega = ct_secrets::seal_delivery(1, vec![], &antiga).unwrap();
    cache
        .store(&ct_secrets::decode_delivery(&entrega).unwrap())
        .unwrap();
    // Porta sem ninguém escutando: o daemon está fora do ar.
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    Command::cargo_bin("ctower")
        .unwrap()
        .env("CTOWER_CONFIG_DIR", &cfg)
        .env("CTOWER_DATA_DIR", &data)
        .env("CTOWER_PORT", port.to_string())
        .args(["secrets", "rotate-env-key"])
        .assert()
        .success()
        .stdout(contains("chave do ambiente trocada aqui"))
        .stdout(contains("cache de secrets recifrado"));
    let nova = cache.public_key().unwrap();
    assert_ne!(nova, antiga);
    assert_eq!(cache.load().unwrap().unwrap().config_rev(), 1);
}

// ---- `ctower secret get NOME` (#165) ----

fn secret_get() -> Command {
    let mut c = Command::cargo_bin("ctower").unwrap();
    for v in [
        "CTOWER_URL",
        "CTOWER_TOKEN",
        "CTOWER_SESSION",
        "CTOWER_SESSION_TOKEN",
    ] {
        c.env_remove(v);
    }
    c
}

/// `ctowerd` falso: atende uma requisição com `status` e `body` e devolve o
/// que recebeu (cabeçalhos e corpo).
fn fake_daemon(
    status: &'static str,
    body: &'static str,
) -> (String, std::thread::JoinHandle<String>) {
    use std::io::{Read, Write};
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    let h = std::thread::spawn(move || {
        let (mut c, _) = l.accept().unwrap();
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
                    break;
                }
            }
            if n == 0 {
                break;
            }
        }
        let resp = format!(
            "HTTP/1.1 {status}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );
        c.write_all(resp.as_bytes()).unwrap();
        String::from_utf8_lossy(&req).to_string()
    });
    (url, h)
}

#[test]
fn secret_get_imprime_o_valor_sem_quebra_de_linha() {
    let (url, h) = fake_daemon("200 OK", "jwt-renovado-bbbbbbbb");
    secret_get()
        .env("CTOWER_URL", &url)
        .env("CTOWER_SESSION_TOKEN", "token-da-sessao")
        .args(["secret", "get", "API_JWT"])
        .assert()
        .success()
        .stdout("jwt-renovado-bbbbbbbb");
    let req = h.join().unwrap();
    assert!(req.starts_with("POST /session/secret "), "{req}");
    assert!(
        req.to_ascii_lowercase()
            .contains("authorization: bearer token-da-sessao"),
        "{req}"
    );
    assert!(req.ends_with(r#"{"name":"API_JWT"}"#), "{req}");
}

#[test]
fn secret_get_erros_saem_no_stderr_sem_valor() {
    // Nome que não vale para a sessão: 2.
    let (url, h) = fake_daemon(
        "404 Not Found",
        "SO_OUTRO não existe para o projeto da sessão",
    );
    secret_get()
        .env("CTOWER_URL", &url)
        .env("CTOWER_SESSION_TOKEN", "t")
        .args(["secret", "get", "SO_OUTRO"])
        .assert()
        .code(2)
        .stdout("")
        .stderr(contains("não existe para o projeto da sessão"));
    h.join().unwrap();
    // Cache ilegível: 1, com a mensagem do daemon.
    let (url, h) = fake_daemon("503 Service Unavailable", "cache do ambiente ilegível: x");
    secret_get()
        .env("CTOWER_URL", &url)
        .env("CTOWER_SESSION_TOKEN", "t")
        .args(["secret", "get", "API_JWT"])
        .assert()
        .code(1)
        .stdout("")
        .stderr(contains("cache do ambiente ilegível"));
    h.join().unwrap();
    // Token recusado: 1.
    let (url, h) = fake_daemon("401 Unauthorized", "token de sessão inválido");
    secret_get()
        .env("CTOWER_URL", &url)
        .env("CTOWER_SESSION_TOKEN", "t")
        .args(["secret", "get", "API_JWT"])
        .assert()
        .code(1)
        .stdout("");
    h.join().unwrap();
    // Fora de uma sessão (sem token): 1, sem chamar o daemon.
    secret_get()
        .env("CTOWER_URL", "http://127.0.0.1:9")
        .args(["secret", "get", "API_JWT"])
        .assert()
        .code(1)
        .stderr(contains("CTOWER_SESSION_TOKEN ausente"));
    // Daemon fora do ar: 1.
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    secret_get()
        .env("CTOWER_URL", format!("http://127.0.0.1:{port}"))
        .env("CTOWER_SESSION_TOKEN", "t")
        .args(["secret", "get", "API_JWT"])
        .assert()
        .code(1)
        .stderr(contains("ctowerd fora do ar"));
    // Sem o nome: erro de argumento, diferente de zero (não é hook).
    secret_get().args(["secret", "get"]).assert().failure();
}
