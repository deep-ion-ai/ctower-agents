//! `ctower hub ...` no ambiente: token de sync e resync. Os tokens são
//! gerados no hub (`cthubd add-env`); o `ctower` não tem mais `add-env`.

use assert_cmd::Command;
use predicates::str::contains;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

fn ctower(dir: &Path) -> Command {
    let mut c = Command::cargo_bin("ctower").unwrap();
    c.env("CTOWER_CONFIG_DIR", dir.join("config"))
        .env("CTOWER_DATA_DIR", dir.join("data"));
    c
}

#[test]
fn set_token_grava_o_env_id_que_vem_junto() {
    let token = "b".repeat(64);
    let id = "0b9d6c52-5c4e-4a3f-9a43-7d0d8b1f6a11";
    let env = tempfile::tempdir().unwrap();
    ctower(env.path())
        .args(["hub", "set-token"])
        .write_stdin(format!("{token}\n{id}\n"))
        .assert()
        .success();
    let cfg = env.path().join("config");
    assert_eq!(
        std::fs::read_to_string(cfg.join("hub.token")).unwrap(),
        token
    );
    assert_eq!(std::fs::read_to_string(cfg.join("env_id")).unwrap(), id);
    // Girar o token do mesmo ambiente repete o env_id: segue valendo.
    ctower(env.path())
        .args(["hub", "set-token"])
        .write_stdin(format!("{}\n{id}\n", "c".repeat(64)))
        .assert()
        .success();
    // Um env_id diferente do que o ambiente já usa é recusado, sem trocar o token.
    ctower(env.path())
        .args(["hub", "set-token"])
        .write_stdin(format!(
            "{}\n1b9d6c52-5c4e-4a3f-9a43-7d0d8b1f6a11\n",
            "d".repeat(64)
        ))
        .assert()
        .failure();
    assert_eq!(
        std::fs::read_to_string(cfg.join("hub.token")).unwrap(),
        "c".repeat(64)
    );
    assert_eq!(std::fs::read_to_string(cfg.join("env_id")).unwrap(), id);
}

#[test]
fn set_token_e_resync() {
    let token = "a".repeat(64);
    let env = tempfile::tempdir().unwrap();
    ctower(env.path())
        .args(["hub", "set-token"])
        .write_stdin(format!("{token}\n"))
        .assert()
        .success();
    let f = env.path().join("config/hub.token");
    assert_eq!(std::fs::read_to_string(&f).unwrap(), token);
    assert_eq!(
        std::fs::metadata(&f).unwrap().permissions().mode() & 0o777,
        0o600
    );
    ctower(env.path())
        .args(["hub", "set-token"])
        .write_stdin("")
        .assert()
        .failure();

    // Ledger vazio: nada a reenviar, mas o comando funciona sem daemon.
    ctower(env.path())
        .args(["hub", "resync"])
        .assert()
        .success()
        .stdout(contains("0 mudança(s)"));

    // Com work items já podados, o resync avisa que eles não voltam.
    let db = rusqlite::Connection::open(env.path().join("data/ledger.db")).unwrap();
    db.execute(
        "INSERT INTO ledger_meta (key, value) VALUES ('pruned_work_items', 3)",
        [],
    )
    .unwrap();
    drop(db);
    ctower(env.path())
        .args(["hub", "resync"])
        .assert()
        .success()
        .stdout(contains("3 work item(s) já podado(s)"));
}

#[test]
fn add_env_saiu_do_ctower() {
    let d = tempfile::tempdir().unwrap();
    for sub in ["add-env", "revoke-env"] {
        // Erro de argumento sai com 0 (regra dos hooks), mas avisa.
        ctower(d.path())
            .args(["hub", sub, "notebook"])
            .assert()
            .stderr(contains("unrecognized subcommand"));
        assert!(!d.path().join("data/hub.db").exists());
    }
}
