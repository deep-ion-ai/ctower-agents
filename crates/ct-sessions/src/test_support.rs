//! Ambiente de teste: repositório git temporário, ledger em memória e um
//! `claude` falso que registra argv/env e sai.

use crate::{AgentOptions, Dirs, Notifier, Sessions, SessionsConfig, StartRequest};
use ct_core::{SessionState, WorkItemKind};
use ct_ledger::{Ledger, NewProject, Session};
use serde_json::Value;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const T: Duration = Duration::from_secs(10);

/// `claude` falso: registra argv/env em `<tmp>/fake-out/<sessão>.txt` e
/// depois executa o corpo (padrão: `sleep N; exit C`). Do ambiente, grava
/// as `CTOWER_*`, as `CT_*` (docs), as `T_*` (secrets de teste), as `GITLAB_*` e `JIRA_*`
/// (projetos Jira + GitLab) e `PATH`/`HOME` com o prefixo `ENV_`.
pub struct FakeClaude {
    body: String,
}

impl FakeClaude {
    pub fn exit(code: i32) -> Self {
        Self::script(&format!("exit {code}"))
    }

    pub fn sleep(secs: u32) -> Self {
        Self::script(&format!("sleep {secs}"))
    }

    /// Corpo bash arbitrário, executado depois do registro de argv/env.
    pub fn script(body: &str) -> Self {
        FakeClaude {
            body: body.to_string(),
        }
    }

    fn write(&self, path: &Path, out_dir: &Path) {
        let script = format!(
            "#!/bin/bash\n\
             out=\"{out}/$CTOWER_SESSION.txt\"\n\
             {{ printf '%s\\n' \"$@\"; echo \"PWD=$(pwd)\"; echo \"SIZE=$(stty size 2>/dev/null)\"; env | grep -E '^(CTOWER|CT|T|GITLAB|JIRA)_' | sort; echo \"ENV_PATH=$PATH\"; echo \"ENV_HOME=$HOME\"; }} > \"$out.tmp\"\n\
             mv \"$out.tmp\" \"$out\"\n\
             {body}\n",
            out = out_dir.display(),
            body = self.body,
        );
        std::fs::write(path, script).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}

pub struct TestEnv {
    pub tmp: tempfile::TempDir,
    pub cfg: SessionsConfig,
    pub sessions: Sessions,
    pub project_id: i64,
    pub repo: PathBuf,
    notices: Arc<Mutex<Vec<(String, Value)>>>,
    forward: Option<Notifier>,
    /// Ledger em `<data>/ledger.db` em vez de em memória.
    file_ledger: bool,
}

impl TestEnv {
    pub fn new(fake: FakeClaude) -> Self {
        Self::build_env(fake, None, false)
    }

    /// Como `new`, repassando também as notificações para `forward`.
    pub fn with_notifier(fake: FakeClaude, forward: Notifier) -> Self {
        Self::build_env(fake, Some(forward), false)
    }

    /// Como `new`, com o ledger em arquivo (`<data>/ledger.db`), para
    /// varrer o que foi persistido.
    pub fn with_file_ledger(fake: FakeClaude) -> Self {
        Self::build_env(fake, None, true)
    }

    fn build_env(fake: FakeClaude, forward: Option<Notifier>, file_ledger: bool) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        git_in(&repo, &["init", "-q", "-b", "develop"]);
        git_in(&repo, &["config", "user.email", "t@t"]);
        git_in(&repo, &["config", "user.name", "t"]);
        git_in(&repo, &["commit", "-q", "--allow-empty", "-m", "init"]);

        let out_dir = tmp.path().join("fake-out");
        std::fs::create_dir_all(&out_dir).unwrap();
        let bin = tmp.path().join("claude");
        fake.write(&bin, &out_dir);

