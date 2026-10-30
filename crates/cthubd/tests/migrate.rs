//! `cthubd migrate` (#182) com o `hub.db` e o `ledger.db` de um `ctowerd`
//! `hub = true` gerados pelo código anterior ao épico #183
//! (`tests/fixtures`).

use assert_cmd::Command;
use ct_ledger::{AssignmentFilter, Hub, SharedHub};
use cthubd::config::Dirs;
use cthubd::migrate::{migrate, migrate_files, MigrateOpts};
use predicates::str::contains;
use serde_json::{json, Value};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

const LOCAL: &str = "0b6c1f7e-1a2b-4c3d-8e9f-000000000001";
const NOTEBOOK: &str = "0b6c1f7e-1a2b-4c3d-8e9f-000000000002";
const HUB_SQL: &str = include_str!("fixtures/hub-antigo.sql");
const LEDGER_SQL: &str = include_str!("fixtures/ledger-antigo.sql");
const NOTEBOOK_TOKEN: &str = include_str!("fixtures/notebook.token");

/// Banco do hub antigo a partir do despejo, em WAL como o `ctowerd` deixava.
fn load_sql(path: &Path, sql: &str) {
    let c = rusqlite::Connection::open(path).unwrap();
    c.execute_batch(sql).unwrap();
    let mode: String = c
        .query_row("PRAGMA journal_mode = WAL", [], |r| r.get(0))
        .unwrap();
    assert_eq!(mode, "wal");
}

struct Fx {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    dirs: Dirs,
    opts: MigrateOpts,
    config: String,
}

const CONFIG: &str = "# config do hub antigo\n\
env_name = \"mac-mini\"\n\
hub = true\n\
bind = \"100.64.0.1\"\n\
hub_url = \"http://ignorado:7878\"\n\
hub_backup_dir = \"/mnt/bk/ctower\"\n\
hub_backup_keep = 3\n\
update_ref = \"develop\"\n\
retention_days = 10\n";

fn fixture() -> Fx {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let (cfg, data) = (root.join("ctower/config"), root.join("ctower/data"));
    std::fs::create_dir_all(&cfg).unwrap();
    std::fs::create_dir_all(&data).unwrap();
    load_sql(&data.join("hub.db"), HUB_SQL);
    load_sql(&data.join("ledger.db"), LEDGER_SQL);
    // Porta livre para o ctowerd (parado).
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let config = format!("{CONFIG}port = {port}\n");
    std::fs::write(cfg.join("config.toml"), &config).unwrap();
    std::fs::write(cfg.join("env_id"), LOCAL).unwrap();
    std::fs::write(cfg.join("github.token"), "gh-do-teste\n").unwrap();
    let dirs = Dirs {
        config: root.join("cthub/config"),
        data: root.join("cthub/data"),
    };
    let opts = MigrateOpts {
        from: data,
        ctower_config: cfg,
        env_name: None,
        force: false,
        hub_port: 7879,
    };
    Fx {
        _tmp: tmp,
        root,
        dirs,
        opts,
        config,
    }
}

fn count(db: &Path, table: &str) -> i64 {
    rusqlite::Connection::open(db)
        .unwrap()
        .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
        .unwrap()
}

