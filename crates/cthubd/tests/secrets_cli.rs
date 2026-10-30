//! `cthubd secrets restore|rotate-keys` (#170) pelo binário: serviço no ar
//! recusa, a lista não tem valores, sem confirmação nada muda, a chave de
//! recuperação pelo stdin e a rotação das chaves.

use assert_cmd::Command;
use ct_ledger::{Hub, SharedHub};
use ct_secrets::{Kind, Scope, SecretValue};
use cthubd::config::Dirs;
use cthubd::secrets::HubSecrets;
use predicates::str::contains;
use std::path::{Path, PathBuf};

const TOKEN: &str = "token-de-api-secreto-123";
const VAR: &str = "valor-da-variavel-789";

/// Porta livre: o teste nunca fala com um `cthubd` de verdade desta máquina.
fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn cthubd(dir: &Path, port: u16) -> Command {
    let mut c = Command::cargo_bin("cthubd").unwrap();
    c.env("CTHUB_CONFIG_DIR", dir.join("config"))
        .env("CTHUB_DATA_DIR", dir.join("data"))
        .env("CTHUB_PORT", port.to_string());
    c
}

/// Hub com recuperação configurada e dois secrets e uma variável; os
/// snapshots vão para o padrão (`<data>/backups`). Devolve a chave de
/// recuperação e o snapshot mais recente.
fn hub_com_secrets(p: &Path) -> (String, PathBuf) {
    let dirs = Dirs {
        config: p.join("config"),
        data: p.join("data"),
    };
    dirs.ensure().unwrap();
    let snaps = p.join("data/backups");
    let hub = SharedHub::new(Hub::open(&dirs.hub_db()).unwrap());
    let s = HubSecrets::open(&dirs, hub)
        .unwrap()
        .with_snapshot_dir(snaps.clone());
    let g = s.recovery_generate().unwrap();
    s.recovery_verify(&g.private_key).unwrap();
    s.set(
        Scope::Global,
        "API_TOKEN",
        Kind::Secret,
        SecretValue::new(TOKEN),
    )
    .unwrap();
    s.set(
        Scope::parse_project("deep-ion-ai/demo").unwrap(),
        "D",
        Kind::Secret,
        SecretValue::new("d-secreto-456"),
    )
    .unwrap();
    s.set(
        Scope::Global,
        "RUST_LOG",
        Kind::Variable,
        SecretValue::new(VAR),
    )
    .unwrap();
    // O mais recente é a cópia byte a byte do store.
    let store = std::fs::read(dirs.secrets_file()).unwrap();
    let newest = std::fs::read_dir(&snaps)
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| std::fs::read(p).unwrap() == store)
        .expect("snapshot do store");
    let snap = p.join("secrets-copia.age");
    std::fs::copy(newest, &snap).unwrap();
    (g.private_key, snap)
}

#[test]
fn restore_recusa_com_servico_no_ar_e_sem_confirmacao_nao_muda_nada() {
    let tmp = tempfile::tempdir().unwrap();
    let p = tmp.path();
    let (rec, snap) = hub_com_secrets(p);
    let rec_file = p.join("rec.key");
    std::fs::write(&rec_file, format!("{rec}\n")).unwrap();

    // Serviço no ar (algo escutando na porta do config): recusa.
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    cthubd(p, port)
        .args(["secrets", "restore"])
        .arg(&snap)
        .arg("--yes")
        .assert()
        .failure()
        .stderr(contains("pare o cthubd"));
    cthubd(p, port)
        .args(["secrets", "rotate-keys"])
        .assert()
        .failure()
        .stderr(contains("pare o cthubd"));
    drop(l);

    // Máquina perdida: sem config. Sem confirmar, nada muda.
    std::fs::remove_dir_all(p.join("config")).unwrap();
    let out = cthubd(p, free_port())
        .args(["secrets", "restore"])
        .arg(&snap)
        .arg("--identity")
        .arg(&rec_file)
        .write_stdin("n\n")
        .assert()
        .code(2)
        .stdout(contains("nada foi alterado"))
        .get_output()
        .clone();
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(
        stdout.contains("aberto com a chave de recuperação"),
        "{stdout}"
    );
    for nome in ["API_TOKEN", "deep-ion-ai/demo", "RUST_LOG", "(variável)"] {
        assert!(stdout.contains(nome), "{nome}: {stdout}");
    }
    let tudo = format!("{stdout}{}", String::from_utf8_lossy(&out.stderr));
    for valor in [TOKEN, VAR, "d-secreto-456", rec.as_str()] {
        assert!(!tudo.contains(valor), "{tudo}");
    }
    let config = p.join("config");
    let sobrou: Vec<_> = std::fs::read_dir(&config)
        .map(|d| d.map(|e| e.unwrap().file_name()).collect())
        .unwrap_or_default();
    assert!(
        !sobrou
            .iter()
            .any(|n| n.to_string_lossy().starts_with("secrets")),
        "{sobrou:?}"
    );
    // Sem stdin (EOF) também não.
    cthubd(p, free_port())
        .args(["secrets", "restore"])
        .arg(&snap)
        .arg("--identity")
        .arg(&rec_file)
        .assert()
        .code(2);
    assert!(!config.join("secrets.age").exists());
}

