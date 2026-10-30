//! `ctower update` contra um repositório de origem local e um `/healthz` falso.

use assert_cmd::Command;
use predicates::str::contains;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::Path;

fn git(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .current_dir(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@t"])
        .args(args)
        .output()
        .unwrap();
    assert!(out.status.success(), "{args:?}: {out:?}");
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

fn commit(dir: &Path, version: &str, msg: &str) -> String {
    std::fs::write(
        dir.join("Cargo.toml"),
        format!("[workspace.package]\nversion = \"{version}\"\n"),
    )
    .unwrap();
    git(dir, &["add", "."]);
    git(dir, &["commit", "-q", "--allow-empty", "-m", msg]);
    git(dir, &["rev-parse", "HEAD"])
}

/// `/healthz` falso que responde o conteúdo atual de `file` (o
/// `systemctl` falso troca o arquivo ao "reiniciar").
fn file_daemon(file: std::path::PathBuf) -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for mut s in l.incoming().flatten() {
            let mut buf = [0u8; 4096];
            let n = s.read(&mut buf).unwrap_or(0);
            // `/status` (sessões vivas) vem de status.json, ao lado.
            let f = if buf[..n].starts_with(b"GET /status") {
                file.with_file_name("status.json")
            } else {
                file.clone()
            };
            let body = std::fs::read_to_string(&f).unwrap_or_default();
            let _ = write!(
                s,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });
    port
}

struct Env {
    tmp: tempfile::TempDir,
    origin: std::path::PathBuf,
}

impl Env {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let origin = tmp.path().join("origin");
        std::fs::create_dir_all(&origin).unwrap();
        git(&origin, &["init", "-q", "-b", "main"]);
        std::fs::create_dir_all(tmp.path().join("config")).unwrap();
        std::fs::write(
            tmp.path().join("config/config.toml"),
            format!("update_repo = \"{}\"\n", origin.display()),
        )
        .unwrap();
        Env { tmp, origin }
    }

    /// `/healthz` falso respondendo com `commit`.
    fn daemon(&self, commit: &str) -> u16 {
        let f = self.tmp.path().join("health.json");
        std::fs::write(&f, health(commit)).unwrap();
        file_daemon(f)
    }

    fn ctower(&self, port: u16) -> Command {
        let mut c = Command::cargo_bin("ctower").unwrap();
        c.env("CTOWER_CONFIG_DIR", self.tmp.path().join("config"))
            .env("CTOWER_DATA_DIR", self.tmp.path().join("data"))
            .env("CTOWER_PORT", port.to_string());
        c
    }
}

#[test]
fn check_lista_commits_novos() {
    let env = Env::new();
    let c1 = commit(&env.origin, "0.1.0", "primeiro");
    commit(&env.origin, "0.2.0", "recurso novo");
    let port = env.daemon(&c1);
    env.ctower(port)
        .args(["update", "--check"])
        .assert()
        .success()
        .stdout(contains("instalado:  0.1.0"))
        .stdout(contains("disponível: 0.2.0"))
        .stdout(contains("1 commit(s) novo(s)"))
        .stdout(contains("recurso novo"));
}

#[test]
fn check_em_dia_e_ref_inexistente() {
    let env = Env::new();
    let c1 = commit(&env.origin, "0.1.0", "primeiro");
    let port = env.daemon(&c1);
    env.ctower(port)
        .args(["update", "--check"])
        .assert()
        .success()
        .stdout(contains("Em dia."));
    std::fs::write(
        env.tmp.path().join("config/config.toml"),
        format!(
            "update_repo = \"{}\"\nupdate_ref = \"nao-existe\"\n",
            env.origin.display()
        ),
    )
    .unwrap();
    env.ctower(port)
        .args(["update", "--check"])
        .assert()
        .failure()
        .stderr(contains("verificação falhou"));
}

