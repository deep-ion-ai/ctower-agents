//! Migrações versionadas por `PRAGMA user_version`. Nunca edite uma migração
//! já publicada; acrescente uma nova ao fim de `MIGRATIONS`.

use crate::{LedgerError, Result};
use rusqlite::Connection;

pub const MIGRATIONS: &[&str] = &[
    r#"
CREATE TABLE project (
  id                 INTEGER PRIMARY KEY,
  name               TEXT NOT NULL UNIQUE,
  gh_owner           TEXT NOT NULL,
  gh_repo            TEXT NOT NULL,
  repo_path          TEXT NOT NULL,
  base_branch        TEXT NOT NULL DEFAULT 'develop',
  protected_branches TEXT NOT NULL DEFAULT '["develop","main","master"]',
  created_at         INTEGER NOT NULL
);

CREATE TABLE issue_cache (
  project_id     INTEGER NOT NULL REFERENCES project(id),
  gh_number      INTEGER NOT NULL,
  title          TEXT NOT NULL,
  state          TEXT NOT NULL,
  labels         TEXT NOT NULL DEFAULT '[]',
  parent_number  INTEGER,
  has_sub_issues INTEGER NOT NULL DEFAULT 0,
  url            TEXT NOT NULL,
  updated_at     INTEGER NOT NULL,
  synced_at      INTEGER NOT NULL,
  PRIMARY KEY (project_id, gh_number)
);

CREATE TABLE work_item (
  id            INTEGER PRIMARY KEY,
  project_id    INTEGER NOT NULL REFERENCES project(id),
  gh_number     INTEGER NOT NULL,
  kind          TEXT NOT NULL,
  status        TEXT NOT NULL DEFAULT 'backlog',
  branch        TEXT,
  worktree_path TEXT,
  created_at    INTEGER NOT NULL,
  closed_at     INTEGER,
  UNIQUE (project_id, gh_number)
);

CREATE TABLE session (
  id              TEXT PRIMARY KEY,
  work_item_id    INTEGER NOT NULL REFERENCES work_item(id),
  tool            TEXT NOT NULL,
  tool_session_id TEXT,
  transcript_path TEXT,
  backup_path     TEXT,
  cwd             TEXT NOT NULL,
  launch_cmd      TEXT NOT NULL,
  state           TEXT NOT NULL,
  exit_code       INTEGER,
  pid             INTEGER,
  started_at      INTEGER NOT NULL,
  last_event_at   INTEGER NOT NULL,
  ended_at        INTEGER
);
CREATE INDEX session_state_idx ON session(state);

CREATE TABLE decision (
  id             INTEGER PRIMARY KEY,
  work_item_id   INTEGER NOT NULL REFERENCES work_item(id),
  session_id     TEXT REFERENCES session(id),
  origin         TEXT NOT NULL,
  title          TEXT NOT NULL,
  context        TEXT NOT NULL,
  decision       TEXT NOT NULL,
  consequences   TEXT,
  reviewed_at    INTEGER,
  reviewed_in_pr INTEGER,
  created_at     INTEGER NOT NULL
);
CREATE INDEX decision_work_item_idx ON decision(work_item_id);

CREATE TABLE event (
  id           INTEGER PRIMARY KEY,
  ts           INTEGER NOT NULL,
  project_id   INTEGER NOT NULL REFERENCES project(id),
  work_item_id INTEGER REFERENCES work_item(id),
  session_id   TEXT REFERENCES session(id),
  type         TEXT NOT NULL,
  payload      TEXT NOT NULL DEFAULT '{}'
);
CREATE INDEX event_work_item_idx ON event(work_item_id, id);

CREATE TRIGGER event_no_update BEFORE UPDATE ON event
BEGIN SELECT RAISE(ABORT, 'event é append-only'); END;
CREATE TRIGGER event_no_delete BEFORE DELETE ON event
BEGIN SELECT RAISE(ABORT, 'event é append-only'); END;
CREATE TRIGGER decision_no_delete BEFORE DELETE ON decision
BEGIN SELECT RAISE(ABORT, 'decision não pode ser apagada'); END;
CREATE TRIGGER decision_immutable
BEFORE UPDATE OF id, work_item_id, session_id, origin, title, context, decision, consequences, created_at ON decision
BEGIN SELECT RAISE(ABORT, 'decision só pode ter a revisão alterada'); END;
CREATE TRIGGER decision_review_once
BEFORE UPDATE OF reviewed_at, reviewed_in_pr ON decision
WHEN OLD.reviewed_at IS NOT NULL
BEGIN SELECT RAISE(ABORT, 'revisão de decision já registrada'); END;
"#,
    // 2: espelho dos PRs das branches dos work items, ETags e estado do sync (Plano 3).
    r#"
CREATE TABLE pr_cache (
    project_id   INTEGER NOT NULL REFERENCES project(id),
    number       INTEGER NOT NULL,
    work_item_id INTEGER NOT NULL REFERENCES work_item(id),
    head         TEXT NOT NULL,
    base         TEXT NOT NULL,
    state        TEXT NOT NULL,
    url          TEXT NOT NULL,
    updated_at   INTEGER NOT NULL,
    synced_at    INTEGER NOT NULL,
    PRIMARY KEY (project_id, number)
);
CREATE INDEX pr_cache_work_item_idx ON pr_cache(work_item_id);
CREATE TABLE gh_etag (
    key        TEXT PRIMARY KEY,
    etag       TEXT NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE TABLE sync_state (
    project_id      INTEGER PRIMARY KEY REFERENCES project(id),
    last_attempt_at INTEGER NOT NULL,
    last_ok_at      INTEGER,
    last_error      TEXT
);
"#,
    // 3: exclusão de projeto. `event` e `decision` seguem append-only, exceto
    // as linhas do projeto marcado em `purging` durante a transação da purga.
    r#"
CREATE TABLE purging (project_id INTEGER PRIMARY KEY);
DROP TRIGGER event_no_delete;
CREATE TRIGGER event_no_delete BEFORE DELETE ON event
WHEN OLD.project_id NOT IN (SELECT project_id FROM purging)
BEGIN SELECT RAISE(ABORT, 'event é append-only'); END;
DROP TRIGGER decision_no_delete;
CREATE TRIGGER decision_no_delete BEFORE DELETE ON decision
WHEN NOT EXISTS (
  SELECT 1 FROM work_item w JOIN purging p ON p.project_id = w.project_id
  WHERE w.id = OLD.work_item_id)
BEGIN SELECT RAISE(ABORT, 'decision não pode ser apagada'); END;
"#,
    // 4: identidade entre ambientes e outbox para o hub (issue #57, spec do
    // ledger centralizado §3, §4, §5 e §9). `uid` é UUIDv7: as linhas antigas
    // recebem um gerado aqui, com o horário de criação nos 48 bits altos.
    // `hlc` é o relógio lógico híbrido (`ct_core::hlc`); eventos antigos usam
    // `ts`. `env_id` nulo = evento deste ambiente gravado antes da migração.
    r#"
CREATE TABLE ledger_meta (
  key   TEXT PRIMARY KEY,
  value INTEGER NOT NULL
);
INSERT INTO ledger_meta (key, value) SELECT 'hlc', coalesce(max(ts), 0) * 65536 FROM event;

ALTER TABLE decision ADD COLUMN uid TEXT;
UPDATE decision SET uid = lower(
  substr(printf('%012x', created_at), 1, 8) || '-' || substr(printf('%012x', created_at), 9, 4)
  || '-7' || substr(hex(randomblob(2)), 2, 3)
  || '-' || substr('89ab', 1 + abs(random() % 4), 1) || substr(hex(randomblob(2)), 2, 3)
  || '-' || hex(randomblob(6)));
CREATE UNIQUE INDEX decision_uid_idx ON decision(uid);
DROP TRIGGER decision_immutable;
CREATE TRIGGER decision_immutable
BEFORE UPDATE OF id, uid, work_item_id, session_id, origin, title, context, decision, consequences, created_at ON decision
BEGIN SELECT RAISE(ABORT, 'decision só pode ter a revisão alterada'); END;

DROP TRIGGER event_no_update;
ALTER TABLE event ADD COLUMN uid TEXT;
ALTER TABLE event ADD COLUMN env_id TEXT;
ALTER TABLE event ADD COLUMN hlc INTEGER;
UPDATE event SET hlc = ts * 65536, uid = lower(
  substr(printf('%012x', ts), 1, 8) || '-' || substr(printf('%012x', ts), 9, 4)
  || '-7' || substr(hex(randomblob(2)), 2, 3)
  || '-' || substr('89ab', 1 + abs(random() % 4), 1) || substr(hex(randomblob(2)), 2, 3)
  || '-' || hex(randomblob(6)));
CREATE UNIQUE INDEX event_uid_idx ON event(uid);
CREATE TRIGGER event_no_update BEFORE UPDATE ON event
BEGIN SELECT RAISE(ABORT, 'event é append-only'); END;

CREATE TABLE sync_outbox (
  seq        INTEGER PRIMARY KEY AUTOINCREMENT,
  kind       TEXT NOT NULL,
  key        TEXT NOT NULL,
  payload    TEXT NOT NULL,
  created_at INTEGER NOT NULL
);
CREATE INDEX sync_outbox_key_idx ON sync_outbox(kind, key);
"#,
    // 5: modelo e modo escolhidos ao iniciar a sessão (issue #94). Sessões
    // antigas: modelo padrão da ferramenta (nulo) e modo `native`.
    r#"
ALTER TABLE session ADD COLUMN model TEXT;
ALTER TABLE session ADD COLUMN mode TEXT NOT NULL DEFAULT 'native';
"#,
    // 6: `project` vira o cache da configuração vinda do hub (issue #177,
    // spec do hub como plano de controle §4.2). `config_rev` = `rev` do hub
    // que gerou a linha (nulo = projeto local legado); `unassigned_at` =
    // atribuição removida (a linha sai da lista; clone e worktrees ficam).
    // `location_*` e `applied_rev` guardam o último relato da aplicação,
    // reenviado em toda fotografia do projeto. `hub_repo_path` é o caminho
    // pedido pelo hub (vazio = `<repos_root>/<repo>`), para resolver de novo
    // quando `repos_root` muda. `ledger_meta` ganha
    // `config_rev` e `config_pulled_at` no primeiro pull.
    r#"
ALTER TABLE project ADD COLUMN config_rev INTEGER;
ALTER TABLE project ADD COLUMN unassigned_at INTEGER;
ALTER TABLE project ADD COLUMN location_state TEXT
  CHECK (location_state IS NULL OR location_state IN ('ok', 'error'));
ALTER TABLE project ADD COLUMN location_error TEXT;
ALTER TABLE project ADD COLUMN applied_rev INTEGER;
ALTER TABLE project ADD COLUMN hub_repo_path TEXT;
"#,
    // 7: poda após o ack do hub (issue #179, spec do hub como plano de
    // controle §6.3). Mesmo padrão da `purging` (migração 3): `event` e
    // `decision` seguem append-only, exceto as linhas do work item marcado em
    // `pruning` durante a transação da poda (o id sai antes do commit). A
    // forma `EXISTS ... = OLD.work_item_id` mantém bloqueado o DELETE de
    // evento sem work item (`NULL` nunca casa).
    // `pruned_uid` guarda o maior sufixo dos `uid` determinísticos (`gh:...`)
    // podados, para uma transição repetida depois da poda não reusar um `uid`
    // que o hub já tem (ele descartaria o evento como duplicado).
    r#"
CREATE TABLE pruning (work_item_id INTEGER PRIMARY KEY);
CREATE TABLE pruned_uid (base TEXT PRIMARY KEY, max_n INTEGER NOT NULL);
DROP TRIGGER event_no_delete;
CREATE TRIGGER event_no_delete BEFORE DELETE ON event
WHEN OLD.project_id NOT IN (SELECT project_id FROM purging)
 AND NOT EXISTS (SELECT 1 FROM pruning WHERE work_item_id = OLD.work_item_id)
BEGIN SELECT RAISE(ABORT, 'event é append-only'); END;
DROP TRIGGER decision_no_delete;
CREATE TRIGGER decision_no_delete BEFORE DELETE ON decision
WHEN NOT EXISTS (
  SELECT 1 FROM work_item w JOIN purging p ON p.project_id = w.project_id
  WHERE w.id = OLD.work_item_id)
 AND NOT EXISTS (SELECT 1 FROM pruning WHERE work_item_id = OLD.work_item_id)
BEGIN SELECT RAISE(ABORT, 'decision não pode ser apagada'); END;
"#,
    // 8: projeto sentinela do gestor (issue #219, spec do gestor §2.1).
    // `kind = 'gestor'` é a linha `_ctower/gestor`, criada sob demanda pela
    // sessão do gestor, com `repo_path` = a pasta do gestor. Fica fora das
    // listas, do pull, do sync do GitHub e do relato `project_location`.
    r#"
ALTER TABLE project ADD COLUMN kind TEXT NOT NULL DEFAULT 'github'
  CHECK (kind IN ('github', 'gestor'));
"#,
    // 9: SHA do head do PR (issue #286): a limpeza só apaga a branch que
    // ainda aponta para o último commit do PR. Nulo nos PRs já gravados.
    r#"
ALTER TABLE pr_cache ADD COLUMN head_sha TEXT;
"#,
    // 10: work items `concluido` no hub (issue #298). Cache somente leitura
    // do conjunto completo que `GET /sync/done` devolve, substituído a cada
    // pull; um `concluido` marcado à mão no hub não passa pelo ledger do
    // ambiente. `since` = quando o hub o concluiu (ms).
    r#"
CREATE TABLE hub_done (
  project_id INTEGER NOT NULL REFERENCES project(id),
  gh_number  INTEGER NOT NULL,
  since      INTEGER,
  PRIMARY KEY (project_id, gh_number)
);
"#,
    // 11: id da sessão no Remote Control do Claude Code (issue #197): o
    // `CLAUDE_CODE_BRIDGE_SESSION_ID` (`session_…`) que o hook relata, ou o
    // que aparece no PTY. Fica depois do fim da sessão (a retomada reconecta
    // na mesma conversa remota).
    r#"
ALTER TABLE session ADD COLUMN rc_session_id TEXT;
"#,
    // 12: rastreador e forja neutros de provedor (issue #347, spec do Jira e
    // GitLab §3.4). As linhas existentes viram GitHub, sem mudança visível.
    // `tracker_config`/`forge_config` = JSON de `JiraConfig`/`GitlabConfig`
    // (`{}` no GitHub); `branch_pattern` nulo = padrão do rastreador;
    // `attribution = 0` = sessões sem linhas de atribuição; `sync_role = 1` =
    // este ambiente envia o espelho do rastreador ao hub. `gh_owner`,
    // `gh_repo` e `gh_number` ficam com o nome (são o `owner`/`repo` da forja
    // e o número do rastreador).
    r#"
ALTER TABLE project ADD COLUMN tracker_kind TEXT NOT NULL DEFAULT 'github'
  CHECK (tracker_kind IN ('github', 'jira'));
ALTER TABLE project ADD COLUMN tracker_config TEXT NOT NULL DEFAULT '{}';
ALTER TABLE project ADD COLUMN forge_kind TEXT NOT NULL DEFAULT 'github'
  CHECK (forge_kind IN ('github', 'gitlab'));
ALTER TABLE project ADD COLUMN forge_config TEXT NOT NULL DEFAULT '{}';
ALTER TABLE project ADD COLUMN branch_pattern TEXT;
ALTER TABLE project ADD COLUMN attribution INTEGER NOT NULL DEFAULT 1;
ALTER TABLE project ADD COLUMN sync_role INTEGER NOT NULL DEFAULT 0;
"#,
    // 13: cache das regras do agente vigentes (issue #357, spec do Jira e
    // GitLab §10.2), substituído a cada pull com mudança. `epic_number = 0`
    // = regras do projeto. Só o pull escreve; nada daqui vai ao outbox.
    r#"
CREATE TABLE agent_rules_cache (
  project_id  INTEGER NOT NULL REFERENCES project(id) ON DELETE CASCADE,
  epic_number INTEGER NOT NULL,
  version     INTEGER NOT NULL,
  content     TEXT NOT NULL,
  PRIMARY KEY (project_id, epic_number)
);
"#,
    // 14: sprints do Jira de cada issue do espelho (JSON de `Vec<Sprint>`,
    // da mais antiga para a mais recente). `[]` nas issues já gravadas e no
    // GitHub; o sync do Jira relê o projeto uma vez para preencher.
    r#"
ALTER TABLE issue_cache ADD COLUMN sprints TEXT NOT NULL DEFAULT '[]';
"#,
    // 15: docs do projeto fora do repositório, como vêm do hub (issue #440):
    // JSON de `DocsConfig`. `{}` = desligado (projeto local legado ou hub
    // anterior).
    r#"
ALTER TABLE project ADD COLUMN docs_config TEXT NOT NULL DEFAULT '{}';
"#,
    // 16: metadados do tracker de cada issue do espelho (JSON de
    // `Vec<IssueMeta>`, na ordem de exibição). `[]` nas issues já gravadas;
    // o sync relê o projeto uma vez (`MIRROR_VERSION`) para preencher.
    r#"
ALTER TABLE issue_cache ADD COLUMN meta TEXT NOT NULL DEFAULT '[]';
"#,
    // 17: falhas seguidas do sync do projeto e o início da sequência (issue
    // #90): a UI só avisa a partir de algumas falhas, para não alarmar por
    // uma queda de rede passageira. Zeram na primeira rodada bem-sucedida.
    r#"
ALTER TABLE sync_state ADD COLUMN failures INTEGER NOT NULL DEFAULT 0;
ALTER TABLE sync_state ADD COLUMN failing_since INTEGER;
"#,
];

/// Aplica as migrações pendentes numa transação `IMMEDIATE`, relendo
/// `user_version` já com o lock de escrita (dois processos abrindo o mesmo
/// banco não aplicam a mesma migração duas vezes).
///
/// Se a aplicação ou o `COMMIT` falharem, a transação é desfeita antes de o
/// erro ser propagado, e a conexão volta ao modo autocommit.
pub fn migrate(conn: &Connection) -> Result<()> {
    migrate_with(conn, MIGRATIONS)
}

pub(crate) fn migrate_with(conn: &Connection, migrations: &[&str]) -> Result<()> {
    conn.execute_batch("BEGIN IMMEDIATE")?;
    let result = apply_pending(conn, migrations)
        .and_then(|()| conn.execute_batch("COMMIT").map_err(Into::into));
    if result.is_err() && !conn.is_autocommit() {
        let _ = conn.execute_batch("ROLLBACK");
    }
    result
}

fn apply_pending(conn: &Connection, migrations: &[&str]) -> Result<()> {
    let current: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    let supported = migrations.len() as i64;
    if current > supported {
        return Err(LedgerError::Corrupt(format!(
            "versão do esquema {current} é maior que a suportada ({supported})"
        )));
    }
    for (i, sql) in migrations.iter().enumerate().skip(current as usize) {
        conn.execute_batch(sql)?;
        conn.pragma_update(None, "user_version", (i + 1) as i64)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    #[test]
    fn migracao_e_idempotente() {
        let conn = Connection::open_in_memory().unwrap();
        super::migrate(&conn).unwrap();
        super::migrate(&conn).unwrap();
        let v: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, super::MIGRATIONS.len() as i64);
    }

    #[test]
    fn migracao_2_preserva_dados_da_versao_1() {
        let conn = Connection::open_in_memory().unwrap();
        super::migrate_with(&conn, &super::MIGRATIONS[..1]).unwrap();
        conn.execute(
            "INSERT INTO project (name, gh_owner, gh_repo, repo_path, created_at) VALUES ('p','o','r','/x',0)",
            [],
        )
        .unwrap();
        super::migrate(&conn).unwrap();
        let v: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, super::MIGRATIONS.len() as i64);
        let n: i64 = conn
            .query_row("SELECT count(*) FROM project", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1);
        for t in ["pr_cache", "gh_etag", "sync_state"] {
            let n: i64 = conn
                .query_row(
                    "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
                    [t],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(n, 1, "{t}");
        }
    }

    #[test]
    fn migracao_4_preserva_dados_e_preenche_uid_e_hlc() {
        let conn = Connection::open_in_memory().unwrap();
        super::migrate_with(&conn, &super::MIGRATIONS[..3]).unwrap();
        conn.execute_batch(
            "INSERT INTO project (name, gh_owner, gh_repo, repo_path, created_at) VALUES ('p','o','r','/x',0);
             INSERT INTO work_item (project_id, gh_number, kind, created_at) VALUES (1, 1, 'epic', 0);
             INSERT INTO event (ts, project_id, type) VALUES (1760000000000, 1, 'a');
             INSERT INTO event (ts, project_id, type) VALUES (1760000000500, 1, 'b');
             INSERT INTO decision (work_item_id, origin, title, context, decision, created_at)
               VALUES (1, 'autonoma', 't', 'c', 'd', 1760000000000);
             INSERT INTO decision (work_item_id, origin, title, context, decision, created_at)
               VALUES (1, 'autonoma', 't2', 'c', 'd', 1760000000000);",
        )
        .unwrap();
        super::migrate(&conn).unwrap();

        let evs: Vec<(String, i64, i64, Option<String>)> = conn
            .prepare("SELECT uid, ts, hlc, env_id FROM event ORDER BY id")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(evs.len(), 2);
        for (uid, ts, hlc, env) in &evs {
            let u = uuid::Uuid::parse_str(uid).unwrap();
            assert_eq!(u.get_version_num(), 7, "{uid}");
            assert_eq!(*hlc, ct_core::hlc::from_ms(*ts));
            // Os 48 bits altos do UUIDv7 são o horário original.
            let (secs, nanos) = u.get_timestamp().unwrap().to_unix();
            assert_eq!(secs as i64 * 1000 + nanos as i64 / 1_000_000, *ts);
            assert!(env.is_none());
        }
        assert_ne!(evs[0].0, evs[1].0);

        let uids: Vec<String> = conn
            .prepare("SELECT uid FROM decision ORDER BY id")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(uids.len(), 2);
        assert_ne!(uids[0], uids[1]);
        assert!(uids
            .iter()
            .all(|u| uuid::Uuid::parse_str(u).unwrap().get_version_num() == 7));

        // HLC parte do evento mais recente; uid da decisão é imutável e o
        // evento continua append-only.
        let hlc: i64 = conn
            .query_row("SELECT value FROM ledger_meta WHERE key = 'hlc'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(hlc, ct_core::hlc::from_ms(1760000000500));
        assert!(conn.execute("UPDATE decision SET uid = 'x'", []).is_err());
        assert!(conn.execute("UPDATE event SET uid = 'x'", []).is_err());
        let outbox: i64 = conn
            .query_row("SELECT count(*) FROM sync_outbox", [], |r| r.get(0))
            .unwrap();
        assert_eq!(outbox, 0);
    }

    /// Migração 7 (poda, #179) num `ledger.db` existente: os dados ficam, a
    /// tabela `pruning` nasce vazia e as triggers seguem bloqueando o DELETE
    /// de `event` e `decision` (a exceção da `purging` continua valendo).
    #[test]
    fn migracao_7_preserva_dados_e_mantem_append_only() {
        let conn = Connection::open_in_memory().unwrap();
        super::migrate_with(&conn, &super::MIGRATIONS[..6]).unwrap();
        conn.execute_batch(
            "INSERT INTO project (name, gh_owner, gh_repo, repo_path, created_at) VALUES ('p','o','r','/x',0);
             INSERT INTO project (name, gh_owner, gh_repo, repo_path, created_at) VALUES ('q','o','q','/y',0);
             INSERT INTO work_item (project_id, gh_number, kind, status, created_at, closed_at)
               VALUES (1, 1, 'issue', 'concluido', 0, 5);
             INSERT INTO work_item (project_id, gh_number, kind, created_at) VALUES (2, 1, 'issue', 0);
             INSERT INTO event (ts, project_id, work_item_id, type, uid) VALUES (1, 1, 1, 'a', 'u1');
             INSERT INTO event (ts, project_id, type, uid) VALUES (2, 1, 'b', 'u2');
             INSERT INTO event (ts, project_id, work_item_id, type, uid) VALUES (3, 2, 2, 'c', 'u3');
             INSERT INTO decision (work_item_id, origin, title, context, decision, created_at, uid)
               VALUES (1, 'autonoma', 't', 'c', 'd', 0, 'd1');",
        )
        .unwrap();
        super::migrate(&conn).unwrap();
        let v: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, super::MIGRATIONS.len() as i64);
        let n = |sql: &str| -> i64 { conn.query_row(sql, [], |r| r.get(0)).unwrap() };
        assert_eq!(n("SELECT count(*) FROM event"), 3);
        assert_eq!(n("SELECT count(*) FROM decision"), 1);
        assert_eq!(n("SELECT count(*) FROM work_item"), 2);
        assert_eq!(n("SELECT count(*) FROM pruning"), 0);
        assert_eq!(n("SELECT count(*) FROM pruned_uid"), 0);
        assert!(conn.execute("DELETE FROM event", []).is_err());
        assert!(conn.execute("DELETE FROM decision", []).is_err());
        assert!(conn
            .execute("DELETE FROM event WHERE work_item_id = 1", [])
            .is_err());

        // A exceção da exclusão de projeto (migração 3) continua.
        conn.execute("INSERT INTO purging (project_id) VALUES (2)", [])
            .unwrap();
        conn.execute("DELETE FROM event WHERE project_id = 2", [])
            .unwrap();
        assert!(conn
            .execute("DELETE FROM event WHERE project_id = 1", [])
            .is_err());
        conn.execute("DELETE FROM purging", []).unwrap();

        // A nova: só as linhas do work item marcado.
        conn.execute("INSERT INTO pruning (work_item_id) VALUES (1)", [])
            .unwrap();
        conn.execute("DELETE FROM decision WHERE work_item_id = 1", [])
            .unwrap();
        conn.execute("DELETE FROM event WHERE work_item_id = 1", [])
            .unwrap();
        assert!(conn
            .execute("DELETE FROM event WHERE work_item_id IS NULL", [])
            .is_err());
    }

    /// Migração 12 (#347): o projeto de um `ledger.db` existente (no esquema
    /// da develop, até a 11) vira GitHub, com o padrão de branch do
    /// rastreador e atribuição ligada.
    #[test]
    fn migracao_12_projetos_existentes_viram_github() {
        let conn = Connection::open_in_memory().unwrap();
        super::migrate_with(&conn, &super::MIGRATIONS[..11]).unwrap();
        conn.execute_batch(
            "INSERT INTO project (name, gh_owner, gh_repo, repo_path, created_at) VALUES ('p','o','r','/x',0);
             INSERT INTO work_item (project_id, gh_number, kind, branch, created_at)
               VALUES (1, 7, 'issue', 'feat/7-x', 0);",
        )
        .unwrap();
        super::migrate(&conn).unwrap();
        let row: (String, String, String, String, Option<String>, i64, i64) = conn
            .query_row(
                "SELECT tracker_kind, tracker_config, forge_kind, forge_config, branch_pattern,
                        attribution, sync_role FROM project",
                [],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                        r.get(6)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(
            row,
            (
                "github".into(),
                "{}".into(),
                "github".into(),
                "{}".into(),
                None,
                1,
                0
            )
        );
        let branch: String = conn
            .query_row("SELECT branch FROM work_item", [], |r| r.get(0))
            .unwrap();
        assert_eq!(branch, "feat/7-x");
        assert!(conn
            .execute("UPDATE project SET tracker_kind = 'linear'", [])
            .is_err());
        assert!(conn
            .execute("UPDATE project SET forge_kind = 'bitbucket'", [])
            .is_err());
    }

    /// Migração 14: as issues já espelhadas ficam sem sprint (`[]`) e a
    /// coluna nova vai e volta com o `Vec<Sprint>`.
    #[test]
    fn migracao_14_issues_existentes_ficam_sem_sprint() {
        let conn = Connection::open_in_memory().unwrap();
        super::migrate_with(&conn, &super::MIGRATIONS[..13]).unwrap();
        conn.execute_batch(
            "INSERT INTO project (name, gh_owner, gh_repo, repo_path, created_at) VALUES ('p','o','r','/x',0);
             INSERT INTO issue_cache (project_id, gh_number, title, state, url, updated_at, synced_at)
               VALUES (1, 7, 't', 'open', 'u', 1, 1);",
        )
        .unwrap();
        super::migrate(&conn).unwrap();
        let gh = crate::github::GhMirror(&conn);
        let mut i = gh.get_issue(1, 7).unwrap().unwrap();
        assert!(i.sprints.is_empty());
        i.sprints = vec![ct_core::Sprint {
            id: 2273,
            name: "Sprint 48, fase=2".into(),
            state: ct_core::SprintState::Active,
            board_id: Some(402),
            start_date: Some(1),
            end_date: None,
            complete_date: None,
        }];
        gh.upsert_issue(1, &i, 2).unwrap();
        assert_eq!(gh.list_issues(1).unwrap(), vec![i]);
    }

    /// Migração 16: as issues já espelhadas ficam sem metadados (`[]`) e a
    /// coluna nova vai e volta com o `Vec<IssueMeta>`.
    #[test]
    fn migracao_16_issues_existentes_ficam_sem_metadados() {
        let conn = Connection::open_in_memory().unwrap();
        super::migrate_with(&conn, &super::MIGRATIONS[..15]).unwrap();
        conn.execute_batch(
            "INSERT INTO project (name, gh_owner, gh_repo, repo_path, created_at) VALUES ('p','o','r','/x',0);
             INSERT INTO issue_cache (project_id, gh_number, title, state, url, updated_at, synced_at)
               VALUES (1, 7, 't', 'open', 'u', 1, 1);",
        )
        .unwrap();
        super::migrate(&conn).unwrap();
        let gh = crate::github::GhMirror(&conn);
        let mut i = gh.get_issue(1, 7).unwrap().unwrap();
        assert!(i.meta.is_empty());
        i.meta = vec![crate::IssueMeta {
            field: "priority".into(),
            label: "Prioridade".into(),
            value: "Alta".into(),
        }];
        gh.upsert_issue(1, &i, 2).unwrap();
        assert_eq!(gh.list_issues(1).unwrap(), vec![i]);
    }

    #[test]
    fn event_e_append_only() {
        let conn = Connection::open_in_memory().unwrap();
        super::migrate(&conn).unwrap();
        conn.execute(
            "INSERT INTO project (name, gh_owner, gh_repo, repo_path, created_at) VALUES ('p','o','r','/x',0)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO event (ts, project_id, type) VALUES (0, 1, 't')",
            [],
        )
        .unwrap();
        assert!(conn.execute("UPDATE event SET type = 'x'", []).is_err());
        assert!(conn.execute("DELETE FROM event", []).is_err());
    }

    #[test]
    fn versao_maior_que_a_suportada_e_erro() {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "user_version", super::MIGRATIONS.len() as i64 + 1)
            .unwrap();
        let err = super::migrate(&conn).unwrap_err();
        assert!(matches!(err, crate::LedgerError::Corrupt(_)));
        // A transação foi desfeita e nenhuma tabela foi criada.
        assert!(conn.is_autocommit());
        let tabelas: i64 = conn
            .query_row("SELECT count(*) FROM sqlite_master", [], |r| r.get(0))
            .unwrap();
        assert_eq!(tabelas, 0);
    }

    fn conn_com_decisao(reviewed: bool) -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        super::migrate(&conn).unwrap();
        conn.execute_batch(
            "INSERT INTO project (name, gh_owner, gh_repo, repo_path, created_at) VALUES ('p','o','r','/x',0);
             INSERT INTO work_item (project_id, gh_number, kind, created_at) VALUES (1, 1, 'epic', 0);",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO decision (work_item_id, origin, title, context, decision, reviewed_at, created_at)
             VALUES (1, 'autonoma', 't', 'c', 'd', ?1, 0)",
            [reviewed.then_some(5i64)],
        )
        .unwrap();
        conn
    }

    #[test]
    fn revisao_registrada_nao_pode_ser_reescrita() {
        let conn = conn_com_decisao(true);
        assert!(conn
            .execute("UPDATE decision SET reviewed_at = 9", [])
            .is_err());
        assert!(conn
            .execute("UPDATE decision SET reviewed_in_pr = 9", [])
            .is_err());
        assert!(conn
            .execute("UPDATE decision SET reviewed_at = NULL", [])
            .is_err());
    }

    #[test]
    fn revisao_pendente_pode_ser_registrada() {
        let conn = conn_com_decisao(false);
        conn.execute(
            "UPDATE decision SET reviewed_at = 9, reviewed_in_pr = 3",
            [],
        )
        .unwrap();
    }

    /// Se o `COMMIT` falha (aqui, por uma FK adiada violada), a transação é
    /// desfeita: a conexão volta ao autocommit e nada da migração fica.
    #[test]
    fn commit_falho_desfaz_a_transacao() {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", true).unwrap();
        let migracoes: &[&str] = &["
            CREATE TABLE a (id INTEGER PRIMARY KEY);
            CREATE TABLE b (a_id INTEGER REFERENCES a(id) DEFERRABLE INITIALLY DEFERRED);
            INSERT INTO b VALUES (99);
        "];
        assert!(super::migrate_with(&conn, migracoes).is_err());
        assert!(conn.is_autocommit());
        let v: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, 0);
        let tabelas: i64 = conn
            .query_row("SELECT count(*) FROM sqlite_master", [], |r| r.get(0))
            .unwrap();
        assert_eq!(tabelas, 0);
    }

    #[test]
    fn id_da_decisao_e_imutavel() {
        let conn = conn_com_decisao(false);
        assert!(conn.execute("UPDATE decision SET id = 99", []).is_err());
    }
}
