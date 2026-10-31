//! Limpeza após a conclusão do work item (issue #287), ponta a ponta: o
//! `ctowerd` de verdade monta o plano no boot sem apagar nada
//! (`cleanup.auto = false`), e o acionamento manual (`cleanup.plan`,
//! `cleanup.run`, `POST /cleanup/*`) responde mesmo com o job desligado.

use common::Daemon;
use ct_core::{WorkItemKind, WorkItemStatus};
use ct_ledger::{Ledger, NewProject};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;

mod common;

const T: Duration = Duration::from_secs(20);

struct Env {
    tmp: tempfile::TempDir,
    /// Porta do daemon; troca se outro teste a pegou antes (ver
    /// `common::start_ctowerd`).
    port: Cell<u16>,
    /// Worktree do work item concluído (#7), que nada pode apagar.
    worktree: PathBuf,
}

impl Env {
    /// Ledger com o projeto local `deep-ion-ai/demo` e o work item #7
    /// concluído, com worktree no disco; `cleanup` vai no `config.toml`.
    fn new(cleanup: &str) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path();
        let dirs = ct_sessions::Dirs {
            config: p.join("config"),
            data: p.join("data"),
        };
        dirs.ensure().unwrap();
        let env_id = ct_sessions::ensure_env_id(&dirs.env_id_file()).unwrap();
        let worktree = p.join("wt/demo/feat__7-x");
        std::fs::create_dir_all(worktree.join("target")).unwrap();
        std::fs::write(worktree.join("target/lixo.bin"), vec![0u8; 4096]).unwrap();
        {
            let l = Ledger::open(&dirs.ledger_db(), &env_id).unwrap();
            let project = l
                .add_project(&NewProject {
                    name: "demo".into(),
                    gh_owner: "deep-ion-ai".into(),
                    gh_repo: "demo".into(),
                    repo_path: p.join("repo").display().to_string(),
                    base_branch: "develop".into(),
                    protected_branches: vec!["develop".into()],
                })
                .unwrap();
            let wi = l
                .ensure_work_item(project.id, 7, WorkItemKind::Issue)
                .unwrap();
            l.set_work_item_branch(wi.id, "feat/7-x", &worktree.display().to_string())
                .unwrap();
            l.set_work_item_status(wi.id, WorkItemStatus::Concluido, "teste")
                .unwrap();
            // Um work item em andamento não entra no plano.
            l.ensure_work_item(project.id, 8, WorkItemKind::Issue)
                .unwrap();
        }
        std::fs::write(
            dirs.config.join("config.toml"),
            format!(
                "claude_bin = \"true\"\nctower_bin = \"true\"\nworktrees_root = \"{}\"\n\
                 gh_sync_interval_secs = 0\nupdate_check_interval_hours = 0\n{cleanup}",
                p.join("wt").display()
            ),
        )
        .unwrap();
        Env {
            tmp,
            port: Cell::new(common::free_port()),
            worktree,
        }
    }

    /// Sessão do work item #7 no worktree, com o transcript do agente em
    /// `<tmp>/claude/projects/<slug>/u-7.jsonl`. Sem PID: o boot a marca
    /// `interrupted`. Devolve o id e a pasta do transcript.
    fn seed_session(&self) -> (String, PathBuf) {
        let dirs = ct_sessions::Dirs {
            config: self.path("config"),
            data: self.path("data"),
        };
        let env_id = ct_sessions::ensure_env_id(&dirs.env_id_file()).unwrap();
        let l = Ledger::open(&dirs.ledger_db(), &env_id).unwrap();
        let cwd = self.worktree.display().to_string();
        let dir = self
            .path("claude/projects")
            .join(ct_sessions::cleanup::claude_project_slug(&cwd));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("u-7.jsonl"), "{}\n").unwrap();
        let project = l.list_all_projects().unwrap().remove(0);
        let wi = l
            .ensure_work_item(project.id, 7, WorkItemKind::Issue)
            .unwrap();
        let s = l
            .create_session(&ct_ledger::NewSession {
                work_item_id: wi.id,
                tool: ct_core::Tool::Claude,
                cwd,
                launch_cmd: vec!["claude".into()],
                model: None,
                mode: ct_core::SessionMode::Native,
            })
            .unwrap();
        l.set_session_tool_info(&s.id, "u-7", &dir.join("u-7.jsonl").display().to_string())
            .unwrap();
        (s.id, dir)
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.tmp.path().join(rel)
    }

    fn start(&self) -> Daemon {
        self.start_with(&[])
    }

    /// Como [`Env::start`], com variáveis a mais no ambiente do daemon.
    fn start_with(&self, extra: &[(&str, String)]) -> Daemon {
        common::start_ctowerd(&self.port, &self.path("config"), || {
            let mut c = Command::new(env!("CARGO_BIN_EXE_ctowerd"));
            c.envs(extra.iter().map(|(k, v)| (*k, v.as_str())));
            c.env("CTOWER_CONFIG_DIR", self.path("config"))
                .env("CTOWER_DATA_DIR", self.path("data"))
                .env("CLAUDE_CONFIG_DIR", self.path("claude"))
                .env("RUST_LOG", "warn");
            c
        })
    }

    fn token(&self) -> String {
        std::fs::read_to_string(self.path("config/ui.token"))
            .unwrap()
            .trim()
            .to_string()
    }

    fn ws(&self, method: &str, params: Value) -> Value {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let mut req = format!("ws://127.0.0.1:{}/ws", self.port.get())
                .into_client_request()
                .unwrap();
            req.headers_mut().insert(
                "authorization",
                format!("Bearer {}", self.token()).parse().unwrap(),
            );
            let (mut ws, _) = tokio_tungstenite::connect_async(req).await.unwrap();
            let body = json!({ "id": 1, "method": method, "params": params });
            ws.send(Message::Text(body.to_string().into()))
                .await
                .unwrap();
            loop {
                let m = tokio::time::timeout(T, ws.next())
                    .await
                    .unwrap()
                    .unwrap()
                    .unwrap();
                if let Message::Text(t) = m {
                    let v: Value = serde_json::from_str(&t).unwrap();
                    if v["id"] == 1 {
                        assert!(v.get("error").is_none(), "{method}: {v}");
                        return v["result"].clone();
                    }
                }
            }
        })
    }

    fn post(&self, path: &str, body: Value) -> Value {
        ureq::post(&format!("http://127.0.0.1:{}{path}", self.port.get()))
            .set("Authorization", &format!("Bearer {}", self.token()))
            .send_json(body)
            .unwrap()
            .into_json()
            .unwrap()
    }
}