fn health(commit: &str) -> String {
    format!(r#"{{"ok":true,"version":"0.1.0","commit":"{commit}"}}"#)
}

fn script(path: &Path, body: &str) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// Ambiente do atualizador: binários "old" em `bin/`, `cargo` e `systemctl`
/// falsos e um `/healthz` lido de `health.json`.
struct Upd {
    env: Env,
    port: u16,
    c1: String,
    c2: String,
}

impl Upd {
    fn new() -> Self {
        let env = Env::new();
        let p = env.tmp.path();
        let c1 = commit(&env.origin, "0.1.0", "primeiro");
        let c2 = commit(&env.origin, "0.2.0", "segundo");
        std::fs::create_dir_all(p.join("bin")).unwrap();
        for b in ["ctowerd", "ctower"] {
            script(&p.join("bin").join(b), "echo old");
        }
        script(
            &p.join("fake-cargo"),
            r#"[ -n "$FAKE_CARGO_FAIL" ] && { echo "error: compilation failed" >&2; exit 101; }
while [ $# -gt 0 ]; do case "$1" in --path) P="$2"; shift;; --root) R="$2"; shift;; esac; shift; done
mkdir -p "$R/bin"; printf '#!/bin/sh\necho new %s\n' "$CTOWER_COMMIT" > "$R/bin/$(basename "$P")"; chmod +x "$R/bin/$(basename "$P")""#,
        );
        script(
            &p.join("fake-systemctl"),
            r#"echo "$@" >> "$T/systemctl.log"
case "$2" in
  restart) [ -f "$T/after-restart.json" ] && cp "$T/after-restart.json" "$T/health.json";;
  start) [ -f "$T/after-start.json" ] && cp "$T/after-start.json" "$T/health.json";;
esac; exit 0"#,
        );
        script(
            &p.join("fake-systemd-run"),
            r#"while [ "${1#--}" != "$1" ]; do shift; done; "$@" > /dev/null 2>&1 &"#,
        );
        std::fs::write(p.join("config/ui.token"), "t").unwrap();
        std::fs::write(
            p.join("status.json"),
            r#"{"version":"0.1.0","interrupted":0,"live_sessions":[]}"#,
        )
        .unwrap();
        std::fs::create_dir_all(p.join("data")).unwrap();
        ct_ledger::Ledger::open(&p.join("data/ledger.db"), "env").unwrap();
        let port = env.daemon(&c1);
        Upd { env, port, c1, c2 }
    }

    fn path(&self, rel: &str) -> std::path::PathBuf {
        self.env.tmp.path().join(rel)
    }

    fn read(&self, rel: &str) -> String {
        std::fs::read_to_string(self.path(rel)).unwrap_or_default()
    }

    fn run(&self, args: &[&str]) -> assert_cmd::assert::Assert {
        let mut all = vec!["--run"];
        all.extend_from_slice(args);
        self.cmd(&all)
    }

    /// `ctower update <args>` com os falsos (`systemd-run` roda o comando
    /// na hora, sem unidade).
    fn cmd(&self, args: &[&str]) -> assert_cmd::assert::Assert {
        let p = self.env.tmp.path();
        self.env
            .ctower(self.port)
            .arg("update")
            .args(args)
            .env("CTOWER_SYSTEMD_RUN", p.join("fake-systemd-run"))
            .env("T", p)
            .env("CTOWER_BIN_DIR", p.join("bin"))
            .env("CTOWER_CARGO", p.join("fake-cargo"))
            .env("CTOWER_SYSTEMCTL", p.join("fake-systemctl"))
            .env("CTOWER_UPDATE_VERIFY_SECS", "2")
            .assert()
    }

    fn state(&self) -> serde_json::Value {
        serde_json::from_str(&self.read("data/update/state.json")).unwrap()
    }

    fn schema(&self, db: &str) -> i64 {
        rusqlite::Connection::open(self.path(db))
            .unwrap()
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap()
    }
}

#[test]
fn atualiza_reinicia_e_volta_com_rollback() {
    let u = Upd::new();
    std::fs::write(u.path("after-restart.json"), health(&u.c2)).unwrap();
    u.run(&[]).success();
    let st = u.state();
    assert_eq!(st["phase"], "done", "{st}");
    assert_eq!(st["from"]["commit"], u.c1.as_str());
    assert_eq!(st["to"]["commit"], u.c2.as_str());
    assert_eq!(st["to"]["version"], "0.2.0");
    assert!(u.read("bin/ctowerd").contains(&format!("new {}", u.c2)));
    assert!(u.read("bin/ctower").contains("new"));
    assert!(u.read("data/update/prev/ctowerd").contains("old"));
    assert!(u.path("data/update/db/ledger.db").is_file());
    assert_eq!(u.read("systemctl.log"), "--user restart ctowerd\n");

    // De novo: já em dia, sem build nem restart.
    u.run(&[]).success();
    assert_eq!(u.state()["message"], "já está em dia");
    assert_eq!(u.read("systemctl.log").lines().count(), 1);

    // O esquema "migrou" depois da atualização; o rollback volta o banco.
    let v = u.schema("data/ledger.db");
    rusqlite::Connection::open(u.path("data/ledger.db"))
        .unwrap()
        .pragma_update(None, "user_version", v + 1)
        .unwrap();
    std::fs::write(u.path("after-start.json"), health(&u.c1)).unwrap();
    u.run(&["--rollback"]).success();
    let st = u.state();
    assert_eq!(
        (st["kind"].as_str(), st["phase"].as_str()),
        (Some("rollback"), Some("done"))
    );
    assert!(u.read("bin/ctowerd").contains("old"));
    assert_eq!(u.schema("data/ledger.db"), v);
    // O ledger.db em uso antes do rollback ficou numa cópia ao lado.
    let pre: Vec<_> = std::fs::read_dir(u.path("data"))
        .unwrap()
        .flatten()
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with("ledger.db.pre-rollback-")
        })
        .collect();
    assert_eq!(pre.len(), 1, "{pre:?}");
    assert_eq!(
        ct_ledger::db_user_version(&pre[0].path()).unwrap(),
        v + 1,
        "a cópia é o banco migrado"
    );
    assert!(u
        .read("systemctl.log")
        .ends_with("--user stop ctowerd\n--user start ctowerd\n"));
}

