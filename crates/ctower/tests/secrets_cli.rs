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