fn assert_intacto(worktree: &Path) {
    assert!(
        worktree.join("target/lixo.bin").exists(),
        "a limpeza apagou o worktree"
    );
}

/// Critério de aceite do #287: no boot o job monta o plano com o work item
/// concluído e as etapas bloqueadas com o motivo, e nada é apagado.
#[test]
fn boot_monta_o_plano_sem_apagar_nada() {
    let env = Env::new("");
    let _d = env.start();
    let deadline = Instant::now() + T;
    let plan = loop {
        let st = env.ws("cleanup.status", json!({}));
        if !st["plan"].is_null() {
            assert_eq!(st["config"]["auto"], false);
            assert_eq!(st["config"]["check_interval_hours"], 6);
            break st["plan"].clone();
        }
        assert!(
            Instant::now() < deadline,
            "o job não montou o plano no boot: {st}"
        );
        std::thread::sleep(Duration::from_millis(50));
    };
    let items = plan["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "{plan}");
    let it = &items[0];
    assert_eq!(
        (it["gh_number"].as_i64(), it["owner"].as_str()),
        (Some(7), Some("deep-ion-ai"))
    );
    assert_eq!(it["completion"]["signal"], "work_item_concluido");
    let steps = it["steps"].as_array().unwrap();
    let kinds: Vec<&str> = steps.iter().map(|s| s["kind"].as_str().unwrap()).collect();
    assert_eq!(
        kinds,
        [
            "sessoes",
            "worktree",
            "target",
            "branch_local",
            "branch_remota"
        ]
    );
    // Sem sessões, a etapa "sessões" (#285) não tem o que fazer; worktree e
    // `target/` ficam bloqueados; sem o clone, não há branch local nem
    // referência remota a apagar (#286).
    assert_eq!(steps[0]["state"], "feita", "{plan}");
    assert!(
        steps[1..3].iter().all(|s| s["state"] == "bloqueada"),
        "{plan}"
    );
    assert!(steps[3..].iter().all(|s| s["state"] == "feita"), "{plan}");
    // O worktree do teste não é um worktree de um clone de verdade: fica,
    // com o motivo (#265).
    let reason = steps[1]["reason"].as_str().unwrap();
    assert!(reason.contains("clone do projeto não encontrado"), "{plan}");
    assert_eq!(plan["pending_items"], 0);
    assert_intacto(&env.worktree);
    assert!(!env.path("data/cleanup/runs.jsonl").exists());
}

/// Com o job desligado, `cleanup.plan`, `cleanup.run` e `/cleanup/*`
/// continuam respondendo; executar só registra o que ficou, sem apagar.
#[test]
fn acionamento_manual_com_o_job_desligado() {
    let env = Env::new("[cleanup]\ncheck_interval_hours = 0\n");
    let _d = env.start();
    // Job desligado: nenhum plano até alguém pedir.
    std::thread::sleep(Duration::from_millis(300));
    assert!(env.ws("cleanup.status", json!({}))["plan"].is_null());

    let plan = env.ws("cleanup.plan", json!({}));
    assert_eq!(plan["items"][0]["gh_number"], 7);
    let http = env.post("/cleanup/plan", json!({}));
    assert_eq!(http["items"][0]["gh_number"], 7);

    let dry = env.ws("cleanup.run", json!({ "dry_run": true }));
    assert_eq!(dry["dry_run"], true);
    assert_eq!(dry["items"][0]["steps"][0]["result"], "bloqueada");
    assert!(!env.path("data/cleanup/runs.jsonl").exists());

    let id = plan["items"][0]["work_item_id"].clone();
    let run = env.post("/cleanup/run", json!({ "work_item_id": id }));
    assert_eq!(run["dry_run"], false);
    assert_eq!(run["freed_bytes"], 0);
    assert_eq!(run["items"][0]["steps"][0]["result"], "bloqueada");
    assert_intacto(&env.worktree);
    // A execução fica registrada, com o que ficou e por quê.
    let log = std::fs::read_to_string(env.path("data/cleanup/runs.jsonl")).unwrap();
    assert_eq!(log.lines().count(), 1);
    assert!(log.contains("clone do projeto não encontrado"), "{log}");
    let st = env.ws("cleanup.status", json!({}));
    assert_eq!(st["last_run"]["scope"]["work_item_id"], id);
}

/// Etapa "sessões" (#285) no daemon de verdade: a sessão interrompida do
/// work item concluído não aparece na Recuperação, o dry-run a lista sem
/// mexer, e a execução a arquiva. Com o worktree no disco, o transcript do
/// agente fica (sai só com o worktree, #265).
#[test]
fn sessoes_de_issue_concluida_saem_da_recuperacao() {
    let env =
        Env::new("[cleanup]\ncheck_interval_hours = 0\nmin_age_hours = 0\nmin_idle_hours = 0\n");
    let (sid, transcripts) = env.seed_session();
    let _d = env.start();
    assert_eq!(env.ws("recovery.list", json!({})), json!([]));
    assert_eq!(
        env.ws("dashboard.get", json!({}))["interrupted_sessions"],
        json!([])
    );

    let plan = env.ws("cleanup.plan", json!({}));
    let st = &plan["items"][0]["steps"][0];
    assert_eq!(st["state"], "pendente", "{plan}");
    assert_eq!(
        st["targets"],
        json!([format!("sessão {sid} (interrupted): sai da Recuperação")])
    );
    let dry = env.ws("cleanup.run", json!({ "dry_run": true }));
    assert_eq!(dry["items"][0]["steps"][0]["result"], "simulada");
    let plan = env.ws("cleanup.plan", json!({}));
    assert_eq!(plan["items"][0]["steps"][0]["state"], "pendente");

    let run = env.ws("cleanup.run", json!({}));
    let st = &run["items"][0]["steps"][0];
    assert_eq!(st["result"], "feita", "{run}");
    assert_eq!(
        st["removed"],
        json!([format!(
            "sessão {sid} (interrupted) arquivada: saiu da Recuperação"
        )])
    );
    let plan = env.ws("cleanup.plan", json!({}));
    assert_eq!(plan["items"][0]["steps"][0]["state"], "feita", "{plan}");
    assert!(transcripts.join("u-7.jsonl").is_file());
    assert_intacto(&env.worktree);
}

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn has_ref(repo: &Path, reference: &str) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["show-ref", "--verify", "--quiet", reference])
        .status()
        .unwrap()
        .success()
}

