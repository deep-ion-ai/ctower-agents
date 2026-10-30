use assert_cmd::Command;
use predicates::prelude::*;
use predicates::str::contains;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::process::Command as StdCommand;
use std::time::Duration;
use tempfile::TempDir;

fn ctower() -> Command {
    let mut c = Command::cargo_bin("ctower").unwrap();
    for v in [
        "CTOWER_SESSION",
        "CTOWER_URL",
        "CTOWER_TOKEN",
        "CTOWER_PROTECTED",
        "CTOWER_SPOOL_DIR",
        "CTOWER_GH",
        "CTOWER_SESSION_KIND",
        "CTOWER_ISSUE",
        "CTOWER_TRACKER",
        "CTOWER_ISSUE_KEY",
        "CLAUDE_CODE_BRIDGE_SESSION_ID",
    ] {
        c.env_remove(v);
    }
    c
}

fn git(dir: &Path, args: &[&str]) {
    let st = StdCommand::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(st.status.success(), "git {args:?}");
}

fn repo_on(branch: &str) -> TempDir {
    let d = TempDir::new().unwrap();
    git(d.path(), &["init", "-q", "-b", branch]);
    git(d.path(), &["config", "user.email", "t@t"]);
    git(d.path(), &["config", "user.name", "t"]);
    git(d.path(), &["commit", "-q", "--allow-empty", "-m", "init"]);
    d
}

fn bash_payload(command: &str, cwd: &Path) -> String {
    serde_json::json!({
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": { "command": command },
        "cwd": cwd,
    })
    .to_string()
}

fn spool_files(dir: &Path) -> Vec<String> {
    match std::fs::read_dir(dir) {
        Ok(rd) => rd
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".json") && !n.starts_with('.'))
            .collect(),
        Err(_) => Vec::new(),
    }
}

#[test]
fn sem_sessao_nunca_bloqueia() {
    let r = repo_on("wip/12-auth");
    ctower()
        .args(["hook", "pre-tool-use"])
        .write_stdin(bash_payload("git push origin develop", r.path()))
        .assert()
        .success();
}

#[test]
fn bloqueia_push_para_develop() {
    let r = repo_on("wip/12-auth");
    let spool = TempDir::new().unwrap();
    ctower()
        .args(["hook", "pre-tool-use"])
        .env("CTOWER_SESSION", "s1")
        .env("CTOWER_SPOOL_DIR", spool.path())
        .write_stdin(bash_payload("git push origin develop", r.path()))
        .assert()
        .code(2)
        .stderr(contains("push para a branch protegida `develop`"));
    assert_eq!(
        spool_files(spool.path()).len(),
        1,
        "evento GuardBlocked vai para o spool"
    );
}

#[test]
fn permite_push_da_branch_da_sessao() {
    let r = repo_on("wip/12-auth");
    let spool = TempDir::new().unwrap();
    ctower()
        .args(["hook", "pre-tool-use"])
        .env("CTOWER_SESSION", "s1")
        .env("CTOWER_SPOOL_DIR", spool.path())
        .write_stdin(bash_payload("git push -u origin wip/12-auth", r.path()))
        .assert()
        .success();
    assert!(spool_files(spool.path()).is_empty());
}

#[test]
fn bloqueia_commit_com_head_em_develop() {
    let r = repo_on("develop");
    let spool = TempDir::new().unwrap();
    ctower()
        .args(["hook", "pre-tool-use"])
        .env("CTOWER_SESSION", "s1")
        .env("CTOWER_SPOOL_DIR", spool.path())
        .write_stdin(bash_payload("git commit -m x", r.path()))
        .assert()
        .code(2);
}

#[test]
fn protegidas_vem_do_ambiente() {
    let r = repo_on("wip/12-auth");
    let spool = TempDir::new().unwrap();
    ctower()
        .args(["hook", "pre-tool-use"])
        .env("CTOWER_SESSION", "s1")
        .env("CTOWER_SPOOL_DIR", spool.path())
        .env("CTOWER_PROTECTED", "release, trunk")
        .write_stdin(bash_payload("git push origin develop", r.path()))
        .assert()
        .success();
    ctower()
        .args(["hook", "pre-tool-use"])
        .env("CTOWER_SESSION", "s1")
        .env("CTOWER_SPOOL_DIR", spool.path())
        .env("CTOWER_PROTECTED", "release, trunk")
        .write_stdin(bash_payload("git push origin trunk", r.path()))
        .assert()
        .code(2);
}

