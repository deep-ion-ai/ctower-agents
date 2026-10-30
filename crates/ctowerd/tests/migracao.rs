//! Migração de ponta a ponta (#182): o `hub.db` e o `ledger.db` de um
//! `ctowerd` com `hub = true` (fixtures geradas pelo código anterior ao
//! épico #183) passam pelo `cthubd migrate`; o `cthubd` migrado sobe e o
//! `ctowerd` novo, com o config convertido, puxa a configuração: os
//! projetos legados atribuídos mantêm o `id` local e são aplicados, e o
//! legado que o hub não conhece fica "fora do hub" (`unassigned_at`).

use ct_ledger::{AssignmentFilter, AssignmentState, Hub, Ledger, SharedHub};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

mod common;

const LOCAL: &str = "0b6c1f7e-1a2b-4c3d-8e9f-000000000001";
const HUB_SQL: &str = include_str!("../../cthubd/tests/fixtures/hub-antigo.sql");
const LEDGER_SQL: &str = include_str!("../../cthubd/tests/fixtures/ledger-antigo.sql");

fn git(dir: &Path, args: &[&str]) {
    assert!(Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .status()
        .unwrap()
        .success());
}

/// Banco antigo a partir do despejo, com os caminhos `/home/u/...` trocados
/// por `<root>/u/...` (o ambiente valida os clones de verdade).
fn load(db: &Path, sql: &str, fix: &str, root: &Path) {
    let c = rusqlite::Connection::open(db).unwrap();
    c.execute_batch(sql).unwrap();
    c.execute(
        &format!("UPDATE {fix} SET repo_path = replace(repo_path, '/home/u/', ?1)"),
        [format!("{}/u/", root.display())],
    )
    .unwrap();
    c.query_row("PRAGMA journal_mode = WAL", [], |r| r.get::<_, String>(0))
        .unwrap();
}

#[test]
fn hub_antigo_migrado_e_ctowerd_puxando() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let (cfg, data) = (root.join("ctower/config"), root.join("ctower/data"));
    std::fs::create_dir_all(&cfg).unwrap();
    std::fs::create_dir_all(&data).unwrap();
    for repo in ["demo", "outro"] {
        let r = root.join("u/projetos").join(repo);
        std::fs::create_dir_all(&r).unwrap();
        git(&r, &["init", "-q", "-b", "develop"]);
        git(
            &r,
            &[
                "remote",
                "add",
                "origin",
                &format!("https://github.com/deep-ion-ai/{repo}.git"),
            ],
        );
    }
    load(&data.join("hub.db"), HUB_SQL, "project_location", root);
    load(&data.join("ledger.db"), LEDGER_SQL, "project", root);
    let gh = root.join("gh-falso");
    std::fs::write(&gh, "#!/bin/sh\necho 'gh: sem rede no teste' >&2\nexit 1\n").unwrap();
    std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();
    // A porta do config só serve à migração; o daemon sobe com
    // `CTOWER_PORT` (ver `common::start_ctowerd`).
    let port = std::cell::Cell::new(common::free_port());
    std::fs::write(
        cfg.join("config.toml"),
        format!(
            "env_name = \"mac-mini\"\nhub = true\nhub_backup_keep = 3\nport = {}\n\
             claude_bin = \"true\"\nctower_bin = \"true\"\nworktrees_root = \"{}\"\n\
             repos_root = \"{}\"\ngh_bin = \"{}\"\ngh_api_url = \"http://127.0.0.1:1\"\n\
             gh_sync_interval_secs = 0\nupdate_check_interval_hours = 0\n",
            port.get(),
            root.join("wt").display(),
            root.join("u/projetos").display(),
            gh.display()
        ),
    )
    .unwrap();
    std::fs::write(cfg.join("env_id"), LOCAL).unwrap();

    // O listener do hub fica preso desde já: sem corrida de porta.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let hub_port = listener.local_addr().unwrap().port();
    let dirs = cthubd::config::Dirs {
        config: root.join("cthub/config"),
        data: root.join("cthub/data"),
    };
    let opts = cthubd::migrate::MigrateOpts {
        from: data.clone(),
        ctower_config: cfg.clone(),
        env_name: None,
        force: false,
        hub_port,
    };
    let rep = cthubd::migrate::migrate_files(&dirs, &opts).unwrap();
    assert_eq!(rep.assignments, 3);

    let hub = SharedHub::new(Hub::open(&dirs.hub_db()).unwrap());
    let app = cthubd::router(cthubd::HubState::new(hub.clone(), "ui", "hub", "hub-id"));
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

    let d = common::start_ctowerd(&port, &cfg, || {
        let mut c = Command::new(env!("CARGO_BIN_EXE_ctowerd"));
        c.env("CTOWER_CONFIG_DIR", &cfg)
            .env("CTOWER_DATA_DIR", &data)
            .env("RUST_LOG", "warn");
        c
    });
    // O ambiente puxa, valida os dois clones e relata ao hub; o outbox
    // antigo (o "rascunho", nunca enviado) vai para a quarentena.
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let (states, quarantine) = {
            let h = hub.lock();
            let a: Vec<_> = h
                .list_assignments(&AssignmentFilter {
                    env: Some(LOCAL.into()),
                    ..Default::default()
                })
                .unwrap()
                .into_iter()
                .map(|a| (a.gh_repo, a.state, a.error))
                .collect();
            (a, h.quarantine_count("deep-ion-ai", "rascunho").unwrap())
        };
        if states.len() == 2
            && states.iter().all(|s| s.1 == AssignmentState::Applied)
            && quarantine > 0
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "o ctowerd não aplicou as atribuições: {states:?}, quarentena {quarantine}"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    drop(d);

    let l = Ledger::open(&data.join("ledger.db"), LOCAL).unwrap();
    let mut ps: Vec<_> = l
        .list_projects()
        .unwrap()
        .into_iter()
        .map(|p| (p.id, p.gh_repo, p.config_rev.is_some()))
        .collect();
    ps.sort();
    // Os legados atribuídos mantêm o id local (o histórico segue ligado).
    assert_eq!(
        ps,
        vec![(1, "demo".into(), true), (2, "outro".into(), true)]
    );
    let fora = l.list_unassigned_projects().unwrap();
    assert_eq!(fora.len(), 1);
    assert_eq!(
        (fora[0].id, fora[0].gh_repo.as_str(), fora[0].config_rev),
        (3, "rascunho", None)
    );
    assert!(fora[0].unassigned_at.is_some());
    // O histórico continua no hub.
    let h = hub.lock();
    let w = h
        .find_work_item("deep-ion-ai", "demo", 12)
        .unwrap()
        .unwrap();
    assert_eq!(h.list_sessions(w.id).unwrap().len(), 1);
}
