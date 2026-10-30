//! Binário `cthubd`: tokens de sync e o serviço no ar recebendo um push.

use assert_cmd::Command;
use predicates::str::contains;
use serde_json::{json, Value};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::{Duration, Instant};

fn cthubd(dir: &Path) -> Command {
    let mut c = Command::cargo_bin("cthubd").unwrap();
    c.env("CTHUB_CONFIG_DIR", dir.join("config"))
        .env("CTHUB_DATA_DIR", dir.join("data"));
    c
}

#[test]
fn add_env_e_revoke_env() {
    let hub = tempfile::tempdir().unwrap();
    let out = cthubd(hub.path())
        .args(["add-env", "notebook"])
        .assert()
        .success()
        .stderr(contains("ctower hub set-token"))
        .get_output()
        .stdout
        .clone();
    let out = String::from_utf8(out).unwrap();
    // Duas linhas: o token e o env_id preso a ele (#195).
    let mut lines = out.lines();
    let token = lines.next().unwrap();
    let env_id = lines.next().unwrap();
    assert_eq!((token.len(), lines.next()), (64, None));
    assert!(uuid::Uuid::parse_str(env_id).is_ok());
    // Girar o token de um nome que existe mantém o env_id.
    let out2 = cthubd(hub.path())
        .args(["add-env", "notebook"])
        .output()
        .unwrap()
        .stdout;
    let out2 = String::from_utf8(out2).unwrap();
    let mut l2 = out2.lines();
    assert_ne!(l2.next().unwrap(), token);
    assert_eq!(l2.next().unwrap(), env_id);
    let db = hub.path().join("data/hub.db");
    assert!(db.exists());
    assert_eq!(
        std::fs::metadata(hub.path().join("data"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );

    cthubd(hub.path())
        .args(["revoke-env", "notebook"])
        .assert()
        .success();
    cthubd(hub.path())
        .args(["revoke-env", "notebook"])
        .assert()
        .failure()
        .stderr(contains("nenhum token"));
}

struct Daemon(std::process::Child);

/// Sobe o `cthubd` numa porta livre e espera o `/healthz`. `None` se ele
/// saiu antes (porta tomada por outro processo entre a escolha e o bind).
fn spawn_hub(p: &std::path::Path, token: &str, env_id: &str) -> Option<(Daemon, String, Value)> {
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let bin = assert_cmd::cargo::cargo_bin("cthubd");
    let mut d = Daemon(
        std::process::Command::new(bin)
            .env("CTHUB_CONFIG_DIR", p.join("config"))
            .env("CTHUB_DATA_DIR", p.join("data"))
            .env("CTHUB_PORT", port.to_string())
            .arg("serve")
            .spawn()
            .unwrap(),
    );
    let url = format!("http://127.0.0.1:{port}");
    let start = Instant::now();
    loop {
        if d.0.try_wait().unwrap().is_some() {
            return None;
        }
        if let Ok(r) = ureq::get(&format!("{url}/healthz")).call() {
            let health: Value = r.into_json().unwrap();
            // Outro serviço na porta não aceitaria o token deste hub.
            let mine = ureq::post(&format!("{url}/sync/push"))
                .set("Authorization", &format!("Bearer {token}"))
                .send_json(json!({ "env_id": env_id, "entries": [], "pending": 0 }))
                .is_ok();
            return mine.then_some((d, url, health));
        }
        assert!(
            start.elapsed() < Duration::from_secs(15),
            "cthubd não subiu"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// `cthubd` sobe sozinho (sem ctowerd nem sessões), aceita o push de um
/// ambiente com o token de sync e serve o `/ws` só com o token de UI.
#[test]
fn serve_recebe_push_e_cria_os_arquivos_proprios() {
    let tmp = tempfile::tempdir().unwrap();
    let p = tmp.path();
    std::fs::create_dir_all(p.join("config")).unwrap();
    std::fs::write(
        p.join("config/config.toml"),
        "name = \"hub-teste\"\ngh_sync_interval_secs = 0\nbackup_interval_hours = 1\n",
    )
    .unwrap();
    let out = cthubd(p).args(["add-env", "wsl"]).output().unwrap();
    let out = String::from_utf8(out.stdout).unwrap();
    let (token, env_id) = out.trim().split_once('\n').unwrap();

    // A porta é escolhida antes de o cthubd subir; se outro processo a
    // pegar nesse meio-tempo, o cthubd sai com erro de bind e a tentativa se
    // repete com outra porta. O token de sync confirma que quem responde é
    // este cthubd.
    let (_d, url, health) = (0..5)
        .find_map(|_| spawn_hub(p, token, env_id))
        .expect("cthubd não subiu em 5 tentativas");
    assert_eq!(health["role"], "hub");
    assert_eq!(health["protocol"], cthubd::PROTOCOL_VERSION);

    // Um env_id que não é o do token: 403, antes de qualquer gravação.
    let e = ureq::post(&format!("{url}/sync/push"))
        .set("Authorization", &format!("Bearer {token}"))
        .send_json(json!({ "env_id": "e1", "entries": [], "pending": 0 }))
        .unwrap_err();
    assert!(matches!(e, ureq::Error::Status(403, _)));
    // Lote vazio do ambiente: aceito.
    let r: Value = ureq::post(&format!("{url}/sync/push"))
        .set("Authorization", &format!("Bearer {token}"))
        .send_json(json!({ "env_id": env_id, "entries": [], "pending": 0 }))
        .unwrap()
        .into_json()
        .unwrap();
    assert_eq!(r["last_seq"], 0);
    let e = ureq::post(&format!("{url}/sync/push"))
        .set("Authorization", "Bearer errado")
        .send_json(json!({ "env_id": env_id, "entries": [] }))
        .unwrap_err();
    assert!(matches!(e, ureq::Error::Status(401, _)));

    // Arquivos próprios do hub: token de UI, identidade e o snapshot do boot.
    let ui = p.join("config/ui.token");
    assert_eq!(std::fs::read_to_string(&ui).unwrap().trim().len(), 64);
    assert_eq!(
        std::fs::metadata(&ui).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert!(p.join("config/hub_id").exists());
    let start = Instant::now();
    while std::fs::read_dir(p.join("data/backups"))
        .map(|d| d.count())
        .unwrap_or(0)
        == 0
    {
        assert!(
            start.elapsed() < Duration::from_secs(15),
            "sem snapshot no boot"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn config_com_chave_do_ctowerd_recusa_subir() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("config")).unwrap();
    std::fs::write(tmp.path().join("config/config.toml"), "hub = true\n").unwrap();
    cthubd(tmp.path())
        .env("CTHUB_PORT", "1")
        .arg("serve")
        .timeout(Duration::from_secs(15))
        .assert()
        .failure();
}

#[test]
fn web_revoke_all_grava_direto_no_hub_db() {
    let d = tempfile::tempdir().unwrap();
    cthubd(d.path())
        .args(["web", "revoke-all"])
        .assert()
        .success()
        .stdout(contains("0 sessão(ões) web ativa(s) revogada(s)"));
    assert!(d.path().join("data/hub.db").exists());
}

#[test]
fn web_com_proxy_pela_metade_recusa_subir() {
    let d = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(d.path().join("config")).unwrap();
    std::fs::write(
        d.path().join("config/config.toml"),
        "[web]\nenabled = true\n[web.cloudflare]\nteam_domain = \"eq.cloudflareaccess.com\"\naud = \"a\"\n",
    )
    .unwrap();
    let out = cthubd(d.path())
        .arg("serve")
        .env("CTHUB_PORT", "1")
        .timeout(Duration::from_secs(15))
        .assert()
        .failure()
        .get_output()
        .clone();
    let all = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        all.contains("[web.cloudflare]") && all.contains("emails"),
        "{all}"
    );
}
