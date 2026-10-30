//! `ctower clean` contra um `/cleanup/plan` e `/cleanup/run` falsos.

use assert_cmd::Command;
use predicates::str::contains;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

/// Pedidos recebidos: (`caminho`, corpo).
type Seen = Arc<Mutex<Vec<(String, String)>>>;

/// Servidor HTTP mínimo: responde por caminho e guarda os pedidos.
fn fake_daemon(routes: Vec<(&'static str, &'static str, String)>) -> (u16, Seen) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&seen);
    std::thread::spawn(move || {
        for s in l.incoming().flatten() {
            let mut r = BufReader::new(s);
            let mut first = String::new();
            r.read_line(&mut first).unwrap();
            let path = first.split_whitespace().nth(1).unwrap_or("").to_string();
            let mut len = 0usize;
            loop {
                let mut h = String::new();
                r.read_line(&mut h).unwrap();
                if h.trim().is_empty() {
                    break;
                }
                if let Some(v) = h.to_ascii_lowercase().strip_prefix("content-length:") {
                    len = v.trim().parse().unwrap_or(0);
                }
            }
            let mut body = vec![0u8; len];
            r.read_exact(&mut body).unwrap();
            log.lock()
                .unwrap()
                .push((path.clone(), String::from_utf8_lossy(&body).into_owned()));
            let (status, resp) = routes
                .iter()
                .find(|(p, _, _)| *p == path)
                .map(|(_, s, b)| (*s, b.clone()))
                .unwrap_or(("404 Not Found", "rota desconhecida".into()));
            let mut s = r.into_inner();
            let _ = write!(
                s,
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{resp}",
                resp.len()
            );
        }
    });
    (port, seen)
}

fn ctower(cfg: &std::path::Path, port: u16) -> Command {
    std::fs::write(cfg.join("ui.token"), "t").unwrap();
    let mut c = Command::cargo_bin("ctower").unwrap();
    c.env("CTOWER_CONFIG_DIR", cfg)
        .env("CTOWER_DATA_DIR", cfg.join("data"))
        .env("CTOWER_PORT", port.to_string())
        .arg("clean");
    c
}

fn step(kind: &str, state: &str, reason: Option<&str>, bytes: u64, targets: &[&str]) -> String {
    serde_json::json!({
        "kind": kind, "state": state, "reason": reason, "bytes": bytes, "targets": targets
    })
    .to_string()
}

fn item(id: i64, repo: &str, n: i64, steps: &[String]) -> String {
    format!(
        r#"{{"work_item_id":{id},"project_id":1,"owner":"deep-ion-ai","repo":"{repo}","gh_number":{n},
            "title":"Issue {n}","branch":"feat/{n}-x","worktree_path":"/w/feat__{n}-x",
            "completion":{{"signal":"pr_mesclado","since":1,"pr":9}},"steps":[{}]}}"#,
        steps.join(",")
    )
}

/// Plano com #7 (worktree pendente de 2 GB, branch bloqueada) e #8 (tudo
/// bloqueado).
fn plan() -> String {
    format!(
        r#"{{"checked_at":1,"pending_items":1,"pending_bytes":2147483648,"items":[{},{}]}}"#,
        item(
            11,
            "demo",
            7,
            &[
                step("sessoes", "feita", None, 0, &[]),
                step("worktree", "pendente", None, 2 << 30, &["/w/feat__7-x"]),
                step(
                    "branch_local",
                    "bloqueada",
                    Some("aguarda a etapa worktree"),
                    0,
                    &[]
                ),
            ]
        ),
        item(
            12,
            "outro",
            8,
            &[step(
                "sessoes",
                "bloqueada",
                Some("sessão viva (issue concluída, sessão ainda aberta)"),
                0,
                &[]
            )]
        ),
    )
}

