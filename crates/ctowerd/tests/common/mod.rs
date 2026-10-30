//! Hub de verdade (`cthubd`, em processo) para os testes que sobem o
//! `ctowerd`: sem hub o ambiente não tem projetos nem inicia sessões (#178).

// Cada teste usa só parte destes auxiliares.
#![allow(dead_code)]

use ct_ledger::{Hub, NewHubProject, SharedHub};
use std::path::Path;
use std::process::Command;

/// Hub no ar até o fim do processo de teste (numa thread própria, com
/// runtime próprio: serve tanto a testes síncronos quanto aos `tokio::test`).
pub struct TestHub {
    pub url: String,
}

/// Sobe um hub com `owner/repo` cadastrado e atribuído ao ambiente de
/// `config_dir`, com o clone em `repo_path` (que ganha o `origin` do
/// GitHub, para a validação da atribuição). Grava `env_id` e `hub.token` em
/// `config_dir`; o chamador põe `hub_url = <url>` no `config.toml`.
pub fn hub_with_assignment(
    config_dir: &Path,
    owner: &str,
    repo: &str,
    repo_path: &Path,
) -> TestHub {
    std::fs::create_dir_all(config_dir).unwrap();
    let env_id = ct_sessions::ensure_env_id(&config_dir.join("env_id")).unwrap();
    let hub = Hub::open_in_memory().unwrap();
    hub.add_project(&NewHubProject {
        owner: owner.into(),
        repo: repo.into(),
        name: repo.into(),
        base_branch: "develop".into(),
        protected_branches: vec!["develop".into(), "main".into()],
    })
    .unwrap();
    let token = hub.add_bound_env_token("teste", &env_id).unwrap().token;
    hub.add_assignment(owner, repo, &env_id, Some(&repo_path.display().to_string()))
        .unwrap();
    std::fs::write(config_dir.join("hub.token"), &token).unwrap();
    assert!(Command::new("git")
        .arg("-C")
        .arg(repo_path)
        .args([
            "remote",
            "add",
            "origin",
            &format!("https://github.com/{owner}/{repo}.git"),
        ])
        .status()
        .unwrap()
        .success());

    // O listener fica preso desde já: sem corrida de porta.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let app = cthubd::router(cthubd::HubState::new(
        SharedHub::new(hub),
        "ui-do-hub",
        "hub",
        "hub-id",
    ));
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async move {
            let l = tokio::net::TcpListener::from_std(listener).unwrap();
            axum::serve(l, app).await.unwrap();
        });
    });
    TestHub { url }
}

/// Espera o pull do boot trazer `repo` para `projects.list` (via `ws`, a
/// chamada WS do teste) e devolve o projeto.
pub fn wait_project(
    ws: impl Fn(&str, serde_json::Value) -> serde_json::Value,
    repo: &str,
) -> serde_json::Value {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    loop {
        let list = ws("projects.list", serde_json::json!({}));
        if let Some(p) = list
            .as_array()
            .and_then(|a| a.iter().find(|p| p["gh_repo"] == repo))
        {
            return p.clone();
        }
        assert!(
            std::time::Instant::now() < deadline,
            "{repo} não chegou do hub: {list}"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

/// `ctowerd` filho, morto ao sair de escopo.
pub struct Daemon(pub std::process::Child);

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Porta livre agora (o listener é solto na volta).
pub fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// Sobe o `ctowerd` de `cmd()` (já com o ambiente, menos a porta) em
/// `port` e espera ele responder com o `ui.token` de `config_dir`. A porta
/// livre é escolhida antes do daemon subir, e outro teste em paralelo pode
/// pegá-la nesse meio-tempo ("Address already in use", #393): aí o daemon
/// sai, e a subida é refeita numa porta nova, gravada em `port`. O
/// `/status` com o token garante que quem responde é este daemon, e não o
/// do outro teste na mesma porta.
pub fn start_ctowerd(
    port: &std::cell::Cell<u16>,
    config_dir: &Path,
    cmd: impl Fn() -> Command,
) -> Daemon {
    use std::time::{Duration, Instant};
    for _ in 0..5 {
        let child = cmd()
            .env("CTOWER_PORT", port.get().to_string())
            .spawn()
            .unwrap();
        let mut d = Daemon(child);
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            if d.0.try_wait().unwrap().is_some() {
                break;
            }
            let ok = std::fs::read_to_string(config_dir.join("ui.token"))
                .ok()
                .is_some_and(|t| {
                    ureq::get(&format!("http://127.0.0.1:{}/status", port.get()))
                        .set("authorization", &format!("Bearer {}", t.trim()))
                        .call()
                        .is_ok()
                });
            if ok {
                return d;
            }
            assert!(Instant::now() < deadline, "ctowerd não subiu");
            std::thread::sleep(Duration::from_millis(50));
        }
        port.set(free_port());
    }
    panic!("ctowerd não subiu: porta sempre ocupada");
}
