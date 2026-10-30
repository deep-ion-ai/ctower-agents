//! `ctowerd` com `hub = true` (papel de hub do #64) recusa subir e aponta a
//! migração para o `cthubd` (spec do hub como plano de controle §2).

use std::process::Command;
use std::time::{Duration, Instant};

#[test]
fn hub_true_recusa_subir_e_aponta_a_migracao() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = tmp.path().join("config");
    std::fs::create_dir_all(&cfg).unwrap();
    std::fs::write(
        cfg.join("config.toml"),
        "hub = true\nhub_backup_keep = 3\nport = 1\n",
    )
    .unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_ctowerd"))
        .env("CTOWER_CONFIG_DIR", &cfg)
        .env("CTOWER_DATA_DIR", tmp.path().join("data"))
        .env("RUST_LOG", "error")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let start = Instant::now();
    while child.try_wait().unwrap().is_none() {
        if start.elapsed() > Duration::from_secs(15) {
            let _ = child.kill();
            panic!("ctowerd subiu com hub = true");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let out = child.wait_with_output().unwrap();
    assert!(!out.status.success());
    let log = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        log.contains("hub = true não existe mais")
            && log.contains("cthubd migrate")
            && log.contains("docs/hub.md"),
        "{log}"
    );
    // Não abriu ledger nem hub.db.
    assert!(!tmp.path().join("data/hub.db").exists());
    assert!(!tmp.path().join("data/ledger.db").exists());
}
