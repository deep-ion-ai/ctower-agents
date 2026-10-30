-- ledger.db de um ctowerd com hub = true (#64), gerado com o código do commit e65dd45
-- (base do épico #183) pelo teste gerar_fixture_hub_antigo e despejado com o
-- sqlite3.iterdump do Python. Esquema na versão 5. Fixture do cthubd migrate (#182):
-- ambiente local 0b6c...0001 (mac-mini, envio em processo), notebook 0b6c...0002
-- (token de sync de teste em notebook.token), token sem-uso sem env_id.
PRAGMA foreign_keys = OFF;
BEGIN TRANSACTION;
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
, uid TEXT);
INSERT INTO "decision" VALUES(1,1,'76a8cfdc-0513-4cc0-bf66-faa8c21b6df6','autonoma','decisão local','c','d',NULL,NULL,NULL,1792367443732,'01a1516c-bf14-720c-bac2-7e22440b3790');
CREATE TABLE event (
  id           INTEGER PRIMARY KEY,
  ts           INTEGER NOT NULL,
  project_id   INTEGER NOT NULL REFERENCES project(id),
  work_item_id INTEGER REFERENCES work_item(id),
  session_id   TEXT REFERENCES session(id),
  type         TEXT NOT NULL,
  payload      TEXT NOT NULL DEFAULT '{}'
, uid TEXT, env_id TEXT, hlc INTEGER);
INSERT INTO "event" VALUES(1,1792367443727,1,1,NULL,'work_item_created','{"gh_number":12,"kind":"epic"}','01a1516c-bf0f-7786-970a-593552837370','0b6c1f7e-1a2b-4c3d-8e9f-000000000001',117464592792092673);
INSERT INTO "event" VALUES(2,1792367443729,1,1,'76a8cfdc-0513-4cc0-bf66-faa8c21b6df6','session_created','{"cwd":"/home/u/worktrees/demo/wip__12","mode":"native","model":null,"tool":"claude"}','01a1516c-bf11-7460-8717-7a2f27ce4231','0b6c1f7e-1a2b-4c3d-8e9f-000000000001',117464592792223744);
INSERT INTO "event" VALUES(3,1792367443732,1,1,'76a8cfdc-0513-4cc0-bf66-faa8c21b6df6','decision_recorded','{"decision_id":1,"decision_uid":"01a1516c-bf14-720c-bac2-7e22440b3790","origin":"autonoma","title":"decisão local"}','01a1516c-bf14-720c-bac2-7e23b046899b','0b6c1f7e-1a2b-4c3d-8e9f-000000000001',117464592792420352);
CREATE TABLE gh_etag (
    key        TEXT PRIMARY KEY,
    etag       TEXT NOT NULL,
    updated_at INTEGER NOT NULL
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
CREATE TABLE ledger_meta (
  key   TEXT PRIMARY KEY,
  value INTEGER NOT NULL
);
INSERT INTO "ledger_meta" VALUES('hlc',117464592793010176);
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
INSERT INTO "project" VALUES(1,'demo','deep-ion-ai','demo','/home/u/projetos/demo','develop','["develop","main"]',1792367443721);
INSERT INTO "project" VALUES(2,'outro','deep-ion-ai','outro','/home/u/projetos/outro','develop','["develop","main"]',1792367443724);
INSERT INTO "project" VALUES(3,'rascunho','deep-ion-ai','rascunho','/home/u/projetos/rascunho','develop','["develop","main"]',1792367443741);
CREATE TABLE purging (project_id INTEGER PRIMARY KEY);
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
, model TEXT, mode TEXT NOT NULL DEFAULT 'native');
INSERT INTO "session" VALUES('76a8cfdc-0513-4cc0-bf66-faa8c21b6df6',1,'claude',NULL,NULL,NULL,'/home/u/worktrees/demo/wip__12','[]','starting',NULL,NULL,1792367443729,1792367443729,NULL,NULL,'native');
CREATE TABLE sync_outbox (
  seq        INTEGER PRIMARY KEY AUTOINCREMENT,
  kind       TEXT NOT NULL,
  key        TEXT NOT NULL,
  payload    TEXT NOT NULL,
  created_at INTEGER NOT NULL
);
INSERT INTO "sync_outbox" VALUES(9,'project','deep-ion-ai/rascunho','{"kind":"project","data":{"owner":"deep-ion-ai","repo":"rascunho","name":"rascunho","repo_path":"/home/u/projetos/rascunho","base_branch":"develop","protected_branches":["develop","main"],"created_at":1792367443741,"hlc":117464592793010176}}',1792367443741);
CREATE TABLE sync_state (
    project_id      INTEGER PRIMARY KEY REFERENCES project(id),
    last_attempt_at INTEGER NOT NULL,
    last_ok_at      INTEGER,
    last_error      TEXT
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
INSERT INTO "work_item" VALUES(1,1,12,'epic','backlog',NULL,NULL,1792367443726,NULL);
CREATE INDEX session_state_idx ON session(state);
CREATE INDEX decision_work_item_idx ON decision(work_item_id);
CREATE INDEX event_work_item_idx ON event(work_item_id, id);
CREATE INDEX pr_cache_work_item_idx ON pr_cache(work_item_id);
CREATE UNIQUE INDEX decision_uid_idx ON decision(uid);
CREATE UNIQUE INDEX event_uid_idx ON event(uid);
CREATE INDEX sync_outbox_key_idx ON sync_outbox(kind, key);
CREATE TRIGGER decision_review_once
BEFORE UPDATE OF reviewed_at, reviewed_in_pr ON decision
WHEN OLD.reviewed_at IS NOT NULL
BEGIN SELECT RAISE(ABORT, 'revisão de decision já registrada'); END;
CREATE TRIGGER event_no_delete BEFORE DELETE ON event
WHEN OLD.project_id NOT IN (SELECT project_id FROM purging)
BEGIN SELECT RAISE(ABORT, 'event é append-only'); END;
CREATE TRIGGER decision_no_delete BEFORE DELETE ON decision
WHEN NOT EXISTS (
  SELECT 1 FROM work_item w JOIN purging p ON p.project_id = w.project_id
  WHERE w.id = OLD.work_item_id)
BEGIN SELECT RAISE(ABORT, 'decision não pode ser apagada'); END;
CREATE TRIGGER decision_immutable
BEFORE UPDATE OF id, uid, work_item_id, session_id, origin, title, context, decision, consequences, created_at ON decision
BEGIN SELECT RAISE(ABORT, 'decision só pode ter a revisão alterada'); END;
CREATE TRIGGER event_no_update BEFORE UPDATE ON event
BEGIN SELECT RAISE(ABORT, 'event é append-only'); END;
DELETE FROM "sqlite_sequence";
INSERT INTO "sqlite_sequence" VALUES('sync_outbox',9);
COMMIT;
PRAGMA user_version = 5;
