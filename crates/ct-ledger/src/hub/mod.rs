//! `hub.db`: a visão consolidada de todos os ambientes (spec do ledger
//! centralizado §2, §4 e §5). Arquivo separado do `ledger.db`, com esquema
//! próprio. Só o `cthubd` o abre.
//!
//! A ingestão é idempotente: cada lote do outbox de um ambiente é aplicado
//! por `uid` (decisões, eventos) ou chave natural (projeto `owner/repo`,
//! work item por projeto e número, sessão por id). Entre ambientes:
//!
//! - **status do work item** = `to` do evento `status_changed` ou
//!   `status_set_manual` (mudança manual pela UI do hub) mais recente por HLC, com desempate por `env_id`; evento atrasado fica no histórico
//!   mas não sobrescreve um status mais novo;
//! - **atributos de work item** seguem a fotografia mais recente (HLC);
//! - **sessão**: vale a cópia com `last_event_at` maior (só o dono a altera);
//! - **revisão de decisão**: vale a primeira.
//!
//! Projetos têm origem no hub (spec do hub como plano de controle §3.1 e
//! §5, issue #175): o hub nunca cria projeto a partir de um push e ignora os
//! atributos de projeto vindos do ambiente (dele só vale o relato
//! `project_location`). Cada linha do lote segue o projeto a que pertence:
//! cadastrado = ingestão normal; com lápide = descartada; desconhecido =
//! `push_quarantine`, reaplicada quando o projeto for cadastrado.

mod agent;
mod assignments;
mod config;
mod env_keys;
mod health;
mod legacy;
mod priority;
mod proposals;
mod push;
mod rules;
mod tracker;
mod views;
mod web_sessions;

pub use agent::{
    AgentAssignment, AgentDecision, AgentDecisionFilter, AgentDecisions, AgentEnv, AgentEvent,
    AgentItemRef, AgentNode, AgentOverview, AgentPr, AgentProject, AgentProjectSummary,
    AgentSession, AgentWorkItem,
};
pub use assignments::{AssignmentAdded, AssignmentFilter, AssignmentState, HubAssignment};
pub use config::{
    DeleteImpact, HubProjectPatch, NewHubProject, ProjectAdded, ProjectProvider, QuarantineGroup,
};
pub use env_keys::{EnvAssignmentRev, EnvPubkey, EnvSecretsInput};
pub use health::{
    HealthCounts, HealthFilter, HealthKind, HealthSignal, HealthThresholds, HubHealth, Severity,
};
pub use legacy::LegacyAdoption;
pub use priority::{
    HubRanking, HubRankingItem, Placement, RankingFilter, RANKING_LIMIT, RANKING_MAX,
};
pub use proposals::{
    HubProposal, HubProposals, NewProposal, ProposalDecided, ProposalFilter, PROPOSAL_PENDING_MAX,
    PROPOSAL_TTL_MS,
};
pub use rules::{
    HubRuleVersion, HubRules, HubRulesHistory, HubRulesProposal, HubRulesProposals,
    RulesProposalFilter,
};
pub use tracker::HubTrackerStatus;

pub use push::{
    ChannelStates, HubPushDevice, NewPushSubscription, PushEvent, PushNotice, PushTarget,
    CHANNEL_STALE_MS, PUSH_FAILURES_MAX, PUSH_SUBSCRIPTIONS_MAX,
};
pub use web_sessions::{HubWebSession, IssuedWebSession, WEB_SESSION_TOUCH_MS};

pub use views::{
    DecisionCounts, DecisionScope, HubDashboard, HubDashboardProject, HubDashboardWorkItem,
    HubHistory, HubLocation, HubProjectView, HubTree, HubTreeItem, HubTreeNode, WorkItemSummary,
};

use crate::github::{sub_issue_pr_trigger, GhMirror, PrRoute};
use crate::schema::migrate_with;
use crate::sync::project_key;
use crate::{
    gh_epic_merged_uid, gh_issue_closed_uid, gh_pr_uid, GithubApplied, GithubSnapshot, IssueRow,
    PrRow, SyncState,
};
use crate::{
    json_col, opt_parse_col, parse_col, DecisionChange, EventChange, LedgerError, OutboxEntry,
    ProjectChange, Result, SessionChange, SyncChange, WorkItemChange,
};
use ct_core::{
    hlc, next_status, now_ms, pr_trigger, DecisionOrigin, ForgeConfig, Priority, SessionMode,
    SessionState, Tool, TrackerConfig, TrackerRef, WorkItemKind, WorkItemStatus, WorkItemTrigger,
};
use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const HUB_MIGRATIONS: &[&str] = &[
    r#"
CREATE TABLE env (
  env_id       TEXT PRIMARY KEY,
  name         TEXT NOT NULL,
  created_at   INTEGER NOT NULL,
  last_seen_at INTEGER,
  last_seq     INTEGER NOT NULL DEFAULT 0
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

CREATE TABLE project_location (
  project_id INTEGER NOT NULL REFERENCES project(id),
  env_id     TEXT NOT NULL REFERENCES env(env_id),
  repo_path  TEXT NOT NULL,
  removed    INTEGER NOT NULL DEFAULT 0,
  hlc        INTEGER NOT NULL,
  PRIMARY KEY (project_id, env_id)
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

CREATE TABLE work_item_location (
  work_item_id  INTEGER NOT NULL REFERENCES work_item(id),
  env_id        TEXT NOT NULL REFERENCES env(env_id),
  worktree_path TEXT,
  hlc           INTEGER NOT NULL,
  PRIMARY KEY (work_item_id, env_id)
);

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
);
CREATE INDEX session_work_item_idx ON session(work_item_id);

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
CREATE INDEX decision_work_item_idx ON decision(work_item_id);

-- `session_id` sem FK: a fotografia da sessão pode chegar depois do evento.
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
CREATE INDEX event_work_item_idx ON event(work_item_id, hlc);
CREATE INDEX event_status_idx ON event(work_item_id, hlc, env_id) WHERE type = 'status_changed';

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
"#,
    // 2: tokens de sync por ambiente (spec §8). Só o hash fica no hub;
    // `env_id` é preso ao token no primeiro envio.
    r#"
CREATE TABLE env_token (
  name       TEXT PRIMARY KEY,
  token_hash TEXT NOT NULL UNIQUE,
  env_id     TEXT UNIQUE,
  created_at INTEGER NOT NULL
);
"#,
    // 3: espelho do GitHub dos projetos centrais, mesmo sem clone no hub
    // (issue #60), e o HLC dos eventos que o próprio hub deriva do GitHub.
    r#"
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
CREATE TABLE hub_meta (
  key   TEXT PRIMARY KEY,
  value INTEGER NOT NULL
);
INSERT INTO hub_meta (key, value) SELECT 'hlc', coalesce(max(hlc), 0) FROM event;
"#,
    // 4: pendências que cada ambiente informa ao enviar (`hub.envs.list`).
    r#"
ALTER TABLE env ADD COLUMN pending INTEGER NOT NULL DEFAULT 0;
"#,
    // 5: modelo e modo da sessão (issue #94).
    r#"
ALTER TABLE session ADD COLUMN model TEXT;
ALTER TABLE session ADD COLUMN mode TEXT NOT NULL DEFAULT 'native';
"#,
    // 6: projetos com origem no hub (issue #175, spec do hub como plano de
    // controle §3). `rev` = `config_rev` da última mudança; `deleted_at` =
    // lápide. `attrs_hlc`/`attrs_env` deixam de ser usados (ficam no
    // esquema). Linhas de push de projeto desconhecido vão para
    // `push_quarantine`. A exclusão apaga o histórico com o projeto em
    // `purging` (mesmo padrão da migração 3 do `ledger.db`).
    r#"
ALTER TABLE project ADD COLUMN rev INTEGER NOT NULL DEFAULT 0;
ALTER TABLE project ADD COLUMN deleted_at INTEGER;
INSERT INTO hub_meta (key, value) VALUES ('config_rev', 0) ON CONFLICT (key) DO NOTHING;

CREATE TABLE push_quarantine (
  id          INTEGER PRIMARY KEY,
  env_id      TEXT NOT NULL,
  seq         INTEGER NOT NULL,
  kind        TEXT NOT NULL,
  key         TEXT NOT NULL,
  project_key TEXT NOT NULL,
  payload     TEXT NOT NULL,
  received_at INTEGER NOT NULL,
  UNIQUE (env_id, seq)
);
CREATE INDEX push_quarantine_project_idx ON push_quarantine(project_key, env_id, seq);

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
    // 7: atribuições de projetos a ambientes (issue #176, spec do hub como
    // plano de controle §3 e §3.3). `assignment` é a intenção do hub (`rev` =
    // `config_rev` da última mudança; `removed_at` = removida, a linha fica
    // para propagar a remoção). `project_location` passa a ser só o relato
    // do ambiente, com o resultado da aplicação. As atribuições de uma
    // instalação existente nascem no `cthubd migrate` (#182), não aqui.
    r#"
CREATE TABLE assignment (
  project_id INTEGER NOT NULL REFERENCES project(id),
  env_id     TEXT NOT NULL,
  repo_path  TEXT NOT NULL DEFAULT '',
  created_at INTEGER NOT NULL,
  removed_at INTEGER,
  rev        INTEGER NOT NULL,
  PRIMARY KEY (project_id, env_id)
);
CREATE INDEX assignment_env_idx ON assignment(env_id, rev);
CREATE INDEX assignment_rev_idx ON assignment(rev);

ALTER TABLE project_location ADD COLUMN state TEXT
  CHECK (state IS NULL OR state IN ('ok', 'error'));
ALTER TABLE project_location ADD COLUMN error TEXT;
ALTER TABLE project_location ADD COLUMN applied_rev INTEGER;
ALTER TABLE project_location ADD COLUMN reported_at INTEGER;
"#,
    // 8: época da configuração (issue #177, spec do hub como plano de
    // controle §4.1). Aleatória, regenerada pelo `cthubd restore`: um hub
    // restaurado pode voltar ao mesmo `config_rev` com conteúdo diferente, e
    // época diferente no pull = sempre o conjunto completo.
    r#"
INSERT INTO hub_meta (key, value) VALUES ('config_epoch', abs(random() % 9007199254740991))
  ON CONFLICT (key) DO NOTHING;
"#,
    // 9: chave pública age de cada ambiente para a entrega de secrets (issue
    // #184). O ambiente a manda em todo `GET /sync/config`; `delivered_pubkey`
    // = chave da última entrega (diferente da atual força reentregar). A
    // revogação do token apaga a linha.
    r#"
CREATE TABLE env_key (
  env_id           TEXT PRIMARY KEY,
  pubkey           TEXT NOT NULL,
  updated_at       INTEGER NOT NULL,
  delivered_pubkey TEXT
);
"#,
    // 10: `config_rev` que o ambiente confirmou ter aplicado, com a época
    // (issue #165): o `since` do pull, gravado só quando a chave dele já
    // recebeu uma entrega. Base do estado "entregue" dos secrets na UI.
    r#"
ALTER TABLE env_key ADD COLUMN applied_rev INTEGER;
ALTER TABLE env_key ADD COLUMN applied_epoch INTEGER;
"#,
    // 11: prioridade e ordem (issue #215, spec do gestor §4.2). Colunas só do
    // hub: a ingestão nunca as escreve. `rank` é a posição dentro de
    // (projeto, prioridade), e nulo = fim da classe; `project.rank` ordena
    // os projetos entre si. O histórico fica nos eventos `priority_set`,
    // `rank_moved` e `project_rank_moved`.
    r#"
ALTER TABLE work_item ADD COLUMN priority TEXT
  CHECK (priority IS NULL OR priority IN ('P0', 'P1', 'P2', 'P3'));
ALTER TABLE work_item ADD COLUMN rank INTEGER;
ALTER TABLE work_item ADD COLUMN priority_at INTEGER;
ALTER TABLE project ADD COLUMN rank INTEGER;
CREATE INDEX work_item_priority_idx ON work_item(project_id, priority, rank);
"#,
    // 12: token de agente (issue #217, spec do gestor §6.1): um por
    // ambiente, preso ao token de sync de mesmo nome. Só lê `/agent/v1/*` e
    // registra propostas; o hub guarda só o hash.
    r#"
CREATE TABLE agent_token (
  env_name     TEXT PRIMARY KEY REFERENCES env_token(name) ON DELETE CASCADE,
  token_hash   TEXT NOT NULL UNIQUE,
  created_at   INTEGER NOT NULL,
  last_used_at INTEGER
);
"#,
    // 13: propostas do gestor (issue #218, spec do gestor §8). O agente
    // cria pendentes; a UI do hub aprova ou rejeita; só o hub marca
    // `obsoleta`. Alvo, valores, justificativa e origem não mudam, e o
    // estado só sai de `pendente` uma vez.
    r#"
CREATE TABLE proposal (
  id                INTEGER PRIMARY KEY,
  uid               TEXT NOT NULL UNIQUE,
  kind              TEXT NOT NULL CHECK (kind IN ('priority', 'status')),
  project_id        INTEGER NOT NULL REFERENCES project(id),
  gh_number         INTEGER NOT NULL,
  current_value     TEXT NOT NULL,
  proposed_value    TEXT NOT NULL,
  justification     TEXT NOT NULL,
  state             TEXT NOT NULL DEFAULT 'pendente'
                    CHECK (state IN ('pendente', 'aprovada', 'rejeitada', 'obsoleta')),
  env_id            TEXT NOT NULL,
  session_id        TEXT,
  created_at        INTEGER NOT NULL,
  decided_at        INTEGER,
  decision_note     TEXT,
  applied_event_uid TEXT
);
CREATE INDEX proposal_target_idx ON proposal(project_id, gh_number, kind, state);
CREATE INDEX proposal_state_idx ON proposal(state, created_at);
CREATE TRIGGER proposal_immutable
BEFORE UPDATE OF id, uid, kind, project_id, gh_number, current_value, proposed_value,
  justification, env_id, session_id, created_at ON proposal
BEGIN SELECT RAISE(ABORT, 'proposta só pode ter a decisão alterada'); END;
CREATE TRIGGER proposal_decided_once
BEFORE UPDATE OF state ON proposal
WHEN OLD.state <> 'pendente'
BEGIN SELECT RAISE(ABORT, 'proposta já decidida'); END;
"#,
    // 14: projeto sentinela do gestor (issue #219, spec do gestor §2.1). As
    // sessões e decisões do gestor chegam pela chave `_ctower/gestor`, que
    // precisa existir no hub (senão iriam para a quarentena). Fica fora das
    // listas, do sync do GitHub, da configuração dos ambientes, do ranking e
    // da saúde.
    r#"
ALTER TABLE project ADD COLUMN kind TEXT NOT NULL DEFAULT 'github'
  CHECK (kind IN ('github', 'gestor'));
INSERT INTO project (key, gh_owner, gh_repo, name, base_branch, protected_branches, created_at,
                     rev, kind)
VALUES ('_ctower/gestor', '_ctower', 'gestor', 'Gestor de projetos', '', '[]',
        CAST(strftime('%s', 'now') AS INTEGER) * 1000, 0, 'gestor')
ON CONFLICT (key) DO NOTHING;
"#,
    // 15: resumo das limpezas pendentes que cada ambiente informa ao enviar
    // (issue #287). Nulo = o ambiente nunca informou (versão anterior ou job
    // ainda não rodou).
    r#"
ALTER TABLE env ADD COLUMN cleanup_checked_at INTEGER;
ALTER TABLE env ADD COLUMN cleanup_items INTEGER;
ALTER TABLE env ADD COLUMN cleanup_bytes INTEGER;
"#,
    // 16: SHA do head do PR (issue #286), no mesmo espelho do `ledger.db`.
    r#"
ALTER TABLE pr_cache ADD COLUMN head_sha TEXT;
"#,
    // 17: HLC da última fotografia aplicada da sessão (issue #293). 0 = cópia
    // de ambiente anterior, sem HLC.
    r#"
ALTER TABLE session ADD COLUMN hlc INTEGER NOT NULL DEFAULT 0;
"#,
    // 18: id da sessão no Remote Control (issue #197), vindo do ambiente.
    r#"
ALTER TABLE session ADD COLUMN rc_session_id TEXT;
"#,
    // 19: sessões web do hub (issue #200, spec da PWA §3.1 e §9.1): só o
    // hash do segredo do cookie `ct_web`.
    r#"
CREATE TABLE web_session (
    id           TEXT PRIMARY KEY,
    secret_hash  TEXT NOT NULL,
    created_at   INTEGER NOT NULL,
    last_used_at INTEGER NOT NULL,
    expires_at   INTEGER NOT NULL,
    revoked_at   INTEGER,
    identity     TEXT,
    user_agent   TEXT
);
"#,
    // 20: Web Push da PWA (issue #203, spec da PWA §7.2 e §9.1): uma
    // inscrição por dispositivo, presa à sessão web que a criou, e as
    // chaves já notificadas (deduplicação). A chave VAPID fica fora do
    // hub.db (`<config>/vapid.key`).
    r#"
CREATE TABLE push_subscription (
    id             INTEGER PRIMARY KEY,
    web_session_id TEXT REFERENCES web_session(id) ON DELETE CASCADE,
    endpoint       TEXT NOT NULL UNIQUE,
    p256dh         TEXT NOT NULL,
    auth           TEXT NOT NULL,
    label          TEXT,
    events         TEXT NOT NULL,
    created_at     INTEGER NOT NULL,
    last_ok_at     INTEGER,
    failures       INTEGER NOT NULL DEFAULT 0,
    disabled_at    INTEGER
);
CREATE INDEX push_subscription_session_idx ON push_subscription(web_session_id);
CREATE TABLE push_sent (
    key     TEXT PRIMARY KEY,
    sent_at INTEGER NOT NULL
);
"#,
    // 21: rastreador e forja neutros de provedor (issue #347, spec do Jira e
    // GitLab §3.4), as mesmas colunas da migração 12 do `ledger.db`. As
    // linhas existentes (inclusive o sentinela do gestor) viram GitHub.
    // `sync_env_id` = ambiente de sync do projeto Jira (§5.1); nulo no
    // GitHub, que o hub sincroniza.
    r#"
ALTER TABLE project ADD COLUMN tracker_kind TEXT NOT NULL DEFAULT 'github'
  CHECK (tracker_kind IN ('github', 'jira'));
ALTER TABLE project ADD COLUMN tracker_config TEXT NOT NULL DEFAULT '{}';
ALTER TABLE project ADD COLUMN forge_kind TEXT NOT NULL DEFAULT 'github'
  CHECK (forge_kind IN ('github', 'gitlab'));
ALTER TABLE project ADD COLUMN forge_config TEXT NOT NULL DEFAULT '{}';
ALTER TABLE project ADD COLUMN branch_pattern TEXT;
ALTER TABLE project ADD COLUMN attribution INTEGER NOT NULL DEFAULT 1;
ALTER TABLE project ADD COLUMN sync_env_id TEXT REFERENCES env(env_id);
"#,
    // 22: último `tracker_status` do ambiente de sync de cada projeto Jira
    // (issue #348, spec do Jira e GitLab §5.3 e §8.4). O sync em si (tentativa,
    // sucesso, erro) vai para o `sync_state`, como o do GitHub; aqui ficam
    // quem relatou e a prontidão (JSON de `TrackerReadiness`).
    r#"
CREATE TABLE tracker_status (
  project_id  INTEGER PRIMARY KEY REFERENCES project(id),
  env_id      TEXT NOT NULL,
  received_at INTEGER NOT NULL,
  readiness   TEXT NOT NULL DEFAULT '{}'
);
"#,
    // 23: regras do agente por projeto e por épico (issue #357, spec do Jira
    // e GitLab §10.2). `epic_number = 0` = regras do projeto. As versões são
    // só inclusão; a proposta do agente (texto completo, pelo outbox) só
    // muda a decisão, uma vez, como a do gestor (migração 13). As duas só
    // saem na purga do projeto (`purging`).
    r#"
CREATE TABLE agent_rules_proposal (
  id              INTEGER PRIMARY KEY,
  uid             TEXT NOT NULL UNIQUE,
  project_id      INTEGER NOT NULL REFERENCES project(id),
  epic_number     INTEGER NOT NULL DEFAULT 0,
  base_version    INTEGER NOT NULL,
  content         TEXT NOT NULL,
  reason          TEXT NOT NULL,
  state           TEXT NOT NULL DEFAULT 'pendente'
                  CHECK (state IN ('pendente', 'aprovada', 'rejeitada', 'obsoleta')),
  env_id          TEXT NOT NULL,
  session_id      TEXT,
  created_at      INTEGER NOT NULL,
  decided_at      INTEGER,
  decision_note   TEXT,
  applied_version INTEGER
);
CREATE INDEX agent_rules_proposal_scope_idx
  ON agent_rules_proposal(project_id, epic_number, state);
CREATE INDEX agent_rules_proposal_state_idx ON agent_rules_proposal(state, created_at);
CREATE TRIGGER agent_rules_proposal_immutable
BEFORE UPDATE OF id, uid, project_id, epic_number, base_version, content, reason, env_id,
  session_id, created_at ON agent_rules_proposal
BEGIN SELECT RAISE(ABORT, 'proposta de regras só pode ter a decisão alterada'); END;
CREATE TRIGGER agent_rules_proposal_decided_once
BEFORE UPDATE OF state ON agent_rules_proposal
WHEN OLD.state <> 'pendente'
BEGIN SELECT RAISE(ABORT, 'proposta de regras já decidida'); END;
CREATE TRIGGER agent_rules_proposal_no_delete BEFORE DELETE ON agent_rules_proposal
WHEN OLD.project_id NOT IN (SELECT project_id FROM purging)
BEGIN SELECT RAISE(ABORT, 'proposta de regras não pode ser apagada'); END;

CREATE TABLE agent_rules (
  id            INTEGER PRIMARY KEY,
  project_id    INTEGER NOT NULL REFERENCES project(id),
  epic_number   INTEGER NOT NULL DEFAULT 0,
  version       INTEGER NOT NULL,
  content       TEXT NOT NULL,
  source        TEXT NOT NULL CHECK (source IN ('usuario', 'proposta', 'importacao')),
  proposal_id   INTEGER REFERENCES agent_rules_proposal(id),
  author_env_id TEXT,
  note          TEXT,
  created_at    INTEGER NOT NULL,
  UNIQUE (project_id, epic_number, version)
);
CREATE TRIGGER agent_rules_no_update BEFORE UPDATE ON agent_rules
BEGIN SELECT RAISE(ABORT, 'agent_rules é só inclusão'); END;
CREATE TRIGGER agent_rules_no_delete BEFORE DELETE ON agent_rules
WHEN OLD.project_id NOT IN (SELECT project_id FROM purging)
BEGIN SELECT RAISE(ABORT, 'agent_rules é só inclusão'); END;
"#,
    // 24: sprints do Jira de cada issue do espelho, a mesma coluna da
    // migração 14 do `ledger.db`. `[]` nas issues já gravadas e no GitHub;
    // o `issue_mirror` do ambiente de sync (que relê o projeto uma vez)
    // preenche.
    r#"
ALTER TABLE issue_cache ADD COLUMN sprints TEXT NOT NULL DEFAULT '[]';
"#,
    // 25: docs do projeto fora do repositório (issue #440, épico #445): JSON
    // de `DocsConfig` (`enabled`, `project_dirs`, `epic_dirs`). `{}` =
    // desligado.
    r#"
ALTER TABLE project ADD COLUMN docs_config TEXT NOT NULL DEFAULT '{}';
"#,
];

const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

const SNAPSHOT_PREFIX: &str = "hub-";

/// Snapshots `hub-AAAAMMDD-HHMM.db` em `dir`, do mais novo para o mais antigo.
pub fn list_snapshots(dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut out: Vec<PathBuf> = match std::fs::read_dir(dir) {
        Ok(rd) => rd
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(is_snapshot_name)
            })
            .collect(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(e),
    };
    // O carimbo no nome ordena cronologicamente.
    out.sort();
    out.reverse();
    Ok(out)
}

