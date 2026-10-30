//! `cthubd update` contra um repositório de origem local, `cargo` e
//! `systemctl` falsos e um hub falso (`/healthz` + `hub.info` no `/ws`).

use assert_cmd::Command;
use futures_util::{SinkExt, StreamExt};
use predicates::str::contains;
use std::path::{Path, PathBuf};
use tokio::io::AsyncWriteExt;
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::Message;

const UI_TOKEN: &str = "tok-ui";

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
    std::fs::create_dir_all(dir.join("ui")).unwrap();
    std::fs::write(dir.join("ui/package.json"), "{}\n").unwrap();
    git(dir, &["add", "."]);
    git(dir, &["commit", "-q", "--allow-empty", "-m", msg]);
    git(dir, &["rev-parse", "HEAD"])
}

fn script(path: &Path, body: &str) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn health(commit: &str) -> String {
    format!(r#"{{"ok":true,"role":"hub","version":"0.1.0","commit":"{commit}","protocol":"1.0"}}"#)
}

/// Hub falso: `/healthz` devolve `<dir>/health.json`; o `/ws` exige o token
/// de UI e responde `hub.info` (ou erro, se existir `<dir>/hubinfo-error`).
fn fake_hub(dir: PathBuf) -> u16 {
    let std_l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = std_l.local_addr().unwrap().port();
    std_l.set_nonblocking(true).unwrap();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async move {
            let l = tokio::net::TcpListener::from_std(std_l).unwrap();
            loop {
                let Ok((mut s, _)) = l.accept().await else {
                    continue;
                };
                let dir = dir.clone();
                tokio::spawn(async move {
                    let mut buf = [0u8; 16];
                    let n = s.peek(&mut buf).await.unwrap_or(0);
                    if buf[..n].starts_with(b"GET /ws") {
                        #[allow(clippy::result_large_err)] // assinatura do tungstenite
                        let check = |req: &Request, resp: Response| {
                            let ok = req
                                .headers()
                                .get("authorization")
                                .and_then(|v| v.to_str().ok())
                                == Some(&format!("Bearer {UI_TOKEN}"));
                            if ok {
                                Ok(resp)
                            } else {
                                let mut e = ErrorResponse::new(None);
                                *e.status_mut() = 401.try_into().unwrap();
                                Err(e)
                            }
                        };
                        let Ok(mut ws) = tokio_tungstenite::accept_hdr_async(s, check).await
                        else {
                            return;
                        };
                        // Um push antes da resposta, como o hub de verdade.
                        let _ = ws
                            .send(Message::Text(r#"{"event":"x","data":{}}"#.into()))
                            .await;
                        while let Some(Ok(Message::Text(t))) = ws.next().await {
                            let v: serde_json::Value = serde_json::from_str(&t).unwrap();
                            let reply = if dir.join("hubinfo-error").exists() {
                                serde_json::json!({ "id": v["id"], "error": { "code": -32000, "message": "hub.db ilegível" } })
                            } else {
                                let h: serde_json::Value = serde_json::from_str(
                                    &std::fs::read_to_string(dir.join("health.json")).unwrap_or_default(),
                                )
                                .unwrap_or_default();
                                let protocol = std::fs::read_to_string(dir.join("protocol"))
                                    .unwrap_or_else(|_| "1.0".into());
                                serde_json::json!({ "id": v["id"], "result": {
                                    "protocol": protocol.trim(), "name": "h", "config_rev": 1,
                                    "version": h["version"], "commit": h["commit"],
                                } })
                            };
                            let _ = ws.send(Message::Text(reply.to_string().into())).await;
                        }
                    } else {
                        let mut req = [0u8; 4096];
                        let _ = tokio::io::AsyncReadExt::read(&mut s, &mut req).await;
                        let body =
                            std::fs::read_to_string(dir.join("health.json")).unwrap_or_default();
                        let resp = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                            body.len()
                        );
                        let _ = s.write_all(resp.as_bytes()).await;
                    }
                });
            }
        });
    });
    port
}

/// Ambiente do atualizador: `cthubd` "old" em `bin/`, `cargo` e `systemctl`
/// falsos, `hub.db` real e o hub falso.
struct Upd {
    tmp: tempfile::TempDir,
    port: u16,
    c1: String,
    c2: String,
}

