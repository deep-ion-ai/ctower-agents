//! Espelho do GitHub (spec §7.5): `issue_cache`, `pr_cache`, ETags e estado
//! do sync. O cache pode ser apagado e recriado a qualquer momento.

use crate::{json_col, parse_col, Ledger, NewEvent, Result};
use ct_core::branch::issue_number_from_branch;
use ct_core::{
    next_status, pr_trigger, PrState, Sprint, TrackerKind, WorkItemKind, WorkItemStatus,
    WorkItemTrigger,
};
use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct IssueRow {
    pub gh_number: i64,
    pub title: String,
    /// `open` | `closed`.
    pub state: String,
    pub labels: Vec<String>,
    pub parent_number: Option<i64>,
    pub has_sub_issues: bool,
    pub url: String,
    pub updated_at: i64,
    /// Sprints do Jira em que a issue está ou esteve, da mais antiga para a
    /// mais recente (a última é a atual). Vazio no GitHub e no espelho
    /// anterior à coluna; ausente no protocolo = vazio (opcional no
    /// TypeScript: o hub e o ambiente anteriores não mandam).
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(as = "Option<Vec<Sprint>>", optional))]
    pub sprints: Vec<Sprint>,
    /// Metadados do tracker prontos para exibir, na ordem da tela (Situação,
    /// Tipo, Prioridade…). Vazio no GitHub e no espelho anterior à coluna;
    /// ausente no protocolo = vazio, como `sprints`.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(as = "Option<Vec<IssueMeta>>", optional))]
    pub meta: Vec<IssueMeta>,
}

/// Um metadado do tracker: `field` é o ID no tracker (`status`,
/// `customfield_10099`…), `label` o rótulo e `value` o valor já em texto.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct IssueMeta {
    pub field: String,
    pub label: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct PrRow {
    pub number: i64,
    pub work_item_id: i64,
    pub head: String,
    pub base: String,
    pub state: PrState,
    pub url: String,
    pub updated_at: i64,
    /// SHA do head do PR (`head.sha`), da mesma listagem com ETag do sync.
    /// Num PR mesclado ou fechado, é o último commit que o PR levou: a
    /// limpeza das branches (#286) só apaga a branch que ainda aponta para
    /// ele. `None` = PR gravado antes da coluna existir.
    #[serde(default)]
    pub head_sha: Option<String>,
}

/// Resultado de uma busca no GitHub para um projeto, aplicado de uma vez.
#[derive(Debug, Clone, Default)]
pub struct GithubSnapshot {
    pub project_id: i64,
    pub issues: Vec<IssueRow>,
    pub prs: Vec<PrRow>,
    /// `(chave, etag)` das consultas que mudaram.
    pub etags: Vec<(String, String)>,
    pub synced_at: i64,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct GithubApplied {
    pub issues: usize,
    pub prs: usize,
    /// `(work_item_id, novo status)` dos gatilhos de PR que mudaram o status.
    pub status_changes: Vec<(i64, WorkItemStatus)>,
    /// Issues que passaram a fechadas sem PR mesclado (evento `issue_closed`).
    pub issues_closed: Vec<i64>,
    /// Issues que mudaram e entraram no outbox como `issue_mirror` (só no
    /// ambiente de sync de um projeto Jira, spec do Jira e GitLab §5.3).
    pub mirrored: usize,
    /// MRs que mudaram e entraram no outbox como `pr_mirror` (idem).
    pub mirrored_prs: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct SyncState {
    pub project_id: i64,
    pub last_attempt_at: i64,
    pub last_ok_at: Option<i64>,
    pub last_error: Option<String>,
    /// Rodadas seguidas com falha (issue #90); zera na primeira rodada
    /// bem-sucedida. Opcional na UI: ambiente ou hub anterior não manda.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(as = "Option<i64>", optional))]
    pub failures: i64,
    /// Início da sequência de falhas; `None` = a última rodada deu certo.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub failing_since: Option<i64>,
}

/// Colunas de `row_to_sync`.
const SYNC_COLS: &str =
    "project_id, last_attempt_at, last_ok_at, last_error, failures, failing_since";

const ISSUE_COLS: &str =
    "gh_number, title, state, labels, parent_number, has_sub_issues, url, updated_at, sprints, meta";
const PR_COLS: &str = "number, work_item_id, head, base, state, url, updated_at, head_sha";

fn row_to_issue(r: &Row) -> rusqlite::Result<IssueRow> {
    Ok(IssueRow {
        gh_number: r.get(0)?,
        title: r.get(1)?,
        state: r.get(2)?,
        labels: json_col(r, 3)?,
        parent_number: r.get(4)?,
        has_sub_issues: r.get(5)?,
        url: r.get(6)?,
        updated_at: r.get(7)?,
        sprints: json_col(r, 8)?,
        meta: json_col(r, 9)?,
    })
}

fn row_to_pr(r: &Row) -> rusqlite::Result<PrRow> {
    Ok(PrRow {
        number: r.get(0)?,
        work_item_id: r.get(1)?,
        head: r.get(2)?,
        base: r.get(3)?,
        state: parse_col(r, 4)?,
        url: r.get(5)?,
        updated_at: r.get(6)?,
        head_sha: r.get(7)?,
    })
}

fn row_to_sync(r: &Row) -> rusqlite::Result<SyncState> {
    Ok(SyncState {
        project_id: r.get(0)?,
        last_attempt_at: r.get(1)?,
        last_ok_at: r.get(2)?,
        last_error: r.get(3)?,
        failures: r.get(4)?,
        failing_since: r.get(5)?,
    })
}

/// `uid` do evento derivado da transição do PR (`gh:pr:<owner>/<repo>#<n>:<estado>`):
/// igual em todos os ambientes que perceberem a mesma mudança.
pub fn gh_pr_uid(owner: &str, repo: &str, number: i64, state: PrState) -> String {
    format!(
        "gh:pr:{}#{number}:{}",
        crate::project_key(owner, repo),
        state.as_str()
    )
}

/// `uid` do evento `issue_closed` (`gh:issue:<owner>/<repo>#<n>:closed`).
pub fn gh_issue_closed_uid(owner: &str, repo: &str, number: i64) -> String {
    format!(
        "gh:issue:{}#{number}:closed",
        crate::project_key(owner, repo)
    )
}

/// `uid` do `status_changed` que conclui a sub-issue `sub` no merge do PR do
/// épico (`gh:pr:<owner>/<repo>#<pr do épico>:merged:sub:<n>`, #454): igual
/// no ambiente e no hub, que percebem o mesmo merge.
pub fn gh_epic_merged_uid(owner: &str, repo: &str, epic_pr: i64, sub: i64) -> String {
    format!(
        "{}:sub:{sub}",
        gh_pr_uid(owner, repo, epic_pr, PrState::Merged)
    )
}

/// Gatilho do PR de uma sub-issue para a branch do épico (#454): só o merge
/// conta (aberto, a sub-issue segue em execução; fechado, nada muda).
pub(crate) fn sub_issue_pr_trigger(prev: Option<PrState>, new: PrState) -> Option<WorkItemTrigger> {
    (new == PrState::Merged && prev != Some(PrState::Merged))
        .then_some(WorkItemTrigger::SubIssuePrMerged)
}

/// Caminho de um PR do sync.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum PrRoute {
    /// PR da branch do work item para outra base: gatilhos comuns.
    Comum,
    /// PR da sub-issue `sub` para a branch do épico (#454).
    SubIssue { sub: i64 },
    /// PR para a branch do épico que não é de uma sub-issue dele (head sem
    /// número, ou de outra issue): não grava nem muda nada.
    Ignorado,
}

/// Espelho do GitHub numa conexão: as mesmas tabelas (`issue_cache`,
/// `pr_cache`, `gh_etag`, `sync_state`) existem no `ledger.db` e no `hub.db`.
pub(crate) struct GhMirror<'a>(pub(crate) &'a Connection);