fn is_snapshot_name(n: &str) -> bool {
    let Some(stamp) = n
        .strip_prefix(SNAPSHOT_PREFIX)
        .and_then(|r| r.strip_suffix(".db"))
    else {
        return false;
    };
    let b = stamp.as_bytes();
    b.len() == 13
        && b[8] == b'-'
        && b.iter()
            .enumerate()
            .all(|(i, c)| i == 8 || c.is_ascii_digit())
}

/// Apaga os snapshots além dos `keep` mais novos. Devolve os apagados.
pub fn rotate_snapshots(dir: &Path, keep: usize) -> std::io::Result<Vec<PathBuf>> {
    let mut removed = Vec::new();
    for p in list_snapshots(dir)?.into_iter().skip(keep.max(1)) {
        std::fs::remove_file(&p)?;
        removed.push(p);
    }
    Ok(removed)
}

/// Transação que se aninha: aberta dentro de outra, não abre nem confirma
/// nada (a de fora decide). Assim as mudanças da UI servem também à
/// aprovação de proposta, que as chama numa transação só.
pub(crate) struct NestedTx<'a>(Option<rusqlite::Transaction<'a>>);

impl NestedTx<'_> {
    pub(crate) fn commit(self) -> Result<()> {
        if let Some(t) = self.0 {
            t.commit()?;
        }
        Ok(())
    }
}

/// Eventos que decidem o status do work item no hub.
fn is_status_event(kind: &str) -> bool {
    matches!(kind, "status_changed" | "status_set_manual")
}

pub struct Hub {
    conn: Connection,
}

/// `hub.db` compartilhado entre o servidor, o envio em processo e o sync
/// com o GitHub.
#[derive(Clone)]
pub struct SharedHub(std::sync::Arc<std::sync::Mutex<Hub>>);

impl SharedHub {
    pub fn new(hub: Hub) -> Self {
        SharedHub(std::sync::Arc::new(std::sync::Mutex::new(hub)))
    }

    pub fn lock(&self) -> std::sync::MutexGuard<'_, Hub> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Resultado da ingestão de um lote. Todas as linhas recebem ack
/// (`last_seq` cobre o lote), aplicadas ou não.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct IngestResult {
    /// Maior `seq` do lote (ou o último já visto, se o lote veio vazio): o
    /// ambiente pode apagar do outbox tudo até ele.
    pub last_seq: i64,
    /// Linhas de projeto cadastrado, aplicadas pela ingestão normal.
    pub applied: usize,
    /// Linhas de projeto com lápide, descartadas.
    pub discarded: usize,
    /// Linhas de projeto desconhecido, guardadas em `push_quarantine`.
    pub quarantined: usize,
    /// Linhas sem efeito no hub: `project_removed` (só o hub remove) e
    /// revisão de decisão que o hub não conhece.
    pub ignored: usize,
    /// Projetos (`owner/repo`, minúsculas) das linhas descartadas.
    pub discarded_projects: Vec<String>,
    /// Projetos (`owner/repo`, minúsculas) das linhas em quarentena.
    pub quarantined_projects: Vec<String>,
    /// Linhas do espelho do rastreador (`issue_mirror`, `pr_mirror`,
    /// `tracker_status`) de quem não é o ambiente de sync do projeto (ou de
    /// projeto que não é Jira), descartadas.
    pub mirror_rejected: usize,
    /// Projetos (`owner/repo`, minúsculas) das linhas de espelho descartadas.
    pub mirror_rejected_projects: Vec<String>,
    /// Propostas de regras do agente recebidas (pendentes): a UI relê.
    pub rules_proposals: usize,
}

/// Situação de um projeto no hub para a ingestão.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProjectState {
    Live,
    Tombstone,
    Unknown,
}

/// A que projeto uma linha do lote pertence.
enum Route {
    Project(String),
    Ignore,
}

/// Ambiente autorizado por um token de sync.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvToken {
    pub name: String,
    /// `None` só em token de instalação antiga, gerado antes do #195.
    pub env_id: Option<String>,
}

/// Token de sync recém-gerado e o `env_id` preso a ele.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssuedEnvToken {
    /// Token em claro (o hub guarda só o hash).
    pub token: String,
    pub env_id: String,
}

/// SHA-256 do token, em hex.
/// Prefixo do token de agente, para o hub dar um erro claro quando um token
/// é usado na rota errada.
pub const AGENT_TOKEN_PREFIX: &str = "cta_";