impl Upd {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path();
        let origin = p.join("origin");
        std::fs::create_dir_all(&origin).unwrap();
        git(&origin, &["init", "-q", "-b", "main"]);
        let c1 = commit(&origin, "0.1.0", "primeiro");
        let c2 = commit(&origin, "0.2.0", "segundo");
        std::fs::create_dir_all(p.join("config")).unwrap();
        std::fs::write(
            p.join("config/config.toml"),
            format!("update_repo = \"{}\"\n", origin.display()),
        )
        .unwrap();
        std::fs::write(p.join("config/ui.token"), UI_TOKEN).unwrap();
        std::fs::create_dir_all(p.join("bin")).unwrap();
        script(&p.join("bin/cthubd"), "echo old");
        script(
            &p.join("fake-cargo"),
            r#"[ -n "$FAKE_CARGO_FAIL" ] && { echo "error: compilation failed" >&2; exit 101; }
while [ $# -gt 0 ]; do case "$1" in --path) P="$2"; shift;; --root) R="$2"; shift;; esac; shift; done
echo "CTHUB_WEB_REQUIRED=$CTHUB_WEB_REQUIRED" >> "$T/cargo.log"
mkdir -p "$R/bin"; printf '#!/bin/sh\necho new %s\n' "$CTOWER_COMMIT" > "$R/bin/$(basename "$P")"; chmod +x "$R/bin/$(basename "$P")""#,
        );
        // Node e npm falsos: `node --version` lê `$T/node-version`.
        script(
            &p.join("fake-node"),
            r#"cat "$T/node-version" 2>/dev/null || echo v22.11.0"#,
        );
        script(
            &p.join("fake-npm"),
            r#"[ "$1" = "--version" ] && { echo 10.9.0; exit 0; }
echo "$(basename "$PWD") $@" >> "$T/npm.log"
[ -n "$FAKE_NPM_FAIL" ] && [ "$1" = "run" ] && { echo "vite: build falhou" >&2; exit 1; }
exit 0"#,
        );
        script(
            &p.join("fake-systemctl"),
            r#"echo "$@" >> "$T/systemctl.log"
case "$2" in
  restart) [ -f "$T/after-restart.json" ] && cp "$T/after-restart.json" "$T/health.json"
    [ -f "$T/migrate-on-restart" ] && python3 -c 'import sqlite3, sys; c = sqlite3.connect(sys.argv[1]); v = c.execute("PRAGMA user_version").fetchone()[0]; c.execute("INSERT INTO env_token (name, token_hash, created_at) VALUES (\x27nova\x27, \x27h\x27, 0)"); c.execute("PRAGMA user_version = %d" % (v + 1)); c.commit()' "$T/data/hub.db";;
  start) [ -f "$T/after-start.json" ] && cp "$T/after-start.json" "$T/health.json"; rm -f "$T/hubinfo-error";;
esac; exit 0"#,
        );
        std::fs::create_dir_all(p.join("data")).unwrap();
        ct_ledger::Hub::open(&p.join("data/hub.db"))
            .unwrap()
            .add_legacy_env_token("wsl")
            .unwrap();
        std::fs::write(p.join("health.json"), health(&c1)).unwrap();
        let port = fake_hub(p.to_path_buf());
        Upd { tmp, port, c1, c2 }
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.tmp.path().join(rel)
    }

    fn read(&self, rel: &str) -> String {
        std::fs::read_to_string(self.path(rel)).unwrap_or_default()
    }

    fn cmd(&self, args: &[&str]) -> Command {
        let p = self.tmp.path();
        let mut c = Command::cargo_bin("cthubd").unwrap();
        c.arg("update")
            .args(args)
            .env("CTHUB_CONFIG_DIR", p.join("config"))
            .env("CTHUB_DATA_DIR", p.join("data"))
            .env("CTHUB_PORT", self.port.to_string())
            .env("CTHUB_SERVICE_MANAGER", "systemd")
            .env("CTHUB_BIN_DIR", p.join("bin"))
            .env("CTHUB_CARGO", p.join("fake-cargo"))
            .env("CTHUB_NODE", p.join("fake-node"))
            .env("CTHUB_NPM", p.join("fake-npm"))
            .env("CTHUB_SYSTEMCTL", p.join("fake-systemctl"))
            .env("CTHUB_UPDATE_VERIFY_SECS", "2")
            .env("T", p);
        c
    }

    fn state(&self) -> serde_json::Value {
        serde_json::from_str(&self.read("data/update/state.json")).unwrap()
    }

    fn schema(&self) -> i64 {
        ct_ledger::db_user_version(&self.path("data/hub.db")).unwrap()
    }
}