fn rev_parse(repo: &Path, rev: &str) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["rev-parse", rev])
        .output()
        .unwrap();
    assert!(out.status.success());
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// Troca a pasta do worktree do #7 por um worktree de verdade: clone em
/// `<tmp>/repo` com `origin` (bare) e a branch `feat/7-x` enviada, com o
/// `target/` (ignorado) e o código.
fn real_worktree(env: &Env) {
    let repo = env.path("repo");
    let origin = env.path("origin.git");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "develop"]);
    git(&repo, &["config", "user.email", "t@t"]);
    git(&repo, &["config", "user.name", "t"]);
    std::fs::write(repo.join(".gitignore"), "/target\n").unwrap();
    git(&repo, &["add", ".gitignore"]);
    git(&repo, &["commit", "-q", "-m", "init"]);
    let o = origin.display().to_string();
    git(env.tmp.path(), &["init", "-q", "--bare", &o]);
    git(&repo, &["remote", "add", "origin", &o]);
    git(&repo, &["push", "-q", "-u", "origin", "develop"]);
    std::fs::remove_dir_all(&env.worktree).unwrap();
    let wt = env.worktree.display().to_string();
    git(
        &repo,
        &["worktree", "add", "-q", "-b", "feat/7-x", &wt, "develop"],
    );
    std::fs::write(env.worktree.join("codigo.rs"), "fn main() {}\n").unwrap();
    git(&env.worktree, &["add", "codigo.rs"]);
    git(&env.worktree, &["commit", "-q", "-m", "codigo"]);
    git(&env.worktree, &["push", "-q", "-u", "origin", "feat/7-x"]);
    std::fs::create_dir_all(env.worktree.join("target")).unwrap();
    std::fs::write(env.worktree.join("target/lixo.bin"), vec![0u8; 4096]).unwrap();
}