impl GhMirror<'_> {
    /// Grava o PR e devolve o estado anterior no cache.
    pub(crate) fn upsert_pr(
        &self,
        project_id: i64,
        pr: &PrRow,
        synced_at: i64,
    ) -> Result<Option<PrState>> {
        let prev: Option<PrState> = self
            .0
            .query_row(
                "SELECT state FROM pr_cache WHERE project_id = ?1 AND number = ?2",
                params![project_id, pr.number],
                |r| parse_col(r, 0),
            )
            .optional()?;
        self.0.execute(
            "INSERT INTO pr_cache (project_id, number, work_item_id, head, base, state, url, updated_at,
                                   synced_at, head_sha)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT (project_id, number) DO UPDATE SET
                work_item_id = excluded.work_item_id, head = excluded.head, base = excluded.base,
                state = excluded.state, url = excluded.url, updated_at = excluded.updated_at,
                synced_at = excluded.synced_at,
                head_sha = COALESCE(excluded.head_sha, pr_cache.head_sha)",
            params![
                project_id,
                pr.number,
                pr.work_item_id,
                pr.head,
                pr.base,
                pr.state.as_str(),
                pr.url,
                pr.updated_at,
                synced_at,
                pr.head_sha
            ],
        )?;
        Ok(prev)
    }

    /// Grava a issue e devolve o estado anterior (`open`/`closed`) no cache.
    pub(crate) fn upsert_issue(
        &self,
        project_id: i64,
        i: &IssueRow,
        synced_at: i64,
    ) -> Result<Option<String>> {
        let prev: Option<String> = self
            .0
            .query_row(
                "SELECT state FROM issue_cache WHERE project_id = ?1 AND gh_number = ?2",
                params![project_id, i.gh_number],
                |r| r.get(0),
            )
            .optional()?;
        let labels = serde_json::to_string(&i.labels).expect("Vec<String> serializa");
        let sprints = serde_json::to_string(&i.sprints).expect("Vec<Sprint> serializa");
        let meta = serde_json::to_string(&i.meta).expect("Vec<IssueMeta> serializa");
        self.0.execute(
            "INSERT INTO issue_cache (project_id, gh_number, title, state, labels, parent_number,
                                      has_sub_issues, url, updated_at, synced_at, sprints, meta)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
             ON CONFLICT (project_id, gh_number) DO UPDATE SET
                title = excluded.title, state = excluded.state, labels = excluded.labels,
                parent_number = excluded.parent_number, has_sub_issues = excluded.has_sub_issues,
                url = excluded.url, updated_at = excluded.updated_at, synced_at = excluded.synced_at,
                sprints = excluded.sprints, meta = excluded.meta",
            params![
                project_id,
                i.gh_number,
                i.title,
                i.state,
                labels,
                i.parent_number,
                i.has_sub_issues,
                i.url,
                i.updated_at,
                synced_at,
                sprints,
                meta
            ],
        )?;
        Ok(prev)
    }

    /// Caminho do PR: a base é a branch de um épico? Então só vale para uma
    /// sub-issue dele, pelo número no head (`<tipo>/<n>-<slug>`). A filha é
    /// conferida nas issues da rodada e, se não vier nelas, no cache.
    pub(crate) fn route_pr(
        &self,
        project_id: i64,
        pr: &PrRow,
        issues: &[IssueRow],
    ) -> Result<PrRoute> {
        // Só no GitHub: Jira e GitLab ficam como estão.
        let epic: Option<i64> = self
            .0
            .query_row(
                "SELECT w.gh_number FROM work_item w JOIN project p ON p.id = w.project_id
                 WHERE w.project_id = ?1 AND p.tracker_kind = ?2 AND w.kind = ?3
                   AND w.branch = ?4
                 ORDER BY w.id LIMIT 1",
                params![
                    project_id,
                    TrackerKind::Github.as_str(),
                    WorkItemKind::Epic.as_str(),
                    pr.base
                ],
                |r| r.get(0),
            )
            .optional()?;
        let Some(epic) = epic else {
            return Ok(PrRoute::Comum);
        };
        let Some(sub) = issue_number_from_branch(&pr.head) else {
            return Ok(PrRoute::Ignorado);
        };
        let parent = match issues.iter().find(|i| i.gh_number == sub) {
            Some(i) => i.parent_number,
            None => self
                .get_issue(project_id, sub)?
                .and_then(|i| i.parent_number),
        };
        Ok(match parent == Some(epic) {
            true => PrRoute::SubIssue { sub },
            false => PrRoute::Ignorado,
        })
    }

    /// Work items das sub-issues do épico `epic` (pelo cache) que aguardam o
    /// merge dele: `(id, número)`.
    pub(crate) fn awaiting_epic(&self, project_id: i64, epic: i64) -> Result<Vec<(i64, i64)>> {
        let mut stmt = self.0.prepare(
            "SELECT w.id, w.gh_number FROM work_item w
             JOIN issue_cache i ON i.project_id = w.project_id AND i.gh_number = w.gh_number
             WHERE w.project_id = ?1 AND i.parent_number = ?2 AND w.status = ?3
             ORDER BY w.gh_number",
        )?;
        let rows = stmt.query_map(
            params![project_id, epic, WorkItemStatus::AguardandoEpico.as_str()],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub(crate) fn has_merged_pr(&self, work_item_id: i64) -> Result<bool> {
        Ok(self.0.query_row(
            "SELECT EXISTS (SELECT 1 FROM pr_cache WHERE work_item_id = ?1 AND state = 'merged')",
            [work_item_id],
            |r| r.get(0),
        )?)
    }

    /// ETags da rodada e sucesso do sync.
    pub(crate) fn finish_ok(&self, s: &GithubSnapshot) -> Result<()> {
        for (key, etag) in &s.etags {
            self.0.execute(
                "INSERT INTO gh_etag (key, etag, updated_at) VALUES (?1, ?2, ?3)
                 ON CONFLICT (key) DO UPDATE SET etag = excluded.etag, updated_at = excluded.updated_at",
                params![key, etag, s.synced_at],
            )?;
        }
        self.0.execute(
            "INSERT INTO sync_state (project_id, last_attempt_at, last_ok_at, last_error)
             VALUES (?1, ?2, ?2, NULL)
             ON CONFLICT (project_id) DO UPDATE SET
                last_attempt_at = excluded.last_attempt_at, last_ok_at = excluded.last_ok_at,
                last_error = NULL, failures = 0, failing_since = NULL",
            params![s.project_id, s.synced_at],
        )?;
        Ok(())
    }

    /// Rodada com falha: conta mais uma na sequência e marca o início dela.
    pub(crate) fn record_sync_error(&self, project_id: i64, error: &str) -> Result<()> {
        self.0.execute(
            "INSERT INTO sync_state (project_id, last_attempt_at, last_error, failures, failing_since)
             VALUES (?1, ?2, ?3, 1, ?2)
             ON CONFLICT (project_id) DO UPDATE SET
                last_attempt_at = excluded.last_attempt_at, last_error = excluded.last_error,
                failures = sync_state.failures + 1,
                failing_since = coalesce(sync_state.failing_since, excluded.failing_since)",
            params![project_id, ct_core::now_ms(), error],
        )?;
        Ok(())
    }

    /// Rodada concluída com aviso (issues não lidas no Jira): guarda a
    /// mensagem sem contar como falha, já que o cursor avançou.
    pub(crate) fn record_sync_warning(&self, project_id: i64, warning: &str) -> Result<()> {
        self.0.execute(
            "UPDATE sync_state SET last_error = ?2 WHERE project_id = ?1",
            params![project_id, warning],
        )?;
        Ok(())
    }

    pub(crate) fn sync_states(&self) -> Result<Vec<SyncState>> {
        let mut stmt = self.0.prepare(&format!(
            "SELECT {SYNC_COLS} FROM sync_state ORDER BY project_id"
        ))?;
        let rows = stmt.query_map([], row_to_sync)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub(crate) fn sync_state(&self, project_id: i64) -> Result<Option<SyncState>> {
        Ok(self
            .0
            .query_row(
                &format!("SELECT {SYNC_COLS} FROM sync_state WHERE project_id = ?1"),
                [project_id],
                row_to_sync,
            )
            .optional()?)
    }

    pub(crate) fn list_issues(&self, project_id: i64) -> Result<Vec<IssueRow>> {
        let mut stmt = self.0.prepare(&format!(
            "SELECT {ISSUE_COLS} FROM issue_cache WHERE project_id = ?1 ORDER BY gh_number"
        ))?;
        let rows = stmt.query_map([project_id], row_to_issue)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub(crate) fn get_issue(&self, project_id: i64, gh_number: i64) -> Result<Option<IssueRow>> {
        Ok(self
            .0
            .query_row(
                &format!(
                    "SELECT {ISSUE_COLS} FROM issue_cache WHERE project_id = ?1 AND gh_number = ?2"
                ),
                params![project_id, gh_number],
                row_to_issue,
            )
            .optional()?)
    }

    pub(crate) fn prs_for_work_item(&self, work_item_id: i64) -> Result<Vec<PrRow>> {
        let mut stmt = self.0.prepare(&format!(
            "SELECT {PR_COLS} FROM pr_cache WHERE work_item_id = ?1 ORDER BY number DESC"
        ))?;
        let rows = stmt.query_map([work_item_id], row_to_pr)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub(crate) fn get_pr(&self, project_id: i64, number: i64) -> Result<Option<PrRow>> {
        Ok(self
            .0
            .query_row(
                &format!("SELECT {PR_COLS} FROM pr_cache WHERE project_id = ?1 AND number = ?2"),
                params![project_id, number],
                row_to_pr,
            )
            .optional()?)
    }

    pub(crate) fn list_prs(&self, project_id: i64) -> Result<Vec<PrRow>> {
        let mut stmt = self.0.prepare(&format!(
            "SELECT {PR_COLS} FROM pr_cache WHERE project_id = ?1 ORDER BY number"
        ))?;
        let rows = stmt.query_map([project_id], row_to_pr)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub(crate) fn open_prs(&self, project_id: i64) -> Result<Vec<PrRow>> {
        let mut stmt = self.0.prepare(&format!(
            "SELECT {PR_COLS} FROM pr_cache WHERE project_id = ?1 AND state = 'open' ORDER BY number"
        ))?;
        let rows = stmt.query_map([project_id], row_to_pr)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub(crate) fn set_pr_head_sha(&self, project_id: i64, number: i64, sha: &str) -> Result<()> {
        self.0.execute(
            "UPDATE pr_cache SET head_sha = ?3 WHERE project_id = ?1 AND number = ?2",
            params![project_id, number, sha],
        )?;
        Ok(())
    }

    pub(crate) fn etag(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .0
            .query_row("SELECT etag FROM gh_etag WHERE key = ?1", [key], |r| {
                r.get(0)
            })
            .optional()?)
    }
}

impl Ledger {
    fn gh(&self) -> GhMirror<'_> {
        GhMirror(&self.conn)
    }

    /// Aplica a busca numa única transação: PRs (com os gatilhos de
    /// transição), issues (com `issue_closed`), ETags e estado do sync. Se
    /// algo falhar, nada é gravado — inclusive os ETags, para que a próxima
    /// busca não receba `304` e perca a mudança.
    ///
    /// No ambiente de sync de um projeto Jira (`sync_role`), cada issue que
    /// mudou entra no outbox como `issue_mirror`, e cada MR que mudou como
    /// `pr_mirror`, na mesma transação.
    pub fn apply_github(&self, s: &GithubSnapshot) -> Result<GithubApplied> {
        self.apply_snapshot(s, true)
    }

    /// Um lote de uma busca em andamento (o sync do Jira, que lê issue por
    /// issue): grava as issues e os PRs como o `apply_github`, numa
    /// transação, mas sem os ETags e sem o `sync_state` — o ciclo só conta
    /// como concluído no `apply_github` do fim. Um ciclo interrompido deixa
    /// os lotes gravados, e o próximo recomeça do mesmo cursor.
    pub fn apply_tracker_batch(&self, s: &GithubSnapshot) -> Result<GithubApplied> {
        self.apply_snapshot(s, false)
    }

    fn apply_snapshot(&self, s: &GithubSnapshot, finish: bool) -> Result<GithubApplied> {
        let (owner, repo) = self.project_slug(s.project_id)?;
        let tx = self.conn.unchecked_transaction()?;
        let mut out = GithubApplied::default();
        let mirror = self.mirrors_tracker(s.project_id)?;

        // `(work item, PR)` dos PRs mesclados nesta rodada: se o work item é
        // um épico, as sub-issues que o aguardam concluem no fim (#454).
        let mut merged = Vec::new();
        for pr in &s.prs {
            // PR para a branch de um épico: é da sub-issue, nunca do épico
            // (nem em revisão nem concluído pelos gatilhos comuns).
            let route = self.gh().route_pr(s.project_id, pr, &s.issues)?;
            let pr = &match route {
                PrRoute::Comum => pr.clone(),
                PrRoute::Ignorado => continue,
                PrRoute::SubIssue { sub } => {
                    let wi = self.ensure_work_item(s.project_id, sub, WorkItemKind::Issue)?;
                    self.set_branch_if_missing(wi.id, &pr.head)?;
                    PrRow {
                        work_item_id: wi.id,
                        ..pr.clone()
                    }
                }
            };
            let before = match mirror {
                true => self.gh().get_pr(s.project_id, pr.number)?,
                false => None,
            };
            let prev = self.gh().upsert_pr(s.project_id, pr, s.synced_at)?;
            out.prs += 1;
            if mirror && before.as_ref() != Some(pr) {
                self.enqueue_pr_row(&owner, &repo, pr)?;
                out.mirrored_prs += 1;
            }
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
            let wi = self.get_work_item(pr.work_item_id)?;
            if let Some(status) = next_status(wi.status, trigger) {
                let uid = gh_pr_uid(&owner, &repo, pr.number, pr.state);
                self.set_status_tx_uid(wi.id, status, trigger.as_str(), Some(&uid))?;
                out.status_changes.push((wi.id, status));
            }
        }

        for i in &s.issues {
            let before = match mirror {
                true => self.gh().get_issue(s.project_id, i.gh_number)?,
                false => None,
            };
            let prev = self.gh().upsert_issue(s.project_id, i, s.synced_at)?;
            out.issues += 1;
            if mirror && before.as_ref() != Some(i) {
                self.enqueue_issue_row(&owner, &repo, i)?;
                out.mirrored += 1;
            }
            if prev.as_deref() == Some("open") && i.state == "closed" {
                if let Some(wi) = self.find_work_item(s.project_id, i.gh_number)? {
                    if wi.status != WorkItemStatus::Concluido && !self.gh().has_merged_pr(wi.id)? {
                        let uid = gh_issue_closed_uid(&owner, &repo, i.gh_number);
                        self.append_event_uid(
                            NewEvent {
                                project_id: s.project_id,
                                work_item_id: Some(wi.id),
                                session_id: None,
                                kind: "issue_closed",
                                payload: json!({ "gh_number": i.gh_number }),
                            },
                            Some(&uid),
                        )?;
                        out.issues_closed.push(i.gh_number);
                    }
                }
            }
        }

        // Depois das issues, para valer a filiação desta rodada.
        for (epic_wi, epic_pr) in merged {
            let epic = self.get_work_item(epic_wi)?.gh_number;
            for (id, sub) in self.gh().awaiting_epic(s.project_id, epic)? {
                let trigger = WorkItemTrigger::EpicMerged;
                let from = self.get_work_item(id)?.status;
                if let Some(status) = next_status(from, trigger) {
                    let uid = gh_epic_merged_uid(&owner, &repo, epic_pr, sub);
                    self.set_status_tx_uid(id, status, trigger.as_str(), Some(&uid))?;
                    out.status_changes.push((id, status));
                }
            }
        }

        if finish {
            self.gh().finish_ok(s)?;
        }
        tx.commit()?;
        Ok(out)
    }

    /// Registra uma falha de sync, preservando `last_ok_at` e o cache.
    pub fn record_sync_error(&self, project_id: i64, error: &str) -> Result<()> {
        self.gh().record_sync_error(project_id, error)
    }

    /// Aviso de uma rodada concluída: fica no `last_error` sem contar falha.
    pub fn record_sync_warning(&self, project_id: i64, warning: &str) -> Result<()> {
        self.gh().record_sync_warning(project_id, warning)
    }

    pub fn sync_states(&self) -> Result<Vec<SyncState>> {
        self.gh().sync_states()
    }

    pub fn sync_state(&self, project_id: i64) -> Result<Option<SyncState>> {
        self.gh().sync_state(project_id)
    }

    /// Issues do projeto no cache, por número.
    pub fn list_issues(&self, project_id: i64) -> Result<Vec<IssueRow>> {
        self.gh().list_issues(project_id)
    }

    pub fn get_issue(&self, project_id: i64, gh_number: i64) -> Result<Option<IssueRow>> {
        self.gh().get_issue(project_id, gh_number)
    }

    /// PRs da branch do work item, do mais recente para o mais antigo.
    pub fn prs_for_work_item(&self, work_item_id: i64) -> Result<Vec<PrRow>> {
        self.gh().prs_for_work_item(work_item_id)
    }

    /// PRs abertos do projeto no cache (head e base ficam fora da limpeza
    /// das branches, #286).
    pub fn open_prs(&self, project_id: i64) -> Result<Vec<PrRow>> {
        self.gh().open_prs(project_id)
    }

    /// Todos os PRs (MRs) do projeto no cache, por número.
    pub(crate) fn gh_list_prs(&self, project_id: i64) -> Result<Vec<PrRow>> {
        self.gh().list_prs(project_id)
    }

    /// Grava o head de um PR que entrou no cache antes da coluna
    /// `head_sha` (buscado sob demanda pela limpeza das branches, #286).
    pub fn set_pr_head_sha(&self, project_id: i64, number: i64, sha: &str) -> Result<()> {
        self.gh().set_pr_head_sha(project_id, number, sha)
    }

    pub fn etag(&self, key: &str) -> Result<Option<String>> {
        self.gh().etag(key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::ledger_with_project;
    use crate::WorkItem;

    fn issue(n: i64, state: &str) -> IssueRow {
        IssueRow {
            gh_number: n,
            title: format!("Issue {n}"),
            state: state.into(),
            labels: vec!["bug".into()],
            parent_number: None,
            has_sub_issues: false,
            url: format!("https://github.com/o/r/issues/{n}"),
            updated_at: 1,
            ..Default::default()
        }
    }

    fn pr(n: i64, wi: i64, state: PrState) -> PrRow {
        PrRow {
            number: n,
            work_item_id: wi,
            head: "feat/7-x".into(),
            base: "develop".into(),
            state,
            url: format!("https://github.com/o/r/pull/{n}"),
            updated_at: 1,
            head_sha: None,
        }
    }

    fn snap(project_id: i64, issues: Vec<IssueRow>, prs: Vec<PrRow>) -> GithubSnapshot {
        GithubSnapshot {
            project_id,
            issues,
            prs,
            etags: vec![("k".into(), "e1".into())],
            synced_at: 100,
        }
    }

    /// `IssueRow` de uma versão anterior (sem `sprints` nem `meta`)
    /// desserializa com as listas vazias, e a vazia vai e volta como `[]`.
    #[test]
    fn issue_sem_meta_desserializa() {
        let v = json!({
            "gh_number": 7, "title": "t", "state": "open", "labels": [],
            "parent_number": null, "has_sub_issues": false, "url": "u", "updated_at": 1,
        });
        let i: IssueRow = serde_json::from_value(v).unwrap();
        assert!(i.meta.is_empty() && i.sprints.is_empty());
        assert_eq!(serde_json::to_value(&i).unwrap()["meta"], json!([]));
        let mut com = issue(8, "open");
        com.meta = vec![IssueMeta {
            field: "status".into(),
            label: "Situação".into(),
            value: "Aberto".into(),
        }];
        let back: IssueRow = serde_json::from_value(serde_json::to_value(&com).unwrap()).unwrap();
        assert_eq!(back, com);
    }

    fn events(l: &Ledger, wi: i64, kind: &str) -> usize {
        l.list_events(wi, 100)
            .unwrap()
            .iter()
            .filter(|e| e.kind == kind)
            .count()
    }

    #[test]
    fn lote_grava_as_issues_sem_concluir_o_ciclo() {
        let (l, p) = ledger_with_project();
        let a = l
            .apply_tracker_batch(&snap(
                p.id,
                vec![issue(1, "open"), issue(2, "open")],
                vec![],
            ))
            .unwrap();
        assert_eq!(a.issues, 2);
        assert_eq!(l.list_issues(p.id).unwrap().len(), 2);
        assert!(l.sync_state(p.id).unwrap().is_none(), "sem sucesso ainda");
        assert_eq!(l.etag("k").unwrap(), None, "a marca só vem no fim");

        l.apply_github(&snap(p.id, vec![issue(3, "open")], vec![]))
            .unwrap();
        assert_eq!(l.list_issues(p.id).unwrap().len(), 3);
        assert_eq!(l.sync_state(p.id).unwrap().unwrap().last_ok_at, Some(100));
        assert_eq!(l.etag("k").unwrap().as_deref(), Some("e1"));
    }

    #[test]
    fn eventos_do_github_tem_uid_deterministico() {
        let (l, p) = ledger_with_project();
        let wi = l.ensure_work_item(p.id, 7, WorkItemKind::Issue).unwrap();
        l.apply_github(&snap(p.id, vec![], vec![pr(30, wi.id, PrState::Open)]))
            .unwrap();
        let e = &l.list_events(wi.id, 1).unwrap()[0];
        assert_eq!(e.uid, "gh:pr:deep-ion-ai/demo#30:open");
        // PR fechado sem merge e reaberto: a mesma transição ganha sufixo.
        l.set_work_item_status(wi.id, WorkItemStatus::EmExecucao, "manual")
            .unwrap();
        l.apply_github(&snap(p.id, vec![], vec![pr(30, wi.id, PrState::Closed)]))
            .unwrap();
        l.apply_github(&snap(p.id, vec![], vec![pr(30, wi.id, PrState::Open)]))
            .unwrap();
        let e = &l.list_events(wi.id, 1).unwrap()[0];
        assert_eq!(e.uid, "gh:pr:deep-ion-ai/demo#30:open:2");
        // Issue fechada sem PR mesclado.
        let w2 = l.ensure_work_item(p.id, 8, WorkItemKind::Issue).unwrap();
        l.apply_github(&snap(p.id, vec![issue(8, "open")], vec![]))
            .unwrap();
        l.apply_github(&snap(p.id, vec![issue(8, "closed")], vec![]))
            .unwrap();
        let e = &l.list_events(w2.id, 1).unwrap()[0];
        assert_eq!(e.uid, "gh:issue:deep-ion-ai/demo#8:closed");
    }

    /// O head do PR (#286) vem do sync, não é apagado por uma linha sem ele
    /// e pode ser completado sob demanda; `open_prs` lista só os abertos.
    #[test]
    fn head_do_pr_e_prs_abertos() {
        let (l, p) = ledger_with_project();
        let wi = l.ensure_work_item(p.id, 7, WorkItemKind::Issue).unwrap();
        let mut aberto = pr(30, wi.id, PrState::Open);
        aberto.head_sha = Some("aaa".into());
        l.apply_github(&snap(
            p.id,
            vec![],
            vec![aberto, pr(31, wi.id, PrState::Closed)],
        ))
        .unwrap();
        let abertos = l.open_prs(p.id).unwrap();
        assert_eq!(abertos.len(), 1);
        assert_eq!(
            (abertos[0].number, abertos[0].head_sha.as_deref()),
            (30, Some("aaa"))
        );
        // Linha sem o head (cliente antigo) não apaga o que já se sabe.
        l.apply_github(&snap(p.id, vec![], vec![pr(30, wi.id, PrState::Merged)]))
            .unwrap();
        let prs = l.prs_for_work_item(wi.id).unwrap();
        assert_eq!(prs[1].head_sha.as_deref(), Some("aaa"));
        assert!(l.open_prs(p.id).unwrap().is_empty());
        assert_eq!(prs[0].head_sha, None);
        l.set_pr_head_sha(p.id, 31, "bbb").unwrap();
        let prs = l.prs_for_work_item(wi.id).unwrap();
        assert_eq!(prs[0].head_sha.as_deref(), Some("bbb"));
    }

    #[test]
    fn grava_issues_etag_e_estado() {
        let (l, p) = ledger_with_project();
        let mut filha = issue(2, "open");
        filha.parent_number = Some(1);
        let mut epico = issue(1, "open");
        epico.has_sub_issues = true;
        let r = l
            .apply_github(&snap(p.id, vec![epico.clone(), filha.clone()], vec![]))
            .unwrap();
        assert_eq!(r.issues, 2);
        assert_eq!(l.list_issues(p.id).unwrap(), vec![epico, filha.clone()]);
        assert_eq!(l.get_issue(p.id, 2).unwrap(), Some(filha));
        assert_eq!(l.etag("k").unwrap().as_deref(), Some("e1"));
        let st = l.sync_state(p.id).unwrap().unwrap();
        assert_eq!((st.last_ok_at, st.last_error), (Some(100), None));
    }

    #[test]
    fn pr_aberto_e_mesclado_movem_o_status_uma_vez() {
        let (l, p) = ledger_with_project();
        let wi = l.ensure_work_item(p.id, 7, WorkItemKind::Issue).unwrap();
        let r = l
            .apply_github(&snap(p.id, vec![], vec![pr(30, wi.id, PrState::Open)]))
            .unwrap();
        assert_eq!(r.status_changes, vec![(wi.id, WorkItemStatus::EmRevisao)]);
        // Mesmo snapshot de novo: nada muda, nenhum evento novo.
        let r = l
            .apply_github(&snap(p.id, vec![], vec![pr(30, wi.id, PrState::Open)]))
            .unwrap();
        assert!(r.status_changes.is_empty());
        assert_eq!(events(&l, wi.id, "status_changed"), 1);
        // Humano pausa com o PR ainda aberto: o ciclo seguinte não desfaz.
        l.set_work_item_status(wi.id, WorkItemStatus::Pausado, "manual")
            .unwrap();
        l.apply_github(&snap(p.id, vec![], vec![pr(30, wi.id, PrState::Open)]))
            .unwrap();
        assert_eq!(
            l.get_work_item(wi.id).unwrap().status,
            WorkItemStatus::Pausado
        );
        let r = l
            .apply_github(&snap(p.id, vec![], vec![pr(30, wi.id, PrState::Merged)]))
            .unwrap();
        assert_eq!(r.status_changes, vec![(wi.id, WorkItemStatus::Concluido)]);
        let wi = l.get_work_item(wi.id).unwrap();
        assert!(wi.closed_at.is_some());
        assert_eq!(
            l.list_events(wi.id, 1).unwrap()[0].payload["cause"],
            "pr_merged"
        );
        assert_eq!(
            l.prs_for_work_item(wi.id).unwrap()[0].state,
            PrState::Merged
        );
    }

    /// PR `head → base` de um work item.
    fn pr_para(n: i64, wi: i64, head: &str, base: &str, state: PrState) -> PrRow {
        PrRow {
            head: head.into(),
            base: base.into(),
            ..pr(n, wi, state)
        }
    }

    /// Épico 10 (branch `wip/10-e`) com as sub-issues 11, 12 e 14; a 13 é
    /// de fora. Devolve o work item do épico e as issues da rodada.
    fn epico(l: &Ledger, p: i64) -> (WorkItem, Vec<IssueRow>) {
        let e = l.ensure_work_item(p, 10, WorkItemKind::Epic).unwrap();
        l.set_work_item_branch(e.id, "wip/10-e", "/wt/wip__10-e")
            .unwrap();
        let mut issues = vec![issue(10, "open")];
        issues[0].has_sub_issues = true;
        for n in [11, 12, 13, 14] {
            let mut i = issue(n, "open");
            i.parent_number = (n != 13).then_some(10);
            issues.push(i);
        }
        (l.get_work_item(e.id).unwrap(), issues)
    }

    fn status(l: &Ledger, p: i64, n: i64) -> Option<WorkItemStatus> {
        l.find_work_item(p, n).unwrap().map(|w| w.status)
    }

    /// #454: o PR da sub-issue para a branch do épico. Aberto, cria o work
    /// item com a branch e não muda o status; mesclado, leva a aguardando o
    /// épico (de backlog ou de em execução); o épico não é tocado.
    #[test]
    fn pr_da_sub_issue_na_branch_do_epico() {
        let (l, p) = ledger_with_project();
        let (e, issues) = epico(&l, p.id);
        let w12 = l.ensure_work_item(p.id, 12, WorkItemKind::Issue).unwrap();
        l.set_work_item_status(w12.id, WorkItemStatus::EmExecucao, "manual")
            .unwrap();
        // O sync manda o PR com o id do épico (o da consulta pela base).
        let aberto = vec![
            pr_para(50, e.id, "feat/11-a", "wip/10-e", PrState::Open),
            pr_para(51, e.id, "feat/12-b", "wip/10-e", PrState::Open),
        ];
        let r = l.apply_github(&snap(p.id, issues.clone(), aberto)).unwrap();
        assert!(r.status_changes.is_empty(), "{:?}", r.status_changes);
        let w11 = l.find_work_item(p.id, 11).unwrap().unwrap();
        assert_eq!(
            (w11.kind, w11.status, w11.branch.as_deref()),
            (
                WorkItemKind::Issue,
                WorkItemStatus::Backlog,
                Some("feat/11-a")
            )
        );
        assert_eq!(l.prs_for_work_item(w11.id).unwrap()[0].number, 50);
        assert!(l.prs_for_work_item(e.id).unwrap().is_empty());
        // A 12 já tinha work item (sem branch): ganha a do PR.
        assert_eq!(
            l.get_work_item(w12.id).unwrap().branch.as_deref(),
            Some("feat/12-b")
        );

        let mesclado = vec![
            pr_para(50, e.id, "feat/11-a", "wip/10-e", PrState::Merged),
            pr_para(51, e.id, "feat/12-b", "wip/10-e", PrState::Merged),
        ];
        let r = l
            .apply_github(&snap(p.id, vec![], mesclado.clone()))
            .unwrap();
        assert_eq!(
            r.status_changes,
            vec![
                (w11.id, WorkItemStatus::AguardandoEpico),
                (w12.id, WorkItemStatus::AguardandoEpico)
            ]
        );
        let ev = &l.list_events(w11.id, 1).unwrap()[0];
        assert_eq!(ev.uid, "gh:pr:deep-ion-ai/demo#50:merged");
        assert_eq!(ev.payload["cause"], "sub_issue_pr_merged");
        assert_eq!(status(&l, p.id, 10), Some(WorkItemStatus::Backlog));
        // De novo: nada muda.
        let r = l.apply_github(&snap(p.id, vec![], mesclado)).unwrap();
        assert!(r.status_changes.is_empty());
        assert_eq!(events(&l, w11.id, "status_changed"), 1);
    }

    /// O mesmo PR vindo da consulta pela branch da sub-issue (com o id dela)
    /// também não segue o caminho comum: aberto não leva a em revisão.
    #[test]
    fn pr_da_sub_issue_pelo_head_nao_vai_para_revisao() {
        let (l, p) = ledger_with_project();
        let (_, issues) = epico(&l, p.id);
        let w = l.ensure_work_item(p.id, 11, WorkItemKind::Issue).unwrap();
        l.set_work_item_status(w.id, WorkItemStatus::EmExecucao, "manual")
            .unwrap();
        let pr = pr_para(50, w.id, "feat/11-a", "wip/10-e", PrState::Open);
        let r = l.apply_github(&snap(p.id, issues, vec![pr])).unwrap();
        assert!(r.status_changes.is_empty());
        assert_eq!(status(&l, p.id, 11), Some(WorkItemStatus::EmExecucao));
    }

    #[test]
    fn pr_fechado_sem_merge_e_head_de_fora_nao_mudam_nada() {
        let (l, p) = ledger_with_project();
        let (e, issues) = epico(&l, p.id);
        let w11 = l.ensure_work_item(p.id, 11, WorkItemKind::Issue).unwrap();
        let prs = vec![
            pr_para(50, e.id, "feat/11-a", "wip/10-e", PrState::Closed),
            // Sem número, de issue que não é filha do épico e de issue
            // fora do cache: ignorados.
            pr_para(52, e.id, "fix/teste-instavel", "wip/10-e", PrState::Merged),
            pr_para(53, e.id, "feat/13-c", "wip/10-e", PrState::Merged),
            pr_para(54, e.id, "feat/99-d", "wip/10-e", PrState::Merged),
        ];
        let r = l.apply_github(&snap(p.id, issues, prs)).unwrap();
        assert!(r.status_changes.is_empty());
        assert_eq!(r.prs, 1);
        assert_eq!(status(&l, p.id, 11), Some(WorkItemStatus::Backlog));
        assert_eq!(
            l.prs_for_work_item(w11.id).unwrap()[0].state,
            PrState::Closed
        );
        assert_eq!(status(&l, p.id, 13), None);
        assert_eq!(status(&l, p.id, 99), None);
        assert_eq!(status(&l, p.id, 10), Some(WorkItemStatus::Backlog));
        for n in [52, 53, 54] {
            assert!(l.gh().get_pr(p.id, n).unwrap().is_none(), "{n}");
        }
    }

    /// Merge do PR do épico na base: só quem aguarda o épico conclui, com
    /// `uid` derivado do PR do épico e da sub-issue; reaplicar não repete.
    #[test]
    fn merge_do_epico_conclui_as_sub_issues_que_o_aguardam() {
        let (l, p) = ledger_with_project();
        let (e, issues) = epico(&l, p.id);
        l.apply_github(&snap(
            p.id,
            issues,
            vec![
                pr_para(50, e.id, "feat/11-a", "wip/10-e", PrState::Merged),
                pr_para(51, e.id, "feat/12-b", "wip/10-e", PrState::Merged),
            ],
        ))
        .unwrap();
        // A 14 segue em execução, sem PR mesclado.
        let w14 = l.ensure_work_item(p.id, 14, WorkItemKind::Issue).unwrap();
        l.set_work_item_status(w14.id, WorkItemStatus::EmExecucao, "manual")
            .unwrap();
        let w11 = l.find_work_item(p.id, 11).unwrap().unwrap().id;
        let w12 = l.find_work_item(p.id, 12).unwrap().unwrap().id;

        let epico_mesclado = vec![pr_para(60, e.id, "wip/10-e", "develop", PrState::Merged)];
        let r = l
            .apply_github(&snap(p.id, vec![], epico_mesclado.clone()))
            .unwrap();
        assert_eq!(
            r.status_changes,
            vec![
                (e.id, WorkItemStatus::Concluido),
                (w11, WorkItemStatus::Concluido),
                (w12, WorkItemStatus::Concluido)
            ]
        );
        assert_eq!(status(&l, p.id, 14), Some(WorkItemStatus::EmExecucao));
        let ev = &l.list_events(w11, 1).unwrap()[0];
        assert_eq!(ev.uid, "gh:pr:deep-ion-ai/demo#60:merged:sub:11");
        assert_eq!(ev.payload["cause"], "epic_merged");
        assert_eq!(
            l.list_events(w12, 1).unwrap()[0].uid,
            "gh:pr:deep-ion-ai/demo#60:merged:sub:12"
        );
        assert!(l.get_work_item(w11).unwrap().closed_at.is_some());
        // Reaplicar: nada muda, nenhum evento novo.
        let r = l.apply_github(&snap(p.id, vec![], epico_mesclado)).unwrap();
        assert!(r.status_changes.is_empty());
        assert_eq!(events(&l, w11, "status_changed"), 2);
    }

    /// Primeiro sync depois da mudança: o épico já mesclado e os PRs das
    /// sub-issues chegam juntos, e a sub-issue vai direto a concluída.
    #[test]
    fn epico_e_sub_issue_mesclados_na_mesma_rodada() {
        let (l, p) = ledger_with_project();
        let (e, issues) = epico(&l, p.id);
        let r = l
            .apply_github(&snap(
                p.id,
                issues,
                vec![
                    pr_para(50, e.id, "feat/11-a", "wip/10-e", PrState::Merged),
                    pr_para(60, e.id, "wip/10-e", "develop", PrState::Merged),
                ],
            ))
            .unwrap();
        let w11 = l.find_work_item(p.id, 11).unwrap().unwrap().id;
        assert_eq!(
            r.status_changes,
            vec![
                (w11, WorkItemStatus::AguardandoEpico),
                (e.id, WorkItemStatus::Concluido),
                (w11, WorkItemStatus::Concluido)
            ]
        );
    }

    #[test]
    fn issue_fechada_sem_pr_gera_evento_e_mantem_status() {
        let (l, p) = ledger_with_project();
        let wi = l.ensure_work_item(p.id, 7, WorkItemKind::Issue).unwrap();
        l.apply_github(&snap(p.id, vec![issue(7, "open")], vec![]))
            .unwrap();
        let r = l
            .apply_github(&snap(p.id, vec![issue(7, "closed")], vec![]))
            .unwrap();
        assert_eq!(r.issues_closed, vec![7]);
        assert_eq!(events(&l, wi.id, "issue_closed"), 1);
        assert_eq!(
            l.get_work_item(wi.id).unwrap().status,
            WorkItemStatus::Backlog
        );
        // Continua fechada: não repete.
        l.apply_github(&snap(p.id, vec![issue(7, "closed")], vec![]))
            .unwrap();
        assert_eq!(events(&l, wi.id, "issue_closed"), 1);
    }

    #[test]
    fn issue_fechada_com_pr_mesclado_nao_gera_evento() {
        let (l, p) = ledger_with_project();
        let wi = l.ensure_work_item(p.id, 7, WorkItemKind::Issue).unwrap();
        l.apply_github(&snap(p.id, vec![issue(7, "open")], vec![]))
            .unwrap();
        let r = l
            .apply_github(&snap(
                p.id,
                vec![issue(7, "closed")],
                vec![pr(30, wi.id, PrState::Merged)],
            ))
            .unwrap();
        assert!(r.issues_closed.is_empty());
        assert_eq!(events(&l, wi.id, "issue_closed"), 0);
    }

    #[test]
    fn falha_no_meio_nao_grava_nada() {
        let (l, p) = ledger_with_project();
        // PR apontando para work item inexistente: a FK falha na transação.
        let s = GithubSnapshot {
            project_id: p.id,
            issues: vec![issue(1, "open")],
            prs: vec![pr(30, 999, PrState::Open)],
            etags: vec![("k".into(), "e1".into())],
            synced_at: 100,
        };
        assert!(l.apply_github(&s).is_err());
        assert!(l.list_issues(p.id).unwrap().is_empty());
        assert_eq!(l.etag("k").unwrap(), None);
        assert_eq!(l.sync_state(p.id).unwrap(), None);
    }

    #[test]
    fn erro_de_sync_preserva_ultimo_sucesso() {
        let (l, p) = ledger_with_project();
        l.apply_github(&snap(p.id, vec![], vec![])).unwrap();
        l.record_sync_error(p.id, "rate limit").unwrap();
        let st = &l.sync_states().unwrap()[0];
        assert_eq!(st.last_ok_at, Some(100));
        assert_eq!(st.last_error.as_deref(), Some("rate limit"));
        assert!(st.last_attempt_at >= 100);
        l.apply_github(&snap(p.id, vec![], vec![])).unwrap();
        assert_eq!(l.sync_state(p.id).unwrap().unwrap().last_error, None);
    }

    #[test]
    fn falhas_seguidas_contam_e_zeram_no_sucesso() {
        let (l, p) = ledger_with_project();
        // Primeira rodada já com falha: sem linha anterior.
        l.record_sync_error(p.id, "HTTP 410").unwrap();
        let st = l.sync_state(p.id).unwrap().unwrap();
        assert_eq!(st.failures, 1);
        let desde = st.failing_since.unwrap();
        assert_eq!(desde, st.last_attempt_at);
        l.record_sync_error(p.id, "HTTP 410").unwrap();
        l.record_sync_error(p.id, "rede").unwrap();
        let st = l.sync_state(p.id).unwrap().unwrap();
        assert_eq!((st.failures, st.failing_since), (3, Some(desde)));
        assert_eq!(st.last_error.as_deref(), Some("rede"));

        l.apply_github(&snap(p.id, vec![], vec![])).unwrap();
        let st = l.sync_state(p.id).unwrap().unwrap();
        assert_eq!(
            (st.failures, st.failing_since, st.last_error),
            (0, None, None)
        );

        // Aviso de rodada concluída não conta como falha.
        l.record_sync_warning(p.id, "2 issues não lidas").unwrap();
        let st = l.sync_state(p.id).unwrap().unwrap();
        assert_eq!((st.failures, st.failing_since), (0, None));
        assert_eq!(st.last_error.as_deref(), Some("2 issues não lidas"));
    }
}