#[test]
fn check_mostra_a_versao_disponivel() {
    let u = Upd::new();
    u.cmd(&["--check"])
        .assert()
        .success()
        .stdout(contains("instalado:  0.1.0"))
        .stdout(contains("disponível: 0.2.0"))
        .stdout(contains("1 commit(s) novo(s)"));
}

#[test]
fn atualiza_reinicia_e_volta_com_rollback() {
    let u = Upd::new();
    std::fs::write(u.path("after-restart.json"), health(&u.c2)).unwrap();
    u.cmd(&[])
        .assert()
        .success()
        .stdout(contains("Concluído: cthubd 0.2.0"));
    let st = u.state();
    assert_eq!(st["phase"], "done", "{st}");
    assert_eq!(st["from"]["commit"], u.c1.as_str());
    assert_eq!(st["to"]["commit"], u.c2.as_str());
    assert!(u.read("bin/cthubd").contains(&format!("new {}", u.c2)));
    assert!(u.read("data/update/prev/cthubd").contains("old"));
    // Bundle web antes do cargo, que roda exigindo o bundle (#199).
    assert_eq!(
        u.read("npm.log"),
        "ui ci --no-audit --no-fund\nui run build:web\n"
    );
    assert_eq!(u.read("cargo.log"), "CTHUB_WEB_REQUIRED=1\n");
    // Snapshot consistente (VACUUM INTO) do hub.db, com os tokens.
    let snap = ct_ledger::Hub::open(&u.path("data/update/db/hub.db")).unwrap();
    assert_eq!(snap.env_token_names().unwrap(), vec!["wsl"]);
    assert_eq!(u.read("systemctl.log"), "--user restart cthubd\n");

    // De novo: já em dia, sem build nem restart.
    u.cmd(&[])
        .assert()
        .success()
        .stdout(contains("já está em dia"));
    assert_eq!(u.read("systemctl.log").lines().count(), 1);

    // O hub novo aceitou dados (token "nova") e o esquema "migrou"; o
    // rollback manual volta o banco do snapshot só com --force.
    let epoch = ct_ledger::Hub::open(&u.path("data/update/db/hub.db"))
        .unwrap()
        .config_epoch()
        .unwrap();
    ct_ledger::Hub::open(&u.path("data/hub.db"))
        .unwrap()
        .add_legacy_env_token("nova")
        .unwrap();
    let v = u.schema();
    bump_schema(&u.path("data/hub.db"), v + 1);
    std::fs::write(u.path("after-start.json"), health(&u.c1)).unwrap();
    u.cmd(&["--rollback"])
        .assert()
        .failure()
        .stderr(contains("--force"))
        .stderr(contains("env_token +1"))
        .stderr(contains(format!("esquema {} → {v}", v + 1)));
    assert!(
        u.read("bin/cthubd").contains("new"),
        "sem --force não troca nada"
    );
    assert_eq!(u.schema(), v + 1);
    assert_eq!(u.read("systemctl.log").lines().count(), 1);

    u.cmd(&["--rollback", "--force"]).assert().success();
    let st = u.state();
    assert_eq!(
        (st["kind"].as_str(), st["phase"].as_str()),
        (Some("rollback"), Some("done"))
    );
    assert!(u.read("bin/cthubd").contains("old"));
    assert_eq!(u.schema(), v);
    assert!(u
        .read("systemctl.log")
        .ends_with("--user stop cthubd\n--user start cthubd\n"));
    // O banco em uso ficou numa cópia, com o que entrou depois do snapshot.
    let pre = pre_rollback(&u);
    assert_eq!(pre.len(), 1, "{pre:?}");
    bump_schema(&pre[0], v); // só para abrir sem recusar o esquema "futuro"
    let kept = ct_ledger::Hub::open(&pre[0]).unwrap();
    assert_eq!(kept.env_token_names().unwrap(), vec!["nova", "wsl"]);
    // O restaurado não tem o "nova" e tem outra época da configuração.
    let h = ct_ledger::Hub::open(&u.path("data/hub.db")).unwrap();
    assert_eq!(h.env_token_names().unwrap(), vec!["wsl"]);
    assert_ne!(h.config_epoch().unwrap(), epoch);
    let log = u.read("data/update/update.log");
    assert!(log.contains("ctower hub resync"), "{log}");
    assert!(log.contains("config_epoch renovada"), "{log}");
}

/// Cópias `hub.db.pre-rollback-*` no diretório de dados.
fn pre_rollback(u: &Upd) -> Vec<PathBuf> {
    let mut v: Vec<_> = std::fs::read_dir(u.path("data"))
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            let n = p.file_name().unwrap().to_string_lossy().into_owned();
            n.starts_with("hub.db.pre-rollback-") && !n.ends_with("-wal") && !n.ends_with("-shm")
        })
        .collect();
    v.sort();
    v
}

