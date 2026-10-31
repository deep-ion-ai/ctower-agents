//! Replicação para o hub (spec do ledger centralizado §3–§5): relógio lógico
//! híbrido, `sync_outbox` e as mudanças que viajam do ambiente para o hub.
//!
//! Toda mudança replicável grava uma linha no outbox **na mesma transação**
//! da mudança. Entre ambientes, as referências usam chave natural
//! (`owner/repo`, número do work item) ou `uid`, nunca os ids `INTEGER`.

use crate::IssueRow;
use crate::{json_col, Ledger, LedgerError, Result};
use ct_core::{
    hlc, now_ms, DecisionOrigin, ForgeConfig, PrState, SessionMode, SessionState, Tool,
    TrackerConfig, TrackerReadiness, WorkItemKind, WorkItemStatus,
};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectChange {
    pub owner: String,
    pub repo: String,
    pub name: String,
    pub repo_path: String,
    pub base_branch: String,
    pub protected_branches: Vec<String>,
    pub created_at: i64,
    /// HLC da mudança: atributos divergentes seguem o mais recente.
    pub hlc: i64,
    /// Relato da aplicação da atribuição (spec do hub como plano de controle
    /// §3.3; emitido a partir da #177). Ausentes em ambientes anteriores: o
    /// hub trata como relato sem `applied_rev` (atribuição pendente).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<LocationState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// `config_rev` da atribuição que o ambiente aplicou (ou tentou).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub applied_rev: Option<i64>,
    /// Rastreador, forja, padrão de branch e atribuição do cache (spec do
    /// Jira e GitLab §3.5), só informativos: o hub é a origem deles.
    /// Ausentes em ambientes anteriores à #347: GitHub.
    #[serde(default)]
    pub tracker: TrackerConfig,
    #[serde(default)]
    pub forge: ForgeConfig,
    #[serde(default)]
    pub branch_pattern: Option<String>,
    #[serde(default = "yes")]
    pub attribution: bool,
}

fn yes() -> bool {
    true
}

/// Resultado relatado pelo ambiente ao aplicar uma atribuição.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum LocationState {
    Ok,
    Error,
}