/// `cthubd` (router de verdade) servindo `hub` numa thread própria.
fn serve(hub: Hub) -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let app = cthubd::router(cthubd::HubState::new(
        SharedHub::new(hub),
        "ui",
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
    url
}

fn pull(url: &str, token: &str, env_id: &str) -> Result<Value, u16> {
    match ureq::get(&format!("{url}/sync/config"))
        .set("Authorization", &format!("Bearer {}", token.trim()))
        .set("X-Ctower-Env-Id", env_id)
        .call()
    {
        Ok(r) => Ok(r.into_json().unwrap()),
        Err(ureq::Error::Status(s, _)) => Err(s),
        Err(e) => panic!("{e}"),
    }
}

fn repos(config: &Value) -> Vec<(String, String)> {
    let mut v: Vec<_> = config["projects"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| {
            (
                p["repo"].as_str().unwrap().to_string(),
                p["repo_path"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    v.sort();
    v
}

/// Critério de aceite da #182: projetos, atribuições, decisões e tokens
/// preservados; o token de sync antigo autentica `/sync/push` e
/// `/sync/config` no `cthubd` migrado, e o `ctowerd` local vira um ambiente
/// comum com o mesmo `env_id`.
#[test]
fn migra_preservando_projetos_atribuicoes_decisoes_e_tokens() {
    let fx = fixture();
    let old_db = fx.opts.from.join("hub.db");
    let before = std::fs::read(&old_db).unwrap();
    let rep = migrate_files(&fx.dirs, &fx.opts).unwrap();
    assert!(!rep.already && !rep.resumed);
    assert_eq!((rep.projects, rep.assignments), (3, 3));
    assert_eq!(rep.env_tokens, vec!["mac-mini", "notebook", "sem-uso"]);
    assert_eq!(
        rep.local_env,
        Some(("mac-mini".to_string(), Some(LOCAL.to_string())))
    );
    // O original fica intocado (nem migrado, nem com WAL novo).
    assert_eq!(std::fs::read(&old_db).unwrap(), before);
    assert_eq!(ct_ledger::db_user_version(&old_db).unwrap(), 5);
    assert!(!fx.dirs.data.join("hub.db.migrate").exists());

    let db = fx.dirs.hub_db();
    for (t, n) in [("decision", 2), ("event", 5), ("env", 2), ("work_item", 1)] {
        assert_eq!(count(&db, t), n, "{t}");
    }
    let hub = Hub::open(&db).unwrap();
    assert!(hub.migrated_at().unwrap().is_some());
    let mut ps: Vec<_> = hub
        .list_projects()
        .unwrap()
        .into_iter()
        .map(|p| p.gh_repo)
        .collect();
    ps.sort();
    // "velho" foi removido do notebook, mas o projeto continua no hub (sem
    // atribuição).
    assert_eq!(ps, vec!["demo", "outro", "velho"]);
    let mut a: Vec<_> = hub
        .list_assignments(&AssignmentFilter::default())
        .unwrap()
        .into_iter()
        .map(|a| (a.gh_repo, a.env_name, a.repo_path))
        .collect();
    a.sort();
    assert_eq!(
        a,
        vec![
            (
                "demo".into(),
                "mac-mini".into(),
                "/home/u/projetos/demo".into()
            ),
            (
                "demo".into(),
                "notebook".into(),
                "/home/b/projetos/demo".into()
            ),
            (
                "outro".into(),
                "mac-mini".into(),
                "/home/u/projetos/outro".into()
            ),
        ]
    );
    for r in ["demo", "outro", "velho"] {
        assert!(hub.project_rev("deep-ion-ai", r).unwrap().unwrap() > 0);
    }
    assert_eq!(hub.config_rev().unwrap(), rep.config_rev);
    assert_eq!(rep.config_rev, 6);

    // Arquivos do cthubd.
    let hub_cfg: toml::Table =
        toml::from_str(&std::fs::read_to_string(fx.dirs.config.join("config.toml")).unwrap())
            .unwrap();
    assert_eq!(hub_cfg["bind"].as_str(), Some("100.64.0.1"));
    assert_eq!(hub_cfg["backup_dir"].as_str(), Some("/mnt/bk/ctower"));
    assert_eq!(hub_cfg["backup_keep"].as_integer(), Some(3));
    assert_eq!(hub_cfg["update_ref"].as_str(), Some("develop"));
    assert!(!hub_cfg.contains_key("port") && !hub_cfg.contains_key("retention_days"));
    cthubd::config::HubConfig::load(&fx.dirs).unwrap();
    let ui = fx.dirs.ui_token_file();
    assert_eq!(std::fs::read_to_string(&ui).unwrap().trim().len(), 64);
    assert_eq!(
        std::fs::metadata(&ui).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        std::fs::read_to_string(fx.dirs.github_token_file()).unwrap(),
        "gh-do-teste\n"
    );

    // ctowerd local: config trocado, original guardado, token preso ao env_id.
    let cfg_dir = &fx.opts.ctower_config;
    let new_cfg = std::fs::read_to_string(cfg_dir.join("config.toml")).unwrap();
    let t: toml::Table = toml::from_str(&new_cfg).unwrap();
    assert_eq!(t["hub_url"].as_str(), Some("http://127.0.0.1:7879"));
    for k in ["hub", "hub_backup_dir", "hub_backup_keep"] {
        assert!(!t.contains_key(k), "{k}: {new_cfg}");
    }
    assert_eq!(t["env_name"].as_str(), Some("mac-mini"));
    assert_eq!(t["bind"].as_str(), Some("100.64.0.1"));
    assert_eq!(t["retention_days"].as_integer(), Some(10));
    assert!(new_cfg.contains("# config do hub antigo"), "{new_cfg}");
    assert_eq!(
        std::fs::read_to_string(cfg_dir.join("config.toml.pre-cthub")).unwrap(),
        fx.config
    );
    let tok_file = cfg_dir.join("hub.token");
    assert_eq!(
        std::fs::metadata(&tok_file).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let local_tok = std::fs::read_to_string(&tok_file).unwrap();
    let auth = hub.authenticate(local_tok.trim()).unwrap().unwrap();
    assert_eq!(
        (auth.name.as_str(), auth.env_id.as_deref()),
        ("mac-mini", Some(LOCAL))
    );
    drop(hub);

    // O cthubd migrado aceita o token antigo do notebook e o novo do local.
    let url = serve(Hub::open(&db).unwrap());
    let r: Value = ureq::post(&format!("{url}/sync/push"))
        .set(
            "Authorization",
            &format!("Bearer {}", NOTEBOOK_TOKEN.trim()),
        )
        .send_json(json!({ "env_id": NOTEBOOK, "entries": [], "pending": 0 }))
        .unwrap()
        .into_json()
        .unwrap();
    // O seq do notebook continua de onde o hub antigo parou.
    assert_eq!(r["last_seq"], 7);
    let c = pull(&url, NOTEBOOK_TOKEN, NOTEBOOK).unwrap();
    assert_eq!(
        repos(&c),
        vec![("demo".into(), "/home/b/projetos/demo".into())]
    );
    let c = pull(&url, &local_tok, LOCAL).unwrap();
    assert_eq!(
        repos(&c),
        vec![
            ("demo".into(), "/home/u/projetos/demo".into()),
            ("outro".into(), "/home/u/projetos/outro".into()),
        ]
    );
    // Preso ao env_id: o token do notebook não serve a outro ambiente.
    assert_eq!(pull(&url, NOTEBOOK_TOKEN, LOCAL), Err(403));
    assert_eq!(pull(&url, "errado", LOCAL), Err(401));
}

/// #347: os projetos das fixtures antigas saem do `cthubd migrate` como
/// GitHub (hub e ledger), com as referências e o pull de sempre, e os campos
/// novos com os padrões.
#[test]
fn fixtures_antigas_viram_projetos_github() {
    let fx = fixture();
    migrate_files(&fx.dirs, &fx.opts).unwrap();
    let db = fx.dirs.hub_db();
    type Linha = (String, String, String, Option<String>, i64, Option<String>);
    let kinds: Vec<Linha> = rusqlite::Connection::open(&db)
        .unwrap()
        .prepare(
            "SELECT key, tracker_kind, forge_kind, branch_pattern, attribution, sync_env_id
                 FROM project ORDER BY key",
        )
        .unwrap()
        .query_map([], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
            ))
        })
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert!(kinds.len() >= 3, "{kinds:?}");
    for (key, tracker, forge, pattern, attribution, sync_env) in &kinds {
        assert_eq!(
            (tracker.as_str(), forge.as_str()),
            ("github", "github"),
            "{key}"
        );
        assert_eq!(
            (pattern, attribution, sync_env),
            (&None, &1, &None),
            "{key}"
        );
    }
    let hub = Hub::open(&db).unwrap();
    for p in hub.list_projects().unwrap() {
        assert_eq!(p.tracker, ct_core::TrackerConfig::Github);
        assert_eq!(p.item_ref(7), format!("{}/{}#7", p.gh_owner, p.gh_repo));
    }
    // O pull leva os padrões do GitHub.
    let url = serve(hub);
    let c = pull(&url, NOTEBOOK_TOKEN, NOTEBOOK).unwrap();
    for p in c["projects"].as_array().unwrap() {
        assert_eq!(p["tracker"], json!({ "kind": "github" }));
        assert_eq!(p["forge"], json!({ "kind": "github" }));
        assert_eq!(
            (&p["attribution"], &p["sync"]),
            (&json!(true), &json!(false))
        );
    }

    // O `ledger.db` antigo do `ctowerd` também abre e migra como GitHub.
    let l = ct_ledger::Ledger::open(&fx.opts.from.join("ledger.db"), LOCAL).unwrap();
    let ps = l.list_all_projects().unwrap();
    assert!(!ps.is_empty());
    for p in ps {
        assert_eq!(p.tracker, ct_core::TrackerConfig::Github);
        assert_eq!(p.forge, ct_core::ForgeConfig::Github);
        assert!(p.attribution && !p.sync_role);
        assert_eq!(p.effective_branch_pattern(), "{prefix}/{n}-{slug}");
    }
}

/// Rodar de novo não muda nada; uma migração interrompida depois da troca do
/// `hub.db` é retomada (só a parte do `ctowerd`, com outro token).
#[test]
fn idempotente_e_retomavel() {
    let fx = fixture();
    migrate_files(&fx.dirs, &fx.opts).unwrap();
    let cfg = fx.opts.ctower_config.join("config.toml");
    let tok = fx.opts.ctower_config.join("hub.token");
    let (cfg1, tok1) = (
        std::fs::read_to_string(&cfg).unwrap(),
        std::fs::read_to_string(&tok).unwrap(),
    );
    let rep = migrate_files(&fx.dirs, &fx.opts).unwrap();
    assert!(rep.already, "{rep:?}");
    assert_eq!(std::fs::read_to_string(&cfg).unwrap(), cfg1);
    assert_eq!(std::fs::read_to_string(&tok).unwrap(), tok1);

    // Interrompida: o hub.db já migrou, mas o ctowerd ainda tem hub = true.
    std::fs::write(&cfg, &fx.config).unwrap();
    std::fs::remove_file(&tok).unwrap();
    let rep = migrate_files(&fx.dirs, &fx.opts).unwrap();
    assert!(rep.resumed && !rep.already, "{rep:?}");
    assert_eq!(rep.assignments, 3);
    assert_eq!(std::fs::read_to_string(&cfg).unwrap(), cfg1);
    let tok2 = std::fs::read_to_string(&tok).unwrap();
    let hub = Hub::open(&fx.dirs.hub_db()).unwrap();
    assert!(hub.authenticate(tok1.trim()).unwrap().is_none());
    let auth = hub.authenticate(tok2.trim()).unwrap().unwrap();
    assert_eq!(auth.env_id.as_deref(), Some(LOCAL));
    assert_eq!(
        hub.env_token_names().unwrap(),
        vec!["mac-mini", "notebook", "sem-uso"]
    );
    // A adoção não rodou de novo.
    assert_eq!(hub.config_rev().unwrap(), 6);
    drop(hub);

    // Voltou atrás (config.toml.pre-cthub de volta) e o hub antigo recebeu
    // um envio: retomar perderia esse envio, então recusa sem --force.
    std::fs::write(&cfg, &fx.config).unwrap();
    let old_db = fx.opts.from.join("hub.db");
    rusqlite::Connection::open(&old_db)
        .unwrap()
        .execute(
            "UPDATE env SET last_seq = last_seq + 1, last_seen_at = 2 WHERE env_id = ?1",
            [NOTEBOOK],
        )
        .unwrap();
    let e = migrate_files(&fx.dirs, &fx.opts).unwrap_err();
    assert!(
        e.contains("mudou depois da migração") && e.contains("--force"),
        "{e}"
    );
    // Nada mudou: o config segue o de antes e não sobra hub.db.migrate.
    assert_eq!(std::fs::read_to_string(&cfg).unwrap(), fx.config);
    assert!(!fx.dirs.data.join("hub.db.migrate").exists());
    // --force refaz do hub.db antigo e guarda o migrado ao lado.
    let mut o = fx.opts.clone();
    o.force = true;
    let rep = migrate_files(&fx.dirs, &o).unwrap();
    assert!(!rep.resumed && !rep.already, "{rep:?}");
    assert!(rep.replaced.is_some());
    assert_eq!(rep.config_rev, 6);
    assert_eq!(std::fs::read_to_string(&cfg).unwrap(), cfg1);
    // O envio que o hub antigo recebeu depois está no hub.db novo.
    let seq: i64 = rusqlite::Connection::open(fx.dirs.hub_db())
        .unwrap()
        .query_row(
            "SELECT last_seq FROM env WHERE env_id = ?1",
            [NOTEBOOK],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(seq, 8);
}

/// #195: sem `env_id` no ctowerd, o token já nasce preso a um novo, e o
/// `env_id` é gravado no ambiente (não é adivinhado no primeiro uso).
#[test]
fn sem_env_id_o_token_nasce_preso_a_um_novo_gravado_no_ambiente() {
    let fx = fixture();
    std::fs::remove_file(fx.opts.ctower_config.join("env_id")).unwrap();
    let rep = migrate_files(&fx.dirs, &fx.opts).unwrap();
    let (_, id) = rep.local_env.unwrap();
    let id = id.unwrap();
    let gravado = std::fs::read_to_string(fx.opts.ctower_config.join("env_id")).unwrap();
    assert_eq!(gravado.trim(), id);
    let token = std::fs::read_to_string(fx.opts.ctower_config.join("hub.token")).unwrap();
    let hub = Hub::open(&fx.dirs.hub_db()).unwrap();
    let e = hub.authenticate(token.trim()).unwrap().unwrap();
    assert_eq!(e.env_id.as_deref(), Some(id.as_str()));
}

#[test]
fn recusas_antes_de_escrever() {
    // ctowerd no ar (porta do config dele).
    let fx = fixture();
    let port: u16 = fx
        .config
        .lines()
        .find_map(|l| l.strip_prefix("port = "))
        .unwrap()
        .parse()
        .unwrap();
    let l = std::net::TcpListener::bind(("127.0.0.1", port)).unwrap();
    let e = migrate(&fx.dirs, &fx.opts).unwrap_err();
    assert!(e.contains("ctowerd está no ar"), "{e}");
    drop(l);
    // cthubd no ar. A porta do ctowerd no config muda a cada tentativa: o SO
    // pode dar a porta que `l` soltou a outro teste em paralelo (flaky no
    // macOS) e a recusa seria a do ctowerd.
    let h = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let cfg = fx.opts.ctower_config.join("config.toml");
    let original = std::fs::read_to_string(&cfg).unwrap();
    let mut o = fx.opts.clone();
    o.hub_port = h.local_addr().unwrap().port();
    let mut e = String::new();
    for _ in 0..20 {
        let livre = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let novo = original.replace(&format!("port = {port}"), &format!("port = {livre}"));
        std::fs::write(&cfg, novo).unwrap();
        e = migrate(&fx.dirs, &o).unwrap_err();
        if !e.contains("ctowerd está no ar") {
            break;
        }
    }
    std::fs::write(&cfg, original).unwrap();
    assert!(e.contains("pare o cthubd"), "{e}");
    drop(h);
    assert!(!fx.dirs.data.exists());

    // Sem hub.db na origem.
    let mut o = fx.opts.clone();
    o.from = fx.root.join("vazio");
    let e = migrate_files(&fx.dirs, &o).unwrap_err();
    assert!(e.contains("não há hub.db"), "{e}");

    // Nome de outro ambiente.
    let mut o = fx.opts.clone();
    o.env_name = Some("notebook".into());
    std::fs::write(
        o.ctower_config.join("config.toml"),
        fx.config.replace("env_name = \"mac-mini\"\n", ""),
    )
    .unwrap();
    let e = migrate_files(&fx.dirs, &o).unwrap_err();
    assert!(e.contains("outro ambiente"), "{e}");
    assert!(!fx.dirs.hub_db().exists());
    assert!(!fx.dirs.data.join("hub.db.migrate").exists());
    assert!(!o.ctower_config.join("hub.token").exists());
    std::fs::write(o.ctower_config.join("config.toml"), &fx.config).unwrap();

    // config.toml.pre-cthub de outra coisa.
    let pre = fx.opts.ctower_config.join("config.toml.pre-cthub");
    std::fs::write(&pre, "outro\n").unwrap();
    let e = migrate_files(&fx.dirs, &fx.opts).unwrap_err();
    assert!(e.contains("pre-cthub"), "{e}");
    std::fs::remove_file(&pre).unwrap();
    assert!(!fx.dirs.hub_db().exists());
    assert_eq!(
        std::fs::read_to_string(fx.opts.ctower_config.join("config.toml")).unwrap(),
        fx.config
    );

    // O cthubd já tem um hub.db: só com --force, e o atual fica ao lado.
    cthubd::cli::add_env(&fx.dirs, "x").unwrap();
    let e = migrate_files(&fx.dirs, &fx.opts).unwrap_err();
    assert!(e.contains("--force"), "{e}");
    let mut o = fx.opts.clone();
    o.force = true;
    let rep = migrate_files(&fx.dirs, &o).unwrap();
    let prev = rep.replaced.unwrap();
    assert!(prev
        .file_name()
        .unwrap()
        .to_string_lossy()
        .starts_with("hub.db.pre-migrate-"));
    assert_eq!(
        Hub::open(&prev).unwrap().env_token_names().unwrap(),
        vec!["x"]
    );
    assert_eq!(rep.env_tokens, vec!["mac-mini", "notebook", "sem-uso"]);
}

/// Config do `ctowerd` sem `hub = true`: migra o `hub.db`, mas não mexe no
/// `ctowerd` (com aviso).
#[test]
fn sem_hub_true_nao_converte_o_ctowerd() {
    let fx = fixture();
    let cfg = fx.opts.ctower_config.join("config.toml");
    let sem = fx.config.replace("hub = true\n", "");
    std::fs::write(&cfg, &sem).unwrap();
    let rep = migrate_files(&fx.dirs, &fx.opts).unwrap();
    assert!(rep.local_env.is_none());
    assert!(rep
        .warnings
        .iter()
        .any(|w| w.contains("não tem hub = true")));
    assert_eq!(std::fs::read_to_string(&cfg).unwrap(), sem);
    assert!(!fx.opts.ctower_config.join("hub.token").exists());
    assert_eq!(rep.env_tokens, vec!["notebook", "sem-uso"]);
}

/// O binário: `cthubd migrate --from` com as variáveis de ambiente dos dois
/// daemons e a saída com os próximos passos.
#[test]
fn comando_migrate() {
    let fx = fixture();
    let free = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    Command::cargo_bin("cthubd")
        .unwrap()
        .env("CTHUB_CONFIG_DIR", &fx.dirs.config)
        .env("CTHUB_DATA_DIR", &fx.dirs.data)
        .env("CTHUB_PORT", free.to_string())
        .env("CTOWER_CONFIG_DIR", &fx.opts.ctower_config)
        .arg("migrate")
        .arg("--from")
        .arg(&fx.opts.from)
        .assert()
        .success()
        .stdout(contains("hub.db migrado"))
        .stdout(contains("Próximos passos"))
        .stdout(contains("ctower update"))
        .stdout(contains(format!("porta de hub_url para {free}")));
    let t: toml::Table = toml::from_str(
        &std::fs::read_to_string(fx.opts.ctower_config.join("config.toml")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        t["hub_url"].as_str(),
        Some(format!("http://127.0.0.1:{free}").as_str())
    );
    // CTHUB_PORT fora do padrão vai para o config do cthubd.
    let hub_cfg = std::fs::read_to_string(fx.dirs.config.join("config.toml")).unwrap();
    assert!(hub_cfg.contains(&format!("port = {free}")), "{hub_cfg}");
    Command::cargo_bin("cthubd")
        .unwrap()
        .env("CTHUB_CONFIG_DIR", &fx.dirs.config)
        .env("CTHUB_DATA_DIR", &fx.dirs.data)
        .env("CTHUB_PORT", free.to_string())
        .env("CTOWER_CONFIG_DIR", &fx.opts.ctower_config)
        .args(["migrate", "--from"])
        .arg(&fx.opts.from)
        .assert()
        .success()
        .stdout(contains("nada a fazer"));
}