#[test]
fn dry_run_lista_pendentes_e_bloqueados_com_o_motivo() {
    let d = tempfile::tempdir().unwrap();
    let (port, seen) = fake_daemon(vec![("/cleanup/plan", "200 OK", plan())]);
    ctower(d.path(), port)
        .arg("--dry-run")
        .assert()
        .success()
        .stdout(contains("deep-ion-ai/demo#7"))
        .stdout(contains("pendente"))
        .stdout(contains("2,0 GB"))
        .stdout(contains("/w/feat__7-x"))
        .stdout(contains("aguarda a etapa worktree"))
        .stdout(contains("deep-ion-ai/outro#8"))
        .stdout(contains("sessão viva"))
        .stdout(contains("1 limpeza(s) pendente(s) · liberaria 2,0 GB"))
        .stdout(contains("nada foi apagado"));
    // Só o plano: nenhum /cleanup/run.
    let seen = seen.lock().unwrap();
    assert!(seen.iter().all(|(p, _)| p == "/cleanup/plan"), "{seen:?}");
}

#[test]
fn item_resolve_o_work_item_e_executa() {
    let d = tempfile::tempdir().unwrap();
    let run = r#"{"started_at":1,"finished_at":2,"dry_run":false,
        "scope":{"work_item_id":11,"step":null},"freed_bytes":2147483648,
        "items":[{"work_item_id":11,"owner":"deep-ion-ai","repo":"demo","gh_number":7,"steps":[
          {"kind":"worktree","result":"feita","reason":null,"bytes":2147483648,
           "removed":["/w/feat__7-x"],"kept":[]},
          {"kind":"branch_local","result":"bloqueada","reason":"commit não enviado",
           "bytes":0,"removed":[],"kept":[]}]}]}"#;
    let (port, seen) = fake_daemon(vec![
        ("/cleanup/plan", "200 OK", plan()),
        ("/cleanup/run", "200 OK", run.into()),
    ]);
    ctower(d.path(), port)
        .args(["--item", "7"])
        .assert()
        .success()
        .stdout(contains("feita"))
        .stdout(contains("commit não enviado"))
        .stdout(contains(
            "1 etapa(s) feita(s), 1 bloqueada(s), 0 com falha · liberado: 2,0 GB",
        ));
    let seen = seen.lock().unwrap();
    let (_, body) = seen.iter().find(|(p, _)| p == "/cleanup/run").unwrap();
    let body: serde_json::Value = serde_json::from_str(body).unwrap();
    assert_eq!(body["work_item_id"], 11);
    assert!(body.get("dry_run").is_none());
}

#[test]
fn item_fora_do_plano_falha() {
    let d = tempfile::tempdir().unwrap();
    let (port, _) = fake_daemon(vec![("/cleanup/plan", "200 OK", plan())]);
    ctower(d.path(), port)
        .args(["--item", "99"])
        .assert()
        .code(1)
        .stderr(contains("#99 não está no plano de limpeza"));
}

#[test]
fn falha_numa_etapa_sai_com_1() {
    let d = tempfile::tempdir().unwrap();
    let run = r#"{"started_at":1,"finished_at":2,"dry_run":false,
        "scope":{"work_item_id":null,"step":null},"freed_bytes":0,
        "items":[{"work_item_id":11,"owner":"deep-ion-ai","repo":"demo","gh_number":7,"steps":[
          {"kind":"worktree","result":"falhou","reason":"git worktree remove recusou",
           "bytes":0,"removed":[],"kept":[]}]}]}"#;
    let (port, _) = fake_daemon(vec![("/cleanup/run", "200 OK", run.into())]);
    ctower(d.path(), port)
        .assert()
        .code(1)
        .stdout(contains("git worktree remove recusou"))
        .stderr(contains("alguma etapa falhou"));
}

#[test]
fn nada_a_limpar() {
    let d = tempfile::tempdir().unwrap();
    let run = r#"{"started_at":1,"finished_at":2,"dry_run":false,
        "scope":{"work_item_id":null,"step":null},"freed_bytes":0,"items":[]}"#;
    let (port, _) = fake_daemon(vec![("/cleanup/run", "200 OK", run.into())]);
    ctower(d.path(), port)
        .assert()
        .success()
        .stdout(contains("nada a limpar"));
}

#[test]
fn daemon_fora_do_ar_falha() {
    let d = tempfile::tempdir().unwrap();
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    ctower(d.path(), port)
        .arg("--dry-run")
        .assert()
        .code(1)
        .stderr(contains("fora do ar"));
}