/// Etapas "worktree" e "target/" (#265) no daemon de verdade: o worktree do
/// work item concluído, limpo, enviado e sem sessão aberta, sai com o
/// `target/` e os transcripts do agente (com backup); o dry-run só relata.
/// Com uma mudança não commitada, nada sai.
#[test]
fn worktree_de_issue_concluida_sai() {
    let env =
        Env::new("[cleanup]\ncheck_interval_hours = 0\nmin_age_hours = 0\nmin_idle_hours = 0\n");
    real_worktree(&env);
    let (_sid, transcripts) = env.seed_session();
    std::fs::write(env.worktree.join("rascunho.txt"), "x").unwrap();
    let _d = env.start();

    let plan = env.ws("cleanup.plan", json!({}));
    let wt = &plan["items"][0]["steps"][1];
    assert_eq!(wt["state"], "bloqueada", "{plan}");
    assert!(
        wt["reason"].as_str().unwrap().contains("rascunho.txt"),
        "{plan}"
    );
    let run = env.ws("cleanup.run", json!({}));
    assert_eq!(run["items"][0]["steps"][1]["result"], "bloqueada", "{run}");
    assert_intacto(&env.worktree);

    std::fs::remove_file(env.worktree.join("rascunho.txt")).unwrap();
    let plan = env.ws("cleanup.plan", json!({}));
    let wt = &plan["items"][0]["steps"][1];
    assert_eq!(wt["state"], "pendente", "{plan}");
    assert!(wt["bytes"].as_u64().unwrap() >= 4096, "{plan}");
    let dry = env.ws("cleanup.run", json!({ "dry_run": true }));
    assert_eq!(dry["items"][0]["steps"][0]["result"], "simulada", "{dry}");
    assert_intacto(&env.worktree);

    let run = env.ws("cleanup.run", json!({}));
    let st = &run["items"][0]["steps"][0];
    assert_eq!(
        (st["kind"].as_str(), st["result"].as_str()),
        (Some("worktree"), Some("feita")),
        "{run}"
    );
    let removed: Vec<&str> = st["removed"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(removed[0], format!("worktree {}", env.worktree.display()));
    assert!(
        removed
            .iter()
            .any(|r| r.starts_with("transcripts do agente")),
        "{run}"
    );
    assert!(!env.worktree.exists());
    assert!(!transcripts.exists());
    assert!(run["freed_bytes"].as_u64().unwrap() >= 4096);
    let log = std::fs::read_to_string(env.path("data/cleanup/runs.jsonl")).unwrap();
    assert!(log.contains("worktree "), "{log}");
    // Com o worktree fora, a branch local (#286), toda enviada e sem PR,
    // sai na mesma execução; a remota só sai com o PR mesclado ou fechado.
    let st = &run["items"][0]["steps"];
    assert_eq!(
        (st[1]["kind"].as_str(), st[1]["result"].as_str()),
        (Some("branch_local"), Some("feita")),
        "{run}"
    );
    assert!(
        st[1]["removed"][0]
            .as_str()
            .unwrap()
            .starts_with("branch local feat/7-x"),
        "{run}"
    );
    assert_eq!(
        (st[2]["kind"].as_str(), st[2]["result"].as_str()),
        (Some("branch_remota"), Some("bloqueada")),
        "{run}"
    );
    let repo = env.path("repo");
    assert!(!has_ref(&repo, "refs/heads/feat/7-x"));
    assert!(has_ref(&repo, "refs/remotes/origin/feat/7-x"));
    let plan = env.ws("cleanup.plan", json!({}));
    let steps = plan["items"][0]["steps"].as_array().unwrap();
    assert!(steps[..4].iter().all(|s| s["state"] == "feita"), "{plan}");
    assert!(
        steps[4]["reason"]
            .as_str()
            .unwrap()
            .starts_with("origin/feat/7-x não removida: sem PR da branch"),
        "{plan}"
    );
}

/// Shell livre (#428) no daemon de verdade: o `$SHELL` do daemon abre com
/// `-l` no worktree, sem nenhuma `CTOWER_*` do ambiente do daemon (nem as
/// de uma sessão do agente que o tenha iniciado), e a limpeza do worktree o
/// encerra antes de remover a pasta.
#[test]
fn shell_no_worktree_sem_variaveis_do_ct_e_encerrado_na_limpeza() {
    use std::os::unix::fs::PermissionsExt;
    let env =
        Env::new("[cleanup]\ncheck_interval_hours = 0\nmin_age_hours = 0\nmin_idle_hours = 0\n");
    real_worktree(&env);
    let out = env.path("shell-env.txt");
    let shell = env.path("shell");
    std::fs::write(
        &shell,
        format!(
            "#!/bin/bash\n{{ echo \"ARGS=$*\"; echo \"PWD=$(pwd)\"; echo \"TERM=$TERM\"; \
             env | grep -E '^CTOWER_' | sort; }} > {out}.tmp\nmv {out}.tmp {out}\n\
             exec bash --norc --noprofile -i\n",
            out = out.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&shell, std::fs::Permissions::from_mode(0o755)).unwrap();
    let _d = env.start_with(&[
        ("SHELL", shell.display().to_string()),
        ("CTOWER_SESSION", "sessao-do-agente".into()),
        ("CTOWER_TOKEN", "token-do-mcp".into()),
    ]);

    let plan = env.ws("cleanup.plan", json!({}));
    let wi = plan["items"][0]["work_item_id"].clone();
    let sh = env.ws("shell.open", json!({ "work_item_id": wi }));
    assert_eq!(sh["program"], shell.display().to_string(), "{sh}");
    assert_eq!(sh["cwd"], env.worktree.display().to_string(), "{sh}");
    let deadline = Instant::now() + T;
    let got = loop {
        if let Ok(s) = std::fs::read_to_string(&out) {
            break s;
        }
        assert!(
            Instant::now() < deadline,
            "o shell não registrou o ambiente"
        );
        std::thread::sleep(Duration::from_millis(50));
    };
    assert!(got.contains("ARGS=-l\n"), "{got}");
    assert!(got.contains("TERM=xterm-256color"), "{got}");
    let real = env.worktree.canonicalize().unwrap();
    assert!(got.contains(&format!("PWD={}", real.display())), "{got}");
    assert!(!got.contains("CTOWER_"), "{got}");
    assert_eq!(env.ws("shell.list", json!({}))[0]["id"], sh["id"]);

    // O shell aberto não segura a limpeza: sai antes do worktree.
    let plan = env.ws("cleanup.plan", json!({}));
    assert_eq!(plan["items"][0]["steps"][1]["state"], "pendente", "{plan}");
    let run = env.ws("cleanup.run", json!({}));
    assert_eq!(run["items"][0]["steps"][0]["result"], "feita", "{run}");
    assert!(!env.worktree.exists());
    assert_eq!(env.ws("shell.list", json!({})), json!([]));
}

/// Etapas de branch (#286) no daemon de verdade, com o head do PR no
/// espelho: a branch local de PR mesclado por squash sai depois do worktree;
/// `develop` e a branch de outro work item com PR aberto nunca são tocadas.
#[test]
fn branch_local_de_pr_mesclado_sai_e_protegidas_ficam() {
    let env =
        Env::new("[cleanup]\ncheck_interval_hours = 0\nmin_age_hours = 0\nmin_idle_hours = 0\n");
    real_worktree(&env);
    let repo = env.path("repo");
    let tip = rev_parse(&repo, "feat/7-x");
    git(&repo, &["branch", "feat/8-y", "develop"]);
    {
        let dirs = ct_sessions::Dirs {
            config: env.path("config"),
            data: env.path("data"),
        };
        let env_id = ct_sessions::ensure_env_id(&dirs.env_id_file()).unwrap();
        let l = Ledger::open(&dirs.ledger_db(), &env_id).unwrap();
        let project = l.list_all_projects().unwrap().remove(0);
        let w7 = l
            .ensure_work_item(project.id, 7, WorkItemKind::Issue)
            .unwrap();
        let w8 = l
            .ensure_work_item(project.id, 8, WorkItemKind::Issue)
            .unwrap();
        let pr = |number, wi, head: &str, state, sha: Option<&str>| ct_ledger::PrRow {
            number,
            work_item_id: wi,
            head: head.into(),
            base: "develop".into(),
            state,
            url: String::new(),
            updated_at: 1_000,
            head_sha: sha.map(String::from),
        };
        l.apply_github(&ct_ledger::GithubSnapshot {
            project_id: project.id,
            issues: vec![],
            prs: vec![
                pr(70, w7.id, "feat/7-x", ct_core::PrState::Merged, Some(&tip)),
                pr(80, w8.id, "feat/8-y", ct_core::PrState::Open, None),
            ],
            etags: vec![],
            synced_at: 1,
            gone: Vec::new(),
        })
        .unwrap();
    }
    let _d = env.start();
    let dry = env.ws("cleanup.run", json!({ "dry_run": true }));
    assert_eq!(dry["items"][0]["steps"][0]["result"], "simulada", "{dry}");
    assert!(has_ref(&repo, "refs/heads/feat/7-x"));

    let run = env.ws("cleanup.run", json!({}));
    let st = &run["items"][0]["steps"];
    assert_eq!(
        (st[1]["kind"].as_str(), st[1]["result"].as_str()),
        (Some("branch_local"), Some("feita")),
        "{run}"
    );
    assert!(
        st[1]["removed"][0]
            .as_str()
            .unwrap()
            .contains("igual ao head do PR #70 mesclado"),
        "{run}"
    );
    assert!(!has_ref(&repo, "refs/heads/feat/7-x"));
    assert!(has_ref(&repo, "refs/heads/feat/8-y"));
    assert!(has_ref(&repo, "refs/heads/develop"));
}

/// Arquivos (caminho relativo e conteúdo) sob `dir`, em ordem.
fn arvore(dir: &Path) -> Vec<(String, String)> {
    fn varre(base: &Path, dir: &Path, out: &mut Vec<(String, String)>) {
        for e in std::fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            let rel = p.strip_prefix(base).unwrap().display().to_string();
            if p.is_dir() {
                out.push((format!("{rel}/"), String::new()));
                varre(base, &p, out);
            } else {
                out.push((rel, std::fs::read_to_string(&p).unwrap()));
            }
        }
    }
    let mut out = Vec::new();
    varre(dir, dir, &mut out);
    out.sort();
    out
}

/// Docs fora do repositório (#440): a limpeza (`ctower clean`, que chama o
/// `cleanup.run`) apaga o worktree do work item concluído e nunca toca em
/// nada sob `docs_root`, nem na pasta do épico nem em um `target/` dentro
/// dos docs.
#[test]
fn limpeza_nunca_toca_em_docs_root() {
    let env = Env::new(
        "[cleanup]\ncheck_interval_hours = 0\nmin_age_hours = 0\nmin_idle_hours = 0\n\
         target_idle_days = 1\n",
    );
    real_worktree(&env);
    let (_sid, _transcripts) = env.seed_session();
    let docs = env.path("docs");
    let proj = docs.join("deep-ion-ai__demo");
    for d in [
        "_projeto/analises",
        "epicos/7-x/prompts",
        "epicos/7-x/target",
    ] {
        std::fs::create_dir_all(proj.join(d)).unwrap();
    }
    std::fs::write(proj.join("_projeto/analises/nota.md"), "análise").unwrap();
    std::fs::write(proj.join("epicos/7-x/README.md"), "# Épico 7\n").unwrap();
    std::fs::write(proj.join("epicos/7-x/target/lixo.bin"), "x").unwrap();
    // Marcador do cthubd (#441): sem ele, `docs_root` não é acessível.
    std::fs::write(docs.join(ct_core::docs::DOCS_ROOT_MARKER), "x").unwrap();
    let antes = arvore(&docs);
    let cfg = env.path("config/config.toml");
    let toml = std::fs::read_to_string(&cfg).unwrap();
    std::fs::write(&cfg, format!("docs_root = \"{}\"\n{toml}", docs.display())).unwrap();
    let _d = env.start();

    let info = env.ws("env.info", json!({}));
    assert_eq!(info["docs"]["root"], docs.display().to_string(), "{info}");
    assert_eq!(info["docs"]["reachable"], true, "{info}");
    let run = env.ws("cleanup.run", json!({}));
    assert_eq!(
        run["items"][0]["steps"][0]["result"], "feita",
        "a limpeza precisa ter rodado: {run}"
    );
    assert!(!env.worktree.exists());
    assert_eq!(arvore(&docs), antes);
}