        std::fs::write(tmp.path().join(".claude.json"), "{}").unwrap();
        let dirs = Dirs {
            config: tmp.path().join("config"),
            data: tmp.path().join("data"),
        };
        dirs.ensure().unwrap();
        let cfg = SessionsConfig {
            dirs,
            port: 7878,
            hook_token: "hook-secreto".into(),
            claude_bin: bin.to_string_lossy().into(),
            ctower_bin: "ctower".into(),
            worktrees_root: tmp.path().join("wt"),
            gestor_dir: tmp.path().join("data/gestor"),
            scrollback: 64 * 1024,
            claude_json: Some(tmp.path().join(".claude.json")),
            claude_projects: Some(tmp.path().join("claude-projects")),
            secrets_max_age: Some(Duration::from_secs(24 * 3600)),
            process_env: Some(Vec::new()),
            shell: Some(write_fake_shell(tmp.path()).to_string_lossy().into()),
            docs_root: Some(tmp.path().join("docs")),
        };
        let notices = Arc::new(Mutex::new(Vec::new()));
        let (sessions, project_id) = build(&cfg, &repo, &notices, forward.clone(), file_ledger);
        TestEnv {
            tmp,
            cfg,
            sessions,
            project_id,
            repo,
            notices,
            forward,
            file_ledger,
        }
    }

    /// Recria o ambiente com outro binário do agente (ledger novo).
    pub fn set_claude_bin(&mut self, bin: &str) {
        self.cfg.claude_bin = bin.into();
        let (s, p) = build(
            &self.cfg,
            &self.repo,
            &self.notices,
            self.forward.clone(),
            self.file_ledger,
        );
        self.sessions = s;
        self.project_id = p;
    }

    /// `docs_root` no ar (#441): `<tmp>/docs` com o marcador do `cthubd`.
    pub fn docs_root_up(&self) -> PathBuf {
        let root = self.tmp.path().join("docs");
        mount_docs_root(&root);
        root
    }

    pub fn request(&self, n: i64, title: &str) -> StartRequest {
        StartRequest {
            project_id: self.project_id,
            gh_number: n,
            kind: WorkItemKind::Issue,
            title: title.into(),
            labels: vec![],
            prompt: "prompt inicial".into(),
            size: None,
            agent: AgentOptions::default(),
        }
    }

    pub fn git(&self, args: &[&str]) {
        git_in(&self.repo, args);
    }

    pub fn fake_output_path(&self, session_id: &str) -> PathBuf {
        self.tmp
            .path()
            .join("fake-out")
            .join(format!("{session_id}.txt"))
    }