fn bump_schema(db: &Path, v: i64) {
    rusqlite::Connection::open(db)
        .unwrap()
        .pragma_update(None, "user_version", v)
        .unwrap();
}

#[test]
fn hub_que_nao_volta_com_o_commit_novo_e_revertido() {
    let u = Upd::new();
    // Sem after-restart.json o /healthz segue no commit antigo.
    std::fs::write(u.path("after-start.json"), health(&u.c1)).unwrap();
    u.cmd(&[])
        .assert()
        .failure()
        .stderr(contains("Falhou e voltou à versão anterior"));
    let st = u.state();
    assert_eq!(st["phase"], "rolled_back", "{st}");
    assert!(
        st["error"].as_str().unwrap().contains("respondeu com"),
        "{st}"
    );
    assert!(u.read("bin/cthubd").contains("old"));
    assert_eq!(
        u.read("systemctl.log"),
        "--user restart cthubd\n--user stop cthubd\n--user start cthubd\n"
    );
}

#[test]
fn hub_info_falho_depois_do_restart_e_revertido() {
    let u = Upd::new();
    // /healthz responde o commit novo, mas o hub.info falha (ex.: hub.db
    // ilegível): não está saudável. O start da versão antiga volta ao normal.
    std::fs::write(u.path("after-restart.json"), health(&u.c2)).unwrap();
    std::fs::write(u.path("after-start.json"), health(&u.c1)).unwrap();
    std::fs::write(u.path("hubinfo-error"), "").unwrap();
    u.cmd(&[]).assert().failure();
    let st = u.state();
    assert_eq!(st["phase"], "rolled_back", "{st}");
    let err = st["error"].as_str().unwrap();
    assert!(err.contains("não ficou saudável"), "{err}");
    assert!(err.contains("hub.info falhou"), "{err}");
    assert!(u.read("bin/cthubd").contains("old"));
    assert!(u.path("data/update/db/hub.db").is_file());
}

#[test]
fn token_de_ui_errado_conta_como_doente() {
    let u = Upd::new();
    std::fs::write(u.path("after-restart.json"), health(&u.c2)).unwrap();
    std::fs::write(u.path("after-start.json"), health(&u.c1)).unwrap();
    std::fs::write(u.path("config/ui.token"), "outro").unwrap();
    u.cmd(&[]).assert().failure();
    assert_eq!(u.state()["phase"], "rolled_back");
    assert!(u.read("bin/cthubd").contains("old"));
}

#[test]
fn build_falho_nao_toca_em_nada() {
    let u = Upd::new();
    u.cmd(&[]).env("FAKE_CARGO_FAIL", "1").assert().failure();
    let st = u.state();
    assert_eq!(st["phase"], "failed");
    assert!(st["log_tail"]
        .as_array()
        .unwrap()
        .iter()
        .any(|l| l.as_str().unwrap().contains("compilation failed")));
    assert!(u.read("bin/cthubd").contains("old"));
    assert!(!u.path("systemctl.log").exists());
    assert!(!u.path("data/update/prev").exists());
}

#[test]
fn rollback_sem_versao_anterior_falha() {
    let u = Upd::new();
    u.cmd(&["--rollback"])
        .assert()
        .failure()
        .stderr(contains("nenhuma versão anterior"));
}

/// A sonda do atualizador contra o router de verdade do cthubd.
#[test]
fn sonda_contra_o_hub_de_verdade() {
    let tmp = tempfile::tempdir().unwrap();
    let hub = ct_ledger::SharedHub::new(ct_ledger::Hub::open(&tmp.path().join("hub.db")).unwrap());
    let state = cthubd::HubState::new(hub, UI_TOKEN, "h", "hub-id");
    let std_l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = std_l.local_addr().unwrap().port();
    std_l.set_nonblocking(true).unwrap();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async move {
            let l = tokio::net::TcpListener::from_std(std_l).unwrap();
            axum::serve(l, cthubd::router(state)).await.unwrap();
        });
    });
    let b = cthubd::update::healthz(port).unwrap();
    assert_eq!(b.version, ct_core::VERSION);
    let info = cthubd::update::hub_info(port, UI_TOKEN).unwrap();
    assert_eq!(info["name"], "h");
    let e = cthubd::update::hub_info(port, "errado").unwrap_err();
    assert!(e.contains("hub.info falhou"), "{e}");
}

