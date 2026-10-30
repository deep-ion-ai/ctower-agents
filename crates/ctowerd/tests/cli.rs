//! `ctowerd --version`/`--help` respondem sem subir o daemon, e argumento
//! desconhecido é recusado (issue #247).

use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

/// Roda o `ctowerd` com diretórios temporários e porta inválida; falha se
/// ele não terminar sozinho (subiu o daemon).
fn ctowerd(dir: &Path, args: &[&str]) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_ctowerd"))
        .args(args)
        .env("CTOWER_CONFIG_DIR", dir.join("config"))
        .env("CTOWER_DATA_DIR", dir.join("data"))
        .env("RUST_LOG", "error")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let start = Instant::now();
    while child.try_wait().unwrap().is_none() {
        if start.elapsed() > Duration::from_secs(15) {
            let _ = child.kill();
            panic!("ctowerd {args:?} subiu o daemon");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    child.wait_with_output().unwrap()
}

/// Nada foi criado: nem config, nem dados, nem ledger.
fn assert_intocado(dir: &Path) {
    assert!(!dir.join("config").exists());
    assert!(!dir.join("data").exists());
}

#[test]
fn version_mostra_versao_e_commit_sem_subir() {
    let tmp = tempfile::tempdir().unwrap();
    let out = ctowerd(tmp.path(), &["--version"]);
    assert!(out.status.success(), "{out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.starts_with("ctowerd "), "{stdout}");
    assert!(stdout.contains(ct_core::VERSION), "{stdout}");
    let commit = match ct_core::BUILD_COMMIT {
        "" => "commit desconhecido",
        c => c,
    };
    assert!(stdout.contains(commit), "{stdout}");
    assert_intocado(tmp.path());
}

#[test]
fn help_sai_com_zero_sem_subir() {
    let tmp = tempfile::tempdir().unwrap();
    let out = ctowerd(tmp.path(), &["--help"]);
    assert!(out.status.success(), "{out:?}");
    assert!(String::from_utf8_lossy(&out.stdout).contains("--version"));
    assert_intocado(tmp.path());
}

#[test]
fn argumento_desconhecido_sai_com_erro_sem_subir() {
    let tmp = tempfile::tempdir().unwrap();
    for args in [&["--qualquer-coisa"][..], &["serve"]] {
        let out = ctowerd(tmp.path(), args);
        assert!(!out.status.success(), "{args:?}: {out:?}");
        assert!(!out.stderr.is_empty(), "{args:?}: {out:?}");
        assert_intocado(tmp.path());
    }
}