#[test]
fn restore_com_a_chave_pelo_stdin_e_rotate_keys() {
    let tmp = tempfile::tempdir().unwrap();
    let p = tmp.path();
    let (rec, snap) = hub_com_secrets(p);
    let antiga = std::fs::read_to_string(p.join("config/secrets.key")).unwrap();
    std::fs::remove_dir_all(p.join("config")).unwrap();

    // Chave errada: erro, sem a chave na mensagem.
    let outra = "AGE-SECRET-KEY-1QQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQ";
    let out = cthubd(p, free_port())
        .args(["secrets", "restore"])
        .arg(&snap)
        .write_stdin(format!("{outra}\ns\n"))
        .assert()
        .failure()
        .get_output()
        .clone();
    assert!(!String::from_utf8_lossy(&out.stderr).contains(outra));

    let out = cthubd(p, free_port())
        .args(["secrets", "restore"])
        .arg(&snap)
        .write_stdin(format!("{rec}\ns\n"))
        .assert()
        .success()
        .stdout(contains("restaurado"))
        .stdout(contains("secrets a rotacionar na origem"))
        .stdout(contains("API_TOKEN"))
        .stdout(contains("config_rev"))
        .get_output()
        .clone();
    let tudo = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    for valor in [TOKEN, VAR, "d-secreto-456", rec.as_str()] {
        assert!(!tudo.contains(valor), "{tudo}");
    }
    let config = p.join("config");
    let nova = std::fs::read_to_string(config.join("secrets.key")).unwrap();
    assert_ne!(nova, antiga);
    assert!(config.join("secrets.rotate-pending.json").exists());
    assert!(!config.join("secrets.recovery.pub").exists());

    // rotate-keys sem flags: as duas chaves; a privada aparece uma vez.
    let out = cthubd(p, free_port())
        .args(["secrets", "rotate-keys"])
        .assert()
        .success()
        .get_output()
        .clone();
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert_eq!(stdout.matches("AGE-SECRET-KEY-1").count(), 1, "{stdout}");
    assert!(stdout.contains("gerenciador de senhas"), "{stdout}");
    assert_ne!(
        std::fs::read_to_string(config.join("secrets.key")).unwrap(),
        nova
    );
    assert!(config.join("secrets.recovery.pending.pub").exists());
    assert!(!config.join("secrets.key.old").exists());
    // A privada nova não fica em disco.
    let chave = stdout
        .split_whitespace()
        .find(|w| w.starts_with("AGE-SECRET-KEY-1"))
        .unwrap();
    for d in [&config, &p.join("data")] {
        for e in walk(d) {
            let bytes = std::fs::read(&e).unwrap();
            assert!(
                !String::from_utf8_lossy(&bytes).contains(chave),
                "{}",
                e.display()
            );
        }
    }
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.is_dir() {
            out.extend(walk(&p));
        } else {
            out.push(p);
        }
    }
    out
}

/// `secrets list --env`: só nomes do que o ambiente recebe (globais e dos
/// projetos atribuídos), com o serviço no ar.
#[test]
fn secrets_list_por_ambiente_sem_valores() {
    let tmp = tempfile::tempdir().unwrap();
    let p = tmp.path();
    hub_com_secrets(p);
    {
        let h = Hub::open(&p.join("data/hub.db")).unwrap();
        h.add_project(&ct_ledger::NewHubProject {
            owner: "deep-ion-ai".into(),
            repo: "demo".into(),
            name: "demo".into(),
            base_branch: "develop".into(),
            protected_branches: vec![],
        })
        .unwrap();
        for (nome, env) in [("notebook", "e1"), ("mac", "e2")] {
            h.add_legacy_env_token(nome).unwrap();
            h.bind_env_token(nome, env).unwrap();
        }
        h.add_assignment("deep-ion-ai", "demo", "e1", None).unwrap();
    }
    // Funciona com algo escutando na porta (só lê).
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let run = |args: &[&str]| {
        let out = cthubd(p, port)
            .args(["secrets", "list"])
            .args(args)
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        String::from_utf8(out).unwrap()
    };
    let todos = run(&[]);
    let notebook = run(&["--env", "notebook"]);
    let mac = run(&["--env", "mac"]);
    for s in [&todos, &notebook, &mac] {
        assert!(s.contains("global  API_TOKEN  (secret)"), "{s}");
        assert!(s.contains("RUST_LOG  (variável)"), "{s}");
        for valor in [TOKEN, VAR, "d-secreto-456"] {
            assert!(!s.contains(valor), "{s}");
        }
    }
    assert!(todos.contains("deep-ion-ai/demo  D  (secret)"), "{todos}");
    assert!(
        notebook.contains("deep-ion-ai/demo  D  (secret)"),
        "{notebook}"
    );
    assert!(!mac.contains("deep-ion-ai/demo"), "{mac}");
    cthubd(p, port)
        .args(["secrets", "list", "--env", "nada"])
        .assert()
        .failure()
        .stderr(contains("ambiente desconhecido"));
    drop(l);
}