fn token_hash(token: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(token.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct HubProject {
    pub id: i64,
    /// `owner` do repositório na forja (no GitLab, o caminho sem o último
    /// segmento). O nome `gh_*` fica por compatibilidade.
    pub gh_owner: String,
    /// Último segmento do caminho do repositório na forja.
    pub gh_repo: String,
    pub name: String,
    pub base_branch: String,
    pub protected_branches: Vec<String>,
    pub created_at: i64,
    /// Ordem entre projetos (§4.3 da spec do gestor); nulo = fim. Ausente em
    /// hubs anteriores à issue #215.
    #[serde(default)]
    pub rank: Option<i64>,
    /// Rastreador e forja (spec do Jira e GitLab §3). Ausentes em hubs
    /// anteriores à #347: GitHub.
    #[serde(default)]
    pub tracker: TrackerConfig,
    #[serde(default)]
    pub forge: ForgeConfig,
    /// Padrão do nome da branch (§4); nulo = o padrão do rastreador.
    #[serde(default)]
    pub branch_pattern: Option<String>,
    /// Linhas de atribuição nas sessões (§9.3).
    #[serde(default = "attribution_default")]
    pub attribution: bool,
    /// Ambiente de sync do projeto Jira (§5.1); nulo no GitHub.
    #[serde(default)]
    pub sync_env_id: Option<String>,
    /// Pasta de docs do projeto fora do repositório (#440). Ausente num hub
    /// anterior: desligada.
    #[serde(default)]
    pub docs: ct_core::DocsConfig,
}

fn attribution_default() -> bool {
    true
}

impl HubProject {
    /// O par de que as referências de work item precisam.
    pub fn tracker_ref(&self) -> TrackerRef<'_> {
        TrackerRef::new(&self.gh_owner, &self.gh_repo, &self.tracker)
    }

    /// Referência do work item `n` (`owner/repo#n` ou `PROJ-n`).
    pub fn item_ref(&self, n: i64) -> String {
        ct_core::item_ref(&self.tracker_ref(), n)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct HubWorkItem {
    pub id: i64,
    pub project_id: i64,
    pub gh_number: i64,
    pub kind: WorkItemKind,
    pub status: WorkItemStatus,
    pub branch: Option<String>,
    pub created_at: i64,
    pub closed_at: Option<i64>,
    /// Só do hub (issue #215): nulo = não triado. Ausentes em hubs
    /// anteriores.
    #[serde(default)]
    pub priority: Option<Priority>,
    /// Posição dentro de (projeto, prioridade); nulo = fim da classe.
    #[serde(default)]
    pub rank: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct HubSession {
    pub id: String,
    pub env_id: String,
    pub work_item_id: i64,
    pub tool: Tool,
    pub tool_session_id: Option<String>,
    pub cwd: String,
    pub state: SessionState,
    pub exit_code: Option<i32>,
    pub started_at: i64,
    pub last_event_at: i64,
    pub ended_at: Option<i64>,
    /// Ausentes em hubs anteriores à issue #94.
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub mode: SessionMode,
    /// Id da sessão no Remote Control (`claude.ai/code/<id>`, issue #197);
    /// `None` = nunca ligou ou ambiente anterior. Ligado = sessão viva com id.
    #[serde(default)]
    pub rc_session_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct HubDecision {
    pub id: i64,
    pub uid: String,
    pub env_id: String,
    pub work_item_id: i64,
    pub session_id: Option<String>,
    pub origin: DecisionOrigin,
    pub title: String,
    pub context: String,
    pub decision: String,
    pub consequences: Option<String>,
    pub reviewed_at: Option<i64>,
    pub reviewed_in_pr: Option<i64>,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct HubEvent {
    pub id: i64,
    pub uid: String,
    pub env_id: String,
    pub hlc: i64,
    pub ts: i64,
    pub project_id: i64,
    pub work_item_id: Option<i64>,
    pub session_id: Option<String>,
    pub kind: String,
    pub payload: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct HubEnv {
    pub env_id: String,
    pub name: String,
    pub created_at: i64,
    pub last_seen_at: Option<i64>,
    pub last_seq: i64,
    /// Mudanças que o ambiente ainda tinha no outbox no último envio.
    pub pending: i64,
    /// Limpezas pendentes do último plano do ambiente (issue #287); `None` =
    /// o ambiente nunca informou.
    pub cleanup: Option<CleanupSummary>,
    /// O ambiente tem o canal reverso (`/sync/channel`) aberto agora (spec da
    /// PWA §5.4, #198). Não fica no `hub.db`: o `cthubd` preenche em
    /// `hub.envs.list` e `hub.dashboard.get` (a leitura do banco dá `false`).
    #[serde(default)]
    pub online: bool,
}

/// Resumo do último plano de limpeza de um ambiente (issue #287): vai a cada
/// envio ao hub e aparece na tela Hub.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct CleanupSummary {
    /// Quando o plano foi montado (ms).
    pub checked_at: i64,
    /// Work items com alguma etapa pendente.
    pub items: i64,
    /// Espaço estimado das etapas pendentes, em bytes.
    pub bytes: i64,
}

const PROJECT_COLS: &str =
    "id, gh_owner, gh_repo, name, base_branch, protected_branches, created_at, rank, \
     tracker_kind, tracker_config, forge_kind, forge_config, branch_pattern, attribution, \
     sync_env_id, docs_config";
const WORK_ITEM_COLS: &str =
    "id, project_id, gh_number, kind, status, branch, created_at, closed_at, priority, rank";
const SESSION_COLS: &str = "id, env_id, work_item_id, tool, tool_session_id, cwd, state, \
                            exit_code, started_at, last_event_at, ended_at, model, mode, \
                            rc_session_id";
const DECISION_COLS: &str = "id, uid, env_id, work_item_id, session_id, origin, title, context, \
                             decision, consequences, reviewed_at, reviewed_in_pr, created_at";
const EVENT_COLS: &str =
    "id, uid, env_id, hlc, ts, project_id, work_item_id, session_id, type, payload";

fn row_to_project(r: &Row) -> rusqlite::Result<HubProject> {
    Ok(HubProject {
        id: r.get(0)?,
        gh_owner: r.get(1)?,
        gh_repo: r.get(2)?,
        name: r.get(3)?,
        base_branch: r.get(4)?,
        protected_branches: json_col(r, 5)?,
        created_at: r.get(6)?,
        rank: r.get(7)?,
        tracker: crate::tracker_col(r, 8)?,
        forge: crate::forge_col(r, 10)?,
        branch_pattern: r.get(12)?,
        attribution: r.get(13)?,
        sync_env_id: r.get(14)?,
        docs: json_col(r, 15)?,
    })
}

fn row_to_work_item(r: &Row) -> rusqlite::Result<HubWorkItem> {
    Ok(HubWorkItem {
        id: r.get(0)?,
        project_id: r.get(1)?,
        gh_number: r.get(2)?,
        kind: parse_col(r, 3)?,
        status: parse_col(r, 4)?,
        branch: r.get(5)?,
        created_at: r.get(6)?,
        closed_at: r.get(7)?,
        priority: opt_parse_col(r, 8)?,
        rank: r.get(9)?,
    })
}

fn row_to_session(r: &Row) -> rusqlite::Result<HubSession> {
    Ok(HubSession {
        id: r.get(0)?,
        env_id: r.get(1)?,
        work_item_id: r.get(2)?,
        tool: parse_col(r, 3)?,
        tool_session_id: r.get(4)?,
        cwd: r.get(5)?,
        state: parse_col(r, 6)?,
        exit_code: r.get(7)?,
        started_at: r.get(8)?,
        last_event_at: r.get(9)?,
        ended_at: r.get(10)?,
        model: r.get(11)?,
        mode: parse_col(r, 12)?,
        rc_session_id: r.get(13)?,
    })
}

fn row_to_decision(r: &Row) -> rusqlite::Result<HubDecision> {
    Ok(HubDecision {
        id: r.get(0)?,
        uid: r.get(1)?,
        env_id: r.get(2)?,
        work_item_id: r.get(3)?,
        session_id: r.get(4)?,
        origin: parse_col(r, 5)?,
        title: r.get(6)?,
        context: r.get(7)?,
        decision: r.get(8)?,
        consequences: r.get(9)?,
        reviewed_at: r.get(10)?,
        reviewed_in_pr: r.get(11)?,
        created_at: r.get(12)?,
    })
}

fn row_to_event(r: &Row) -> rusqlite::Result<HubEvent> {
    Ok(HubEvent {
        id: r.get(0)?,
        uid: r.get(1)?,
        env_id: r.get(2)?,
        hlc: r.get(3)?,
        ts: r.get(4)?,
        project_id: r.get(5)?,
        work_item_id: r.get(6)?,
        session_id: r.get(7)?,
        kind: r.get(8)?,
        payload: json_col(r, 9)?,
    })
}

impl Hub {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.busy_timeout(BUSY_TIMEOUT)?;
        conn.query_row("PRAGMA journal_mode=WAL", [], |_| Ok(()))?;
        Self::init(conn)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.busy_timeout(BUSY_TIMEOUT)?;
        conn.pragma_update(None, "foreign_keys", true)?;
        migrate_with(&conn, HUB_MIGRATIONS)?;
        Ok(Hub { conn })
    }

    /// Conexão crua (backup com `VACUUM INTO`, consultas da UI).
    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    /// Aplica um lote do outbox de `env_id` numa transação. Reaplicar o
    /// mesmo lote (ou um resync inteiro) não muda nada. Cada linha segue o
    /// projeto a que pertence (cadastrado, com lápide ou desconhecido); em
    /// todos os casos ela recebe ack.
    pub fn ingest(
        &self,
        env_id: &str,
        env_name: &str,
        entries: &[OutboxEntry],
    ) -> Result<IngestResult> {
        let tx = self.conn.unchecked_transaction()?;
        let max_seq = entries.iter().map(|e| e.seq).max().unwrap_or(0);
        let now = now_ms();
        self.conn.execute(
            "INSERT INTO env (env_id, name, created_at, last_seen_at, last_seq)
             VALUES (?1, ?2, ?3, ?3, ?4)
             ON CONFLICT (env_id) DO UPDATE SET name = excluded.name,
                last_seen_at = excluded.last_seen_at, last_seq = max(last_seq, excluded.last_seq)",
            params![env_id, env_name, now, max_seq],
        )?;
        let mut out = IngestResult::default();
        let mut states: std::collections::HashMap<String, ProjectState> = Default::default();
        for e in entries {
            let key = match self.route(&e.change)? {
                Route::Project(k) => k,
                Route::Ignore => {
                    out.ignored += 1;
                    continue;
                }
            };
            let state = match states.get(&key) {
                Some(s) => *s,
                None => {
                    let s = self.project_state(&key)?;
                    states.insert(key.clone(), s);
                    s
                }
            };
            match state {
                ProjectState::Live => {
                    if self.apply(env_id, &e.change)? {
                        out.applied += 1;
                        if matches!(e.change, SyncChange::RulesProposal(_)) {
                            out.rules_proposals += 1;
                        }
                    } else {
                        out.mirror_rejected += 1;
                        if !out.mirror_rejected_projects.contains(&key) {
                            out.mirror_rejected_projects.push(key);
                        }
                    }
                }
                ProjectState::Tombstone => {
                    out.discarded += 1;
                    if !out.discarded_projects.contains(&key) {
                        out.discarded_projects.push(key);
                    }
                }
                ProjectState::Unknown => {
                    self.quarantine(env_id, e, &key, now)?;
                    out.quarantined += 1;
                    if !out.quarantined_projects.contains(&key) {
                        out.quarantined_projects.push(key);
                    }
                }
            }
        }
        let last_seq: i64 = self.conn.query_row(
            "SELECT last_seq FROM env WHERE env_id = ?1",
            [env_id],
            |r| r.get(0),
        )?;
        tx.commit()?;
        out.last_seq = if entries.is_empty() {
            last_seq
        } else {
            max_seq
        };
        Ok(out)
    }

    /// Projeto da linha. A revisão de decisão não traz `owner/repo`: vale o
    /// da decisão no hub ou, se ela está em quarentena, o dela.
    fn route(&self, change: &SyncChange) -> Result<Route> {
        let key = match change {
            SyncChange::Project(p) => project_key(&p.owner, &p.repo),
            // Só o hub remove projeto (spec §5).
            SyncChange::ProjectRemoved { .. } => return Ok(Route::Ignore),
            SyncChange::WorkItem(w) => project_key(&w.owner, &w.repo),
            SyncChange::Session(s) => project_key(&s.owner, &s.repo),
            SyncChange::Decision(d) => project_key(&d.owner, &d.repo),
            SyncChange::Event(e) => project_key(&e.owner, &e.repo),
            SyncChange::IssueMirror(m) => project_key(&m.owner, &m.repo),
            SyncChange::PrMirror(m) => project_key(&m.owner, &m.repo),
            SyncChange::TrackerStatus(t) => project_key(&t.owner, &t.repo),
            SyncChange::RulesProposal(r) => project_key(&r.owner, &r.repo),
            SyncChange::DecisionReview { uid, .. } => {
                let in_hub: Option<String> = self
                    .conn
                    .query_row(
                        "SELECT p.key FROM decision d
                         JOIN work_item w ON w.id = d.work_item_id
                         JOIN project p ON p.id = w.project_id
                         WHERE d.uid = ?1",
                        [uid],
                        |r| r.get(0),
                    )
                    .optional()?;
                let key = match in_hub {
                    Some(k) => Some(k),
                    None => self
                        .conn
                        .query_row(
                            "SELECT project_key FROM push_quarantine
                             WHERE kind = 'decision' AND key = ?1 LIMIT 1",
                            [uid],
                            |r| r.get(0),
                        )
                        .optional()?,
                };
                match key {
                    Some(k) => k,
                    None => return Ok(Route::Ignore),
                }
            }
        };
        Ok(Route::Project(key))
    }

    fn project_state(&self, key: &str) -> Result<ProjectState> {
        let row: Option<Option<i64>> = self
            .conn
            .query_row(
                "SELECT deleted_at FROM project WHERE key = ?1",
                [key],
                |r| r.get(0),
            )
            .optional()?;
        Ok(match row {
            None => ProjectState::Unknown,
            Some(None) => ProjectState::Live,
            Some(Some(_)) => ProjectState::Tombstone,
        })
    }

    /// Guarda a linha intacta em `push_quarantine`. Reenviar o mesmo lote
    /// não duplica (`UNIQUE (env_id, seq)`).
    fn quarantine(&self, env_id: &str, e: &OutboxEntry, key: &str, now: i64) -> Result<()> {
        let payload = serde_json::to_string(&e.change).expect("SyncChange serializa");
        self.conn.execute(
            "INSERT INTO push_quarantine (env_id, seq, kind, key, project_key, payload, received_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) ON CONFLICT (env_id, seq) DO NOTHING",
            params![
                env_id,
                e.seq,
                e.change.kind(),
                e.change.key(),
                key,
                payload,
                now
            ],
        )?;
        Ok(())
    }

    /// Aplica a linha de um projeto cadastrado. `false` = linha do espelho
    /// do rastreador recusada por não vir do ambiente de sync do projeto.
    fn apply(&self, env: &str, change: &SyncChange) -> Result<bool> {
        if change.is_mirror() {
            return self.apply_mirror(env, change);
        }
        match change {
            SyncChange::Project(p) => self.apply_project(env, p),
            SyncChange::ProjectRemoved { .. } => Ok(()),
            SyncChange::WorkItem(w) => self.apply_work_item(env, w),
            SyncChange::Session(s) => self.apply_session(env, s),
            SyncChange::Decision(d) => self.apply_decision(env, d),
            SyncChange::DecisionReview {
                uid,
                reviewed_at,
                reviewed_in_pr,
            } => self.review_decision(uid, *reviewed_at, *reviewed_in_pr),
            SyncChange::Event(e) => self.apply_event(e),
            SyncChange::RulesProposal(r) => self.ingest_rules_proposal(env, r),
            SyncChange::IssueMirror(_) | SyncChange::PrMirror(_) | SyncChange::TrackerStatus(_) => {
                unreachable!("espelho tratado acima")
            }
        }?;
        Ok(true)
    }

    /// Id do projeto cadastrado (sem lápide) `owner/repo`. O hub nunca cria
    /// projeto a partir de um push: a ingestão só chega aqui com o projeto
    /// cadastrado.
    fn ensure_project(&self, owner: &str, repo: &str) -> Result<i64> {
        self.live_project_id(owner, repo)?.ok_or_else(|| {
            LedgerError::NotFound("projeto", format!("{owner}/{repo} não está no hub"))
        })
    }

    fn live_project_id(&self, owner: &str, repo: &str) -> Result<Option<i64>> {
        Ok(self
            .conn
            .query_row(
                "SELECT id FROM project WHERE key = ?1 AND deleted_at IS NULL",
                [project_key(owner, repo)],
                |r| r.get(0),
            )
            .optional()?)
    }

    fn ensure_work_item(&self, owner: &str, repo: &str, gh_number: i64) -> Result<i64> {
        let pid = self.ensure_project(owner, repo)?;
        self.conn.execute(
            "INSERT INTO work_item (project_id, gh_number, created_at) VALUES (?1, ?2, ?3)
             ON CONFLICT (project_id, gh_number) DO NOTHING",
            params![pid, gh_number, now_ms()],
        )?;
        Ok(self.conn.query_row(
            "SELECT id FROM work_item WHERE project_id = ?1 AND gh_number = ?2",
            params![pid, gh_number],
            |r| r.get(0),
        )?)
    }

    /// Da fotografia do projeto vinda do ambiente só vale o relato de onde
    /// ele está clonado (`project_location`); nome, `base_branch` e
    /// protegidas são do hub e ficam como estão (spec §5).
    fn apply_project(&self, env: &str, p: &ProjectChange) -> Result<()> {
        let id = self.ensure_project(&p.owner, &p.repo)?;
        // Fotografia com relato (`applied_rev`) substitui o relato inteiro.
        // Sem relato (ambiente anterior à #177, ou fotografia que não é da
        // aplicação), o relato anterior fica como está: uma fotografia sem
        // `applied_rev` nunca volta uma atribuição aplicada para pendente.
        self.conn.execute(
            "INSERT INTO project_location
                (project_id, env_id, repo_path, hlc, state, error, applied_rev, reported_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT (project_id, env_id) DO UPDATE SET
                repo_path = excluded.repo_path, removed = 0, hlc = excluded.hlc,
                state = CASE WHEN excluded.applied_rev IS NULL
                             THEN project_location.state ELSE excluded.state END,
                error = CASE WHEN excluded.applied_rev IS NULL
                             THEN project_location.error ELSE excluded.error END,
                reported_at = CASE WHEN excluded.applied_rev IS NULL
                                   AND project_location.applied_rev IS NOT NULL
                             THEN project_location.reported_at ELSE excluded.reported_at END,
                applied_rev = coalesce(excluded.applied_rev, project_location.applied_rev)
             WHERE excluded.hlc >= project_location.hlc",
            params![
                id,
                env,
                p.repo_path,
                p.hlc,
                p.state.map(|s| s.as_str()),
                p.error,
                p.applied_rev,
                now_ms()
            ],
        )?;
        Ok(())
    }

    fn apply_work_item(&self, env: &str, w: &WorkItemChange) -> Result<()> {
        let id = self.ensure_work_item(&w.owner, &w.repo, w.gh_number)?;
        self.conn.execute(
            "UPDATE work_item SET kind = ?2, branch = coalesce(?3, branch),
                    attrs_hlc = ?4, attrs_env = ?5
             WHERE id = ?1 AND (?4, ?5) > (attrs_hlc, attrs_env)",
            params![id, w.kind.as_str(), w.branch, w.hlc, env],
        )?;
        self.conn.execute(
            "UPDATE work_item SET created_at = min(created_at, ?2) WHERE id = ?1",
            params![id, w.created_at],
        )?;
        self.conn.execute(
            "INSERT INTO work_item_location (work_item_id, env_id, worktree_path, hlc)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (work_item_id, env_id) DO UPDATE SET
                worktree_path = excluded.worktree_path, hlc = excluded.hlc
             WHERE excluded.hlc >= work_item_location.hlc",
            params![id, env, w.worktree_path, w.hlc],
        )?;
        Ok(())
    }

    fn apply_session(&self, env: &str, s: &SessionChange) -> Result<()> {
        let wi = self.ensure_work_item(&s.owner, &s.repo, s.gh_number)?;
        self.conn.execute(
            "INSERT INTO session (id, env_id, work_item_id, tool, tool_session_id, transcript_path,
                                  cwd, state, exit_code, pid, started_at, last_event_at, ended_at,
                                  model, mode, hlc, rc_session_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)
             ON CONFLICT (id) DO UPDATE SET
                tool_session_id = excluded.tool_session_id,
                transcript_path = excluded.transcript_path, cwd = excluded.cwd,
                state = excluded.state, exit_code = excluded.exit_code, pid = excluded.pid,
                last_event_at = excluded.last_event_at, ended_at = excluded.ended_at,
                hlc = excluded.hlc,
                -- Ambiente anterior à #197 não manda o id: fica o que já havia.
                rc_session_id = COALESCE(excluded.rc_session_id, session.rc_session_id)
             -- A sessão é de um só ambiente, cujo HLC é estritamente crescente:
             -- com as duas cópias carimbadas, vale o HLC (o `last_event_at` empata
             -- no mesmo milissegundo e regride se o relógio recuar).
             WHERE CASE WHEN excluded.hlc > 0 AND session.hlc > 0
                        THEN excluded.hlc >= session.hlc
                        ELSE excluded.last_event_at >= session.last_event_at END",
            params![
                s.id,
                env,
                wi,
                s.tool.as_str(),
                s.tool_session_id,
                s.transcript_path,
                s.cwd,
                s.state.as_str(),
                s.exit_code,
                s.pid,
                s.started_at,
                s.last_event_at,
                s.ended_at,
                s.model,
                s.mode.as_str(),
                s.hlc,
                s.rc_session_id
            ],
        )?;
        Ok(())
    }

    fn apply_decision(&self, env: &str, d: &DecisionChange) -> Result<()> {
        let wi = self.ensure_work_item(&d.owner, &d.repo, d.gh_number)?;
        self.conn.execute(
            "INSERT INTO decision (uid, env_id, work_item_id, session_id, origin, title, context,
                                   decision, consequences, reviewed_at, reviewed_in_pr, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
             ON CONFLICT (uid) DO NOTHING",
            params![
                d.uid,
                env,
                wi,
                d.session_id,
                d.origin.as_str(),
                d.title,
                d.context,
                d.decision,
                d.consequences,
                d.reviewed_at,
                d.reviewed_in_pr,
                d.created_at
            ],
        )?;
        if let Some(at) = d.reviewed_at {
            self.review_decision(&d.uid, at, d.reviewed_in_pr)?;
        }
        Ok(())
    }

    /// Revisão: vale a primeira que chegar.
    fn review_decision(&self, uid: &str, reviewed_at: i64, pr: Option<i64>) -> Result<()> {
        self.conn.execute(
            "UPDATE decision SET reviewed_at = ?2, reviewed_in_pr = ?3
             WHERE uid = ?1 AND reviewed_at IS NULL",
            params![uid, reviewed_at, pr],
        )?;
        Ok(())
    }

    fn apply_event(&self, e: &EventChange) -> Result<()> {
        let pid = self.ensure_project(&e.owner, &e.repo)?;
        let wi = e
            .gh_number
            .map(|n| self.ensure_work_item(&e.owner, &e.repo, n))
            .transpose()?;
        // O ambiente que enviou já existe em `env`; o do evento pode ser
        // outro (o hub reenviando o que recebeu, eventos do GitHub).
        self.conn.execute(
            "INSERT INTO env (env_id, name, created_at) VALUES (?1, ?1, ?2)
             ON CONFLICT (env_id) DO NOTHING",
            params![e.env_id, now_ms()],
        )?;
        let inserted = self.conn.execute(
            "INSERT INTO event (uid, env_id, hlc, ts, project_id, work_item_id, session_id, type, payload)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT (uid) DO NOTHING",
            params![
                e.uid,
                e.env_id,
                e.hlc,
                e.ts,
                pid,
                wi,
                e.session_id,
                e.kind,
                e.payload.to_string()
            ],
        )?;
        if inserted == 1 && is_status_event(&e.kind) {
            if let Some(wi) = wi {
                self.recompute_status(wi)?;
            }
        }
        Ok(())
    }

    /// Status = `to` do `status_changed`/`status_set_manual` mais recente por (HLC, env_id).
    fn recompute_status(&self, work_item_id: i64) -> Result<()> {
        let last: Option<(String, i64)> = self
            .conn
            .query_row(
                "SELECT json_extract(payload, '$.to'), ts FROM event
                 WHERE work_item_id = ?1 AND type IN ('status_changed', 'status_set_manual')
                 ORDER BY hlc DESC, env_id DESC LIMIT 1",
                [work_item_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let Some((to, ts)) = last else {
            return Ok(());
        };
        let status: WorkItemStatus = to.parse()?;
        let closed_at = (status == WorkItemStatus::Concluido).then_some(ts);
        self.conn.execute(
            "UPDATE work_item SET status = ?2, closed_at = ?3 WHERE id = ?1",
            params![work_item_id, status.as_str(), closed_at],
        )?;
        self.obsolete_changed(work_item_id)?;
        Ok(())
    }

    /// Mudança manual de status pela UI do hub (`hub.workitem.set_status`).
    /// Grava um `status_set_manual` com HLC do hub, como qualquer ambiente:
    /// vale até o próximo evento de status mais novo (um gatilho automático
    /// de outra natureza). Issue sem work item ganha um (`epic` se tem
    /// sub-issues no espelho, senão `issue`). Mesmo status = nada a gravar.
    pub fn set_status_manual(
        &self,
        owner: &str,
        repo: &str,
        gh_number: i64,
        status: WorkItemStatus,
        reason: Option<&str>,
        env_id: &str,
    ) -> Result<HubWorkItem> {
        self.set_status_with(owner, repo, gh_number, status, reason, None, env_id)
    }

    /// Como `set_status_manual`; `proposal_id` = aplicado por uma proposta
    /// aprovada (`cause: "proposal"`, #218).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn set_status_with(
        &self,
        owner: &str,
        repo: &str,
        gh_number: i64,
        status: WorkItemStatus,
        reason: Option<&str>,
        proposal_id: Option<i64>,
        env_id: &str,
    ) -> Result<HubWorkItem> {
        // `find_project`: o sentinela do gestor não tem prioridade nem
        // status manual.
        let Some(pid) = self.find_project(owner, repo)?.map(|p| p.id) else {
            return Err(LedgerError::NotFound(
                "projeto",
                format!("{owner}/{repo} não está no hub"),
            ));
        };
        let tx = self.tx()?;
        let (wi, from) = match self.find_work_item(owner, repo, gh_number)? {
            Some(w) => (w.id, w.status),
            None => (
                self.manual_work_item(pid, owner, repo, gh_number)?,
                WorkItemStatus::Backlog,
            ),
        };
        if from != status {
            let mut payload = json!({ "from": from, "to": status });
            priority::cause(&mut payload, proposal_id);
            if let Some(r) = reason.map(str::trim).filter(|r| !r.is_empty()) {
                payload["reason"] = json!(r);
            }
            self.hub_event(env_id, pid, Some(wi), "status_set_manual", &payload)?;
            self.recompute_status(wi)?;
        }
        tx.commit()?;
        self.work_item(wi)
    }

    /// Work item criado pelo hub para uma issue do espelho que ainda não
    /// tem um (mudança manual de status ou de prioridade): `epic` se tem
    /// sub-issues no espelho, senão `issue`. Issue fora do espelho = erro.
    fn manual_work_item(&self, pid: i64, owner: &str, repo: &str, gh_number: i64) -> Result<i64> {
        if self.get_issue(pid, gh_number)?.is_none() {
            return Err(LedgerError::NotFound(
                "issue",
                format!("{owner}/{repo}#{gh_number} não está no hub"),
            ));
        }
        let has_children: bool = self.conn.query_row(
            "SELECT EXISTS (SELECT 1 FROM issue_cache
                            WHERE project_id = ?1 AND parent_number = ?2)",
            params![pid, gh_number],
            |r| r.get(0),
        )?;
        let kind = if has_children {
            WorkItemKind::Epic
        } else {
            WorkItemKind::Issue
        };
        let wi = self.ensure_work_item(owner, repo, gh_number)?;
        self.conn.execute(
            "UPDATE work_item SET kind = ?2 WHERE id = ?1",
            params![wi, kind.as_str()],
        )?;
        Ok(wi)
    }

    /// Evento gravado pelo próprio hub (mudanças pela UI do hub), com o HLC
    /// do hub e `env_id` = `hub_id`.
    fn hub_event(
        &self,
        env_id: &str,
        project_id: i64,
        work_item_id: Option<i64>,
        kind: &str,
        payload: &serde_json::Value,
    ) -> Result<()> {
        let hlc = self.tick_hlc()?;
        let ts = now_ms();
        self.conn.execute(
            "INSERT INTO env (env_id, name, created_at) VALUES (?1, ?1, ?2)
             ON CONFLICT (env_id) DO NOTHING",
            params![env_id, ts],
        )?;
        self.conn.execute(
            "INSERT INTO event (uid, env_id, hlc, ts, project_id, work_item_id, type, payload)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                uuid::Uuid::new_v4().to_string(),
                env_id,
                hlc,
                ts,
                project_id,
                work_item_id,
                kind,
                payload.to_string()
            ],
        )?;
        Ok(())
    }

    pub fn work_item(&self, id: i64) -> Result<HubWorkItem> {
        Ok(self.conn.query_row(
            &format!("SELECT {WORK_ITEM_COLS} FROM work_item WHERE id = ?1"),
            [id],
            row_to_work_item,
        )?)
    }

    pub(crate) fn tx(&self) -> Result<NestedTx<'_>> {
        Ok(NestedTx(if self.conn.is_autocommit() {
            Some(self.conn.unchecked_transaction()?)
        } else {
            None
        }))
    }

    // --- espelho do GitHub (#60) -------------------------------------------

    fn gh(&self) -> GhMirror<'_> {
        GhMirror(&self.conn)
    }

    /// HLC dos eventos que o hub deriva do GitHub: depois de tudo o que ele
    /// já recebeu.
    fn tick_hlc(&self) -> Result<i64> {
        let max_event: i64 =
            self.conn
                .query_row("SELECT coalesce(max(hlc), 0) FROM event", [], |r| r.get(0))?;
        let last: i64 =
            self.conn
                .query_row("SELECT value FROM hub_meta WHERE key = 'hlc'", [], |r| {
                    r.get(0)
                })?;
        let next = hlc::recv(last, max_event, now_ms());
        self.conn
            .execute("UPDATE hub_meta SET value = ?1 WHERE key = 'hlc'", [next])?;
        Ok(next)
    }

    /// Work items com branch e não concluídos (os PRs a observar).
    pub fn github_work_items(&self, project_id: i64) -> Result<Vec<HubWorkItem>> {
        Ok(self
            .list_work_items(project_id)?
            .into_iter()
            .filter(|w| w.branch.is_some() && w.status != WorkItemStatus::Concluido)
            .collect())
    }

    /// Aplica a busca no GitHub de um projeto central numa transação. Os
    /// gatilhos de PR e o `issue_closed` viram eventos do hub (`env_id`)
    /// com `uid` determinístico: se um ambiente já enviou o mesmo, fica um só.
    pub fn apply_github(&self, s: &GithubSnapshot, env_id: &str) -> Result<GithubApplied> {
        let (owner, repo, deleted_at): (String, String, Option<i64>) = self.conn.query_row(
            "SELECT gh_owner, gh_repo, deleted_at FROM project WHERE id = ?1",
            [s.project_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        // Excluído enquanto a busca rodava sem o lock: nada a aplicar.
        if deleted_at.is_some() {
            return Ok(GithubApplied::default());
        }
        let tx = self.conn.unchecked_transaction()?;
        let mut out = GithubApplied::default();
        // Como no ambiente: o PR para a branch de um épico é da sub-issue, e
        // o merge do PR do épico conclui no fim as que o aguardam (#454).
        let mut merged = Vec::new();
        for pr in &s.prs {
            let route = self.gh().route_pr(s.project_id, pr, &s.issues)?;
            let pr = &match route {
                PrRoute::Comum => pr.clone(),
                PrRoute::Ignorado => continue,
                PrRoute::SubIssue { sub } => {
                    let wi = self.ensure_work_item(&owner, &repo, sub)?;
                    self.conn.execute(
                        "UPDATE work_item SET branch = ?2 WHERE id = ?1 AND branch IS NULL",
                        params![wi, pr.head],
                    )?;
                    PrRow {
                        work_item_id: wi,
                        ..pr.clone()
                    }
                }
            };
            let prev = self.gh().upsert_pr(s.project_id, pr, s.synced_at)?;
            out.prs += 1;
            let trigger = match route {
                PrRoute::SubIssue { .. } => sub_issue_pr_trigger(prev, pr.state),
                _ => pr_trigger(prev, pr.state),
            };
            let Some(trigger) = trigger else {
                continue;
            };
            if trigger == WorkItemTrigger::PrMerged {
                merged.push((pr.work_item_id, pr.number));
            }
            let status = self.work_item_status(pr.work_item_id)?;
            if let Some(next) = next_status(status, trigger) {
                self.gh_event(
                    env_id,
                    s.project_id,
                    pr.work_item_id,
                    &gh_pr_uid(&owner, &repo, pr.number, pr.state),
                    "status_changed",
                    json!({ "from": status, "to": next, "cause": trigger.as_str() }),
                )?;
                self.recompute_status(pr.work_item_id)?;
                out.status_changes.push((pr.work_item_id, next));
            }
        }
        for i in &s.issues {
            let prev = self.gh().upsert_issue(s.project_id, i, s.synced_at)?;
            out.issues += 1;
            if prev.as_deref() != Some("open") || i.state != "closed" {
                continue;
            }
            let wi: Option<i64> = self
                .conn
                .query_row(
                    "SELECT id FROM work_item WHERE project_id = ?1 AND gh_number = ?2",
                    params![s.project_id, i.gh_number],
                    |r| r.get(0),
                )
                .optional()?;
            if let Some(wi) = wi {
                if self.work_item_status(wi)? != WorkItemStatus::Concluido
                    && !self.gh().has_merged_pr(wi)?
                {
                    self.gh_event(
                        env_id,
                        s.project_id,
                        wi,
                        &gh_issue_closed_uid(&owner, &repo, i.gh_number),
                        "issue_closed",
                        json!({ "gh_number": i.gh_number }),
                    )?;
                    out.issues_closed.push(i.gh_number);
                }
            }
        }
        for (epic_wi, epic_pr) in merged {
            let epic: i64 = self.conn.query_row(
                "SELECT gh_number FROM work_item WHERE id = ?1",
                [epic_wi],
                |r| r.get(0),
            )?;
            for (wi, sub) in self.gh().awaiting_epic(s.project_id, epic)? {
                let trigger = WorkItemTrigger::EpicMerged;
                let status = self.work_item_status(wi)?;
                if let Some(next) = next_status(status, trigger) {
                    self.gh_event(
                        env_id,
                        s.project_id,
                        wi,
                        &gh_epic_merged_uid(&owner, &repo, epic_pr, sub),
                        "status_changed",
                        json!({ "from": status, "to": next, "cause": trigger.as_str() }),
                    )?;
                    self.recompute_status(wi)?;
                    out.status_changes.push((wi, next));
                }
            }
        }
        self.gh().finish_ok(s)?;
        tx.commit()?;
        Ok(out)
    }

    fn work_item_status(&self, id: i64) -> Result<WorkItemStatus> {
        let s: String =
            self.conn
                .query_row("SELECT status FROM work_item WHERE id = ?1", [id], |r| {
                    r.get(0)
                })?;
        Ok(s.parse()?)
    }

    fn gh_event(
        &self,
        env_id: &str,
        project_id: i64,
        work_item_id: i64,
        uid: &str,
        kind: &str,
        payload: serde_json::Value,
    ) -> Result<()> {
        let hlc = self.tick_hlc()?;
        self.conn.execute(
            "INSERT INTO env (env_id, name, created_at) VALUES (?1, ?1, ?2)
             ON CONFLICT (env_id) DO NOTHING",
            params![env_id, now_ms()],
        )?;
        self.conn.execute(
            "INSERT INTO event (uid, env_id, hlc, ts, project_id, work_item_id, type, payload)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) ON CONFLICT (uid) DO NOTHING",
            params![
                uid,
                env_id,
                hlc,
                now_ms(),
                project_id,
                work_item_id,
                kind,
                payload.to_string()
            ],
        )?;
        Ok(())
    }

    /// Registra a falha do sync com o GitHub. Projeto com lápide (excluído
    /// enquanto a busca rodava sem o lock) não ganha `sync_state` de novo.
    pub fn record_sync_error(&self, project_id: i64, error: &str) -> Result<()> {
        if self.live_project_rev(project_id)?.is_none() {
            return Ok(());
        }
        self.gh().record_sync_error(project_id, error)
    }

    /// `rev` do projeto cadastrado (sem lápide); `None` se não existe ou é
    /// lápide. O sync com o GitHub compara antes e depois da busca: se
    /// mudou (excluído, recadastrado ou editado), o resultado é descartado.
    pub fn live_project_rev(&self, project_id: i64) -> Result<Option<i64>> {
        Ok(self
            .conn
            .query_row(
                "SELECT rev FROM project WHERE id = ?1 AND deleted_at IS NULL",
                [project_id],
                |r| r.get(0),
            )
            .optional()?)
    }

    pub fn sync_states(&self) -> Result<Vec<SyncState>> {
        self.gh().sync_states()
    }

    pub fn sync_state(&self, project_id: i64) -> Result<Option<SyncState>> {
        self.gh().sync_state(project_id)
    }

    pub fn list_issues(&self, project_id: i64) -> Result<Vec<IssueRow>> {
        self.gh().list_issues(project_id)
    }

    pub fn get_issue(&self, project_id: i64, gh_number: i64) -> Result<Option<IssueRow>> {
        self.gh().get_issue(project_id, gh_number)
    }

    pub fn prs_for_work_item(&self, work_item_id: i64) -> Result<Vec<PrRow>> {
        self.gh().prs_for_work_item(work_item_id)
    }

    pub fn etag(&self, key: &str) -> Result<Option<String>> {
        self.gh().etag(key)
    }

    /// Registra quantas mudanças o ambiente ainda tem no outbox.
    pub fn set_env_pending(&self, env_id: &str, pending: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE env SET pending = ?2 WHERE env_id = ?1",
            params![env_id, pending.max(0)],
        )?;
        Ok(())
    }

    /// Guarda o resumo das limpezas pendentes que o ambiente informou no
    /// envio (issue #287). Um resumo mais velho que o guardado é ignorado.
    pub fn set_env_cleanup(&self, env_id: &str, c: &CleanupSummary) -> Result<()> {
        self.conn.execute(
            "UPDATE env SET cleanup_checked_at = ?2, cleanup_items = ?3, cleanup_bytes = ?4
             WHERE env_id = ?1 AND (cleanup_checked_at IS NULL OR cleanup_checked_at <= ?2)",
            params![env_id, c.checked_at, c.items.max(0), c.bytes.max(0)],
        )?;
        Ok(())
    }

    /// Revisão da configuração (`hub_meta.config_rev`, spec do hub como
    /// plano de controle §3.4). Zero até a primeira mudança de projeto ou
    /// atribuição feita no hub.
    pub fn config_rev(&self) -> Result<i64> {
        Ok(self
            .conn
            .query_row(
                "SELECT value FROM hub_meta WHERE key = 'config_rev'",
                [],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(0))
    }

    /// Época da configuração (`hub_meta.config_epoch`, §4.1).
    pub fn config_epoch(&self) -> Result<i64> {
        Ok(self.conn.query_row(
            "SELECT value FROM hub_meta WHERE key = 'config_epoch'",
            [],
            |r| r.get(0),
        )?)
    }

    /// Sorteia uma época nova (`cthubd restore`): os ambientes recebem o
    /// conjunto completo no próximo pull, mesmo com o `config_rev` igual.
    pub fn renew_config_epoch(&self) -> Result<i64> {
        let old = self.config_epoch()?;
        loop {
            let e: i64 = self.conn.query_row(
                "UPDATE hub_meta SET value = abs(random() % 9007199254740991)
                 WHERE key = 'config_epoch' RETURNING value",
                [],
                |r| r.get(0),
            )?;
            if e != old {
                return Ok(e);
            }
        }
    }

    /// Sorteia uma época nova direto no arquivo, **sem** aplicar migrações
    /// (o rollback do `cthubd update` volta um `hub.db` de esquema anterior
    /// e não pode migrá-lo de novo). Hub sem `config_epoch` (anterior à
    /// migração 8) não tem o que renovar: `None`.
    pub fn renew_config_epoch_at(path: &std::path::Path) -> Result<Option<i64>> {
        let conn = Connection::open(path)?;
        conn.busy_timeout(crate::BUSY_TIMEOUT)?;
        let old: Option<i64> = match conn
            .query_row(
                "SELECT value FROM hub_meta WHERE key = 'config_epoch'",
                [],
                |r| r.get(0),
            )
            .optional()
        {
            Ok(v) => v,
            // Sem a tabela hub_meta: esquema antigo, nada a renovar.
            Err(rusqlite::Error::SqliteFailure(_, Some(m))) if m.contains("no such table") => {
                return Ok(None)
            }
            Err(e) => return Err(e.into()),
        };
        let Some(old) = old else {
            return Ok(None);
        };
        loop {
            let e: i64 = conn.query_row(
                "UPDATE hub_meta SET value = abs(random() % 9007199254740991)
                 WHERE key = 'config_epoch' RETURNING value",
                [],
                |r| r.get(0),
            )?;
            if e != old {
                return Ok(Some(e));
            }
        }
    }

    // --- snapshots (#63) ---------------------------------------------------

    /// Grava uma cópia consistente do `hub.db` em
    /// `<dir>/hub-<AAAAMMDD-HHMM>.db` (UTC) com `VACUUM INTO`. Se já existe
    /// um snapshot daquele minuto, devolve o caminho sem regravar.
    pub fn snapshot(&self, dir: &Path, now_ms: i64) -> Result<PathBuf> {
        std::fs::create_dir_all(dir).map_err(|e| LedgerError::Conflict(e.to_string()))?;
        let path = dir.join(format!(
            "{SNAPSHOT_PREFIX}{}.db",
            ct_core::utc_stamp(now_ms)
        ));
        if !path.exists() {
            self.conn
                .execute("VACUUM INTO ?1", [path.to_string_lossy().as_ref()])?;
        }
        Ok(path)
    }

    // --- tokens de sync ---------------------------------------------------

    /// Gera o token de sync do ambiente `name` (substitui um anterior) e
    /// devolve só o token em claro. Veja [`Hub::issue_env_token`].
    pub fn add_env_token(&self, name: &str) -> Result<String> {
        Ok(self.issue_env_token(name)?.token)
    }

    /// Gera o token de sync do ambiente `name` já preso a um `env_id` (#195):
    /// um nome que já existe mantém o `env_id` do ambiente (girar o token não
    /// o solta); um nome novo (ou um token antigo ainda sem `env_id`) ganha um
    /// `env_id` novo, gerado aqui. O token em claro e o `env_id` voltam juntos;
    /// o hub guarda só o hash do token.
    pub fn issue_env_token(&self, name: &str) -> Result<IssuedEnvToken> {
        self.issue_env_token_for(name, None)
    }

    /// Como [`Hub::issue_env_token`], mas prende o token ao `env_id` que o
    /// ambiente já usa (ledger gravado antes de ele ter token). Recusa se o
    /// nome já está preso a outro `env_id`: girar o token nunca troca o dono.
    pub fn issue_env_token_with_env_id(&self, name: &str, env_id: &str) -> Result<IssuedEnvToken> {
        if let Some(Some(atual)) = self.env_token_env_id(name)? {
            if atual != env_id {
                return Err(LedgerError::Conflict(format!(
                    "o ambiente \"{}\" já está preso ao env_id {atual}, não a {env_id}",
                    name.trim()
                )));
            }
        }
        self.issue_env_token_for(name, Some(env_id))
    }

    /// Token de sync **sem** `env_id`, como o de instalações anteriores ao
    /// #195: prende-se ao `env_id` do primeiro uso. Só para testar esse
    /// caminho de compatibilidade; o `cthubd` usa [`Hub::issue_env_token`].
    #[doc(hidden)]
    pub fn add_legacy_env_token(&self, name: &str) -> Result<String> {
        let name = name.trim();
        let token = format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
        self.conn.execute(
            "INSERT INTO env_token (name, token_hash, created_at) VALUES (?1, ?2, ?3)
             ON CONFLICT (name) DO UPDATE SET token_hash = excluded.token_hash,
                env_id = NULL, created_at = excluded.created_at",
            params![name, token_hash(&token), now_ms()],
        )?;
        Ok(token)
    }

    fn issue_env_token_for(&self, name: &str, env_id: Option<&str>) -> Result<IssuedEnvToken> {
        let name = name.trim();
        if name.is_empty() {
            return Err(LedgerError::Conflict("nome do ambiente vazio".into()));
        }
        let tx = self.conn.unchecked_transaction()?;
        let atual: Option<Option<String>> = tx
            .query_row(
                "SELECT env_id FROM env_token WHERE name = ?1",
                [name],
                |r| r.get(0),
            )
            .optional()?;
        let env_id = match (env_id, atual.flatten()) {
            (Some(id), _) => id.to_string(),
            (None, Some(id)) => id,
            (None, None) => uuid::Uuid::new_v4().to_string(),
        };
        let token = format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
        tx.execute(
            "INSERT INTO env_token (name, token_hash, env_id, created_at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (name) DO UPDATE SET token_hash = excluded.token_hash,
                env_id = excluded.env_id, created_at = excluded.created_at",
            params![name, token_hash(&token), env_id, now_ms()],
        )
        .map_err(|e| match e {
            rusqlite::Error::SqliteFailure(f, _)
                if f.code == rusqlite::ErrorCode::ConstraintViolation =>
            {
                LedgerError::Conflict(format!("o ambiente {env_id} já usa outro token"))
            }
            e => e.into(),
        })?;
        tx.commit()?;
        Ok(IssuedEnvToken { token, env_id })
    }

    /// Revoga o token do ambiente e, na mesma transação, o de agente e a
    /// chave pública da entrega de secrets do `env_id` preso a ele (#184).
    /// Retorna `false` se não havia.
    pub fn revoke_env_token(&self, name: &str) -> Result<bool> {
        let name = name.trim();
        let tx = self.conn.unchecked_transaction()?;
        tx.execute("DELETE FROM agent_token WHERE env_name = ?1", [name])?;
        tx.execute(
            "DELETE FROM env_key WHERE env_id IN
               (SELECT env_id FROM env_token WHERE name = ?1 AND env_id IS NOT NULL)",
            [name],
        )?;
        let n = tx.execute("DELETE FROM env_token WHERE name = ?1", [name])?;
        tx.commit()?;
        Ok(n > 0)
    }

    /// Gera (ou troca) o token de agente do ambiente `name`, que precisa ter
    /// token de sync. O anterior deixa de valer na hora. Formato: o do token
    /// de sync com o prefixo [`AGENT_TOKEN_PREFIX`].
    pub fn add_agent_token(&self, name: &str) -> Result<String> {
        let name = name.trim();
        if !self.env_token_names()?.iter().any(|n| n == name) {
            return Err(LedgerError::NotFound(
                "env_token",
                format!("{name} (gere o token de sync antes, com cthubd add-env {name})"),
            ));
        }
        let token = format!(
            "{AGENT_TOKEN_PREFIX}{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
        self.conn.execute(
            "INSERT INTO agent_token (env_name, token_hash, created_at) VALUES (?1, ?2, ?3)
             ON CONFLICT (env_name) DO UPDATE SET token_hash = excluded.token_hash,
                created_at = excluded.created_at, last_used_at = NULL",
            params![name, token_hash(&token), now_ms()],
        )?;
        Ok(token)
    }

    /// Revoga só o token de agente. Retorna `false` se não havia.
    pub fn revoke_agent_token(&self, name: &str) -> Result<bool> {
        Ok(self
            .conn
            .execute("DELETE FROM agent_token WHERE env_name = ?1", [name.trim()])?
            > 0)
    }

    /// Ambiente dono do token de agente, se válido; marca o uso.
    pub fn authenticate_agent(&self, token: &str) -> Result<Option<EnvToken>> {
        if !token.starts_with(AGENT_TOKEN_PREFIX) {
            return Ok(None);
        }
        let hash = token_hash(token);
        let found = self
            .conn
            .query_row(
                "SELECT e.name, e.env_id FROM agent_token a
                 JOIN env_token e ON e.name = a.env_name WHERE a.token_hash = ?1",
                [&hash],
                |r| {
                    Ok(EnvToken {
                        name: r.get(0)?,
                        env_id: r.get(1)?,
                    })
                },
            )
            .optional()?;
        if found.is_some() {
            self.conn.execute(
                "UPDATE agent_token SET last_used_at = ?2 WHERE token_hash = ?1",
                params![hash, now_ms()],
            )?;
        }
        Ok(found)
    }

    /// Ambiente dono do token, se válido.
    pub fn authenticate(&self, token: &str) -> Result<Option<EnvToken>> {
        if token.is_empty() {
            return Ok(None);
        }
        Ok(self
            .conn
            .query_row(
                "SELECT name, env_id FROM env_token WHERE token_hash = ?1",
                [token_hash(token)],
                |r| {
                    Ok(EnvToken {
                        name: r.get(0)?,
                        env_id: r.get(1)?,
                    })
                },
            )
            .optional()?)
    }

    /// Confere o `env_id` do primeiro uso com o preso ao token de `name`
    /// (#195): recusa se forem diferentes. Só um token de instalação antiga,
    /// gerado sem `env_id` (hoje [`Hub::issue_env_token`] sempre prende), é
    /// preso aqui no primeiro envio, com aviso no log. Recusa também se o
    /// `env_id` usa outro token.
    pub fn bind_env_token(&self, name: &str, env_id: &str) -> Result<()> {
        let atual: Option<Option<String>> = self
            .conn
            .query_row(
                "SELECT env_id FROM env_token WHERE name = ?1",
                [name],
                |r| r.get(0),
            )
            .optional()?;
        match atual {
            None => Err(LedgerError::NotFound("env_token", name.to_string())),
            Some(Some(id)) if id == env_id => Ok(()),
            Some(Some(id)) => Err(LedgerError::Conflict(format!(
                "o token de {name} já pertence ao ambiente {id}"
            ))),
            Some(None) => {
                self.conn
                    .execute(
                        "UPDATE env_token SET env_id = ?2 WHERE name = ?1",
                        params![name, env_id],
                    )
                    .map_err(|e| match e {
                        rusqlite::Error::SqliteFailure(f, _)
                            if f.code == rusqlite::ErrorCode::ConstraintViolation =>
                        {
                            LedgerError::Conflict(format!("o ambiente {env_id} já usa outro token"))
                        }
                        e => e.into(),
                    })?;
                Ok(())
            }
        }
    }

    /// Nomes dos ambientes com token.
    pub fn env_token_names(&self) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT name FROM env_token ORDER BY name")?;
        let rows = stmt.query_map([], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    // --- leitura ---------------------------------------------------------

    pub fn list_projects(&self) -> Result<Vec<HubProject>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {PROJECT_COLS} FROM project WHERE deleted_at IS NULL AND kind = 'github'
             ORDER BY rank NULLS LAST, name, key"
        ))?;
        let rows = stmt.query_map([], row_to_project)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Projeto cadastrado (sem o sentinela do gestor, que não é editável,
    /// sincronizável nem atribuível).
    pub fn find_project(&self, owner: &str, repo: &str) -> Result<Option<HubProject>> {
        Ok(self
            .conn
            .query_row(
                &format!(
                    "SELECT {PROJECT_COLS} FROM project
                     WHERE key = ?1 AND deleted_at IS NULL AND kind = 'github'"
                ),
                [project_key(owner, repo)],
                row_to_project,
            )
            .optional()?)
    }

    pub fn find_work_item(
        &self,
        owner: &str,
        repo: &str,
        gh_number: i64,
    ) -> Result<Option<HubWorkItem>> {
        Ok(self
            .conn
            .query_row(
                &format!(
                    "SELECT {WORK_ITEM_COLS} FROM work_item
                     WHERE project_id = (SELECT id FROM project WHERE key = ?1 AND deleted_at IS NULL)
                       AND gh_number = ?2"
                ),
                params![project_key(owner, repo), gh_number],
                row_to_work_item,
            )
            .optional()?)
    }

    pub fn list_work_items(&self, project_id: i64) -> Result<Vec<HubWorkItem>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {WORK_ITEM_COLS} FROM work_item WHERE project_id = ?1 ORDER BY gh_number"
        ))?;
        let rows = stmt.query_map([project_id], row_to_work_item)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn list_sessions(&self, work_item_id: i64) -> Result<Vec<HubSession>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {SESSION_COLS} FROM session WHERE work_item_id = ?1 ORDER BY started_at"
        ))?;
        let rows = stmt.query_map([work_item_id], row_to_session)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Decisões dos work items, na ordem de criação.
    pub fn list_decisions(
        &self,
        work_item_ids: &[i64],
        only_unreviewed: bool,
    ) -> Result<Vec<HubDecision>> {
        if work_item_ids.is_empty() {
            return Ok(Vec::new());
        }
        let ids = work_item_ids
            .iter()
            .map(|i| i.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        let filter = if only_unreviewed {
            " AND reviewed_at IS NULL"
        } else {
            ""
        };
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {DECISION_COLS} FROM decision WHERE work_item_id IN ({ids}){filter}
             ORDER BY created_at, uid"
        ))?;
        let rows = stmt.query_map([], row_to_decision)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Eventos do work item, do mais recente para o mais antigo (por HLC).
    pub fn list_events(&self, work_item_id: i64, limit: u32) -> Result<Vec<HubEvent>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {EVENT_COLS} FROM event WHERE work_item_id = ?1
             ORDER BY hlc DESC, env_id DESC LIMIT ?2"
        ))?;
        let rows = stmt.query_map(params![work_item_id, limit], row_to_event)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Ambientes que já enviaram ao hub. Ficam de fora os autores que só
    /// aparecem em eventos (o próprio `cthubd`, no status manual e nos
    /// gatilhos do GitHub).
    pub fn list_envs(&self) -> Result<Vec<HubEnv>> {
        let mut stmt = self.conn.prepare(
            "SELECT env_id, name, created_at, last_seen_at, last_seq, pending,
                    cleanup_checked_at, cleanup_items, cleanup_bytes FROM env
             WHERE last_seen_at IS NOT NULL ORDER BY name",
        )?;
        let rows = stmt.query_map([], |r| {
            let checked_at: Option<i64> = r.get(6)?;
            Ok(HubEnv {
                env_id: r.get(0)?,
                name: r.get(1)?,
                created_at: r.get(2)?,
                last_seen_at: r.get(3)?,
                last_seq: r.get(4)?,
                pending: r.get(5)?,
                cleanup: checked_at.map(|checked_at| CleanupSummary {
                    checked_at,
                    items: r.get::<_, Option<i64>>(7).ok().flatten().unwrap_or(0),
                    bytes: r.get::<_, Option<i64>>(8).ok().flatten().unwrap_or(0),
                }),
                online: false,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Caminhos do repositório por ambiente (`env_id`, `repo_path`), sem os removidos.
    pub fn project_locations(&self, project_id: i64) -> Result<Vec<(String, String)>> {
        let mut stmt = self.conn.prepare(
            "SELECT env_id, repo_path FROM project_location
             WHERE project_id = ?1 AND removed = 0 ORDER BY env_id",
        )?;
        let rows = stmt.query_map([project_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Worktrees do work item por ambiente (`env_id`, `worktree_path`).
    pub fn work_item_locations(&self, work_item_id: i64) -> Result<Vec<(String, Option<String>)>> {
        let mut stmt = self.conn.prepare(
            "SELECT env_id, worktree_path FROM work_item_location
             WHERE work_item_id = ?1 ORDER BY env_id",
        )?;
        let rows = stmt.query_map([work_item_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Ledger, NewDecision, NewProject, NewSession, ProjectPatch};
    use ct_core::WorkItemTrigger;

    fn ledger(path: &str) -> Ledger {
        let l = Ledger::open_in_memory().unwrap();
        l.add_project(&NewProject {
            name: "demo".into(),
            gh_owner: "deep-ion-ai".into(),
            gh_repo: "demo".into(),
            repo_path: path.into(),
            base_branch: "develop".into(),
            protected_branches: vec!["develop".into(), "main".into()],
        })
        .unwrap();
        l
    }

    fn novo(owner: &str, repo: &str) -> NewHubProject {
        NewHubProject {
            owner: owner.into(),
            repo: repo.into(),
            name: repo.into(),
            base_branch: "develop".into(),
            protected_branches: vec!["develop".into(), "main".into()],
        }
    }

    /// Hub com `owner/repo` cadastrado (o hub nunca cria projeto por push).
    fn hub_com(owner: &str, repo: &str) -> Hub {
        let hub = Hub::open_in_memory().unwrap();
        hub.add_project(&novo(owner, repo)).unwrap();
        hub
    }

    /// Envia ao hub tudo o que está no outbox do ledger e confirma.
    fn push(hub: &Hub, l: &Ledger, name: &str) -> IngestResult {
        let batch = l.outbox_pending(10_000).unwrap();
        let r = hub.ingest(l.env_id(), name, &batch).unwrap();
        l.outbox_ack(r.last_seq).unwrap();
        r
    }

    /// Linhas da tabela; em `project`, sem o sentinela do gestor.
    fn contar(hub: &Hub, tabela: &str) -> i64 {
        let filtro = if tabela == "project" {
            " WHERE kind = 'github'"
        } else {
            ""
        };
        hub.conn
            .query_row(&format!("SELECT count(*) FROM {tabela}{filtro}"), [], |r| {
                r.get(0)
            })
            .unwrap()
    }

    #[test]
    fn sentinela_do_gestor_recebe_o_push_e_fica_fora_das_visoes() {
        let hub = hub_com("deep-ion-ai", "demo");
        let l = ledger("/x");
        let (_, wi) = l.ensure_gestor_project("/g").unwrap();
        l.record_decision(&crate::NewDecision {
            work_item_id: wi.id,
            session_id: None,
            origin: DecisionOrigin::Orientada,
            title: "P0 em deep-ion-ai/demo#1".into(),
            context: "c".into(),
            decision: "d".into(),
            consequences: None,
        })
        .unwrap();
        let r = push(&hub, &l, "wsl");
        assert_eq!(r.quarantined, 0);
        let g = hub.find_work_item("_ctower", "gestor", 0).unwrap().unwrap();
        assert_eq!(hub.list_decisions(&[g.id], false).unwrap().len(), 1);
        assert_eq!(hub.dashboard().unwrap().gestor_work_item_id, Some(g.id));
        // Fora das listas, da árvore, do ranking, da saúde e do dashboard.
        assert!(hub.find_project("_ctower", "gestor").unwrap().is_none());
        assert!(hub.tree("_ctower", "gestor").unwrap().is_none());
        assert!(hub
            .list_projects()
            .unwrap()
            .iter()
            .all(|p| p.gh_owner != "_ctower"));
        assert!(hub
            .project_views()
            .unwrap()
            .iter()
            .all(|p| p.project.gh_owner != "_ctower"));
        assert!(hub
            .dashboard()
            .unwrap()
            .projects
            .iter()
            .all(|p| p.project.gh_owner != "_ctower"));
        assert!(hub
            .ranking(&RankingFilter::default(), 50)
            .unwrap()
            .items
            .iter()
            .all(|i| i.owner != "_ctower"));
        let h = hub
            .health(
                &HealthThresholds::default(),
                60,
                &HealthFilter::default(),
                now_ms() + 1_000_000_000,
            )
            .unwrap();
        assert!(h
            .signals
            .iter()
            .all(|s| s.owner.as_deref() != Some("_ctower")));
        assert!(matches!(
            hub.set_priority(
                "_ctower",
                "gestor",
                0,
                Some(Priority::P1),
                None,
                None,
                "hub"
            ),
            Err(LedgerError::NotFound(..))
        ));
    }

    #[test]
    fn ingestao_e_idempotente() {
        let hub = hub_com("deep-ion-ai", "demo");
        let l = ledger("/a/demo");
        let wi = l.ensure_work_item(1, 12, WorkItemKind::Epic).unwrap();
        let s = l
            .create_session(&NewSession {
                work_item_id: wi.id,
                tool: Tool::Claude,
                cwd: "/w".into(),
                launch_cmd: vec![],
                model: None,
                mode: ct_core::SessionMode::Native,
            })
            .unwrap();
        l.record_decision(&NewDecision {
            work_item_id: wi.id,
            session_id: Some(s.id.clone()),
            origin: DecisionOrigin::Autonoma,
            title: "t".into(),
            context: "c".into(),
            decision: "d".into(),
            consequences: None,
        })
        .unwrap();
        let batch = l.outbox_pending(10_000).unwrap();
        let r1 = hub.ingest(l.env_id(), "wsl", &batch).unwrap();
        assert_eq!(r1.last_seq, batch.last().unwrap().seq);
        let antes: Vec<i64> = ["project", "work_item", "session", "decision", "event"]
            .iter()
            .map(|t| contar(&hub, t))
            .collect();
        assert_eq!(antes, vec![1, 1, 1, 1, antes[4]]);
        // Mesmo lote de novo (o ack se perdeu): nada muda.
        hub.ingest(l.env_id(), "wsl", &batch).unwrap();
        let depois: Vec<i64> = ["project", "work_item", "session", "decision", "event"]
            .iter()
            .map(|t| contar(&hub, t))
            .collect();
        assert_eq!(antes, depois);
        // Lote vazio devolve o último seq já visto.
        assert_eq!(
            hub.ingest(l.env_id(), "wsl", &[]).unwrap().last_seq,
            r1.last_seq
        );
        let envs = hub.list_envs().unwrap();
        assert_eq!(envs.len(), 1);
        assert_eq!(envs[0].name, "wsl");
        assert!(envs[0].last_seen_at.is_some());
    }

    #[test]
    fn funde_projeto_e_work_item_de_dois_ambientes() {
        let hub = hub_com("deep-ion-ai", "demo");
        let a = ledger("/a/demo");
        let b = ledger("/b/Demo");
        // Mesmo repositório com outra caixa no segundo ambiente.
        b.conn
            .execute("UPDATE project SET gh_owner = 'Deep-Ion-AI'", [])
            .unwrap();
        let wa = a.ensure_work_item(1, 12, WorkItemKind::Epic).unwrap();
        a.set_work_item_branch(wa.id, "wip/12-x", "/a/wt/12")
            .unwrap();
        let wb = b.ensure_work_item(1, 12, WorkItemKind::Epic).unwrap();
        b.set_work_item_branch(wb.id, "wip/12-x", "/b/wt/12")
            .unwrap();
        push(&hub, &a, "a");
        push(&hub, &b, "b");

        let ps = hub.list_projects().unwrap();
        assert_eq!(ps.len(), 1);
        let locs = hub.project_locations(ps[0].id).unwrap();
        assert_eq!(locs.len(), 2);
        let w = hub
            .find_work_item("DEEP-ION-AI", "demo", 12)
            .unwrap()
            .unwrap();
        assert_eq!(w.branch.as_deref(), Some("wip/12-x"));
        assert_eq!(w.kind, WorkItemKind::Epic);
        let mut wt: Vec<_> = hub
            .work_item_locations(w.id)
            .unwrap()
            .into_iter()
            .filter_map(|(_, p)| p)
            .collect();
        wt.sort();
        assert_eq!(wt, vec!["/a/wt/12", "/b/wt/12"]);
        // Eventos dos dois ambientes no mesmo work item.
        let envs: std::collections::BTreeSet<_> = hub
            .list_events(w.id, 100)
            .unwrap()
            .into_iter()
            .map(|e| e.env_id)
            .collect();
        assert_eq!(envs.len(), 2);
    }

    #[test]
    fn push_com_atributo_divergente_nao_muda_o_hub() {
        let hub = hub_com("deep-ion-ai", "demo");
        let antes = hub.find_project("deep-ion-ai", "demo").unwrap().unwrap();
        let rev = hub.config_rev().unwrap();
        let a = ledger("/a");
        let b = ledger("/b");
        a.update_project(
            1,
            &ProjectPatch {
                name: Some("outro nome".into()),
                base_branch: Some("main".into()),
                protected_branches: Some(vec![]),
                ..Default::default()
            },
        )
        .unwrap();
        b.update_project(
            1,
            &ProjectPatch {
                base_branch: Some("trunk".into()),
                ..Default::default()
            },
        )
        .unwrap();
        let r = push(&hub, &a, "a");
        assert_eq!((r.quarantined, r.discarded), (0, 0));
        push(&hub, &b, "b");
        // Os atributos são do hub; do ambiente só vale onde ele clonou.
        let p = hub.find_project("deep-ion-ai", "demo").unwrap().unwrap();
        assert_eq!(p, antes);
        assert_eq!(hub.config_rev().unwrap(), rev);
        let mut locs = hub.project_locations(p.id).unwrap();
        locs.sort_by(|x, y| x.1.cmp(&y.1));
        assert_eq!(
            locs.iter().map(|l| l.1.as_str()).collect::<Vec<_>>(),
            vec!["/a", "/b"]
        );
    }

    /// Põe o HLC de `depois` à frente do de `antes`, sem depender do relógio de parede.
    fn adianta_hlc(depois: &Ledger, antes: &Ledger) {
        let atual: i64 = antes
            .conn
            .query_row("SELECT value FROM ledger_meta WHERE key = 'hlc'", [], |r| {
                r.get(0)
            })
            .unwrap();
        depois
            .conn
            .execute(
                "UPDATE ledger_meta SET value = max(value, ?1) WHERE key = 'hlc'",
                [atual + 1],
            )
            .unwrap();
    }

    #[test]
    fn evento_atrasado_nao_sobrescreve_status_mais_novo() {
        let hub = hub_com("deep-ion-ai", "demo");
        let a = ledger("/a");
        let b = ledger("/b");
        let wa = a.ensure_work_item(1, 7, WorkItemKind::Issue).unwrap();
        let wb = b.ensure_work_item(1, 7, WorkItemKind::Issue).unwrap();
        // a (offline) pausa; depois b leva a em_revisao.
        a.set_work_item_status(wa.id, WorkItemStatus::Pausado, "manual")
            .unwrap();
        adianta_hlc(&b, &a);
        b.apply_trigger(wb.id, WorkItemTrigger::PrOpened).unwrap();
        push(&hub, &b, "b");
        let w = hub
            .find_work_item("deep-ion-ai", "demo", 7)
            .unwrap()
            .unwrap();
        assert_eq!(w.status, WorkItemStatus::EmRevisao);
        // a volta e envia o evento antigo: fica no histórico, status não muda.
        push(&hub, &a, "a");
        let w = hub
            .find_work_item("deep-ion-ai", "demo", 7)
            .unwrap()
            .unwrap();
        assert_eq!(w.status, WorkItemStatus::EmRevisao);
        let status_evs = hub
            .list_events(w.id, 100)
            .unwrap()
            .into_iter()
            .filter(|e| e.kind == "status_changed")
            .count();
        assert_eq!(status_evs, 2);
        // Mudança nova de a vence.
        a.apply_trigger(wa.id, WorkItemTrigger::PrMerged).unwrap();
        push(&hub, &a, "a");
        let w = hub
            .find_work_item("deep-ion-ai", "demo", 7)
            .unwrap()
            .unwrap();
        assert_eq!(w.status, WorkItemStatus::Concluido);
        assert!(w.closed_at.is_some());
    }

    #[test]
    fn hub_aceita_aguardando_epico_vindo_do_ambiente() {
        let hub = hub_com("deep-ion-ai", "demo");
        let a = ledger("/a");
        let wa = a.ensure_work_item(1, 7, WorkItemKind::Issue).unwrap();
        a.apply_trigger(wa.id, WorkItemTrigger::SubIssuePrMerged)
            .unwrap();
        push(&hub, &a, "a");
        let w = hub
            .find_work_item("deep-ion-ai", "demo", 7)
            .unwrap()
            .unwrap();
        assert_eq!(w.status, WorkItemStatus::AguardandoEpico);
        assert!(w.closed_at.is_none());
        a.apply_trigger(wa.id, WorkItemTrigger::EpicMerged).unwrap();
        push(&hub, &a, "a");
        let w = hub
            .find_work_item("deep-ion-ai", "demo", 7)
            .unwrap()
            .unwrap();
        assert_eq!(w.status, WorkItemStatus::Concluido);
        assert!(w.closed_at.is_some());
    }

    #[test]
    fn empate_de_hlc_desempata_por_env_id() {
        let hub = hub_com("o", "r");
        let mk = |env: &str, uid: &str, to: &str| OutboxEntry {
            seq: 1,
            created_at: 0,
            change: SyncChange::Event(EventChange {
                uid: uid.into(),
                env_id: env.into(),
                hlc: hlc::from_ms(1_000),
                ts: 1_000,
                owner: "o".into(),
                repo: "r".into(),
                gh_number: Some(1),
                session_id: None,
                kind: "status_changed".into(),
                payload: serde_json::json!({ "from": "backlog", "to": to }),
            }),
        };
        hub.ingest("env-b", "b", &[mk("env-b", "u2", "pausado")])
            .unwrap();
        hub.ingest("env-a", "a", &[mk("env-a", "u1", "em_execucao")])
            .unwrap();
        let w = hub.find_work_item("o", "r", 1).unwrap().unwrap();
        assert_eq!(w.status, WorkItemStatus::Pausado);
    }

    fn issue_row(n: i64, parent: Option<i64>, has_sub: bool) -> IssueRow {
        IssueRow {
            gh_number: n,
            title: format!("issue {n}"),
            state: "open".into(),
            labels: vec![],
            parent_number: parent,
            has_sub_issues: has_sub,
            url: "u".into(),
            updated_at: 1,
            sprints: Vec::new(),
        }
    }

    #[test]
    fn status_manual_grava_evento_com_motivo_e_devolve_o_item() {
        use ct_core::WorkItemStatus::*;
        let hub = hub_com("deep-ion-ai", "demo");
        let a = ledger("/a");
        a.ensure_work_item(1, 7, WorkItemKind::Issue).unwrap();
        push(&hub, &a, "a");
        let w = hub
            .set_status_manual(
                "deep-ion-ai",
                "demo",
                7,
                Bloqueado,
                Some(" falta API "),
                "hub",
            )
            .unwrap();
        assert_eq!(w.status, Bloqueado);
        let evs = hub.list_events(w.id, 100).unwrap();
        let ev = evs.iter().find(|e| e.kind == "status_set_manual").unwrap();
        assert_eq!(ev.payload["to"], "bloqueado");
        assert_eq!(ev.payload["from"], "backlog");
        assert_eq!(ev.payload["reason"], "falta API");
        // Mesmo status: nada novo no histórico.
        hub.set_status_manual("deep-ion-ai", "demo", 7, Bloqueado, None, "hub")
            .unwrap();
        let n = hub
            .list_events(w.id, 100)
            .unwrap()
            .iter()
            .filter(|e| e.kind == "status_set_manual")
            .count();
        assert_eq!(n, 1);
        // Sem motivo, o campo não existe.
        let w = hub
            .set_status_manual("deep-ion-ai", "demo", 7, Pausado, None, "hub")
            .unwrap();
        let ev = hub
            .list_events(w.id, 100)
            .unwrap()
            .into_iter()
            .find(|e| e.payload["to"] == "pausado")
            .unwrap();
        assert!(ev.payload.get("reason").is_none());
    }

    #[test]
    fn status_manual_vale_ate_o_proximo_gatilho_automatico() {
        use crate::{GithubSnapshot, PrRow};
        use ct_core::{PrState, WorkItemStatus::*};
        let hub = hub_com("deep-ion-ai", "demo");
        let a = ledger("/a");
        let wa = a.ensure_work_item(1, 7, WorkItemKind::Issue).unwrap();
        a.set_work_item_branch(wa.id, "feat/7", "/a/wt").unwrap();
        push(&hub, &a, "a");
        let p = hub.find_project("deep-ion-ai", "demo").unwrap().unwrap();
        let w = hub
            .set_status_manual("deep-ion-ai", "demo", 7, Pausado, None, "hub")
            .unwrap();
        let pr = |state| PrRow {
            number: 30,
            work_item_id: w.id,
            head: "feat/7".into(),
            base: "develop".into(),
            state,
            url: "u".into(),
            updated_at: 1,
            head_sha: None,
        };
        let snap = |state| GithubSnapshot {
            project_id: p.id,
            prs: vec![pr(state)],
            synced_at: 5,
            ..Default::default()
        };
        // O PR aberto é um gatilho de outra natureza: vence o manual.
        hub.apply_github(&snap(PrState::Open), "hub").unwrap();
        let w = hub
            .find_work_item("deep-ion-ai", "demo", 7)
            .unwrap()
            .unwrap();
        assert_eq!(w.status, EmRevisao);
        // E um novo manual vence o automático anterior.
        let w = hub
            .set_status_manual("deep-ion-ai", "demo", 7, Bloqueado, None, "hub")
            .unwrap();
        assert_eq!(w.status, Bloqueado);
        hub.apply_github(&snap(PrState::Merged), "hub").unwrap();
        let w = hub
            .find_work_item("deep-ion-ai", "demo", 7)
            .unwrap()
            .unwrap();
        assert_eq!(w.status, Concluido);
        assert!(w.closed_at.is_some());
    }

    #[test]
    fn status_manual_cria_work_item_epic_ou_issue_pelo_espelho() {
        use crate::GithubSnapshot;
        use ct_core::WorkItemStatus::*;
        let hub = hub_com("deep-ion-ai", "demo");
        let a = ledger("/a");
        a.ensure_work_item(1, 1, WorkItemKind::Issue).unwrap();
        push(&hub, &a, "a");
        let p = hub.find_project("deep-ion-ai", "demo").unwrap().unwrap();
        hub.apply_github(
            &GithubSnapshot {
                project_id: p.id,
                issues: vec![issue_row(10, None, true), issue_row(11, Some(10), false)],
                synced_at: 5,
                ..Default::default()
            },
            "hub",
        )
        .unwrap();
        let e = hub
            .set_status_manual("deep-ion-ai", "demo", 10, EmExecucao, None, "hub")
            .unwrap();
        assert_eq!((e.kind, e.status), (WorkItemKind::Epic, EmExecucao));
        let i = hub
            .set_status_manual("deep-ion-ai", "demo", 11, Backlog, None, "hub")
            .unwrap();
        assert_eq!((i.kind, i.status), (WorkItemKind::Issue, Backlog));
        assert!(hub
            .history("deep-ion-ai", "demo", 11, 10, None, false)
            .unwrap()
            .is_some());
    }

    #[test]
    fn status_manual_recusa_projeto_e_issue_desconhecidos() {
        use ct_core::WorkItemStatus::*;
        let hub = hub_com("deep-ion-ai", "demo");
        let a = ledger("/a");
        push(&hub, &a, "a");
        let e = hub
            .set_status_manual("x", "y", 1, Pausado, None, "hub")
            .unwrap_err();
        assert!(e.to_string().contains("x/y"), "{e}");
        let e = hub
            .set_status_manual("deep-ion-ai", "demo", 99, Pausado, None, "hub")
            .unwrap_err();
        assert!(e.to_string().contains("#99"), "{e}");
        assert!(hub
            .find_work_item("deep-ion-ai", "demo", 99)
            .unwrap()
            .is_none());
    }

    #[test]
    fn status_manual_ingerido_de_ambiente_segue_o_hlc() {
        use ct_core::WorkItemStatus::*;
        let hub = hub_com("o", "r");
        let mk = |hlc, to: &str, kind: &str| OutboxEntry {
            seq: 1,
            created_at: 0,
            change: SyncChange::Event(EventChange {
                uid: format!("u{hlc}"),
                env_id: "env-a".into(),
                hlc,
                ts: 1_000,
                owner: "o".into(),
                repo: "r".into(),
                gh_number: Some(1),
                session_id: None,
                kind: kind.into(),
                payload: serde_json::json!({ "to": to }),
            }),
        };
        // Replay fora de ordem: o manual mais novo vence o automático antigo.
        let novo = mk(hlc::from_ms(2_000), "bloqueado", "status_set_manual");
        hub.ingest("env-a", "a", &[novo]).unwrap();
        let velho = mk(hlc::from_ms(1_000), "em_execucao", "status_changed");
        hub.ingest("env-a", "a", &[velho]).unwrap();
        let w = hub.find_work_item("o", "r", 1).unwrap().unwrap();
        assert_eq!(w.status, Bloqueado);
    }

    #[test]
    fn revisao_de_decisao_vale_a_primeira() {
        let hub = hub_com("deep-ion-ai", "demo");
        let a = ledger("/a");
        let wi = a.ensure_work_item(1, 3, WorkItemKind::Issue).unwrap();
        let d = a
            .record_decision(&NewDecision {
                work_item_id: wi.id,
                session_id: None,
                origin: DecisionOrigin::Autonoma,
                title: "t".into(),
                context: "c".into(),
                decision: "d".into(),
                consequences: None,
            })
            .unwrap();
        a.mark_reviewed(&[d.id], Some(10)).unwrap();
        push(&hub, &a, "a");
        // Uma revisão posterior (outro PR) não reescreve a primeira.
        hub.ingest(
            a.env_id(),
            "a",
            &[OutboxEntry {
                seq: 99,
                created_at: 0,
                change: SyncChange::DecisionReview {
                    uid: d.uid.clone(),
                    reviewed_at: 1,
                    reviewed_in_pr: Some(11),
                },
            }],
        )
        .unwrap();
        let w = hub
            .find_work_item("deep-ion-ai", "demo", 3)
            .unwrap()
            .unwrap();
        let ds = hub.list_decisions(&[w.id], false).unwrap();
        assert_eq!(ds.len(), 1);
        assert_eq!(ds[0].reviewed_in_pr, Some(10));
        assert!(hub.list_decisions(&[w.id], true).unwrap().is_empty());
    }

    #[test]
    fn revisao_sem_pr_chega_ao_hub() {
        let hub = hub_com("deep-ion-ai", "demo");
        let a = ledger("/a");
        let wi = a.ensure_work_item(1, 3, WorkItemKind::Issue).unwrap();
        let d = a
            .record_decision(&NewDecision {
                work_item_id: wi.id,
                session_id: None,
                origin: DecisionOrigin::Autonoma,
                title: "t".into(),
                context: "c".into(),
                decision: "d".into(),
                consequences: None,
            })
            .unwrap();
        a.mark_reviewed(&[d.id], None).unwrap();
        push(&hub, &a, "a");
        let w = hub
            .find_work_item("deep-ion-ai", "demo", 3)
            .unwrap()
            .unwrap();
        let ds = hub.list_decisions(&[w.id], false).unwrap();
        assert!(ds[0].reviewed_at.is_some());
        assert_eq!(ds[0].reviewed_in_pr, None);
        assert!(hub.list_decisions(&[w.id], true).unwrap().is_empty());
    }

    #[test]
    fn sessao_aplica_a_copia_mais_recente() {
        let hub = hub_com("deep-ion-ai", "demo");
        let a = ledger("/a");
        let wi = a.ensure_work_item(1, 3, WorkItemKind::Issue).unwrap();
        let s = a
            .create_session(&NewSession {
                work_item_id: wi.id,
                tool: Tool::Claude,
                cwd: "/w".into(),
                launch_cmd: vec![],
                model: Some("opus".into()),
                mode: SessionMode::Subagents,
            })
            .unwrap();
        let mut velha = a.outbox_pending(100).unwrap();
        a.transition_session(&s.id, SessionState::Running, None)
            .unwrap();
        // Empate no milissegundo (issue #293): o HLC é que desempata.
        let ultimo = a.get_session(&s.id).unwrap().last_event_at;
        for e in &mut velha {
            if let SyncChange::Session(c) = &mut e.change {
                c.last_event_at = ultimo;
            }
        }
        push(&hub, &a, "a");
        hub.ingest(a.env_id(), "a", &velha).unwrap();
        let w = hub
            .find_work_item("deep-ion-ai", "demo", 3)
            .unwrap()
            .unwrap();
        let ss = hub.list_sessions(w.id).unwrap();
        assert_eq!(ss.len(), 1);
        assert_eq!(ss[0].state, SessionState::Running);
        assert_eq!(ss[0].env_id, a.env_id());
        assert_eq!(
            (ss[0].model.as_deref(), ss[0].mode),
            (Some("opus"), SessionMode::Subagents)
        );
    }

    /// O id do Remote Control gravado no ambiente chega ao hub pelo sync
    /// (#197); uma fotografia sem o campo (ambiente anterior) não o apaga.
    #[test]
    fn rc_session_id_chega_ao_hub_e_fica() {
        let hub = hub_com("deep-ion-ai", "demo");
        let a = ledger("/a");
        let wi = a.ensure_work_item(1, 3, WorkItemKind::Issue).unwrap();
        let s = a
            .create_session(&NewSession {
                work_item_id: wi.id,
                tool: Tool::Claude,
                cwd: "/w".into(),
                launch_cmd: vec![],
                model: None,
                mode: SessionMode::Native,
            })
            .unwrap();
        assert!(a.set_session_rc_id(&s.id, "session_01Abc").unwrap());
        assert!(!a.set_session_rc_id(&s.id, "session_01Abc").unwrap());
        push(&hub, &a, "a");
        let w = hub
            .find_work_item("deep-ion-ai", "demo", 3)
            .unwrap()
            .unwrap();
        let rc = |hub: &Hub| hub.list_sessions(w.id).unwrap()[0].rc_session_id.clone();
        assert_eq!(rc(&hub).as_deref(), Some("session_01Abc"));

        a.transition_session(&s.id, SessionState::Running, None)
            .unwrap();
        let mut sem_campo = a.outbox_pending(100).unwrap();
        for e in &mut sem_campo {
            if let SyncChange::Session(c) = &mut e.change {
                c.rc_session_id = None;
            }
        }
        hub.ingest(a.env_id(), "a", &sem_campo).unwrap();
        let ss = hub.list_sessions(w.id).unwrap();
        assert_eq!(ss[0].state, SessionState::Running);
        assert_eq!(ss[0].rc_session_id.as_deref(), Some("session_01Abc"));
    }

    /// Cópias sem HLC (ambiente anterior à #293) seguem o `last_event_at`.
    #[test]
    fn sessao_sem_hlc_segue_o_last_event_at() {
        let hub = hub_com("deep-ion-ai", "demo");
        let a = ledger("/a");
        let wi = a.ensure_work_item(1, 3, WorkItemKind::Issue).unwrap();
        let s = a
            .create_session(&NewSession {
                work_item_id: wi.id,
                tool: Tool::Claude,
                cwd: "/w".into(),
                launch_cmd: vec![],
                model: None,
                mode: SessionMode::Native,
            })
            .unwrap();
        let mut velha = a.outbox_pending(100).unwrap();
        a.transition_session(&s.id, SessionState::Running, None)
            .unwrap();
        let mut nova = a.outbox_pending(100).unwrap();
        for (lote, at) in [(&mut velha, 10), (&mut nova, 20)] {
            for e in lote.iter_mut() {
                if let SyncChange::Session(c) = &mut e.change {
                    (c.hlc, c.last_event_at) = (0, at);
                }
            }
        }
        hub.ingest(a.env_id(), "a", &nova).unwrap();
        hub.ingest(a.env_id(), "a", &velha).unwrap();
        let w = hub
            .find_work_item("deep-ion-ai", "demo", 3)
            .unwrap()
            .unwrap();
        let ss = hub.list_sessions(w.id).unwrap();
        assert_eq!(ss[0].state, SessionState::Running);
    }

    #[test]
    fn push_de_projeto_desconhecido_vai_para_a_quarentena_com_ack() {
        let hub = Hub::open_in_memory().unwrap();
        let a = ledger("/a");
        let wi = a.ensure_work_item(1, 3, WorkItemKind::Epic).unwrap();
        let d = a
            .record_decision(&NewDecision {
                work_item_id: wi.id,
                session_id: None,
                origin: DecisionOrigin::Autonoma,
                title: "t".into(),
                context: "c".into(),
                decision: "d".into(),
                consequences: None,
            })
            .unwrap();
        a.mark_reviewed(&[d.id], Some(10)).unwrap();
        let batch = a.outbox_pending(1000).unwrap();
        let r = hub.ingest(a.env_id(), "a", &batch).unwrap();
        // Ack do lote inteiro, sem criar projeto.
        assert_eq!(r.last_seq, batch.last().unwrap().seq);
        assert_eq!(r.applied, 0);
        assert_eq!(r.quarantined, batch.len());
        assert_eq!(r.quarantined_projects, vec!["deep-ion-ai/demo"]);
        assert_eq!(contar(&hub, "project"), 0);
        assert_eq!(contar(&hub, "work_item"), 0);
        assert_eq!(contar(&hub, "event"), 0);
        assert_eq!(contar(&hub, "push_quarantine"), batch.len() as i64);
        // Reenviar o mesmo lote (ack perdido) não duplica.
        hub.ingest(a.env_id(), "a", &batch).unwrap();
        assert_eq!(contar(&hub, "push_quarantine"), batch.len() as i64);
        let q = hub.quarantine_summary().unwrap();
        assert_eq!(q.len(), 1);
        assert_eq!(
            (q[0].project_key.as_str(), q[0].env_name.as_str(), q[0].rows),
            ("deep-ion-ai/demo", "a", batch.len() as i64)
        );
        assert_eq!(hub.config_rev().unwrap(), 0);
        assert!(hub.list_envs().unwrap()[0].last_seq == r.last_seq);
    }

    #[test]
    fn cadastro_reaplica_a_quarentena_na_ordem() {
        let hub = Hub::open_in_memory().unwrap();
        let a = ledger("/a");
        let b = ledger("/b");
        let wa = a.ensure_work_item(1, 7, WorkItemKind::Issue).unwrap();
        let d = a
            .record_decision(&NewDecision {
                work_item_id: wa.id,
                session_id: None,
                origin: DecisionOrigin::Autonoma,
                title: "t".into(),
                context: "c".into(),
                decision: "d".into(),
                consequences: None,
            })
            .unwrap();
        a.set_work_item_status(wa.id, WorkItemStatus::Pausado, "manual")
            .unwrap();
        // A revisão chega num lote separado: segue a decisão em quarentena.
        push(&hub, &a, "a");
        a.mark_reviewed(&[d.id], Some(10)).unwrap();
        push(&hub, &a, "a");
        let wb = b.ensure_work_item(1, 7, WorkItemKind::Issue).unwrap();
        b.set_work_item_branch(wb.id, "feat/7", "/b/wt").unwrap();
        push(&hub, &b, "b");
        // Outro projeto desconhecido fica onde está.
        let c = Ledger::open_in_memory().unwrap();
        c.add_project(&NewProject {
            name: "outro".into(),
            gh_owner: "x".into(),
            gh_repo: "y".into(),
            repo_path: "/y".into(),
            base_branch: "develop".into(),
            protected_branches: vec![],
        })
        .unwrap();
        push(&hub, &c, "c");
        assert_eq!(contar(&hub, "project"), 0);

        // Caixa diferente: mesma chave.
        let added = hub.add_project(&novo("Deep-Ion-AI", "Demo")).unwrap();
        assert!(added.reapplied > 0);
        assert!(!added.restored);
        assert_eq!(added.rev, 1);
        assert_eq!(added.project.gh_owner, "Deep-Ion-AI");
        assert_eq!(hub.quarantine_count("deep-ion-ai", "demo").unwrap(), 0);
        assert_eq!(hub.quarantine_count("x", "y").unwrap(), 1);
        let w = hub
            .find_work_item("deep-ion-ai", "demo", 7)
            .unwrap()
            .unwrap();
        assert_eq!(w.status, WorkItemStatus::Pausado);
        assert_eq!(w.branch.as_deref(), Some("feat/7"));
        let ds = hub.list_decisions(&[w.id], false).unwrap();
        assert_eq!(ds.len(), 1);
        assert_eq!(ds[0].reviewed_in_pr, Some(10));
        assert_eq!(hub.project_locations(added.project.id).unwrap().len(), 2);
        // Os atributos continuam os do cadastro, não os do ambiente.
        let p = hub.find_project("deep-ion-ai", "demo").unwrap().unwrap();
        assert_eq!(p.protected_branches, vec!["develop", "main"]);
        // Depois do cadastro, o push segue a ingestão normal.
        a.ensure_work_item(1, 8, WorkItemKind::Issue).unwrap();
        let r = push(&hub, &a, "a");
        assert!(r.applied > 0 && r.quarantined == 0);
        assert!(hub
            .find_work_item("deep-ion-ai", "demo", 8)
            .unwrap()
            .is_some());
        // Cadastrar de novo o mesmo projeto é recusado.
        assert!(matches!(
            hub.add_project(&novo("deep-ion-ai", "demo")),
            Err(LedgerError::Conflict(_))
        ));
        assert_eq!(hub.config_rev().unwrap(), 1);
        // Descartar a quarentena do outro projeto.
        assert_eq!(hub.discard_quarantine("X", "Y").unwrap(), 1);
        assert!(hub.quarantine_summary().unwrap().is_empty());
    }

    #[test]
    fn project_removed_do_ambiente_e_ignorado() {
        let hub = hub_com("deep-ion-ai", "demo");
        let a = ledger("/a");
        push(&hub, &a, "a");
        // Só ambientes anteriores à #178 emitem `project_removed` (o
        // ambiente atual não exclui projetos); o hub segue tolerante.
        a.enqueue(&SyncChange::ProjectRemoved {
            owner: "deep-ion-ai".into(),
            repo: "demo".into(),
            hlc: a.tick_hlc().unwrap(),
        })
        .unwrap();
        let r = push(&hub, &a, "a");
        assert_eq!((r.ignored, r.applied), (1, 0));
        // Só o hub remove: o projeto e o relato do ambiente ficam.
        let p = hub.find_project("deep-ion-ai", "demo").unwrap().unwrap();
        assert_eq!(hub.project_locations(p.id).unwrap().len(), 1);
    }

    #[test]
    fn criar_editar_excluir_e_config_rev() {
        let hub = Hub::open_in_memory().unwrap();
        assert_eq!(hub.config_rev().unwrap(), 0);
        let added = hub.add_project(&novo("deep-ion-ai", "demo")).unwrap();
        assert_eq!((added.rev, hub.config_rev().unwrap()), (1, 1));
        assert_eq!(hub.project_rev("deep-ion-ai", "demo").unwrap(), Some(1));
        let p = hub
            .update_project(
                "Deep-Ion-AI",
                "demo",
                &HubProjectPatch {
                    name: Some("Demo".into()),
                    base_branch: Some("main".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!((p.name.as_str(), p.base_branch.as_str()), ("Demo", "main"));
        assert_eq!(p.protected_branches, vec!["develop", "main"]);
        let p = hub
            .update_project(
                "deep-ion-ai",
                "demo",
                &HubProjectPatch {
                    protected_branches: Some(vec!["main".into()]),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(p.protected_branches, vec!["main"]);
        assert_eq!(p.name, "Demo");
        assert_eq!(hub.config_rev().unwrap(), 3);
        assert_eq!(hub.project_rev("deep-ion-ai", "demo").unwrap(), Some(3));
        assert!(matches!(
            hub.update_project("x", "y", &HubProjectPatch::default()),
            Err(LedgerError::NotFound(..))
        ));
        // Falha não gasta revisão.
        assert_eq!(hub.config_rev().unwrap(), 3);
        let rev = hub.delete_project("deep-ion-ai", "demo").unwrap();
        assert_eq!((rev, hub.config_rev().unwrap()), (4, 4));
        assert!(hub.find_project("deep-ion-ai", "demo").unwrap().is_none());
        assert!(hub.list_projects().unwrap().is_empty());
        assert_eq!(hub.project_rev("deep-ion-ai", "demo").unwrap(), Some(4));
        assert!(hub
            .project_tombstone("deep-ion-ai", "demo")
            .unwrap()
            .is_some());
        assert!(matches!(
            hub.update_project(
                "deep-ion-ai",
                "demo",
                &HubProjectPatch {
                    name: Some("z".into()),
                    ..Default::default()
                }
            ),
            Err(LedgerError::NotFound(..))
        ));
        assert!(matches!(
            hub.delete_project("deep-ion-ai", "demo"),
            Err(LedgerError::NotFound(..))
        ));
        assert_eq!(hub.config_rev().unwrap(), 4);
    }

    #[test]
    fn exclusao_apaga_historico_e_deixa_lapide() {
        use crate::{GithubSnapshot, PrRow};
        use ct_core::PrState;
        let hub = hub_com("deep-ion-ai", "demo");
        // Outro projeto, que não pode ser atingido.
        hub.add_project(&novo("o", "r")).unwrap();
        let a = ledger("/a");
        let wi = a.ensure_work_item(1, 7, WorkItemKind::Issue).unwrap();
        a.set_work_item_branch(wi.id, "feat/7", "/a/wt").unwrap();
        let s = a
            .create_session(&NewSession {
                work_item_id: wi.id,
                tool: Tool::Claude,
                cwd: "/w".into(),
                launch_cmd: vec![],
                model: None,
                mode: SessionMode::Native,
            })
            .unwrap();
        a.transition_session(&s.id, SessionState::Running, None)
            .unwrap();
        a.record_decision(&NewDecision {
            work_item_id: wi.id,
            session_id: None,
            origin: DecisionOrigin::Autonoma,
            title: "t".into(),
            context: "c".into(),
            decision: "d".into(),
            consequences: None,
        })
        .unwrap();
        push(&hub, &a, "a");
        let outro = Ledger::open_in_memory().unwrap();
        let po = outro
            .add_project(&NewProject {
                name: "r".into(),
                gh_owner: "o".into(),
                gh_repo: "r".into(),
                repo_path: "/r".into(),
                base_branch: "develop".into(),
                protected_branches: vec![],
            })
            .unwrap();
        outro
            .ensure_work_item(po.id, 1, WorkItemKind::Issue)
            .unwrap();
        push(&hub, &outro, "outro");
        let p = hub.find_project("deep-ion-ai", "demo").unwrap().unwrap();
        let w = hub
            .find_work_item("deep-ion-ai", "demo", 7)
            .unwrap()
            .unwrap();
        hub.apply_github(
            &GithubSnapshot {
                project_id: p.id,
                issues: vec![issue_row(7, None, false)],
                prs: vec![PrRow {
                    number: 30,
                    work_item_id: w.id,
                    head: "feat/7".into(),
                    base: "develop".into(),
                    state: PrState::Open,
                    url: "u".into(),
                    updated_at: 1,
                    head_sha: None,
                }],
                etags: vec![("/repos/deep-ion-ai/demo/issues?x".into(), "e".into())],
                synced_at: 5,
            },
            "hub",
        )
        .unwrap();
        hub.add_assignment("deep-ion-ai", "demo", "a", None)
            .unwrap();

        let imp = hub.delete_impact("deep-ion-ai", "demo").unwrap().unwrap();
        assert_eq!(imp.assignments.len(), 1);
        assert_eq!(imp.assignments[0].env_name, "a");
        assert_eq!(imp.live_sessions.len(), 1);
        assert_eq!(imp.live_sessions[0].id, s.id);
        assert!(hub.delete_impact("x", "y").unwrap().is_none());

        let d = tempfile::tempdir().unwrap();
        let snap = hub
            .snapshot_pre_delete(d.path(), 1_792_158_300_000)
            .unwrap();
        assert_eq!(snap.file_name().unwrap(), "hub-20261016-1345-pre-delete.db");
        let snap2 = hub
            .snapshot_pre_delete(d.path(), 1_792_158_300_000)
            .unwrap();
        assert_eq!(
            snap2.file_name().unwrap(),
            "hub-20261016-1345-pre-delete-2.db"
        );
        // Fora da rotação.
        assert!(list_snapshots(d.path()).unwrap().is_empty());

        hub.delete_project("deep-ion-ai", "demo").unwrap();
        assert_eq!(contar(&hub, "purging"), 0);
        // Só o projeto excluído perdeu o histórico.
        let left = |t: &str| -> i64 {
            hub.conn
                .query_row(
                    &format!("SELECT count(*) FROM {t} WHERE project_id = ?1"),
                    [p.id],
                    |r| r.get(0),
                )
                .unwrap()
        };
        for t in [
            "event",
            "work_item",
            "issue_cache",
            "pr_cache",
            "sync_state",
        ] {
            assert_eq!(left(t), 0, "{t}");
        }
        assert_eq!(contar(&hub, "decision"), 0);
        assert_eq!(contar(&hub, "session"), 0);
        // Resta só a do work item do outro projeto.
        assert_eq!(contar(&hub, "work_item_location"), 1);
        assert_eq!(contar(&hub, "gh_etag"), 0);
        assert!(hub.project_locations(p.id).unwrap().is_empty());
        assert!(hub.find_work_item("o", "r", 1).unwrap().is_some());
        // A lápide fica.
        assert_eq!(contar(&hub, "project"), 2);
        // O snapshot tem o histórico de antes.
        let copia = Hub::open(&snap).unwrap();
        assert!(copia
            .find_work_item("deep-ion-ai", "demo", 7)
            .unwrap()
            .is_some());
        // Fora da purga, o histórico segue append-only.
        assert!(hub.conn.execute("DELETE FROM event", []).is_err());
    }

    #[test]
    fn push_de_projeto_com_lapide_e_descartado_e_recadastro_limpa() {
        let hub = hub_com("deep-ion-ai", "demo");
        let a = ledger("/a");
        let wi = a.ensure_work_item(1, 7, WorkItemKind::Issue).unwrap();
        push(&hub, &a, "a");
        let old_id = hub.find_project("deep-ion-ai", "demo").unwrap().unwrap().id;
        hub.delete_project("deep-ion-ai", "demo").unwrap();
        // Linhas atrasadas do projeto excluído: ack, descartadas, sem quarentena.
        a.set_work_item_status(wi.id, WorkItemStatus::Pausado, "manual")
            .unwrap();
        let batch = a.outbox_pending(1000).unwrap();
        let r = hub.ingest(a.env_id(), "a", &batch).unwrap();
        a.outbox_ack(r.last_seq).unwrap();
        assert_eq!(r.last_seq, batch.last().unwrap().seq);
        assert_eq!((r.applied, r.quarantined), (0, 0));
        assert_eq!(r.discarded, batch.len());
        assert_eq!(r.discarded_projects, vec!["deep-ion-ai/demo"]);
        assert_eq!(contar(&hub, "push_quarantine"), 0);
        assert_eq!(contar(&hub, "work_item"), 0);
        assert!(hub.find_project("deep-ion-ai", "demo").unwrap().is_none());

        // Recadastrar reaproveita a linha e limpa a lápide.
        let rev_antes = hub.config_rev().unwrap();
        let added = hub
            .add_project(&NewHubProject {
                name: "Demo 2".into(),
                ..novo("deep-ion-ai", "demo")
            })
            .unwrap();
        assert!(added.restored);
        assert_eq!(added.project.id, old_id);
        assert_eq!(added.project.name, "Demo 2");
        assert_eq!(added.rev, rev_antes + 1);
        assert_eq!(added.reapplied, 0);
        assert!(hub
            .project_tombstone("deep-ion-ai", "demo")
            .unwrap()
            .is_none());
        assert_eq!(contar(&hub, "project"), 1);
        // E o push volta a ser ingerido.
        a.set_work_item_status(wi.id, WorkItemStatus::EmExecucao, "manual")
            .unwrap();
        let r = push(&hub, &a, "a");
        assert!(r.applied > 0 && r.discarded == 0);
        let w = hub
            .find_work_item("deep-ion-ai", "demo", 7)
            .unwrap()
            .unwrap();
        assert_eq!(w.status, WorkItemStatus::EmExecucao);
    }

    #[test]
    fn token_de_sync_guarda_so_o_hash_e_nasce_preso_ao_env() {
        let hub = hub_com("deep-ion-ai", "demo");
        let i = hub.issue_env_token("notebook").unwrap();
        let t = i.token.clone();
        assert_eq!(t.len(), 64);
        let guardado: String = hub
            .conn
            .query_row("SELECT token_hash FROM env_token", [], |r| r.get(0))
            .unwrap();
        assert_ne!(guardado, t);
        let e = hub.authenticate(&t).unwrap().unwrap();
        assert_eq!(e.name, "notebook");
        // Nome novo: o hub gera o env_id e o devolve junto com o token (#195).
        assert_eq!(e.env_id.as_deref(), Some(i.env_id.as_str()));
        assert!(hub.authenticate("outro").unwrap().is_none());
        assert!(hub.authenticate("").unwrap().is_none());

        // Só o env_id devolvido vale; outro é recusado, também depois de girar.
        hub.bind_env_token("notebook", &i.env_id).unwrap();
        assert!(matches!(
            hub.bind_env_token("notebook", "env-2"),
            Err(LedgerError::Conflict(_))
        ));
        let m = hub.issue_env_token("mac").unwrap();
        assert_ne!(m.env_id, i.env_id);
        assert!(matches!(
            hub.bind_env_token("mac", &i.env_id),
            Err(LedgerError::Conflict(_))
        ));

        // Girar o token mantém o env_id do ambiente: não dá para prendê-lo a outro.
        let i2 = hub.issue_env_token("notebook").unwrap();
        assert_ne!(i2.token, t);
        assert_eq!(i2.env_id, i.env_id);
        assert!(hub.authenticate(&t).unwrap().is_none());
        assert_eq!(
            hub.authenticate(&i2.token)
                .unwrap()
                .unwrap()
                .env_id
                .as_deref(),
            Some(i.env_id.as_str())
        );
        assert!(matches!(
            hub.bind_env_token("notebook", &m.env_id),
            Err(LedgerError::Conflict(_))
        ));
        hub.bind_env_token("notebook", &i.env_id).unwrap();

        // Revogar invalida o token; o nome volta a existir com env_id novo.
        assert!(hub.revoke_env_token("mac").unwrap());
        assert!(hub.authenticate(&m.token).unwrap().is_none());
        assert_eq!(hub.env_token_names().unwrap(), vec!["notebook"]);
    }

    #[test]
    fn token_com_env_id_do_ambiente_existente_e_sem_trocar_o_dono() {
        let hub = hub_com("deep-ion-ai", "demo");
        let i = hub
            .issue_env_token_with_env_id("mac", "env-do-ledger")
            .unwrap();
        assert_eq!(i.env_id, "env-do-ledger");
        // Girar com o mesmo id vale; com outro, recusa e nada muda.
        let j = hub
            .issue_env_token_with_env_id("mac", "env-do-ledger")
            .unwrap();
        assert!(matches!(
            hub.issue_env_token_with_env_id("mac", "outro"),
            Err(LedgerError::Conflict(_))
        ));
        assert_eq!(
            hub.authenticate(&j.token)
                .unwrap()
                .unwrap()
                .env_id
                .as_deref(),
            Some("env-do-ledger")
        );
        // O env_id de outro ambiente: recusa.
        assert!(hub
            .issue_env_token_with_env_id("nb", "env-do-ledger")
            .is_err());
    }

    /// Token de instalação antiga (sem `env_id`) ainda se prende no primeiro
    /// uso; ao girar, ganha um `env_id` e passa a nascer preso.
    #[test]
    fn token_antigo_sem_env_id_prende_no_primeiro_uso_e_ao_girar_nasce_preso() {
        let hub = hub_com("deep-ion-ai", "demo");
        hub.conn
            .execute(
                "INSERT INTO env_token (name, token_hash, created_at) VALUES ('velho', 'h', 1)",
                [],
            )
            .unwrap();
        hub.bind_env_token("velho", "env-1").unwrap();
        assert!(hub.bind_env_token("velho", "env-2").is_err());
        hub.conn
            .execute("UPDATE env_token SET env_id = NULL", [])
            .unwrap();
        let i = hub.issue_env_token("velho").unwrap();
        assert_ne!(i.env_id, "env-1");
        assert_eq!(
            hub.authenticate(&i.token)
                .unwrap()
                .unwrap()
                .env_id
                .as_deref(),
            Some(i.env_id.as_str())
        );
    }

    #[test]
    fn mesmo_pr_mesclado_visto_por_dois_ambientes_vira_um_evento() {
        use crate::{GithubSnapshot, PrRow};
        use ct_core::PrState;
        let hub = hub_com("deep-ion-ai", "demo");
        let a = ledger("/a");
        let b = ledger("/b");
        for l in [&a, &b] {
            let wi = l.ensure_work_item(1, 7, WorkItemKind::Issue).unwrap();
            l.apply_github(&GithubSnapshot {
                project_id: 1,
                prs: vec![PrRow {
                    number: 30,
                    work_item_id: wi.id,
                    head: "feat/7".into(),
                    base: "develop".into(),
                    state: PrState::Merged,
                    url: "u".into(),
                    updated_at: 1,
                    head_sha: None,
                }],
                synced_at: 1,
                ..Default::default()
            })
            .unwrap();
        }
        push(&hub, &a, "a");
        push(&hub, &b, "b");
        let w = hub
            .find_work_item("deep-ion-ai", "demo", 7)
            .unwrap()
            .unwrap();
        let merged: Vec<_> = hub
            .list_events(w.id, 100)
            .unwrap()
            .into_iter()
            .filter(|e| e.kind == "status_changed")
            .collect();
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].uid, "gh:pr:deep-ion-ai/demo#30:merged");
        assert_eq!(w.status, WorkItemStatus::Concluido);
    }

    /// Épico 10 (branch `wip/10-e`, vindo do ambiente `a`) com as
    /// sub-issues 11, 12 e 14 no espelho do hub; a 13 é de fora.
    fn hub_com_epico() -> (Hub, Ledger, i64, i64, Vec<IssueRow>) {
        let hub = hub_com("deep-ion-ai", "demo");
        let a = ledger("/a");
        let e = a.ensure_work_item(1, 10, WorkItemKind::Epic).unwrap();
        a.set_work_item_branch(e.id, "wip/10-e", "/a/wt").unwrap();
        push(&hub, &a, "a");
        let p = hub.find_project("deep-ion-ai", "demo").unwrap().unwrap().id;
        let epic = hub
            .find_work_item("deep-ion-ai", "demo", 10)
            .unwrap()
            .unwrap()
            .id;
        let mut issues = vec![issue_row(10, None, true)];
        for n in [11, 12, 13, 14] {
            issues.push(issue_row(n, (n != 13).then_some(10), false));
        }
        (hub, a, p, epic, issues)
    }

    fn pr_para(n: i64, wi: i64, head: &str, base: &str, state: ct_core::PrState) -> PrRow {
        PrRow {
            number: n,
            work_item_id: wi,
            head: head.into(),
            base: base.into(),
            state,
            url: "u".into(),
            updated_at: 1,
            head_sha: None,
        }
    }

    fn hub_status(hub: &Hub, n: i64) -> Option<WorkItemStatus> {
        hub.find_work_item("deep-ion-ai", "demo", n)
            .unwrap()
            .map(|w| w.status)
    }

    /// #454 no hub: o PR da sub-issue para a branch do épico cria o work
    /// item; aberto ou fechado sem merge não muda o status, mesclado leva a
    /// aguardando o épico; o merge do épico conclui só quem o aguarda.
    #[test]
    fn hub_sub_issue_aguarda_e_conclui_com_o_epico() {
        use crate::GithubSnapshot;
        use ct_core::PrState::*;
        use WorkItemStatus::*;
        let (hub, _a, p, epic, issues) = hub_com_epico();
        let snap = |issues: Vec<IssueRow>, prs| GithubSnapshot {
            project_id: p,
            issues,
            prs,
            synced_at: 5,
            ..Default::default()
        };
        hub.apply_github(&snap(issues, vec![]), "hub").unwrap();
        hub.set_status_manual("deep-ion-ai", "demo", 12, EmExecucao, None, "hub")
            .unwrap();
        let r = hub
            .apply_github(
                &snap(
                    vec![],
                    vec![
                        pr_para(50, epic, "feat/11-a", "wip/10-e", Open),
                        pr_para(51, epic, "feat/12-b", "wip/10-e", Open),
                        pr_para(52, epic, "fix/teste-instavel", "wip/10-e", Merged),
                        pr_para(53, epic, "feat/13-c", "wip/10-e", Merged),
                        pr_para(55, epic, "feat/14-d", "wip/10-e", Closed),
                    ],
                ),
                "hub",
            )
            .unwrap();
        assert!(r.status_changes.is_empty(), "{:?}", r.status_changes);
        assert_eq!(r.prs, 3);
        let w11 = hub
            .find_work_item("deep-ion-ai", "demo", 11)
            .unwrap()
            .unwrap();
        assert_eq!(
            (w11.kind, w11.status, w11.branch.as_deref()),
            (WorkItemKind::Issue, Backlog, Some("feat/11-a"))
        );
        assert_eq!(hub.prs_for_work_item(w11.id).unwrap()[0].number, 50);
        assert_eq!(hub_status(&hub, 12), Some(EmExecucao));
        assert_eq!(hub_status(&hub, 13), None);
        assert_eq!(hub_status(&hub, 14), Some(Backlog));
        assert_eq!(hub_status(&hub, 10), Some(Backlog));

        let r = hub
            .apply_github(
                &snap(
                    vec![],
                    vec![
                        pr_para(50, epic, "feat/11-a", "wip/10-e", Merged),
                        pr_para(51, epic, "feat/12-b", "wip/10-e", Merged),
                    ],
                ),
                "hub",
            )
            .unwrap();
        assert_eq!(r.status_changes.len(), 2);
        assert_eq!(hub_status(&hub, 11), Some(AguardandoEpico));
        assert_eq!(hub_status(&hub, 12), Some(AguardandoEpico));
        assert_eq!(hub_status(&hub, 10), Some(Backlog));

        let epico_mesclado = || {
            snap(
                vec![],
                vec![pr_para(60, epic, "wip/10-e", "develop", Merged)],
            )
        };
        let r = hub.apply_github(&epico_mesclado(), "hub").unwrap();
        assert_eq!(r.status_changes.len(), 3);
        for n in [10, 11, 12] {
            assert_eq!(hub_status(&hub, n), Some(Concluido), "{n}");
        }
        assert_eq!(hub_status(&hub, 14), Some(Backlog));
        let ev = &hub.list_events(w11.id, 1).unwrap()[0];
        assert_eq!(ev.uid, "gh:pr:deep-ion-ai/demo#60:merged:sub:11");
        assert_eq!(ev.payload["cause"], "epic_merged");
        // Reaplicar não repete.
        assert!(hub
            .apply_github(&epico_mesclado(), "hub")
            .unwrap()
            .status_changes
            .is_empty());
        assert_eq!(hub.list_events(w11.id, 100).unwrap().len(), 2);
    }

    /// O ambiente percebe os mesmos merges depois do hub: os `uid`
    /// determinísticos deixam um evento só por transição.
    #[test]
    fn hub_e_ambiente_nao_duplicam_a_cascata_do_epico() {
        use crate::GithubSnapshot;
        use ct_core::PrState::*;
        let (hub, a, p, epic, issues) = hub_com_epico();
        let rodadas = |epic: i64| {
            [
                vec![pr_para(50, epic, "feat/11-a", "wip/10-e", Merged)],
                vec![pr_para(60, epic, "wip/10-e", "develop", Merged)],
            ]
        };
        for prs in rodadas(epic) {
            hub.apply_github(
                &GithubSnapshot {
                    project_id: p,
                    issues: issues.clone(),
                    prs,
                    synced_at: 5,
                    ..Default::default()
                },
                "hub",
            )
            .unwrap();
        }
        let local_epic = a.find_work_item(1, 10).unwrap().unwrap().id;
        for prs in rodadas(local_epic) {
            a.apply_github(&GithubSnapshot {
                project_id: 1,
                issues: issues.clone(),
                prs,
                synced_at: 6,
                ..Default::default()
            })
            .unwrap();
        }
        assert_eq!(
            a.find_work_item(1, 11).unwrap().unwrap().status,
            WorkItemStatus::Concluido
        );
        push(&hub, &a, "a");
        let w11 = hub
            .find_work_item("deep-ion-ai", "demo", 11)
            .unwrap()
            .unwrap();
        assert_eq!(w11.status, WorkItemStatus::Concluido);
        let status: Vec<String> = hub
            .list_events(w11.id, 100)
            .unwrap()
            .into_iter()
            .filter(|e| e.kind == "status_changed")
            .map(|e| e.uid)
            .collect();
        assert_eq!(
            status,
            vec![
                "gh:pr:deep-ion-ai/demo#60:merged:sub:11",
                "gh:pr:deep-ion-ai/demo#50:merged"
            ]
        );
    }

    #[test]
    fn hub_sincroniza_projeto_sem_clone_e_deduplica_com_o_ambiente() {
        use crate::{GithubSnapshot, PrRow};
        use ct_core::PrState;
        let hub = hub_com("deep-ion-ai", "demo");
        let a = ledger("/a");
        let wa = a.ensure_work_item(1, 7, WorkItemKind::Issue).unwrap();
        a.set_work_item_branch(wa.id, "feat/7", "/a/wt").unwrap();
        a.ensure_work_item(1, 8, WorkItemKind::Issue).unwrap();
        push(&hub, &a, "a");

        let p = hub.find_project("deep-ion-ai", "demo").unwrap().unwrap();
        let ws = hub.github_work_items(p.id).unwrap();
        assert_eq!(ws.iter().map(|w| w.gh_number).collect::<Vec<_>>(), vec![7]);
        let w7 = ws[0].id;
        let w8 = hub
            .find_work_item("deep-ion-ai", "demo", 8)
            .unwrap()
            .unwrap()
            .id;
        let pr = |state| PrRow {
            number: 30,
            work_item_id: w7,
            head: "feat/7".into(),
            base: "develop".into(),
            state,
            url: "u".into(),
            updated_at: 1,
            head_sha: None,
        };
        let issue = |state: &str| IssueRow {
            gh_number: 8,
            title: "oito".into(),
            state: state.into(),
            labels: vec![],
            parent_number: None,
            has_sub_issues: false,
            url: "u".into(),
            updated_at: 1,
            sprints: Vec::new(),
        };
        let snap = |prs, issues| GithubSnapshot {
            project_id: p.id,
            issues,
            prs,
            etags: vec![("k".into(), "e".into())],
            synced_at: 5,
        };
        let r = hub
            .apply_github(&snap(vec![pr(PrState::Open)], vec![issue("open")]), "hub")
            .unwrap();
        assert_eq!(r.status_changes, vec![(w7, WorkItemStatus::EmRevisao)]);
        assert_eq!(hub.etag("k").unwrap().as_deref(), Some("e"));
        assert_eq!(hub.list_issues(p.id).unwrap().len(), 1);
        assert_eq!(hub.sync_state(p.id).unwrap().unwrap().last_ok_at, Some(5));
        let r = hub
            .apply_github(
                &snap(vec![pr(PrState::Merged)], vec![issue("closed")]),
                "hub",
            )
            .unwrap();
        assert_eq!(r.status_changes, vec![(w7, WorkItemStatus::Concluido)]);
        assert_eq!(r.issues_closed, vec![8]);
        assert_eq!(
            hub.find_work_item("deep-ion-ai", "demo", 7)
                .unwrap()
                .unwrap()
                .status,
            WorkItemStatus::Concluido
        );

        // O ambiente percebe o mesmo merge depois: o hub guarda um só evento.
        a.apply_github(&GithubSnapshot {
            project_id: 1,
            prs: vec![PrRow {
                work_item_id: wa.id,
                ..pr(PrState::Merged)
            }],
            synced_at: 6,
            ..Default::default()
        })
        .unwrap();
        push(&hub, &a, "a");
        let merged = hub
            .list_events(w7, 100)
            .unwrap()
            .into_iter()
            .filter(|e| e.uid == "gh:pr:deep-ion-ai/demo#30:merged")
            .count();
        assert_eq!(merged, 1);
        assert_eq!(
            hub.list_events(w8, 100)
                .unwrap()
                .iter()
                .filter(|e| e.kind == "issue_closed")
                .count(),
            1
        );
        hub.record_sync_error(p.id, "falhou").unwrap();
        let st = hub.sync_states().unwrap();
        assert_eq!(st[0].last_error.as_deref(), Some("falhou"));
        assert_eq!(st[0].last_ok_at, Some(5));
    }

    #[test]
    fn snapshot_consistente_e_rotacao() {
        let d = tempfile::tempdir().unwrap();
        let hub = Hub::open(&d.path().join("hub.db")).unwrap();
        hub.add_project(&novo("deep-ion-ai", "demo")).unwrap();
        let a = ledger("/a");
        a.ensure_work_item(1, 3, WorkItemKind::Issue).unwrap();
        push(&hub, &a, "a");
        let dir = d.path().join("backup");
        let min = 60_000;
        let base = 1_792_158_300_000; // 2026-10-16T13:45Z
        let p = hub.snapshot(&dir, base).unwrap();
        assert_eq!(p.file_name().unwrap(), "hub-20261016-1345.db");
        // Mesmo minuto: não regrava.
        assert_eq!(hub.snapshot(&dir, base + 1_000).unwrap(), p);
        // O snapshot abre como hub e tem os dados.
        let copia = Hub::open(&p).unwrap();
        assert!(copia
            .find_work_item("deep-ion-ai", "demo", 3)
            .unwrap()
            .is_some());
        drop(copia);

        for i in 1..5 {
            hub.snapshot(&dir, base + i * min).unwrap();
        }
        std::fs::write(dir.join("outro.db"), "x").unwrap();
        std::fs::write(dir.join("hub-2026-x.db"), "x").unwrap();
        assert_eq!(list_snapshots(&dir).unwrap().len(), 5);
        let removidos = rotate_snapshots(&dir, 3).unwrap();
        assert_eq!(removidos.len(), 2);
        let restam: Vec<_> = list_snapshots(&dir)
            .unwrap()
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert_eq!(
            restam,
            vec![
                "hub-20261016-1349.db",
                "hub-20261016-1348.db",
                "hub-20261016-1347.db"
            ]
        );
        assert!(dir.join("outro.db").exists());
        assert!(list_snapshots(&d.path().join("nada")).unwrap().is_empty());
    }

    #[test]
    fn historico_e_append_only_no_hub() {
        let hub = hub_com("deep-ion-ai", "demo");
        let a = ledger("/a");
        a.ensure_work_item(1, 3, WorkItemKind::Issue).unwrap();
        push(&hub, &a, "a");
        assert!(hub.conn.execute("UPDATE event SET type = 'x'", []).is_err());
        assert!(hub.conn.execute("DELETE FROM event", []).is_err());
    }

    // --- atribuições (#176) ---------------------------------------------

    /// Relato do ambiente para `deep-ion-ai/demo`, como a #177 vai emitir.
    fn relato(
        seq: i64,
        hlc: i64,
        state: Option<crate::LocationState>,
        error: Option<&str>,
        applied_rev: Option<i64>,
    ) -> OutboxEntry {
        OutboxEntry {
            seq,
            created_at: 0,
            change: SyncChange::Project(ProjectChange {
                owner: "deep-ion-ai".into(),
                repo: "demo".into(),
                name: "demo".into(),
                repo_path: "/a/projetos/demo".into(),
                base_branch: "develop".into(),
                protected_branches: vec![],
                created_at: 0,
                hlc,
                state,
                error: error.map(String::from),
                applied_rev,
                tracker: TrackerConfig::Github,
                forge: ForgeConfig::Github,
                branch_pattern: None,
                attribution: true,
            }),
        }
    }

    fn sessao_viva(l: &Ledger, n: i64) -> String {
        let wi = l.ensure_work_item(1, n, WorkItemKind::Issue).unwrap();
        let s = l
            .create_session(&NewSession {
                work_item_id: wi.id,
                tool: Tool::Claude,
                cwd: "/a/wt".into(),
                launch_cmd: vec![],
                model: None,
                mode: ct_core::SessionMode::Native,
            })
            .unwrap();
        l.transition_session(&s.id, SessionState::Running, None)
            .unwrap();
        s.id
    }

    #[test]
    fn atribuir_remover_reatribuir_e_config_rev() {
        let hub = hub_com("deep-ion-ai", "demo");
        hub.add_project(&novo("o", "r")).unwrap();
        let a = ledger("/a/demo");
        push(&hub, &a, "a");
        let b = ledger("/b/demo");
        push(&hub, &b, "b");
        assert_eq!(hub.config_rev().unwrap(), 2);

        // Atribuir: `config_rev + 1`, gravado na linha.
        let r = hub
            .add_assignment("Deep-Ion-AI", "Demo", "a", None)
            .unwrap();
        assert!(r.changed && !r.reassigned);
        assert_eq!((r.assignment.rev, hub.config_rev().unwrap()), (3, 3));
        assert_eq!(r.assignment.env_id, a.env_id());
        assert_eq!(r.assignment.env_name, "a");
        assert_eq!(r.assignment.repo_path, "");
        // O relato de hoje (sem `applied_rev`) não aplica a atribuição.
        assert_eq!(r.assignment.state, AssignmentState::Pending);
        assert_eq!(r.assignment.reported_path.as_deref(), Some("/a/demo"));
        // A mesma atribuição, igual: nada muda.
        let r = hub
            .add_assignment("deep-ion-ai", "demo", "a", Some(" "))
            .unwrap();
        assert!(!r.changed);
        assert_eq!(hub.config_rev().unwrap(), 3);
        // Mudar o caminho: `rev` novo.
        let r = hub
            .add_assignment("deep-ion-ai", "demo", "a", Some("~/src/demo"))
            .unwrap();
        assert!(r.changed);
        assert_eq!(
            (r.assignment.rev, r.assignment.repo_path.as_str()),
            (4, "~/src/demo")
        );
        // Por `env_id` também; e outro projeto no mesmo ambiente.
        hub.add_assignment("deep-ion-ai", "demo", b.env_id(), Some("/b/demo"))
            .unwrap();
        hub.add_assignment("o", "r", "a", None).unwrap();
        assert_eq!(hub.config_rev().unwrap(), 6);

        // Recusas: não mudam o `config_rev`.
        for (o, r, env, path) in [
            ("deep-ion-ai", "demo", "a", Some("relativo/demo")),
            ("deep-ion-ai", "demo", "sumiu", None),
            ("x", "y", "a", None),
        ] {
            assert!(
                hub.add_assignment(o, r, env, path).is_err(),
                "{o}/{r} {env}"
            );
        }
        assert_eq!(hub.config_rev().unwrap(), 6);

        // Filtros.
        let all = hub.list_assignments(&AssignmentFilter::default()).unwrap();
        assert_eq!(all.len(), 3);
        let f = |env: Option<&str>, owner: Option<&str>, repo: Option<&str>| {
            hub.list_assignments(&AssignmentFilter {
                env: env.map(String::from),
                owner: owner.map(String::from),
                repo: repo.map(String::from),
            })
            .unwrap()
            .len()
        };
        assert_eq!(f(Some("a"), None, None), 2);
        assert_eq!(f(Some("b"), Some("DEEP-ION-AI"), None), 1);
        assert_eq!(f(None, None, Some("demo")), 2);
        assert_eq!(f(Some("b"), Some("o"), Some("r")), 0);
        let v = hub.project_views().unwrap();
        let demo = v.iter().find(|p| p.project.gh_repo == "demo").unwrap();
        let mut envs: Vec<_> = demo
            .assignments
            .iter()
            .map(|a| a.env_name.clone())
            .collect();
        envs.sort();
        assert_eq!(envs, vec!["a", "b"]);

        // Remover: `removed_at` e `rev` novo; a linha fica.
        let rev = hub.remove_assignment("deep-ion-ai", "demo", "a").unwrap();
        assert_eq!((rev, hub.config_rev().unwrap()), (7, 7));
        let (row_rev, removed_at) = hub
            .assignment_row("deep-ion-ai", "demo", a.env_id())
            .unwrap()
            .unwrap();
        assert_eq!(row_rev, 7);
        assert!(removed_at.is_some());
        assert_eq!(f(Some("a"), None, Some("demo")), 0);
        assert!(hub.remove_assignment("deep-ion-ai", "demo", "a").is_err());
        assert_eq!(hub.config_rev().unwrap(), 7);

        // Reatribuir: reativa a linha com `rev` novo.
        let r = hub
            .add_assignment("deep-ion-ai", "demo", "a", None)
            .unwrap();
        assert!(r.changed && r.reassigned);
        assert_eq!((r.assignment.rev, r.assignment.repo_path.as_str()), (8, ""));
        assert_eq!(
            hub.assignment_row("deep-ion-ai", "demo", a.env_id())
                .unwrap(),
            Some((8, None))
        );
        assert_eq!(contar(&hub, "assignment"), 3);
    }

    #[test]
    fn nome_de_ambiente_ambiguo_pede_env_id() {
        let hub = hub_com("deep-ion-ai", "demo");
        let (a1, a2) = (ledger("/1"), ledger("/2"));
        push(&hub, &a1, "a");
        push(&hub, &a2, "a");
        let err = hub
            .add_assignment("deep-ion-ai", "demo", "a", None)
            .unwrap_err()
            .to_string();
        assert!(err.contains("env_id"), "{err}");
        assert!(hub
            .add_assignment("deep-ion-ai", "demo", a2.env_id(), None)
            .is_ok());
        // Ambiente com token preso mas que ainda não enviou nada.
        hub.add_bound_env_token("novo", "env-novo").unwrap();
        let r = hub
            .add_assignment("deep-ion-ai", "demo", "novo", None)
            .unwrap();
        assert_eq!(
            (r.assignment.env_id.as_str(), r.assignment.env_name.as_str()),
            ("env-novo", "novo")
        );
    }

    #[test]
    fn estado_reflete_o_relato_do_ambiente() {
        use crate::LocationState::{Error as Erro, Ok as Certo};
        let hub = hub_com("deep-ion-ai", "demo");
        let env = "env-a";
        hub.ingest(env, "a", &[]).unwrap();
        let estado = || hub.list_assignments(&AssignmentFilter::default()).unwrap()[0].clone();
        let rev = hub
            .add_assignment("deep-ion-ai", "demo", "a", None)
            .unwrap()
            .assignment
            .rev;
        // Sem relato: pendente.
        let s = estado();
        assert_eq!((s.state, s.reported_at), (AssignmentState::Pending, None));

        // Relato de erro da revisão atual: com erro, com a mensagem.
        hub.ingest(
            env,
            "a",
            &[relato(
                1,
                10,
                Some(Erro),
                Some("clone falhou: 403"),
                Some(rev),
            )],
        )
        .unwrap();
        let s = estado();
        assert_eq!(s.state, AssignmentState::Error);
        assert_eq!(s.error.as_deref(), Some("clone falhou: 403"));
        assert_eq!(s.applied_rev, Some(rev));
        assert!(s.reported_at.is_some());

        // Relato ok: aplicada, e o caminho relatado.
        hub.ingest(env, "a", &[relato(2, 11, Some(Certo), None, Some(rev))])
            .unwrap();
        let s = estado();
        assert_eq!((s.state, s.error.clone()), (AssignmentState::Applied, None));
        assert_eq!(s.reported_path.as_deref(), Some("/a/projetos/demo"));

        // Relato atrasado (HLC menor) não volta o estado.
        hub.ingest(
            env,
            "a",
            &[relato(3, 9, Some(Erro), Some("velho"), Some(rev))],
        )
        .unwrap();
        assert_eq!(estado().state, AssignmentState::Applied);

        // Mudança na atribuição: pendente até o ambiente aplicar o `rev` novo,
        // mesmo com relato de erro de uma revisão anterior.
        let rev2 = hub
            .add_assignment("deep-ion-ai", "demo", "a", Some("/outro/demo"))
            .unwrap()
            .assignment
            .rev;
        assert_eq!(estado().state, AssignmentState::Pending);
        hub.ingest(env, "a", &[relato(4, 12, Some(Erro), Some("x"), Some(rev))])
            .unwrap();
        assert_eq!(estado().state, AssignmentState::Pending);
        hub.ingest(
            env,
            "a",
            &[relato(5, 13, Some(Certo), None, Some(rev2 + 3))],
        )
        .unwrap();
        assert_eq!(estado().state, AssignmentState::Applied);
        // Fotografia sem relato (ambiente anterior à #177, ou uma que não é
        // da aplicação): o relato anterior fica, a atribuição segue aplicada.
        hub.ingest(env, "a", &[relato(6, 14, None, None, None)])
            .unwrap();
        let s = estado();
        assert_eq!(
            (s.state, s.applied_rev),
            (AssignmentState::Applied, Some(rev2 + 3))
        );
        // Um relato novo (com `applied_rev`) volta a valer por inteiro.
        hub.ingest(
            env,
            "a",
            &[relato(7, 15, Some(Erro), Some("sumiu"), Some(rev2))],
        )
        .unwrap();
        let s = estado();
        assert_eq!(
            (s.state, s.error.as_deref()),
            (AssignmentState::Error, Some("sumiu"))
        );
    }

    #[test]
    fn relato_e_compativel_com_ambientes_anteriores() {
        let e = relato(1, 1, None, None, None);
        let SyncChange::Project(p) = &e.change else {
            unreachable!()
        };
        let v = serde_json::to_value(p).unwrap();
        for k in ["state", "error", "applied_rev"] {
            assert!(v.get(k).is_none(), "{k}: {v}");
        }
        let mut legado = v.clone();
        legado.as_object_mut().unwrap().remove("hlc");
        legado["hlc"] = json!(1);
        let back: ProjectChange = serde_json::from_value(legado).unwrap();
        assert_eq!(&back, p);
        let mut novo = v;
        novo["state"] = json!("error");
        novo["applied_rev"] = json!(4);
        let back: ProjectChange = serde_json::from_value(novo).unwrap();
        assert_eq!(back.state, Some(crate::LocationState::Error));
        assert_eq!(back.applied_rev, Some(4));
    }

    #[test]
    fn excluir_e_recadastrar_zera_atribuicoes_e_localizacoes() {
        let hub = hub_com("deep-ion-ai", "demo");
        let a = ledger("/a/demo");
        let sid = sessao_viva(&a, 7);
        push(&hub, &a, "a");
        hub.add_assignment("deep-ion-ai", "demo", "a", None)
            .unwrap();
        let imp = hub.delete_impact("deep-ion-ai", "demo").unwrap().unwrap();
        assert_eq!(imp.assignments.len(), 1);
        assert_eq!(imp.live_sessions[0].id, sid);

        // Exclusão: a atribuição fica removida com o `rev` da lápide.
        let rev = hub.delete_project("deep-ion-ai", "demo").unwrap();
        let (row_rev, removed_at) = hub
            .assignment_row("deep-ion-ai", "demo", a.env_id())
            .unwrap()
            .unwrap();
        assert_eq!(row_rev, rev);
        assert!(removed_at.is_some());
        assert_eq!(contar(&hub, "project_location"), 0);
        assert!(hub
            .add_assignment("deep-ion-ai", "demo", "a", None)
            .is_err());

        // Recadastro: nenhuma atribuição ativa, nenhuma localização removida
        // sobrando, nada a confirmar.
        let added = hub.add_project(&novo("deep-ion-ai", "demo")).unwrap();
        assert!(added.restored);
        let pid = added.project.id;
        assert!(hub.project_assignments(pid).unwrap().is_empty());
        assert_eq!(contar(&hub, "project_location"), 0);
        let imp = hub.delete_impact("deep-ion-ai", "demo").unwrap().unwrap();
        assert!(imp.assignments.is_empty() && imp.live_sessions.is_empty());

        // O ambiente volta a relatar (fotografia do projeto e a sessão viva):
        // a localização aparece e a exclusão volta a pedir confirmação, mesmo
        // sem atribuição.
        a.update_project(
            1,
            &ProjectPatch {
                name: Some("demo!".into()),
                ..Default::default()
            },
        )
        .unwrap();
        a.transition_session(&sid, SessionState::Idle, None)
            .unwrap();
        push(&hub, &a, "a");
        assert_eq!(hub.project_locations(pid).unwrap().len(), 1);
        let v = hub.project_views().unwrap();
        assert_eq!(v[0].locations[0].env_name, "a");
        assert!(v[0].assignments.is_empty());
        let imp = hub.delete_impact("deep-ion-ai", "demo").unwrap().unwrap();
        assert!(imp.assignments.is_empty());
        assert_eq!(imp.live_sessions.len(), 1);

        // Reatribuir depois do recadastro: `rev` novo, pendente.
        let r = hub
            .add_assignment("deep-ion-ai", "demo", "a", None)
            .unwrap();
        assert!(r.reassigned);
        assert!(r.assignment.rev > rev);
        assert_eq!(r.assignment.state, AssignmentState::Pending);
        assert_eq!(
            hub.delete_impact("deep-ion-ai", "demo")
                .unwrap()
                .unwrap()
                .assignments
                .len(),
            1
        );
    }

    // --- pull da configuração (#177) ------------------------------------

    /// Docs do projeto (#440): o cadastro e a edição aceitam o modelo, a
    /// edição muda o `config_rev` e o modelo chega ao ambiente pelo pull.
    #[test]
    fn docs_do_projeto_no_cadastro_na_edicao_e_no_pull() {
        let hub = Hub::open_in_memory().unwrap();
        let docs = ct_core::DocsConfig {
            enabled: true,
            project_dirs: vec![" analises/ ".into(), "analises".into(), "specs".into()],
            epic_dirs: vec!["prompts".into()],
        };
        let added = hub
            .add_project_with(
                &novo("deep-ion-ai", "demo"),
                &ProjectProvider {
                    docs: docs.clone(),
                    ..Default::default()
                },
            )
            .unwrap();
        // Normalizado: aparado e sem repetidos.
        assert_eq!(added.project.docs.project_dirs, vec!["analises", "specs"]);
        hub.ingest("env-a", "a", &[]).unwrap();
        hub.add_assignment("deep-ion-ai", "demo", "env-a", None)
            .unwrap();
        let ep = Some(hub.config_epoch().unwrap());
        let c = hub.env_config("env-a", None, ep).unwrap();
        assert_eq!(c.projects.as_ref().unwrap()[0].docs, added.project.docs);

        let rev = hub.config_rev().unwrap();
        let p = hub
            .update_project(
                "deep-ion-ai",
                "demo",
                &HubProjectPatch {
                    docs: Some(ct_core::DocsConfig {
                        enabled: false,
                        ..docs.clone()
                    }),
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(!p.docs.enabled);
        assert!(hub.config_rev().unwrap() > rev);
        let c = hub.env_config("env-a", Some(rev), ep).unwrap();
        assert!(c.changed);
        assert!(!c.projects.unwrap()[0].docs.enabled);
        // Outra edição não mexe no modelo.
        let p = hub
            .update_project(
                "deep-ion-ai",
                "demo",
                &HubProjectPatch {
                    name: Some("Demo".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(p.docs.epic_dirs, vec!["prompts"]);
        // Pasta fora da raiz é recusada.
        let err = hub
            .update_project(
                "deep-ion-ai",
                "demo",
                &HubProjectPatch {
                    docs: Some(ct_core::DocsConfig {
                        enabled: true,
                        project_dirs: vec!["../fora".into()],
                        epic_dirs: vec![],
                    }),
                    ..Default::default()
                },
            )
            .unwrap_err()
            .to_string();
        assert!(err.contains("docs.project_dirs"), "{err}");

        // `HubProject` de um hub anterior (sem `docs`): desligado.
        let mut v = serde_json::to_value(&p).unwrap();
        v.as_object_mut().unwrap().remove("docs");
        let antigo: HubProject = serde_json::from_value(v).unwrap();
        assert_eq!(antigo.docs, ct_core::DocsConfig::default());
    }

    #[test]
    fn config_do_ambiente_so_traz_os_atribuidos() {
        let hub = hub_com("deep-ion-ai", "demo");
        hub.add_project(&novo("o", "outro")).unwrap();
        hub.add_project(&novo("o", "de-b")).unwrap();
        for env in ["env-a", "env-b"] {
            hub.ingest(env, &env[4..], &[]).unwrap();
        }
        hub.add_assignment("deep-ion-ai", "demo", "env-a", Some("/src/demo"))
            .unwrap();
        hub.add_assignment("o", "outro", "env-a", None).unwrap();
        hub.add_assignment("o", "de-b", "env-b", None).unwrap();
        let rev = hub.config_rev().unwrap();
        let ep = Some(hub.config_epoch().unwrap());

        let c = hub.env_config("env-a", None, ep).unwrap();
        assert!(c.changed);
        assert_eq!(c.config_rev, rev);
        let ps = c.projects.unwrap();
        let keys: Vec<_> = ps
            .iter()
            .map(|p| format!("{}/{}", p.owner, p.repo))
            .collect();
        assert_eq!(keys, vec!["deep-ion-ai/demo", "o/outro"]);
        assert_eq!(ps[0].repo_path, "/src/demo");
        assert_eq!(ps[0].protected_branches, vec!["develop", "main"]);
        assert!(c.removed.unwrap().is_empty());
        // Sem mudança desde `since`.
        let c = hub.env_config("env-a", Some(rev), ep).unwrap();
        assert!(!c.changed && c.projects.is_none() && c.removed.is_none());
        assert_eq!(Some(c.config_epoch), ep);
        // Mesmo `config_rev` em outra época (hub restaurado) ou sem época:
        // conjunto completo.
        for outra in [ep.map(|e| e + 1), None] {
            let c = hub.env_config("env-a", Some(rev), outra).unwrap();
            assert!(c.changed && c.projects.unwrap().len() == 2);
        }
        // Ambiente sem atribuição: nada (nem os de outros ambientes).
        let c = hub.env_config("env-c", None, ep).unwrap();
        assert!(c.projects.unwrap().is_empty() && c.removed.unwrap().is_empty());

        // Mudar o projeto no hub muda o `rev` dele no pull.
        hub.update_project(
            "deep-ion-ai",
            "demo",
            &HubProjectPatch {
                base_branch: Some("main".into()),
                ..Default::default()
            },
        )
        .unwrap();
        let c = hub.env_config("env-a", Some(rev), ep).unwrap();
        let demo = &c.projects.as_ref().unwrap()[0];
        assert_eq!((demo.base_branch.as_str(), demo.rev), ("main", rev + 1));

        // Remoção da atribuição e exclusão do projeto chegam em `removed`
        // (só as deste ambiente, depois de `since`).
        let since = hub.config_rev().unwrap();
        hub.remove_assignment("o", "outro", "env-a").unwrap();
        hub.delete_project("deep-ion-ai", "demo").unwrap();
        hub.remove_assignment("o", "de-b", "env-b").unwrap();
        let c = hub.env_config("env-a", Some(since), ep).unwrap();
        assert!(c.projects.unwrap().is_empty());
        let rm: Vec<_> = c.removed.unwrap().into_iter().map(|r| r.repo).collect();
        assert_eq!(rm, vec!["outro", "demo"]);
        let c = hub
            .env_config("env-a", Some(hub.config_rev().unwrap() - 1), ep)
            .unwrap();
        assert!(c.removed.unwrap().is_empty());
        // Hub restaurado com `config_rev` menor que o do ambiente: conjunto completo.
        let antes = hub.config_epoch().unwrap();
        assert_ne!(hub.renew_config_epoch().unwrap(), antes);
        let c = hub.env_config("env-b", Some(1_000), ep).unwrap();
        assert!(c.changed && c.projects.unwrap().is_empty());
    }

    /// Um `hub.db` da `develop` (épico #171, migrações 1 a 10, com a chave
    /// de entrega de secrets de um ambiente) sobe para as migrações do
    /// gestor (11 a 14) sem perder dados, e a revogação do token de sync
    /// apaga na mesma transação o token de agente e a chave do ambiente.
    #[test]
    fn banco_da_develop_sobe_para_as_migracoes_do_gestor() {
        let d = tempfile::tempdir().unwrap();
        let db = d.path().join("hub.db");
        {
            let conn = Connection::open(&db).unwrap();
            for m in &HUB_MIGRATIONS[..10] {
                conn.execute_batch(m).unwrap();
            }
            conn.execute_batch(
                "PRAGMA user_version = 10;
                 INSERT INTO env_token (name, token_hash, env_id, created_at)
                   VALUES ('nb', 'h', 'env-nb', 1);
                 INSERT INTO env_key (env_id, pubkey, updated_at, delivered_pubkey, applied_rev,
                                      applied_epoch)
                   VALUES ('env-nb', 'age1chave', 1, 'age1chave', 3, 7);",
            )
            .unwrap();
        }
        let hub = Hub::open(&db).unwrap();
        let v: i64 = hub
            .conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, HUB_MIGRATIONS.len() as i64);
        assert_eq!(v, 25);
        let k = hub.env_pubkey("env-nb").unwrap().unwrap();
        assert_eq!(k.pubkey, "age1chave");
        assert_eq!(k.delivered.as_deref(), Some("age1chave"));
        // Sentinela do gestor (migração 14) e colunas de prioridade (11).
        let kind: String = hub
            .conn
            .query_row(
                "SELECT kind FROM project WHERE key = '_ctower/gestor'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(kind, "gestor");
        assert_eq!(contar(&hub, "proposal"), 0);
        hub.conn
            .execute("UPDATE work_item SET priority = 'P1', rank = 1", [])
            .unwrap();

        hub.add_agent_token("nb").unwrap();
        assert!(hub.revoke_env_token("nb").unwrap());
        assert_eq!(contar(&hub, "agent_token"), 0);
        assert!(hub.env_pubkey("env-nb").unwrap().is_none());
        assert_eq!(contar(&hub, "env_token"), 0);
    }

    /// Migração 24 num `hub.db` da migração 23: as issues já espelhadas
    /// ficam sem sprint e a árvore segue lendo o espelho.
    #[test]
    fn migracao_24_issues_existentes_ficam_sem_sprint() {
        let d = tempfile::tempdir().unwrap();
        let db = d.path().join("hub.db");
        {
            let conn = Connection::open(&db).unwrap();
            for m in &HUB_MIGRATIONS[..23] {
                conn.execute_batch(m).unwrap();
            }
            conn.execute_batch(
                "PRAGMA user_version = 23;
                 INSERT INTO project (key, gh_owner, gh_repo, name, created_at, rev)
                   VALUES ('deep-ion-ai/demo', 'deep-ion-ai', 'demo', 'demo', 1, 1);
                 INSERT INTO issue_cache (project_id, gh_number, title, state, url, updated_at, synced_at)
                   SELECT id, 7, 't', 'open', 'u', 1, 1 FROM project WHERE key = 'deep-ion-ai/demo';",
            )
            .unwrap();
        }
        let hub = Hub::open(&db).unwrap();
        let tree = hub.tree("deep-ion-ai", "demo").unwrap().unwrap();
        assert_eq!(tree.roots.len(), 1);
        assert!(tree.roots[0].issue.sprints.is_empty());
        let v = serde_json::to_value(&tree.roots[0].issue).unwrap();
        assert_eq!(v["sprints"], json!([]));
    }

    /// Migração 21 (#347) num `hub.db` da develop (até a migração 20, com
    /// o Remote Control, as sessões web e o Web Push): os projetos (e o
    /// sentinela do gestor) viram GitHub, sem ambiente de sync, e as
    /// respostas não mudam fora dos campos novos.
    #[test]
    fn migracao_21_projetos_existentes_viram_github() {
        let d = tempfile::tempdir().unwrap();
        let db = d.path().join("hub.db");
        {
            let conn = Connection::open(&db).unwrap();
            for m in &HUB_MIGRATIONS[..20] {
                conn.execute_batch(m).unwrap();
            }
            conn.execute_batch(
                "PRAGMA user_version = 20;
                 INSERT INTO project (key, gh_owner, gh_repo, name, created_at, rev)
                   VALUES ('deep-ion-ai/demo', 'deep-ion-ai', 'demo', 'demo', 1, 1);",
            )
            .unwrap();
        }
        let hub = Hub::open(&db).unwrap();
        let p = hub.find_project("deep-ion-ai", "demo").unwrap().unwrap();
        assert_eq!(
            (&p.tracker, &p.forge),
            (&TrackerConfig::Github, &ForgeConfig::Github)
        );
        assert_eq!((p.branch_pattern.as_deref(), p.attribution), (None, true));
        assert_eq!(p.sync_env_id, None);
        assert_eq!(p.item_ref(12), "deep-ion-ai/demo#12");
        let gestor: (String, String) = hub
            .conn
            .query_row(
                "SELECT tracker_kind, forge_kind FROM project WHERE kind = 'gestor'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(gestor, ("github".into(), "github".into()));
        // Um projeto GitHub serializa os campos novos com os padrões.
        let v = serde_json::to_value(&p).unwrap();
        assert_eq!(v["tracker"], json!({ "kind": "github" }));
        assert_eq!(v["forge"], json!({ "kind": "github" }));
        // E um `HubProject` de hub anterior (sem os campos) é lido como GitHub.
        let antigo: HubProject = serde_json::from_value(json!({
            "id": 1, "gh_owner": "o", "gh_repo": "r", "name": "r", "base_branch": "develop",
            "protected_branches": [], "created_at": 0
        }))
        .unwrap();
        assert_eq!(antigo.tracker, TrackerConfig::Github);
        assert!(antigo.attribution);
        // O ambiente de sync precisa existir.
        assert!(hub
            .conn
            .execute("UPDATE project SET sync_env_id = 'nao-existe'", [])
            .is_err());
    }

    /// Torna `grupo/projeto/app` um projeto Jira + GitLab, como a #348 fará
    /// no cadastro, com `env` como ambiente de sync.
    fn projeto_jira(hub: &Hub, env: &str) -> TrackerConfig {
        let tracker = TrackerConfig::Jira(ct_core::JiraConfig {
            url: "https://jira.example.com".into(),
            project_key: "PROJ".into(),
            ..Default::default()
        });
        let forge = ForgeConfig::Gitlab(ct_core::GitlabConfig {
            host: "gitlab.example.com".into(),
            path: "grupo/projeto/app".into(),
            ..Default::default()
        });
        hub.add_project(&novo("grupo/projeto", "app")).unwrap();
        hub.ingest(env, "exemplo", &[]).unwrap();
        hub.conn
            .execute(
                "UPDATE project SET tracker_kind = 'jira', tracker_config = ?1,
                        forge_kind = 'gitlab', forge_config = ?2, branch_pattern = '{key}',
                        attribution = 0, sync_env_id = ?3
                 WHERE key = 'grupo/projeto/app'",
                params![tracker.config_json(), forge.config_json(), env],
            )
            .unwrap();
        tracker
    }

    /// Critério de aceite da #347: um work item com chave Jira passa pelo
    /// pull da configuração, pelo ledger, pelo outbox e pelo hub, e as
    /// referências saem como `PROJ-5450`.
    #[test]
    fn work_item_jira_passa_por_ledger_outbox_e_hub() {
        let hub = hub_com("deep-ion-ai", "demo");
        let l = Ledger::open_in_memory().unwrap();
        let tracker = projeto_jira(&hub, l.env_id());
        hub.add_assignment("grupo/projeto", "app", l.env_id(), None)
            .unwrap();
        hub.ingest("outro", "outro", &[]).unwrap();
        hub.add_assignment("grupo/projeto", "app", "outro", None)
            .unwrap();

        // Pull: só o ambiente de sync recebe `sync: true`.
        let ep = Some(hub.config_epoch().unwrap());
        let cfg = hub.env_config(l.env_id(), None, ep).unwrap();
        let cp = &cfg.projects.as_ref().unwrap()[0];
        assert_eq!(
            (cp.owner.as_str(), cp.repo.as_str()),
            ("grupo/projeto", "app")
        );
        assert_eq!(cp.tracker, tracker);
        assert_eq!(cp.forge.kind(), ct_core::ForgeKind::Gitlab);
        assert_eq!(cp.branch_pattern.as_deref(), Some("{key}"));
        assert!(!cp.attribution && cp.sync);
        let outro = hub.env_config("outro", None, ep).unwrap();
        assert!(!outro.projects.unwrap()[0].sync);

        l.apply_hub_config(&cfg, |_| "/src/app".into()).unwrap();
        let p = l.list_projects().unwrap().remove(0);
        assert!(p.sync_role);
        assert_eq!(p.effective_branch_pattern(), "{key}");
        let wi = l.ensure_work_item(p.id, 5450, WorkItemKind::Issue).unwrap();
        let branch = ct_core::branch::branch_from_pattern(
            p.effective_branch_pattern(),
            wi.kind,
            wi.gh_number,
            &ct_core::item_key(&p.tracker_ref(), wi.gh_number),
            "Portar o plugin",
            &[],
        )
        .unwrap();
        assert_eq!(branch, "PROJ-5450");
        l.set_work_item_branch(wi.id, &branch, "/wt/PROJ-5450")
            .unwrap();
        l.record_decision(&NewDecision {
            work_item_id: wi.id,
            session_id: None,
            origin: DecisionOrigin::Autonoma,
            title: "t".into(),
            context: "c".into(),
            decision: "d".into(),
            consequences: None,
        })
        .unwrap();

        // O outbox usa a chave de replicação de sempre; o hub aplica.
        let out = l.outbox_pending(100).unwrap();
        assert!(out
            .iter()
            .any(|e| e.change.key() == "grupo/projeto/app#5450"));
        let r = push(&hub, &l, "exemplo");
        assert_eq!((r.quarantined, r.discarded), (0, 0));
        let hw = hub
            .find_work_item("grupo/projeto", "app", 5450)
            .unwrap()
            .unwrap();
        assert_eq!(hw.branch.as_deref(), Some("PROJ-5450"));
        hub.conn
            .execute(
                "UPDATE work_item SET priority = 'P1' WHERE id = ?1",
                [hw.id],
            )
            .unwrap();

        // Referências prontas nas respostas do hub.
        let hp = hub.find_project("grupo/projeto", "app").unwrap().unwrap();
        assert_eq!(hp.item_ref(5450), "PROJ-5450");
        assert_eq!(hp.sync_env_id.as_deref(), Some(l.env_id()));
        let a = hub
            .agent_work_item("grupo/projeto", "app", 5450, 5)
            .unwrap()
            .unwrap();
        assert_eq!(a.item_ref, "PROJ-5450");
        assert_eq!(a.decisions[0].item_ref, "PROJ-5450");
        let ds = hub
            .agent_decisions(&AgentDecisionFilter::default(), 10)
            .unwrap()
            .unwrap();
        assert_eq!(ds.decisions[0].item_ref, "PROJ-5450");
        let ap = hub
            .agent_project("grupo/projeto", "app", true, 10)
            .unwrap()
            .unwrap();
        assert_eq!(ap.items[0].item_ref, "PROJ-5450");
        // O gestor lê a issue do Jira pelo jira-cli, com o comando de
        // credencial do cadastro (#351).
        assert_eq!(ap.tracker, ct_core::TrackerKind::Jira);
        assert!(
            ap.issue_view.ends_with("jira issue view PROJ-<n> --plain"),
            "{}",
            ap.issue_view
        );
        let ov = hub
            .agent_overview(&HealthThresholds::default(), 0, now_ms())
            .unwrap();
        let s = ov.projects.iter().find(|p| p.repo == "app").unwrap();
        assert_eq!(s.tracker, ct_core::TrackerKind::Jira);
        assert_eq!(s.issue_view, ap.issue_view);
        let rk = hub.ranking(&RankingFilter::default(), 10).unwrap();
        assert_eq!(rk.items[0].item_ref, "PROJ-5450");
        let v = serde_json::to_value(&rk.items[0]).unwrap();
        assert_eq!(v["ref"], "PROJ-5450");
        let h = hub
            .health(
                &HealthThresholds::default(),
                0,
                &HealthFilter::default(),
                now_ms(),
            )
            .unwrap();
        assert!(h
            .signals
            .iter()
            .filter(|s| s.number.is_some())
            .all(|s| s.item_ref.as_deref() == Some("PROJ-5450")));
        let pr = hub
            .create_proposal(
                &NewProposal {
                    kind: ct_core::ProposalKind::Priority,
                    owner: "grupo/projeto".into(),
                    repo: "app".into(),
                    number: 5450,
                    proposed: json!({ "priority": "P0" }),
                    justification: "urgente".into(),
                },
                l.env_id(),
                None,
                now_ms(),
            )
            .unwrap();
        assert_eq!(pr.item_ref, "PROJ-5450");
        let pr = hub
            .decide_proposal(pr.id, true, None, "hub", now_ms())
            .unwrap();
        assert_eq!(pr.item_ref, "PROJ-5450");
        let titulo = &hub.list_decisions(&[hw.id], false).unwrap()[1].title;
        assert!(titulo.ends_with("para PROJ-5450"), "{titulo}");

        // O projeto GitHub ao lado continua com as referências de hoje.
        let demo = hub.find_project("deep-ion-ai", "demo").unwrap().unwrap();
        assert_eq!(demo.item_ref(3), "deep-ion-ai/demo#3");
    }
}