impl LocationState {
    pub fn as_str(self) -> &'static str {
        match self {
            LocationState::Ok => "ok",
            LocationState::Error => "error",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkItemChange {
    pub owner: String,
    pub repo: String,
    pub gh_number: i64,
    pub kind: WorkItemKind,
    /// Status local; no hub o status é derivado dos eventos (§4).
    pub status: WorkItemStatus,
    pub branch: Option<String>,
    pub worktree_path: Option<String>,
    pub created_at: i64,
    pub closed_at: Option<i64>,
    pub hlc: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionChange {
    pub id: String,
    pub owner: String,
    pub repo: String,
    pub gh_number: i64,
    pub tool: Tool,
    pub tool_session_id: Option<String>,
    pub transcript_path: Option<String>,
    pub cwd: String,
    pub state: SessionState,
    pub exit_code: Option<i32>,
    pub pid: Option<i64>,
    pub started_at: i64,
    pub last_event_at: i64,
    pub ended_at: Option<i64>,
    /// Ausentes em ambientes anteriores à issue #94.
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub mode: SessionMode,
    /// HLC da fotografia: desempata cópias com o mesmo `last_event_at` (issue
    /// #293). Ausente (0) em ambientes anteriores: o hub cai no `last_event_at`.
    #[serde(default)]
    pub hlc: i64,
    /// Id da sessão no Remote Control (issue #197). Ausente em ambientes
    /// anteriores.
    #[serde(default)]
    pub rc_session_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DecisionChange {
    pub uid: String,
    pub owner: String,
    pub repo: String,
    pub gh_number: i64,
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
pub struct EventChange {
    pub uid: String,
    pub env_id: String,
    pub hlc: i64,
    pub ts: i64,
    pub owner: String,
    pub repo: String,
    pub gh_number: Option<i64>,
    pub session_id: Option<String>,
    #[serde(rename = "type")]
    pub kind: String,
    pub payload: serde_json::Value,
}

/// Fotografia de uma issue do rastreador no espelho do ambiente de sync
/// (spec do Jira e GitLab §5.3). Só projeto Jira: o hub não sincroniza esses
/// projetos e aplica no `issue_cache` dele.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IssueMirrorChange {
    pub owner: String,
    pub repo: String,
    pub issue: IssueRow,
    /// A issue saiu do espelho (fora do JQL do projeto): o hub a apaga do
    /// `issue_cache`, se não tiver work item. Ausente = `false`; o hub
    /// anterior ignora o campo e só regrava a issue.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub removed: bool,
}

/// MR (ou PR) do espelho, sem o `work_item_id` local: o hub acha o work item
/// pelo número (`gh_number`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MirrorPr {
    /// Número do MR na forja (o `iid` no GitLab).
    pub number: i64,
    pub head: String,
    pub base: String,
    pub state: PrState,
    pub url: String,
    pub updated_at: i64,
    #[serde(default)]
    pub head_sha: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PrMirrorChange {
    pub owner: String,
    pub repo: String,
    /// Número do work item dono do MR.
    pub gh_number: i64,
    pub pr: MirrorPr,
}

/// Fim de um ciclo de sync do ambiente de sync, com ou sem erro, e a
/// prontidão que ele conferiu (§8.4). No hub vira o `sync_state` do projeto.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TrackerStatusChange {
    pub owner: String,
    pub repo: String,
    pub last_attempt_at: i64,
    #[serde(default)]
    pub last_ok_at: Option<i64>,
    /// Motivo da falha do último ciclo, sem a senha (§6.5).
    #[serde(default)]
    pub last_error: Option<String>,
    /// Rodadas seguidas com falha e o início da sequência (issue #90).
    /// Ambiente anterior não manda: 0 e `None`.
    #[serde(default)]
    pub failures: i64,
    #[serde(default)]
    pub failing_since: Option<i64>,
    #[serde(default)]
    pub readiness: TrackerReadiness,
}

/// Uma mudança replicável, como vai no outbox e no `POST /sync/push`.
// A fotografia do projeto (com o rastreador, #347) é a maior variante; as
// mudanças vivem pouco (serializadas no outbox), então não vale o `Box`.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum SyncChange {
    Project(ProjectChange),
    /// O projeto deixou de existir no ambiente. Legado: só ambientes
    /// anteriores à #178 (que ainda excluíam projetos) emitem; o hub aceita e
    /// ignora (só o hub remove projetos).
    ProjectRemoved {
        owner: String,
        repo: String,
        hlc: i64,
    },
    WorkItem(WorkItemChange),
    Session(SessionChange),
    Decision(DecisionChange),
    /// Revisão de decisão; no hub vale a primeira.
    DecisionReview {
        uid: String,
        reviewed_at: i64,
        /// Nulo na revisão sem PR (decisões do gestor).
        #[serde(default)]
        reviewed_in_pr: Option<i64>,
    },
    Event(EventChange),
    /// Espelho do rastreador e da forja do projeto Jira, só do ambiente de
    /// sync (spec do Jira e GitLab §5.3). Um hub anterior à 1.3 não os
    /// entende: o hub é atualizado antes dos ambientes.
    IssueMirror(IssueMirrorChange),
    PrMirror(PrMirrorChange),
    TrackerStatus(TrackerStatusChange),
    /// Proposta de regras do agente (issue #357, spec §10.3): fica pendente
    /// no hub até o usuário decidir. Cada uma é única (`uid`).
    RulesProposal(crate::RulesProposalChange),
}

impl SyncChange {
    pub fn kind(&self) -> &'static str {
        match self {
            SyncChange::Project(_) => "project",
            SyncChange::ProjectRemoved { .. } => "project_removed",
            SyncChange::WorkItem(_) => "work_item",
            SyncChange::Session(_) => "session",
            SyncChange::Decision(_) => "decision",
            SyncChange::DecisionReview { .. } => "decision_review",
            SyncChange::Event(_) => "event",
            SyncChange::IssueMirror(_) => "issue_mirror",
            SyncChange::PrMirror(_) => "pr_mirror",
            SyncChange::TrackerStatus(_) => "tracker_status",
            SyncChange::RulesProposal(_) => "rules_proposal",
        }
    }

    /// Chave natural ou `uid` da linha afetada.
    pub fn key(&self) -> String {
        match self {
            SyncChange::Project(p) => project_key(&p.owner, &p.repo),
            SyncChange::ProjectRemoved { owner, repo, .. } => project_key(owner, repo),
            SyncChange::WorkItem(w) => work_item_key(&w.owner, &w.repo, w.gh_number),
            SyncChange::Session(s) => s.id.clone(),
            SyncChange::Decision(d) => d.uid.clone(),
            SyncChange::DecisionReview { uid, .. } => uid.clone(),
            SyncChange::Event(e) => e.uid.clone(),
            SyncChange::IssueMirror(m) => work_item_key(&m.owner, &m.repo, m.issue.gh_number),
            SyncChange::PrMirror(m) => {
                format!("{}!{}", project_key(&m.owner, &m.repo), m.pr.number)
            }
            SyncChange::TrackerStatus(t) => project_key(&t.owner, &t.repo),
            SyncChange::RulesProposal(r) => r.uid.clone(),
        }
    }

    /// Linha do espelho do rastreador, que o hub só aceita do ambiente de
    /// sync do projeto.
    pub fn is_mirror(&self) -> bool {
        matches!(
            self,
            SyncChange::IssueMirror(_) | SyncChange::PrMirror(_) | SyncChange::TrackerStatus(_)
        )
    }

    /// Fotografias completas da linha: uma pendente mais nova substitui a
    /// anterior no outbox, que não precisa mais ser enviada.
    fn coalesces(&self) -> bool {
        matches!(
            self,
            SyncChange::Project(_)
                | SyncChange::ProjectRemoved { .. }
                | SyncChange::WorkItem(_)
                | SyncChange::Session(_)
                | SyncChange::IssueMirror(_)
                | SyncChange::PrMirror(_)
                | SyncChange::TrackerStatus(_)
        )
    }
}

/// Chave global do projeto: `owner/repo` sem diferenciar maiúsculas.
pub fn project_key(owner: &str, repo: &str) -> String {
    format!("{owner}/{repo}").to_lowercase()
}

/// Chave do work item no outbox (`owner/repo#n`, minúsculas). É chave de
/// replicação, não referência para pessoas: em projeto Jira continua com o
/// `owner/repo` da forja e o número (a referência é `ct_core::item_ref`).
pub fn work_item_key(owner: &str, repo: &str, gh_number: i64) -> String {
    format!("{}#{gh_number}", project_key(owner, repo))
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OutboxEntry {
    pub seq: i64,
    pub created_at: i64,
    pub change: SyncChange,
}

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct OutboxStats {
    pub pending: i64,
    /// Quando entrou a mudança pendente mais antiga.
    pub oldest_created_at: Option<i64>,
}

/// UUIDv7 novo, em texto.
pub fn new_uid() -> String {
    uuid::Uuid::now_v7().to_string()
}

impl Ledger {
    /// Identidade deste ambiente (`<config_dir>/env_id`).
    pub fn env_id(&self) -> &str {
        &self.env_id
    }

    /// Avança e devolve o HLC persistido deste ledger.
    pub(crate) fn tick_hlc(&self) -> Result<i64> {
        Ok(self.conn.query_row(
            "UPDATE ledger_meta SET value = max(?1, value + 1) WHERE key = 'hlc' RETURNING value",
            [hlc::from_ms(now_ms())],
            |r| r.get(0),
        )?)
    }

    /// Roda `f` num SAVEPOINT: atômico sozinho ou dentro de uma transação já aberta.
    pub(crate) fn atomic<T>(&self, f: impl FnOnce() -> Result<T>) -> Result<T> {
        self.conn.execute_batch("SAVEPOINT ledger_atomic")?;
        match f() {
            Ok(v) => {
                self.conn.execute_batch("RELEASE ledger_atomic")?;
                Ok(v)
            }
            Err(e) => {
                let _ = self
                    .conn
                    .execute_batch("ROLLBACK TO ledger_atomic; RELEASE ledger_atomic");
                Err(e)
            }
        }
    }

    pub(crate) fn enqueue(&self, change: &SyncChange) -> Result<()> {
        let (kind, key) = (change.kind(), change.key());
        if change.coalesces() {
            self.conn.execute(
                "DELETE FROM sync_outbox WHERE kind = ?1 AND key = ?2",
                params![kind, key],
            )?;
        }
        let payload = serde_json::to_string(change).expect("SyncChange serializa");
        self.conn.execute(
            "INSERT INTO sync_outbox (kind, key, payload, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![kind, key, payload, now_ms()],
        )?;
        Ok(())
    }

    /// `(owner, repo)` do projeto local.
    pub(crate) fn project_slug(&self, project_id: i64) -> Result<(String, String)> {
        self.conn
            .query_row(
                "SELECT gh_owner, gh_repo FROM project WHERE id = ?1",
                [project_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?
            .ok_or_else(|| LedgerError::NotFound("project", project_id.to_string()))
    }

    /// `(owner, repo, número)` do work item local.
    pub(crate) fn work_item_slug(&self, work_item_id: i64) -> Result<(String, String, i64)> {
        self.conn
            .query_row(
                "SELECT p.gh_owner, p.gh_repo, w.gh_number
                 FROM work_item w JOIN project p ON p.id = w.project_id WHERE w.id = ?1",
                [work_item_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?
            .ok_or_else(|| LedgerError::NotFound("work_item", work_item_id.to_string()))
    }

    pub(crate) fn project_change(&self, id: i64) -> Result<SyncChange> {
        let p = self.get_project(id)?;
        Ok(SyncChange::Project(ProjectChange {
            owner: p.gh_owner,
            repo: p.gh_repo,
            name: p.name,
            repo_path: p.repo_path,
            base_branch: p.base_branch,
            protected_branches: p.protected_branches,
            created_at: p.created_at,
            hlc: self.tick_hlc()?,
            // A fotografia leva sempre o último relato da aplicação: o hub
            // trata cada fotografia como o relato inteiro, e uma sem
            // `applied_rev` voltaria a atribuição para pendente. Projeto
            // local legado: sem relato.
            state: p.location_state,
            error: p.location_error,
            applied_rev: p.applied_rev,
            tracker: p.tracker,
            forge: p.forge,
            branch_pattern: p.branch_pattern,
            attribution: p.attribution,
        }))
    }

    pub(crate) fn work_item_change(&self, id: i64) -> Result<SyncChange> {
        let w = self.get_work_item(id)?;
        let (owner, repo) = self.project_slug(w.project_id)?;
        Ok(SyncChange::WorkItem(WorkItemChange {
            owner,
            repo,
            gh_number: w.gh_number,
            kind: w.kind,
            status: w.status,
            branch: w.branch,
            worktree_path: w.worktree_path,
            created_at: w.created_at,
            closed_at: w.closed_at,
            hlc: self.tick_hlc()?,
        }))
    }

    pub(crate) fn session_change(&self, id: &str) -> Result<SyncChange> {
        let s = self.get_session(id)?;
        let (owner, repo, gh_number) = self.work_item_slug(s.work_item_id)?;
        Ok(SyncChange::Session(SessionChange {
            id: s.id,
            owner,
            repo,
            gh_number,
            tool: s.tool,
            tool_session_id: s.tool_session_id,
            transcript_path: s.transcript_path,
            cwd: s.cwd,
            state: s.state,
            exit_code: s.exit_code,
            pid: s.pid,
            started_at: s.started_at,
            last_event_at: s.last_event_at,
            ended_at: s.ended_at,
            model: s.model,
            mode: s.mode,
            hlc: self.tick_hlc()?,
            rc_session_id: s.rc_session_id,
        }))
    }

    pub(crate) fn decision_change(&self, id: i64) -> Result<SyncChange> {
        let d = self.get_decision(id)?;
        let (owner, repo, gh_number) = self.work_item_slug(d.work_item_id)?;
        Ok(SyncChange::Decision(DecisionChange {
            uid: d.uid,
            owner,
            repo,
            gh_number,
            session_id: d.session_id,
            origin: d.origin,
            title: d.title,
            context: d.context,
            decision: d.decision,
            consequences: d.consequences,
            reviewed_at: d.reviewed_at,
            reviewed_in_pr: d.reviewed_in_pr,
            created_at: d.created_at,
        }))
    }

    pub(crate) fn event_change(&self, id: i64) -> Result<SyncChange> {
        let e = self.conn.query_row(
            "SELECT e.uid, coalesce(e.env_id, ?2), e.hlc, e.ts, p.gh_owner, p.gh_repo,
                    w.gh_number, e.session_id, e.type, e.payload
             FROM event e JOIN project p ON p.id = e.project_id
             LEFT JOIN work_item w ON w.id = e.work_item_id
             WHERE e.id = ?1",
            params![id, self.env_id],
            |r| {
                Ok(EventChange {
                    uid: r.get(0)?,
                    env_id: r.get(1)?,
                    hlc: r.get(2)?,
                    ts: r.get(3)?,
                    owner: r.get(4)?,
                    repo: r.get(5)?,
                    gh_number: r.get(6)?,
                    session_id: r.get(7)?,
                    kind: r.get(8)?,
                    payload: json_col(r, 9)?,
                })
            },
        )?;
        Ok(SyncChange::Event(e))
    }

    /// O sentinela do gestor não vai: o hub já o tem e ignora o relato dele.
    pub(crate) fn enqueue_project(&self, id: i64) -> Result<()> {
        if self.get_project(id)?.kind == ct_core::ProjectKind::Gestor {
            return Ok(());
        }
        self.enqueue(&self.project_change(id)?)
    }

    pub(crate) fn enqueue_work_item(&self, id: i64) -> Result<()> {
        self.enqueue(&self.work_item_change(id)?)
    }

    pub(crate) fn enqueue_session(&self, id: &str) -> Result<()> {
        self.enqueue(&self.session_change(id)?)
    }

    /// Põe no outbox tudo o que o ledger tem (resync, primeira conexão ao
    /// hub), em ordem: projetos, work items, sessões, decisões e eventos.
    /// Retorna quantas mudanças entraram.
    pub fn enqueue_all(&self) -> Result<usize> {
        self.atomic(|| self.enqueue_all_inner())
    }

    fn enqueue_all_inner(&self) -> Result<usize> {
        let ids = |sql: &str| -> Result<Vec<i64>> {
            let mut stmt = self.conn.prepare(sql)?;
            let rows = stmt.query_map([], |r| r.get(0))?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        };
        let mut n = 0;
        for id in ids("SELECT id FROM project ORDER BY id")? {
            self.enqueue_project(id)?;
            n += 1;
        }
        for id in ids("SELECT id FROM work_item ORDER BY id")? {
            self.enqueue_work_item(id)?;
            n += 1;
        }
        let sessions: Vec<String> = {
            let mut stmt = self
                .conn
                .prepare("SELECT id FROM session ORDER BY started_at")?;
            let rows = stmt.query_map([], |r| r.get(0))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        for id in sessions {
            self.enqueue_session(&id)?;
            n += 1;
        }
        for id in ids("SELECT id FROM decision ORDER BY id")? {
            self.enqueue(&self.decision_change(id)?)?;
            n += 1;
        }
        for id in ids("SELECT id FROM event ORDER BY id")? {
            self.enqueue(&self.event_change(id)?)?;
            n += 1;
        }
        Ok(n)
    }

    /// Na primeira vez que o envio ao hub é ligado, põe o ledger inteiro no
    /// outbox (o que existia antes da migração 4 não está lá). Retorna
    /// quantas mudanças entraram (0 se já foi feito).
    pub fn ensure_backfilled(&self) -> Result<usize> {
        let feito: bool = self.conn.query_row(
            "SELECT EXISTS (SELECT 1 FROM ledger_meta WHERE key = 'hub_backfill')",
            [],
            |r| r.get(0),
        )?;
        if feito {
            return Ok(0);
        }
        self.atomic(|| {
            let n = self.enqueue_all_inner()?;
            self.conn.execute(
                "INSERT INTO ledger_meta (key, value) VALUES ('hub_backfill', ?1)",
                [now_ms()],
            )?;
            Ok(n)
        })
    }

    /// Até `limit` mudanças pendentes, da mais antiga para a mais nova.
    pub fn outbox_pending(&self, limit: u32) -> Result<Vec<OutboxEntry>> {
        let mut stmt = self
            .conn
            .prepare("SELECT seq, created_at, payload FROM sync_outbox ORDER BY seq LIMIT ?1")?;
        let rows = stmt.query_map([limit], |r| {
            Ok(OutboxEntry {
                seq: r.get(0)?,
                created_at: r.get(1)?,
                change: json_col(r, 2)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Apaga do outbox o que o hub confirmou (`seq <= upto`). Retorna quantas saíram.
    pub fn outbox_ack(&self, upto: i64) -> Result<usize> {
        Ok(self
            .conn
            .execute("DELETE FROM sync_outbox WHERE seq <= ?1", [upto])?)
    }

    pub fn outbox_stats(&self) -> Result<OutboxStats> {
        Ok(self.conn.query_row(
            "SELECT count(*), min(created_at) FROM sync_outbox",
            [],
            |r| {
                Ok(OutboxStats {
                    pending: r.get(0)?,
                    oldest_created_at: r.get(1)?,
                })
            },
        )?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::ledger_with_project;
    use crate::{NewDecision, NewSession, ProjectPatch};
    use ct_core::WorkItemKind;

    fn kinds(l: &Ledger) -> Vec<String> {
        l.outbox_pending(1000)
            .unwrap()
            .into_iter()
            .map(|e| e.change.kind().to_string())
            .collect()
    }

    #[test]
    fn hlc_avanca_e_persiste() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("l.db");
        let a = {
            let l = Ledger::open(&path, "env-a").unwrap();
            l.tick_hlc().unwrap();
            l.tick_hlc().unwrap()
        };
        let l = Ledger::open(&path, "env-a").unwrap();
        // Mesmo com o relógio voltando, o HLC reaberto continua crescendo.
        l.conn
            .execute("UPDATE ledger_meta SET value = ?1", [a + (1 << 40)])
            .unwrap();
        assert_eq!(l.tick_hlc().unwrap(), a + (1 << 40) + 1);
    }

    #[test]
    fn toda_mudanca_replicavel_entra_no_outbox() {
        let (l, p) = ledger_with_project();
        assert_eq!(kinds(&l), vec!["project"]);
        let wi = l.ensure_work_item(p.id, 12, WorkItemKind::Epic).unwrap();
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
        l.set_session_pid(&s.id, 42).unwrap();
        l.transition_session(&s.id, SessionState::Running, None)
            .unwrap();
        let d = l
            .record_decision(&NewDecision {
                work_item_id: wi.id,
                session_id: Some(s.id.clone()),
                origin: DecisionOrigin::Autonoma,
                title: "t".into(),
                context: "c".into(),
                decision: "d".into(),
                consequences: None,
            })
            .unwrap();
        l.mark_reviewed(&[d.id], Some(7)).unwrap();

        let entries = l.outbox_pending(1000).unwrap();
        // Fotografias de sessão se fundem: só a última fica pendente.
        let sessions: Vec<_> = entries
            .iter()
            .filter_map(|e| match &e.change {
                SyncChange::Session(s) => Some(s.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].state, SessionState::Running);
        assert_eq!(sessions[0].pid, Some(42));
        assert_eq!(
            (sessions[0].owner.as_str(), sessions[0].gh_number),
            ("deep-ion-ai", 12)
        );

        let review = entries
            .iter()
            .find_map(|e| match &e.change {
                SyncChange::DecisionReview {
                    uid,
                    reviewed_in_pr,
                    ..
                } => Some((uid.clone(), *reviewed_in_pr)),
                _ => None,
            })
            .unwrap();
        assert_eq!(review, (d.uid.clone(), Some(7)));

        // Cada evento do ledger tem o seu par no outbox, com uid, env e HLC.
        let n_events: i64 = l
            .conn
            .query_row("SELECT count(*) FROM event", [], |r| r.get(0))
            .unwrap();
        let evs: Vec<_> = entries
            .iter()
            .filter_map(|e| match &e.change {
                SyncChange::Event(e) => Some(e.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(evs.len() as i64, n_events);
        assert!(evs.iter().all(|e| e.env_id == l.env_id()));
        assert!(evs.windows(2).all(|w| w[0].hlc < w[1].hlc));
        let rec = evs.iter().find(|e| e.kind == "decision_recorded").unwrap();
        assert_eq!(rec.payload["decision_uid"], d.uid.as_str());
        assert_eq!(rec.gh_number, Some(12));
        assert!(entries
            .iter()
            .any(|e| matches!(&e.change, SyncChange::Decision(x) if x.uid == d.uid)));
        assert!(entries.windows(2).all(|w| w[0].seq < w[1].seq));
    }

    #[test]
    fn fotografia_de_projeto_e_work_item_substitui_a_pendente() {
        let (l, p) = ledger_with_project();
        let wi = l.ensure_work_item(p.id, 3, WorkItemKind::Issue).unwrap();
        l.set_work_item_branch(wi.id, "feat/3-x", "/w/3").unwrap();
        l.update_project(
            p.id,
            &ProjectPatch {
                base_branch: Some("main".into()),
                ..Default::default()
            },
        )
        .unwrap();
        let entries = l.outbox_pending(1000).unwrap();
        let projetos: Vec<_> = entries
            .iter()
            .filter_map(|e| match &e.change {
                SyncChange::Project(p) => Some(p.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(projetos.len(), 1);
        assert_eq!(projetos[0].base_branch, "main");
        let wis: Vec<_> = entries
            .iter()
            .filter_map(|e| match &e.change {
                SyncChange::WorkItem(w) => Some(w.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(wis.len(), 1);
        assert_eq!(wis[0].branch.as_deref(), Some("feat/3-x"));
    }

    #[test]
    fn falha_na_mudanca_nao_deixa_nada_no_outbox() {
        let (l, p) = ledger_with_project();
        l.outbox_ack(i64::MAX).unwrap();
        // Decisão num work item inexistente: nada é gravado.
        assert!(l
            .record_decision(&NewDecision {
                work_item_id: 999,
                session_id: None,
                origin: DecisionOrigin::Autonoma,
                title: "t".into(),
                context: "c".into(),
                decision: "d".into(),
                consequences: None,
            })
            .is_err());
        // Evento que viola a FK de sessão: nem evento nem outbox.
        assert!(l
            .append_event(crate::NewEvent {
                project_id: p.id,
                work_item_id: None,
                session_id: Some("nao-existe"),
                kind: "x",
                payload: serde_json::json!({}),
            })
            .is_err());
        assert_eq!(l.outbox_stats().unwrap(), OutboxStats::default());
    }

    #[test]
    fn rollback_da_transacao_externa_desfaz_o_outbox() {
        let (l, p) = ledger_with_project();
        l.outbox_ack(i64::MAX).unwrap();
        {
            let _tx = l.conn.unchecked_transaction().unwrap();
            l.update_project(
                p.id,
                &ProjectPatch {
                    name: Some("novo".into()),
                    ..Default::default()
                },
            )
            .unwrap();
            l.append_event(crate::NewEvent {
                project_id: p.id,
                work_item_id: None,
                session_id: None,
                kind: "x",
                payload: serde_json::json!({}),
            })
            .unwrap();
            assert_eq!(l.outbox_stats().unwrap().pending, 2);
            // _tx cai sem commit: ROLLBACK
        }
        assert_eq!(l.outbox_stats().unwrap().pending, 0);
        assert_eq!(l.get_project(p.id).unwrap().name, "demo");
    }

    #[test]
    fn ack_apaga_so_o_confirmado() {
        let (l, p) = ledger_with_project();
        l.ensure_work_item(p.id, 1, WorkItemKind::Issue).unwrap();
        let entries = l.outbox_pending(1000).unwrap();
        let meio = entries[1].seq;
        assert_eq!(l.outbox_ack(meio).unwrap(), 2);
        let resto = l.outbox_pending(1000).unwrap();
        assert_eq!(resto.len(), entries.len() - 2);
        assert!(resto.iter().all(|e| e.seq > meio));
        let st = l.outbox_stats().unwrap();
        assert_eq!(st.pending, resto.len() as i64);
        assert!(st.oldest_created_at.is_some());
    }

    #[test]
    fn backfill_reenfileira_tudo_uma_vez() {
        let (l, p) = ledger_with_project();
        let wi = l.ensure_work_item(p.id, 1, WorkItemKind::Issue).unwrap();
        l.create_session(&NewSession {
            work_item_id: wi.id,
            tool: Tool::Claude,
            cwd: "/w".into(),
            launch_cmd: vec![],
            model: None,
            mode: ct_core::SessionMode::Native,
        })
        .unwrap();
        l.outbox_ack(i64::MAX).unwrap();
        let eventos: i64 = l
            .conn
            .query_row("SELECT count(*) FROM event", [], |r| r.get(0))
            .unwrap();
        let n = l.ensure_backfilled().unwrap();
        assert_eq!(n as i64, 3 + eventos);
        assert_eq!(
            kinds(&l)[..3],
            ["project".to_string(), "work_item".into(), "session".into()]
        );
        assert_eq!(l.ensure_backfilled().unwrap(), 0);
        // O resync manual sempre reenfileira.
        l.outbox_ack(i64::MAX).unwrap();
        assert_eq!(l.enqueue_all().unwrap(), n);
    }

    #[test]
    fn mudanca_serializa_com_kind() {
        let c = SyncChange::DecisionReview {
            uid: "u".into(),
            reviewed_at: 1,
            reviewed_in_pr: Some(2),
        };
        let v = serde_json::to_value(&c).unwrap();
        assert_eq!(v["kind"], "decision_review");
        assert_eq!(v["data"]["uid"], "u");
        assert_eq!(serde_json::from_value::<SyncChange>(v).unwrap(), c);
        assert_eq!(project_key("Deep-Ion-AI", "Demo"), "deep-ion-ai/demo");
    }

    #[test]
    fn espelho_do_rastreador_tem_chave_propria_e_se_funde() {
        let (l, _) = ledger_with_project();
        l.outbox_ack(i64::MAX).unwrap();
        let issue = |title: &str| {
            SyncChange::IssueMirror(IssueMirrorChange {
                owner: "grupo/projeto".into(),
                repo: "app".into(),
                issue: IssueRow {
                    gh_number: 5450,
                    title: title.into(),
                    state: "open".into(),
                    labels: vec![],
                    parent_number: None,
                    has_sub_issues: false,
                    url: "https://jira.example.com/browse/PROJ-5450".into(),
                    updated_at: 1,
                    ..Default::default()
                },
                removed: false,
            })
        };
        let mr = SyncChange::PrMirror(PrMirrorChange {
            owner: "grupo/projeto".into(),
            repo: "app".into(),
            gh_number: 5450,
            pr: MirrorPr {
                number: 77,
                head: "PROJ-5450".into(),
                base: "develop".into(),
                state: PrState::Merged,
                url: "u".into(),
                updated_at: 2,
                head_sha: None,
            },
        });
        let st = |err: Option<&str>| {
            SyncChange::TrackerStatus(TrackerStatusChange {
                owner: "grupo/projeto".into(),
                repo: "app".into(),
                last_attempt_at: 3,
                last_ok_at: None,
                last_error: err.map(String::from),
                failures: 0,
                failing_since: None,
                readiness: TrackerReadiness::default(),
            })
        };
        assert_eq!(issue("a").key(), "grupo/projeto/app#5450");
        assert_eq!(mr.key(), "grupo/projeto/app!77");
        assert_eq!(st(None).key(), "grupo/projeto/app");
        assert!([issue("a"), mr.clone(), st(None)]
            .iter()
            .all(SyncChange::is_mirror));
        for c in [
            issue("a"),
            issue("b"),
            mr.clone(),
            mr.clone(),
            st(None),
            st(Some("x")),
        ] {
            l.enqueue(&c).unwrap();
        }
        // Fotografias: só a última de cada linha fica pendente.
        let out = l.outbox_pending(100).unwrap();
        assert_eq!(kinds(&l), ["issue_mirror", "pr_mirror", "tracker_status"]);
        assert!(matches!(&out[0].change, SyncChange::IssueMirror(m) if m.issue.title == "b"));
        assert!(
            matches!(&out[2].change, SyncChange::TrackerStatus(t) if t.last_error.as_deref() == Some("x"))
        );
        let v = serde_json::to_value(&out[1].change).unwrap();
        assert_eq!(v["kind"], "pr_mirror");
        assert_eq!(v["data"]["pr"]["state"], "merged");
        assert!(v["data"]["pr"].get("work_item_id").is_none());
        // Sem a prontidão (ambiente que ainda não calcula): padrão vazio.
        let t: SyncChange = serde_json::from_value(serde_json::json!({
            "kind": "tracker_status",
            "data": { "owner": "o", "repo": "r", "last_attempt_at": 1 }
        }))
        .unwrap();
        assert!(
            matches!(t, SyncChange::TrackerStatus(t) if t.readiness == TrackerReadiness::default())
        );
    }

    #[test]
    fn sessao_de_ambiente_antigo_chega_sem_modelo_e_native() {
        let v = serde_json::json!({ "kind": "session", "data": {
            "id": "s", "owner": "o", "repo": "r", "gh_number": 1, "tool": "claude",
            "tool_session_id": null, "transcript_path": null, "cwd": "/w", "state": "running",
            "exit_code": null, "pid": null, "started_at": 1, "last_event_at": 1, "ended_at": null
        }});
        let SyncChange::Session(s) = serde_json::from_value(v).unwrap() else {
            panic!("esperava sessão");
        };
        assert_eq!((s.model, s.mode), (None, SessionMode::Native));
    }
}