#[test]
fn gh_pr_merge_sem_base_conhecida_bloqueia() {
    let r = repo_on("wip/12-auth");
    let spool = TempDir::new().unwrap();
    ctower()
        .args(["hook", "pre-tool-use"])
        .env("CTOWER_SESSION", "s1")
        .env("CTOWER_SPOOL_DIR", spool.path())
        .env("CTOWER_GH", "false")
        .write_stdin(bash_payload("gh pr merge 5 --squash", r.path()))
        .assert()
        .code(2);
}

/// #350: `glab mr merge` é bloqueado sempre, sem consultar nada (o hook não
/// tem o token do GitLab) e sem variável da sessão.
#[test]
fn glab_mr_merge_sempre_bloqueia() {
    let r = repo_on("PROJ-5450");
    let spool = TempDir::new().unwrap();
    ctower()
        .args(["hook", "pre-tool-use"])
        .env("CTOWER_SESSION", "s1")
        .env("CTOWER_SPOOL_DIR", spool.path())
        .write_stdin(bash_payload(
            "GITLAB_TOKEN=\"$(pass show gitlab | head -1)\" glab mr merge 12 -R grupo/projeto/app",
            r.path(),
        ))
        .assert()
        .code(2)
        .stderr(contains("glab mr merge"));
}

#[test]
fn ferramenta_que_nao_e_bash_passa() {
    let spool = TempDir::new().unwrap();
    let payload =
        serde_json::json!({"tool_name": "Edit", "tool_input": {"file_path": "x"}}).to_string();
    ctower()
        .args(["hook", "pre-tool-use"])
        .env("CTOWER_SESSION", "s1")
        .env("CTOWER_SPOOL_DIR", spool.path())
        .write_stdin(payload)
        .assert()
        .success();
}

#[test]
fn stdin_invalido_nao_bloqueia() {
    let spool = TempDir::new().unwrap();
    for event in ["pre-tool-use", "stop", "session-start"] {
        ctower()
            .args(["hook", event])
            .env("CTOWER_SESSION", "s1")
            .env("CTOWER_SPOOL_DIR", spool.path())
            .write_stdin("isto não é json")
            .assert()
            .success();
    }
}

/// URL de uma porta local sem ninguém escutando: reserva uma porta efêmera e
/// solta o listener, então a conexão é recusada na hora. Uma porta fixa como
/// a 9 não serve: em alguns ambientes (WSL2) o SYN é descartado e a conexão
/// fica pendurada em vez de ser recusada.
fn url_sem_daemon() -> String {
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    format!("http://127.0.0.1:{port}")
}

#[test]
fn sem_daemon_vai_para_o_spool() {
    let spool = TempDir::new().unwrap();
    ctower()
        .args(["hook", "stop"])
        .env("CTOWER_SESSION", "s1")
        .env("CTOWER_URL", url_sem_daemon())
        .env("CTOWER_SPOOL_DIR", spool.path())
        .write_stdin(r#"{"session_id":"abc"}"#)
        .timeout(Duration::from_secs(10))
        .assert()
        .success();
    let files = spool_files(spool.path());
    assert_eq!(files.len(), 1);
    let body = std::fs::read_to_string(spool.path().join(&files[0])).unwrap();
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["event"], "Stop");
    assert_eq!(v["session"], "s1");
    assert_eq!(v["payload"]["session_id"], "abc");
    assert!(v.get("rc_session_id").is_none(), "{v}");
}