    pub fn wait_fake_output(&self, session_id: &str) -> String {
        let p = self.fake_output_path(session_id);
        let deadline = Instant::now() + T;
        while Instant::now() < deadline {
            if let Ok(s) = std::fs::read_to_string(&p) {
                return s;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("claude falso não escreveu {p:?}");
    }

    pub fn wait_state(&self, session_id: &str, state: SessionState) -> Session {
        let deadline = Instant::now() + T;
        loop {
            let s = self.sessions.ledger().get_session(session_id).unwrap();
            if s.state == state {
                return s;
            }
            assert!(
                Instant::now() < deadline,
                "sessão ficou em {:?}, esperado {state:?}",
                s.state
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Grava o cache de secrets do ambiente como se o hub tivesse entregue
    /// `entries` (`(projeto, nome, valor)`; `None` = global), sem validar
    /// os nomes (o cache pode trazer reservados).
    pub fn write_secrets_cache(&self, entries: &[(Option<&str>, &str, &str)]) {
        let entries: Vec<_> = entries
            .iter()
            .map(|(p, n, v)| (*p, *n, ct_secrets::Kind::Secret, *v))
            .collect();
        self.write_secrets_cache_kinds(&entries);
    }

    /// Como [`TestEnv::write_secrets_cache`], escolhendo o tipo de cada
    /// entrada (secret ou variável).
    pub fn write_secrets_cache_kinds(
        &self,
        entries: &[(Option<&str>, &str, ct_secrets::Kind, &str)],
    ) {
        let cache = self.sessions.env_cache();
        let entries = entries
            .iter()
            .map(|(project, name, kind, value)| ct_secrets::Entry {
                scope: match project {
                    Some(p) => ct_secrets::Scope::parse_project(p).unwrap(),
                    None => ct_secrets::Scope::Global,
                },
                name: (*name).to_string(),
                kind: *kind,
                value: ct_secrets::SecretValue::new(*value),
                rev: 1,
                updated_at: 0,
            })
            .collect();
        let b64 = ct_secrets::seal_delivery(1, entries, &cache.public_key().unwrap()).unwrap();
        cache
            .store(&ct_secrets::decode_delivery(&b64).unwrap())
            .unwrap();
    }

    /// O projeto do ambiente (`deep-ion-ai/demo`) passa a ter o rastreador e
    /// a forja dados, pelo pull do hub (`rev` cresce a cada chamada); `sync`
    /// = este é o ambiente de sync.
    pub fn assign_provider(
        &self,
        tracker: ct_core::TrackerConfig,
        forge: ct_core::ForgeConfig,
        sync: bool,
        rev: i64,
    ) {
        self.assign_with_rules(tracker, forge, sync, rev, ct_ledger::ConfigRules::default());
    }

    /// Como [`TestEnv::assign_provider`], com as regras do agente vigentes
    /// que o hub entregaria no pull (#357).
    pub fn assign_with_rules(
        &self,
        tracker: ct_core::TrackerConfig,
        forge: ct_core::ForgeConfig,
        sync: bool,
        rev: i64,
        rules: ct_ledger::ConfigRules,
    ) {
        self.assign_config(rev, |p| {
            p.tracker = tracker;
            p.forge = forge;
            p.sync = sync;
            p.rules = rules;
        });
    }

    /// Pull do hub com o projeto do ambiente (`deep-ion-ai/demo`, GitHub,
    /// atribuição ligada) ajustado por `edit`: padrão de branch, atribuição,
    /// rastreador e forja (#351).
    pub fn assign_config(&self, rev: i64, edit: impl FnOnce(&mut ct_ledger::ConfigProject)) {
        use ct_ledger::{ConfigProject, ConfigResponse};
        let mut project = ConfigProject {
            docs: Default::default(),
            owner: "deep-ion-ai".into(),
            repo: "demo".into(),
            name: "demo".into(),
            base_branch: "develop".into(),
            protected_branches: vec![],
            repo_path: self.repo.display().to_string(),
            rev,
            tracker: ct_core::TrackerConfig::Github,
            forge: ct_core::ForgeConfig::Github,
            branch_pattern: None,
            attribution: true,
            sync: false,
            rules: Default::default(),
        };
        edit(&mut project);
        self.sessions
            .ledger()
            .apply_hub_config(
                &ConfigResponse {
                    config_rev: rev,
                    config_epoch: 1,
                    changed: true,
                    projects: Some(vec![project]),
                    removed: Some(vec![]),
                },
                |p| p.repo_path.clone(),
            )
            .unwrap();
    }

    /// Grava no espelho as issues `(número, épico pai, tem sub-issues)`.
    pub fn mirror_issues(&self, issues: &[(i64, Option<i64>, bool)]) {
        let issues = issues
            .iter()
            .map(|(n, parent, subs)| ct_ledger::IssueRow {
                gh_number: *n,
                title: format!("Issue {n}"),
                state: "open".into(),
                labels: vec![],
                parent_number: *parent,
                has_sub_issues: *subs,
                url: String::new(),
                updated_at: 0,
                ..Default::default()
            })
            .collect();
        self.sessions
            .ledger()
            .apply_github(&ct_ledger::GithubSnapshot {
                project_id: self.project_id,
                issues,
                ..Default::default()
            })
            .unwrap();
    }

    /// Saída do shell falso até o fim do registro (`FIM-ENV`): argv,
    /// pasta, tamanho do terminal, `TERM`/`COLORTERM` e as `CTOWER_*`.
    pub fn wait_shell_output(&self, shell_id: &str) -> String {
        let pty = self.sessions.terminal(shell_id).expect("shell aberto");
        let deadline = Instant::now() + T;
        loop {
            let s = String::from_utf8_lossy(&pty.snapshot()).to_string();
            if s.contains("FIM-ENV") {
                return s;
            }
            assert!(Instant::now() < deadline, "shell falso não terminou: {s:?}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    pub fn notices(&self) -> Vec<(String, Value)> {
        self.notices.lock().unwrap().clone()
    }

    /// Espera um aviso que satisfaça `pred` e devolve todos os avisos até
    /// ali. A saída grava o estado no ledger antes de avisar: depois do
    /// `wait_state`, o aviso pode ainda não ter chegado.
    pub fn wait_notice(&self, pred: impl Fn(&str, &Value) -> bool) -> Vec<(String, Value)> {
        let deadline = Instant::now() + T;
        loop {
            let notices = self.notices();
            if notices.iter().any(|(n, d)| pred(n, d)) {
                return notices;
            }
            assert!(
                Instant::now() < deadline,
                "aviso esperado não chegou: {notices:?}"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

/// Shell de login falso (`<tmp>/shell`): imprime argv, pasta, tamanho do
/// terminal, `TERM`, `COLORTERM` e as `CTOWER_*` do ambiente, depois
/// `FIM-ENV`, e vira um bash interativo sem rc (prompt previsível).
pub fn write_fake_shell(dir: &Path) -> PathBuf {
    let path = dir.join("shell");
    let script = "#!/bin/bash\n\
         for a in \"$@\"; do echo \"ARG=$a\"; done\n\
         echo \"PWD=$(pwd)\"; echo \"SIZE=$(stty size 2>/dev/null)\"\n\
         echo \"TERM=$TERM\"; echo \"COLORTERM=$COLORTERM\"\n\
         env | grep -E '^(CTOWER_|CT_DOCS_DIR=|CT_EPIC_DOCS_DIR=)' | sort\n\
         echo FIM-ENV\n\
         exec bash --norc --noprofile -i\n";
    std::fs::write(&path, script).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

fn build(
    cfg: &SessionsConfig,
    repo: &Path,
    notices: &Arc<Mutex<Vec<(String, Value)>>>,
    forward: Option<Notifier>,
    file_ledger: bool,
) -> (Sessions, i64) {
    let ledger = if file_ledger {
        Ledger::open(&cfg.dirs.ledger_db(), "env-teste").unwrap()
    } else {
        Ledger::open_in_memory().unwrap()
    };
    let project = ledger
        .add_project(&NewProject {
            name: "demo".into(),
            gh_owner: "deep-ion-ai".into(),
            gh_repo: "demo".into(),
            repo_path: repo.to_string_lossy().into(),
            base_branch: "develop".into(),
            protected_branches: vec!["develop".into(), "main".into()],
        })
        .unwrap();
    let sink = Arc::clone(notices);
    let notify: Notifier = Arc::new(move |name: &str, data: Value| {
        if let Some(f) = &forward {
            f(name, data.clone());
        }
        sink.lock().unwrap().push((name.to_string(), data));
    });
    (Sessions::new(cfg.clone(), ledger, notify), project.id)
}

/// `git -C <dir> <args>`, com uma identidade fixa (`t <t@t>`, a mesma do
/// `TestEnv`): o commit não depende do `user.name`/`user.email` global, que
/// o runner do CI não tem.
/// Cria `root` com o marcador [`ct_core::docs::DOCS_ROOT_MARKER`], como a
/// pasta real do `cthubd` vista pela montagem.
pub fn mount_docs_root(root: &Path) {
    std::fs::create_dir_all(root).unwrap();
    std::fs::write(root.join(ct_core::docs::DOCS_ROOT_MARKER), "x").unwrap();
}

pub fn git_in(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}
