-- hub.db de um ctowerd com hub = true (#64), gerado com o código do commit e65dd45
-- (base do épico #183) pelo teste gerar_fixture_hub_antigo e despejado com o
-- sqlite3.iterdump do Python. Esquema na versão 5. Fixture do cthubd migrate (#182):
-- ambiente local 0b6c...0001 (mac-mini, envio em processo), notebook 0b6c...0002
-- (token de sync de teste em notebook.token), token sem-uso sem env_id.
PRAGMA foreign_keys = OFF;
BEGIN TRANSACTION;
CREATE TABLE decision (
  id             INTEGER PRIMARY KEY,
  uid            TEXT NOT NULL UNIQUE,
  env_id         TEXT NOT NULL REFERENCES env(env_id),
  work_item_id   INTEGER NOT NULL REFERENCES work_item(id),
  session_id     TEXT,
  origin         TEXT NOT NULL,
  title          TEXT NOT NULL,
  context        TEXT NOT NULL,
  decision       TEXT NOT NULL,
  consequences   TEXT,
  reviewed_at    INTEGER,
  reviewed_in_pr INTEGER,
  created_at     INTEGER NOT NULL
);
INSERT INTO "decision" VALUES(1,'01a1516c-bf14-720c-bac2-7e22440b3790','0b6c1f7e-1a2b-4c3d-8e9f-000000000001',1,'76a8cfdc-0513-4cc0-bf66-faa8c21b6df6','autonoma','decisão local','c','d',NULL,NULL,NULL,1792367443732);
INSERT INTO "decision" VALUES(2,'01a1516c-bf3e-7614-a4b2-bd5585267c60','0b6c1f7e-1a2b-4c3d-8e9f-000000000002',1,NULL,'orientada','decisão do notebook','c','d',NULL,1792367443774,NULL,1792367443774);
CREATE TABLE env (
  env_id       TEXT PRIMARY KEY,
  name         TEXT NOT NULL,
  created_at   INTEGER NOT NULL,
  last_seen_at INTEGER,
  last_seq     INTEGER NOT NULL DEFAULT 0
, pending INTEGER NOT NULL DEFAULT 0);
INSERT INTO "env" VALUES('0b6c1f7e-1a2b-4c3d-8e9f-000000000001','mac-mini',1792367443735,1792367443735,8,0);
INSERT INTO "env" VALUES('0b6c1f7e-1a2b-4c3d-8e9f-000000000002','notebook',1792367443782,1792367443782,7,0);
CREATE TABLE env_token (
  name       TEXT PRIMARY KEY,
  token_hash TEXT NOT NULL UNIQUE,
  env_id     TEXT UNIQUE,
  created_at INTEGER NOT NULL
);
INSERT INTO "env_token" VALUES('notebook','394462a938eeb0659d0b37169d44fafaaae7838c89863da813e0019f6d91441a','0b6c1f7e-1a2b-4c3d-8e9f-000000000002',1792367443744);
INSERT INTO "env_token" VALUES('sem-uso','85aec89d27a9b226e2e2c65f5f1e80de9cc5ce65f04862a9c12e9b4ca3980070',NULL,1792367443746);
CREATE TABLE event (
  id           INTEGER PRIMARY KEY,
  uid          TEXT NOT NULL UNIQUE,
  env_id       TEXT NOT NULL REFERENCES env(env_id),
  hlc          INTEGER NOT NULL,
  ts           INTEGER NOT NULL,
  project_id   INTEGER NOT NULL REFERENCES project(id),
  work_item_id INTEGER REFERENCES work_item(id),
  session_id   TEXT,
  type         TEXT NOT NULL,
  payload      TEXT NOT NULL DEFAULT '{}'
);
INSERT INTO "event" VALUES(1,'01a1516c-bf0f-7786-970a-593552837370','0b6c1f7e-1a2b-4c3d-8e9f-000000000001',117464592792092673,1792367443727,1,1,NULL,'work_item_created','{"gh_number":12,"kind":"epic"}');
INSERT INTO "event" VALUES(2,'01a1516c-bf11-7460-8717-7a2f27ce4231','0b6c1f7e-1a2b-4c3d-8e9f-000000000001',117464592792223744,1792367443729,1,1,'76a8cfdc-0513-4cc0-bf66-faa8c21b6df6','session_created','{"cwd":"/home/u/worktrees/demo/wip__12","mode":"native","model":null,"tool":"claude"}');
INSERT INTO "event" VALUES(3,'01a1516c-bf14-720c-bac2-7e23b046899b','0b6c1f7e-1a2b-4c3d-8e9f-000000000001',117464592792420352,1792367443732,1,1,'76a8cfdc-0513-4cc0-bf66-faa8c21b6df6','decision_recorded','{"decision_id":1,"decision_uid":"01a1516c-bf14-720c-bac2-7e22440b3790","origin":"autonoma","title":"decisão local"}');
INSERT INTO "event" VALUES(4,'01a1516c-bf3b-70ef-b538-2ddf606b9592','0b6c1f7e-1a2b-4c3d-8e9f-000000000002',117464592794976257,1792367443771,1,1,NULL,'work_item_created','{"gh_number":12,"kind":"epic"}');
INSERT INTO "event" VALUES(5,'01a1516c-bf3e-7614-a4b2-bd5688e984e2','0b6c1f7e-1a2b-4c3d-8e9f-000000000002',117464592795172864,1792367443774,1,1,NULL,'decision_recorded','{"decision_id":1,"decision_uid":"01a1516c-bf3e-7614-a4b2-bd5585267c60","origin":"orientada","title":"decisão do notebook"}');
CREATE TABLE gh_etag (
  key        TEXT PRIMARY KEY,
  etag       TEXT NOT NULL,
  updated_at INTEGER NOT NULL
);
CREATE TABLE hub_meta (
  key   TEXT PRIMARY KEY,
  value INTEGER NOT NULL
);
INSERT INTO "hub_meta" VALUES('hlc',0);
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
  key                TEXT NOT NULL UNIQUE,
  gh_owner           TEXT NOT NULL,
  gh_repo            TEXT NOT NULL,
  name               TEXT NOT NULL,
  base_branch        TEXT NOT NULL DEFAULT 'develop',
  protected_branches TEXT NOT NULL DEFAULT '["develop","main","master"]',
  created_at         INTEGER NOT NULL,
  attrs_hlc          INTEGER NOT NULL DEFAULT 0,
  attrs_env          TEXT NOT NULL DEFAULT ''
);
INSERT INTO "project" VALUES(1,'deep-ion-ai/demo','deep-ion-ai','demo','demo','develop','["develop","main"]',1792367443721,117464592794583040,'0b6c1f7e-1a2b-4c3d-8e9f-000000000002');
INSERT INTO "project" VALUES(2,'deep-ion-ai/outro','deep-ion-ai','outro','outro','develop','["develop","main"]',1792367443724,117464592791896064,'0b6c1f7e-1a2b-4c3d-8e9f-000000000001');
INSERT INTO "project" VALUES(3,'deep-ion-ai/velho','deep-ion-ai','velho','velho','develop','["develop","main"]',1792367443768,117464592794779648,'0b6c1f7e-1a2b-4c3d-8e9f-000000000002');
CREATE TABLE project_location (
  project_id INTEGER NOT NULL REFERENCES project(id),
  env_id     TEXT NOT NULL REFERENCES env(env_id),
  repo_path  TEXT NOT NULL,
  removed    INTEGER NOT NULL DEFAULT 0,
  hlc        INTEGER NOT NULL,
  PRIMARY KEY (project_id, env_id)
);
INSERT INTO "project_location" VALUES(1,'0b6c1f7e-1a2b-4c3d-8e9f-000000000001','/home/u/projetos/demo',0,117464592791699456);
INSERT INTO "project_location" VALUES(2,'0b6c1f7e-1a2b-4c3d-8e9f-000000000001','/home/u/projetos/outro',0,117464592791896064);
INSERT INTO "project_location" VALUES(1,'0b6c1f7e-1a2b-4c3d-8e9f-000000000002','/home/b/projetos/demo',0,117464592794583040);
INSERT INTO "project_location" VALUES(3,'0b6c1f7e-1a2b-4c3d-8e9f-000000000002','/home/b/projetos/velho',1,117464592795303936);
CREATE TABLE session (
  id              TEXT PRIMARY KEY,
  env_id          TEXT NOT NULL REFERENCES env(env_id),
  work_item_id    INTEGER NOT NULL REFERENCES work_item(id),
  tool            TEXT NOT NULL,
  tool_session_id TEXT,
  transcript_path TEXT,
  cwd             TEXT NOT NULL,
  state           TEXT NOT NULL,
  exit_code       INTEGER,
  pid             INTEGER,
  started_at      INTEGER NOT NULL,
  last_event_at   INTEGER NOT NULL,
  ended_at        INTEGER
, model TEXT, mode TEXT NOT NULL DEFAULT 'native');
INSERT INTO "session" VALUES('76a8cfdc-0513-4cc0-bf66-faa8c21b6df6','0b6c1f7e-1a2b-4c3d-8e9f-000000000001',1,'claude',NULL,NULL,'/home/u/worktrees/demo/wip__12','starting',NULL,NULL,1792367443729,1792367443729,NULL,NULL,'native');
CREATE TABLE sync_state (
  project_id      INTEGER PRIMARY KEY REFERENCES project(id),
  last_attempt_at INTEGER NOT NULL,
  last_ok_at      INTEGER,
  last_error      TEXT
);
CREATE TABLE work_item (
  id         INTEGER PRIMARY KEY,
  project_id INTEGER NOT NULL REFERENCES project(id),
  gh_number  INTEGER NOT NULL,
  kind       TEXT NOT NULL DEFAULT 'issue',
  status     TEXT NOT NULL DEFAULT 'backlog',
  branch     TEXT,
  created_at INTEGER NOT NULL,
  closed_at  INTEGER,
  attrs_hlc  INTEGER NOT NULL DEFAULT 0,
  attrs_env  TEXT NOT NULL DEFAULT '',
  UNIQUE (project_id, gh_number)
);
INSERT INTO "work_item" VALUES(1,1,12,'epic','backlog',NULL,1792367443726,NULL,117464592794976256,'0b6c1f7e-1a2b-4c3d-8e9f-000000000002');
CREATE TABLE work_item_location (
  work_item_id  INTEGER NOT NULL REFERENCES work_item(id),
  env_id        TEXT NOT NULL REFERENCES env(env_id),
  worktree_path TEXT,
  hlc           INTEGER NOT NULL,
  PRIMARY KEY (work_item_id, env_id)
);
INSERT INTO "work_item_location" VALUES(1,'0b6c1f7e-1a2b-4c3d-8e9f-000000000001',NULL,117464592792092672);
INSERT INTO "work_item_location" VALUES(1,'0b6c1f7e-1a2b-4c3d-8e9f-000000000002',NULL,117464592794976256);
CREATE INDEX session_work_item_idx ON session(work_item_id);
CREATE INDEX decision_work_item_idx ON decision(work_item_id);
CREATE INDEX event_work_item_idx ON event(work_item_id, hlc);
CREATE INDEX event_status_idx ON event(work_item_id, hlc, env_id) WHERE type = 'status_changed';
CREATE INDEX pr_cache_work_item_idx ON pr_cache(work_item_id);
CREATE TRIGGER event_no_update BEFORE UPDATE ON event
BEGIN SELECT RAISE(ABORT, 'event é append-only'); END;
CREATE TRIGGER event_no_delete BEFORE DELETE ON event
BEGIN SELECT RAISE(ABORT, 'event é append-only'); END;
CREATE TRIGGER decision_no_delete BEFORE DELETE ON decision
BEGIN SELECT RAISE(ABORT, 'decision não pode ser apagada'); END;
CREATE TRIGGER decision_immutable
BEFORE UPDATE OF id, uid, env_id, work_item_id, session_id, origin, title, context, decision, consequences, created_at ON decision
BEGIN SELECT RAISE(ABORT, 'decision só pode ter a revisão alterada'); END;
CREATE TRIGGER decision_review_once
BEFORE UPDATE OF reviewed_at, reviewed_in_pr ON decision
WHEN OLD.reviewed_at IS NOT NULL
BEGIN SELECT RAISE(ABORT, 'revisão de decision já registrada'); END;
COMMIT;
PRAGMA user_version = 5;