/// Com o Remote Control ligado, o id da sessão remota vai no envelope, fora
/// do `payload` (#197).
#[test]
fn rc_session_id_vai_no_envelope() {
    let spool = TempDir::new().unwrap();
    ctower()
        .args(["hook", "session-start"])
        .env("CTOWER_SESSION", "s1")
        .env("CTOWER_URL", url_sem_daemon())
        .env("CTOWER_SPOOL_DIR", spool.path())
        .env("CLAUDE_CODE_BRIDGE_SESSION_ID", "session_01Wf")
        .write_stdin(r#"{"session_id":"abc"}"#)
        .timeout(Duration::from_secs(10))
        .assert()
        .success();
    let files = spool_files(spool.path());
    let body = std::fs::read_to_string(spool.path().join(&files[0])).unwrap();
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["rc_session_id"], "session_01Wf");
    assert!(v["payload"].get("rc_session_id").is_none());
}

fn serve_once(listener: TcpListener) -> std::thread::JoinHandle<String> {
    serve_once_with(listener, "")
}

/// Como `serve_once`, respondendo `body` (JSON).
fn serve_once_with(listener: TcpListener, body: &str) -> std::thread::JoinHandle<String> {
    let body = body.to_string();
    std::thread::spawn(move || {
        let (mut s, _) = listener.accept().unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut buf = Vec::new();
        let mut chunk = [0u8; 4096];
        loop {
            let n = s.read(&mut chunk).unwrap_or(0);
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&chunk[..n]);
            let text = String::from_utf8_lossy(&buf).into_owned();
            if let Some(h) = text.find("\r\n\r\n") {
                let len = text[..h]
                    .lines()
                    .find_map(|l| {
                        l.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .map(|v| v.trim().parse::<usize>().unwrap_or(0))
                    })
                    .unwrap_or(0);
                if buf.len() >= h + 4 + len {
                    break;
                }
            }
        }
        let resp = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        s.write_all(resp.as_bytes()).unwrap();
        String::from_utf8_lossy(&buf).into_owned()
    })
}