/// O hub novo migrou o esquema e aceitou dados, mas não ficou saudável: o
/// rollback automático volta o hub.db do snapshot, guarda o atual numa
/// cópia e renova a época, sem pedir --force.
#[test]
fn rollback_automatico_guarda_o_banco_e_renova_a_epoca() {
    let u = Upd::new();
    let v = u.schema();
    std::fs::write(u.path("after-restart.json"), health(&u.c2)).unwrap();
    std::fs::write(u.path("after-start.json"), health(&u.c1)).unwrap();
    std::fs::write(u.path("hubinfo-error"), "").unwrap();
    std::fs::write(u.path("migrate-on-restart"), "").unwrap();
    u.cmd(&[]).assert().failure();
    let st = u.state();
    assert_eq!(st["phase"], "rolled_back", "{st}");
    assert_eq!(u.schema(), v);
    let pre = pre_rollback(&u);
    assert_eq!(pre.len(), 1, "{pre:?}");
    assert_eq!(ct_ledger::db_user_version(&pre[0]).unwrap(), v + 1);
    let log = u.read("data/update/update.log");
    assert!(log.contains("ctower hub resync"), "{log}");
    assert!(log.contains("config_epoch renovada"), "{log}");
    let h = ct_ledger::Hub::open(&u.path("data/hub.db")).unwrap();
    assert_eq!(h.env_token_names().unwrap(), vec!["wsl"]);
}

/// Um release com a major do protocolo do hub nova passa na sonda.
#[test]
fn protocolo_novo_do_hub_passa_na_sonda() {
    let u = Upd::new();
    std::fs::write(u.path("after-restart.json"), health(&u.c2)).unwrap();
    std::fs::write(u.path("protocol"), "2.0").unwrap();
    u.cmd(&[]).assert().success();
    assert_eq!(u.state()["phase"], "done");
    // Protocolo ilegível não passa.
    let u = Upd::new();
    std::fs::write(u.path("after-restart.json"), health(&u.c2)).unwrap();
    std::fs::write(u.path("after-start.json"), health(&u.c1)).unwrap();
    std::fs::write(u.path("protocol"), "x").unwrap();
    u.cmd(&[]).assert().failure();
    let st = u.state();
    assert!(
        st["error"].as_str().unwrap().contains("protocolo inválido"),
        "{st}"
    );
}

/// Sem Node (ou com um antigo, ou sem npm) o update falha antes do fetch,
/// do backup e da troca do binário, com mensagem clara (#199).
#[test]
fn sem_node_falha_antes_de_trocar_o_binario() {
    let u = Upd::new();
    std::fs::write(u.path("after-restart.json"), health(&u.c2)).unwrap();
    u.cmd(&[])
        .env("CTHUB_NODE", u.path("nao-existe/node"))
        .assert()
        .failure()
        .stderr(contains("Node não encontrado"))
        .stderr(contains("Nada foi alterado"));
    let st = u.state();
    assert_eq!(st["phase"], "failed", "{st}");
    assert!(
        st["error"].as_str().unwrap().contains("docs/hub.md"),
        "{st}"
    );
    assert!(u.read("bin/cthubd").contains("old"));
    assert!(!u.path("data/update/prev").exists());
    assert!(!u.path("data/update/src").exists(), "nem buscou o fonte");
    assert!(!u.path("npm.log").exists());
    assert!(!u.path("cargo.log").exists());
    assert!(!u.path("systemctl.log").exists());

    std::fs::write(u.path("node-version"), "v18.19.0\n").unwrap();
    u.cmd(&[])
        .assert()
        .failure()
        .stderr(contains("Node v18.19.0 é antigo demais"));
    std::fs::remove_file(u.path("node-version")).unwrap();
    u.cmd(&[])
        .env("CTHUB_NPM", u.path("nao-existe/npm"))
        .assert()
        .failure()
        .stderr(contains("npm não encontrado"));
    assert!(u.read("bin/cthubd").contains("old"));
    assert!(!u.path("cargo.log").exists());
}

/// O build web que falha não chega ao cargo nem toca no binário.
#[test]
fn build_web_falho_nao_toca_em_nada() {
    let u = Upd::new();
    u.cmd(&[]).env("FAKE_NPM_FAIL", "1").assert().failure();
    let st = u.state();
    assert_eq!(st["phase"], "failed", "{st}");
    assert!(st["error"].as_str().unwrap().contains("build:web"), "{st}");
    assert!(u.read("bin/cthubd").contains("old"));
    assert!(!u.path("cargo.log").exists());
    assert!(!u.path("data/update/prev").exists());
}