#[test]
fn build_falho_nao_toca_em_nada() {
    let u = Upd::new();
    let p = u.env.tmp.path();
    u.env
        .ctower(u.port)
        .args(["update", "--run"])
        .env("T", p)
        .env("CTOWER_BIN_DIR", p.join("bin"))
        .env("CTOWER_CARGO", p.join("fake-cargo"))
        .env("CTOWER_SYSTEMCTL", p.join("fake-systemctl"))
        .env("FAKE_CARGO_FAIL", "1")
        .assert()
        .failure();
    let st = u.state();
    assert_eq!(st["phase"], "failed");
    assert!(st["error"].as_str().unwrap().contains("falhou"));
    assert!(st["log_tail"]
        .as_array()
        .unwrap()
        .iter()
        .any(|l| l.as_str().unwrap().contains("compilation failed")));
    assert!(u.read("bin/ctowerd").contains("old"));
    assert!(!u.path("systemctl.log").exists());
    assert!(!u.path("data/update/prev").exists());
}

#[test]
fn daemon_novo_que_nao_sobe_volta_sozinho() {
    let u = Upd::new();
    // Sem after-restart.json o /healthz segue no commit antigo.
    std::fs::write(u.path("after-start.json"), health(&u.c1)).unwrap();
    u.run(&[]).failure();
    let st = u.state();
    assert_eq!(st["phase"], "rolled_back", "{st}");
    assert!(
        st["error"].as_str().unwrap().contains("respondeu com"),
        "{st}"
    );
    assert!(u.read("bin/ctowerd").contains("old"));
    assert!(u.read("bin/ctower").contains("old"));
    assert_eq!(
        u.read("systemctl.log"),
        "--user restart ctowerd\n--user stop ctowerd\n--user start ctowerd\n"
    );
}

#[test]
fn rollback_sem_versao_anterior_falha() {
    let u = Upd::new();
    u.run(&["--rollback"]).failure();
    let st = u.state();
    assert_eq!(st["phase"], "failed");
    assert!(st["error"]
        .as_str()
        .unwrap()
        .contains("nenhuma versão anterior"));
}

#[test]
fn update_dispara_a_unidade_e_acompanha_ate_o_fim() {
    let u = Upd::new();
    std::fs::write(u.path("after-restart.json"), health(&u.c2)).unwrap();
    u.cmd(&[])
        .success()
        .stdout(contains("unidade ctower-update"))
        .stdout(contains("Concluído: ctowerd 0.2.0"));
    assert!(u.read("bin/ctowerd").contains("new"));
}

#[test]
fn update_recusa_com_sessao_viva_sem_force() {
    let u = Upd::new();
    std::fs::write(
        u.path("status.json"),
        r#"{"version":"0.1.0","interrupted":0,"live_sessions":[{"id":"s-1","state":"running","cwd":"/w/x"}]}"#,
    )
    .unwrap();
    u.cmd(&[])
        .failure()
        .stderr(contains("Recusado: 1 sessão(ões) viva(s)"))
        .stderr(contains("ctower update --force"));
    assert!(!u.path("data/update/state.json").exists());

    // Com --force segue; o daemon novo não sobe e volta sozinho.
    std::fs::write(u.path("after-start.json"), health(&u.c1)).unwrap();
    u.cmd(&["--force"])
        .failure()
        .stderr(contains("Falhou e voltou à versão anterior"));
    assert!(u.read("bin/ctowerd").contains("old"));
}