#[test]
fn entrega_ao_daemon_com_token() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let server = serve_once(listener);
    let spool = TempDir::new().unwrap();
    ctower()
        .args(["hook", "notification"])
        .env("CTOWER_SESSION", "s1")
        .env("CTOWER_URL", &url)
        .env("CTOWER_TOKEN", "tok")
        .env("CTOWER_SPOOL_DIR", spool.path())
        .write_stdin(r#"{"message":"precisa de permissão"}"#)
        .assert()
        .success();
    let req = server.join().unwrap();
    assert!(req.starts_with("POST /hooks"), "{req}");
    assert!(req.contains("Bearer tok"), "{req}");
    assert!(req.contains(r#""event":"Notification""#), "{req}");
    assert!(spool_files(spool.path()).is_empty());
}

/// Roda `ctower hook permission-request` contra um daemon que responde
/// `answer`; devolve (requisição recebida, stdout).
fn permission(answer: &str, payload: serde_json::Value) -> (String, String) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let server = serve_once_with(listener, answer);
    let out = ctower()
        .args(["hook", "permission-request"])
        .env("CTOWER_SESSION", "s1")
        .env("CTOWER_URL", &url)
        .env("CTOWER_TOKEN", "tok")
        .write_stdin(payload.to_string())
        .timeout(Duration::from_secs(10))
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    (
        server.join().unwrap(),
        String::from_utf8_lossy(&out).into_owned(),
    )
}

#[test]
fn permissao_respondida_pela_ui_vira_a_decisao_do_hook() {
    let bash = serde_json::json!({
        "hook_event_name": "PermissionRequest",
        "tool_name": "Bash",
        "tool_input": { "command": "cargo test" },
    });
    let (req, out) = permission(r#"{"decision":"allow"}"#, bash.clone());
    assert!(req.starts_with("POST /hooks/permission"), "{req}");
    assert!(req.contains("Bearer tok") && req.contains(r#""event":"PermissionRequest""#));
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(
        v["hookSpecificOutput"]["hookEventName"],
        "PermissionRequest"
    );
    assert_eq!(v["hookSpecificOutput"]["decision"]["behavior"], "allow");

    let (_, out) = permission(r#"{"decision":"deny","message":null}"#, bash.clone());
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["hookSpecificOutput"]["decision"]["behavior"], "deny");
    assert!(v["hookSpecificOutput"]["decision"]["message"]
        .as_str()
        .unwrap()
        .contains("Negado"));

    // Sempre nesta sessão, sem sugestões: só o comando exato.
    let (_, out) = permission(r#"{"decision":"allow_session"}"#, bash);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    let up = &v["hookSpecificOutput"]["decision"]["updatedPermissions"][0];
    assert_eq!(up["destination"], "session");
    assert_eq!(up["rules"][0]["toolName"], "Bash");
    assert_eq!(up["rules"][0]["ruleContent"], "cargo test");

    // Com as sugestões do Claude Code: elas, com destino da sessão.
    let (_, out) = permission(
        r#"{"decision":"allow_session"}"#,
        serde_json::json!({
            "tool_name": "Edit",
            "permission_suggestions": [
                { "type": "setMode", "mode": "acceptEdits", "destination": "localSettings" }
            ],
        }),
    );
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    let up = &v["hookSpecificOutput"]["decision"]["updatedPermissions"];
    assert_eq!(
        up,
        &serde_json::json!([{ "type": "setMode", "mode": "acceptEdits", "destination": "session" }])
    );

    // Sem decisão: nada no stdout, o Claude Code pergunta no terminal.
    let (_, out) = permission(r#"{"decision":null}"#, serde_json::json!({}));
    assert_eq!(out.trim(), "");
}

#[test]
fn permissao_sem_daemon_nao_decide() {
    ctower()
        .args(["hook", "permission-request"])
        .env("CTOWER_SESSION", "s1")
        .env("CTOWER_URL", url_sem_daemon())
        .write_stdin("{}")
        .timeout(Duration::from_secs(10))
        .assert()
        .success()
        .stdout(predicate::str::is_empty());
}

#[test]
fn stdin_invalido_com_algo_a_entregar_vai_para_o_spool() {
    let spool = TempDir::new().unwrap();
    ctower()
        .args(["hook", "stop"])
        .env("CTOWER_SESSION", "s1")
        .env("CTOWER_SPOOL_DIR", spool.path())
        .write_stdin("isto não é json")
        .assert()
        .success();
    let files = spool_files(spool.path());
    assert_eq!(files.len(), 1);
    let body = std::fs::read_to_string(spool.path().join(&files[0])).unwrap();
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["event"], "Stop");
    assert!(v["payload"].is_null());
}

#[test]
fn bloqueia_git_dash_c_para_repo_em_develop() {
    let wip = repo_on("wip/12-auth");
    let dev = repo_on("develop");
    let spool = TempDir::new().unwrap();
    let cmd = format!("git -C {} commit -m x", dev.path().display());
    ctower()
        .args(["hook", "pre-tool-use"])
        .env("CTOWER_SESSION", "s1")
        .env("CTOWER_SPOOL_DIR", spool.path())
        .write_stdin(bash_payload(&cmd, wip.path()))
        .assert()
        .code(2)
        .stderr(contains(
            "`git commit` com HEAD na branch protegida `develop`",
        ));
}

#[test]
fn caminho_relativo_e_til_resolvem_a_partir_do_cwd_e_home() {
    let base = TempDir::new().unwrap();
    let wip = base.path().join("wip");
    let dev = base.path().join("dev");
    for (d, b) in [(&wip, "wip/12-auth"), (&dev, "develop")] {
        std::fs::create_dir(d).unwrap();
        git(d, &["init", "-q", "-b", b]);
        git(
            d,
            &[
                "-c",
                "user.email=t@t",
                "-c",
                "user.name=t",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "init",
            ],
        );
    }
    let spool = TempDir::new().unwrap();
    for cmd in [
        "git -C ../dev commit -m x",
        "cd ../dev && git commit -m x",
        "git -C ~/dev commit -m x",
    ] {
        ctower()
            .args(["hook", "pre-tool-use"])
            .env("CTOWER_SESSION", "s1")
            .env("CTOWER_SPOOL_DIR", spool.path())
            .env("HOME", base.path())
            .write_stdin(bash_payload(cmd, &wip))
            .assert()
            .code(2);
    }
    ctower()
        .args(["hook", "pre-tool-use"])
        .env("CTOWER_SESSION", "s1")
        .env("CTOWER_SPOOL_DIR", spool.path())
        .env("HOME", base.path())
        .write_stdin(bash_payload("git -C ~/wip commit -m x", &dev))
        .assert()
        .success();
}

#[test]
fn mensagem_de_bloqueio_nao_duplica_sufixo() {
    let r = repo_on("wip/12-auth");
    let spool = TempDir::new().unwrap();
    ctower()
        .args(["hook", "pre-tool-use"])
        .env("CTOWER_SESSION", "s1")
        .env("CTOWER_SPOOL_DIR", spool.path())
        .write_stdin(bash_payload("git push origin develop", r.path()))
        .assert()
        .code(2)
        .stderr(contains("(branch protegida)").not());
}

#[test]
fn gh_lento_estoura_prazo_e_bloqueia() {
    use std::os::unix::fs::PermissionsExt;
    let r = repo_on("wip/12-auth");
    let spool = TempDir::new().unwrap();
    let bin = TempDir::new().unwrap();
    let gh = bin.path().join("gh");
    std::fs::write(&gh, "#!/bin/sh\nsleep 30\n").unwrap();
    std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();
    let t0 = std::time::Instant::now();
    ctower()
        .args(["hook", "pre-tool-use"])
        .env("CTOWER_SESSION", "s1")
        .env("CTOWER_SPOOL_DIR", spool.path())
        .env("CTOWER_GH", &gh)
        .write_stdin(bash_payload("gh pr merge 5", r.path()))
        .timeout(Duration::from_secs(20))
        .assert()
        .code(2)
        .stderr(contains("não foi possível determinar a base"));
    assert!(
        t0.elapsed() < Duration::from_secs(7),
        "levou {:?}",
        t0.elapsed()
    );
}

#[test]
fn gh_recebe_ambiente_nao_interativo() {
    use std::os::unix::fs::PermissionsExt;
    let r = repo_on("wip/12-auth");
    let spool = TempDir::new().unwrap();
    let bin = TempDir::new().unwrap();
    let gh = bin.path().join("gh");
    // Só responde com uma base não protegida se o ambiente vier configurado.
    std::fs::write(
        &gh,
        "#!/bin/sh\n[ \"$GH_PROMPT_DISABLED\" = 1 ] && [ \"$GH_NO_UPDATE_NOTIFIER\" = 1 ] && echo feature && exit 0\nexit 1\n",
    )
    .unwrap();
    std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();
    ctower()
        .args(["hook", "pre-tool-use"])
        .env("CTOWER_SESSION", "s1")
        .env("CTOWER_SPOOL_DIR", spool.path())
        .env("CTOWER_GH", &gh)
        .write_stdin(bash_payload("gh pr merge 5", r.path()))
        .assert()
        .success();
}

#[test]
fn erro_de_argumento_sai_com_zero() {
    ctower()
        .args(["hook", "pre-tool-usee"])
        .write_stdin("{}")
        .assert()
        .code(0)
        .stderr(contains("pre-tool-usee"));
    ctower().assert().code(0);
}

#[test]
fn help_e_version_continuam_funcionando() {
    ctower()
        .arg("--help")
        .assert()
        .success()
        .stdout(contains("hook"));
    ctower()
        .arg("--version")
        .assert()
        .success()
        .stdout(contains("ctower"));
}

#[test]
fn spool_e_privado() {
    use std::os::unix::fs::PermissionsExt;
    let base = TempDir::new().unwrap();
    let spool = base.path().join("a/b/spool");
    ctower()
        .args(["hook", "stop"])
        .env("CTOWER_SESSION", "s1")
        .env("CTOWER_SPOOL_DIR", &spool)
        .write_stdin("{}")
        .assert()
        .success();
    let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&spool), 0o700);
    let files = spool_files(&spool);
    assert_eq!(files.len(), 1);
    assert_eq!(mode(&spool.join(&files[0])), 0o600);
}

#[test]
fn sem_home_nem_spool_dir_nao_grava() {
    let cwd = TempDir::new().unwrap();
    ctower()
        .args(["hook", "stop"])
        .env("CTOWER_SESSION", "s1")
        .env_remove("HOME")
        .current_dir(cwd.path())
        .write_stdin("{}")
        .assert()
        .success();
    assert_eq!(
        std::fs::read_dir(cwd.path()).unwrap().count(),
        0,
        "nada deve ser gravado no cwd"
    );
}

#[test]
fn protegidas_vazias_usam_o_padrao() {
    let r = repo_on("wip/12-auth");
    let spool = TempDir::new().unwrap();
    ctower()
        .args(["hook", "pre-tool-use"])
        .env("CTOWER_SESSION", "s1")
        .env("CTOWER_SPOOL_DIR", spool.path())
        .env("CTOWER_PROTECTED", " , ")
        .write_stdin(bash_payload("git push origin develop", r.path()))
        .assert()
        .code(2);
}

#[test]
fn til_com_nome_de_usuario() {
    let home = TempDir::new().unwrap();
    let dev = home.path().join("dev");
    std::fs::create_dir(&dev).unwrap();
    git(&dev, &["init", "-q", "-b", "develop"]);
    git(
        &dev,
        &[
            "-c",
            "user.email=t@t",
            "-c",
            "user.name=t",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "init",
        ],
    );
    let wip = repo_on("wip/12-auth");
    let spool = TempDir::new().unwrap();
    ctower()
        .args(["hook", "pre-tool-use"])
        .env("CTOWER_SESSION", "s1")
        .env("CTOWER_SPOOL_DIR", spool.path())
        .env("HOME", home.path())
        .env("USER", "fulano")
        .write_stdin(bash_payload("git -C ~fulano/dev commit -m x", wip.path()))
        .assert()
        .code(2);
    // Outro usuário: HEAD desconhecido, o guard permite.
    ctower()
        .args(["hook", "pre-tool-use"])
        .env("CTOWER_SESSION", "s1")
        .env("CTOWER_SPOOL_DIR", spool.path())
        .env("HOME", home.path())
        .env("USER", "fulano")
        .write_stdin(bash_payload("git -C ~beltrano/dev commit -m x", wip.path()))
        .assert()
        .success();
}

#[test]
fn gestor_bloqueia_escritas_e_tokens_e_pede_confirmacao_para_issue() {
    let r = repo_on("main");
    let spool = TempDir::new().unwrap();
    let run = |payload: String| {
        ctower()
            .args(["hook", "pre-tool-use"])
            .env("CTOWER_SESSION", "s-gestor")
            .env("CTOWER_SESSION_KIND", "gestor")
            .env("CTOWER_SPOOL_DIR", spool.path())
            .write_stdin(payload)
            .assert()
    };
    run(bash_payload("gh issue close 3 -R o/r", r.path()))
        .code(2)
        .stderr(contains("gh issue close"));
    run(bash_payload(
        "cat ~/.config/ctower/hub-agent.token",
        r.path(),
    ))
    .code(2);
    let read = serde_json::json!({
        "hook_event_name": "PreToolUse",
        "tool_name": "Read",
        "tool_input": { "file_path": "/home/u/.config/cthub/ui.token" },
    })
    .to_string();
    run(read).code(2).stderr(contains("ui.token"));
    run(bash_payload("gh issue create -R o/r -t x -b y", r.path()))
        .success()
        .stdout(contains("\"permissionDecision\":\"ask\""));
    run(bash_payload("gh issue view 3 -R o/r", r.path()))
        .success()
        .stdout(predicate::str::is_empty());
    // Fora do gestor, criar issue segue livre.
    ctower()
        .args(["hook", "pre-tool-use"])
        .env("CTOWER_SESSION", "s1")
        .env("CTOWER_SPOOL_DIR", spool.path())
        .write_stdin(bash_payload("gh issue create -R o/r -t x -b y", r.path()))
        .assert()
        .success()
        .stdout(predicate::str::is_empty());
}

#[test]
fn sessao_de_issue_exige_closes_no_pr_create() {
    let r = repo_on("fix/375-closes");
    std::fs::write(r.path().join("corpo.md"), "Corrige.\n\nCloses #375\n").unwrap();
    std::fs::write(r.path().join("ruim.md"), "Fecha #375\n").unwrap();
    let spool = TempDir::new().unwrap();
    let run = |cmd: &str, issue: Option<&str>| {
        let mut c = ctower();
        c.args(["hook", "pre-tool-use"])
            .env("CTOWER_SESSION", "s1")
            .env("CTOWER_SPOOL_DIR", spool.path())
            .write_stdin(bash_payload(cmd, r.path()));
        if let Some(n) = issue {
            c.env("CTOWER_ISSUE", n);
        }
        c.assert()
    };
    run("gh pr create -t x -b 'Fecha #375'", Some("375"))
        .code(2)
        .stderr(contains("sem `Closes #375`"))
        .stderr(contains("o merge na base é feito pelo humano").not());
    run("gh pr create -t x --body-file ruim.md", Some("375")).code(2);
    run("gh pr create -t x -b 'Closes #375'", Some("375")).success();
    run("gh pr create -t x --body-file corpo.md", Some("375")).success();
    // Fora de sessão de issue (épico, planejamento, gestor), não confere.
    run("gh pr create -t x -b 'Fecha #375'", None).success();
}

/// Perfil Jira (#351): com `CTOWER_TRACKER=jira`, transição e edição são
/// bloqueadas, criar issue pede confirmação e comentar passa; sem a
/// variável (projeto GitHub), nada muda.
#[test]
fn projeto_jira_bloqueia_transicao_e_pede_confirmacao_para_criar() {
    let r = repo_on("PROJ-5450");
    let spool = TempDir::new().unwrap();
    let token = "JIRA_API_TOKEN=\"$(pass show jira | head -1)\"";
    let run = |cmd: &str, jira: bool| {
        let mut c = ctower();
        c.args(["hook", "pre-tool-use"])
            .env("CTOWER_SESSION", "s-jira")
            .env("CTOWER_SPOOL_DIR", spool.path())
            .write_stdin(bash_payload(cmd, r.path()));
        if jira {
            c.env("CTOWER_TRACKER", "jira")
                .env("CTOWER_ISSUE_KEY", "PROJ-5450");
        }
        c.assert()
    };
    for cmd in [
        format!("{token} jira issue move PROJ-5450 'Em andamento'"),
        "jira issue transition PROJ-5450 Done".to_string(),
        "jira issue edit PROJ-5450 -s novo".to_string(),
        "jira issue delete PROJ-5450".to_string(),
        "jira issue assign PROJ-5450 fulano".to_string(),
        "jira issue link PROJ-5450 PROJ-1 Blocks".to_string(),
        "jira epic add PROJ-5014 PROJ-5450".to_string(),
    ] {
        run(&cmd, true)
            .code(2)
            .stderr(contains("transição de status"))
            .stderr(contains("o merge na base é feito pelo humano").not());
    }
    for cmd in [
        format!("{token} jira issue create -p PROJ -t Tarefa -s x -P PROJ-5014 --no-input"),
        "jira epic create -n x -s x --no-input".to_string(),
    ] {
        run(&cmd, true)
            .success()
            .stdout(contains("\"permissionDecision\":\"ask\""))
            .stdout(contains("ok do usuário"));
    }
    for cmd in [
        format!("{token} jira issue view PROJ-5450 --comments 20 --plain"),
        format!("{token} jira issue comment add PROJ-5450 'MR aberto: x' --no-input"),
    ] {
        run(&cmd, true).success().stdout(predicate::str::is_empty());
    }
    // Sem CTOWER_TRACKER (projeto GitHub), o perfil não se aplica.
    run("jira issue move PROJ-5450 Done", false)
        .success()
        .stdout(predicate::str::is_empty());
    // O push para a protegida continua com o complemento de branch.
    run("git push origin develop", true)
        .code(2)
        .stderr(contains("o merge na base é feito pelo humano"));
}
