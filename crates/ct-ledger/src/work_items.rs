use crate::{parse_col, Ledger, LedgerError, NewEvent, Result};
use ct_core::{next_status, now_ms, WorkItemKind, WorkItemStatus, WorkItemTrigger};
use rusqlite::{params, OptionalExtension, Row};
use serde::Serialize;
use serde_json::json;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct WorkItem {
    pub id: i64,
    pub project_id: i64,
    pub gh_number: i64,
    pub kind: WorkItemKind,
    pub status: WorkItemStatus,
    pub branch: Option<String>,
    pub worktree_path: Option<String>,
    pub created_at: i64,
    pub closed_at: Option<i64>,
}

const COLS: &str =
    "id, project_id, gh_number, kind, status, branch, worktree_path, created_at, closed_at";

fn row_to_work_item(r: &Row) -> rusqlite::Result<WorkItem> {
    Ok(WorkItem {
        id: r.get(0)?,
        project_id: r.get(1)?,
        gh_number: r.get(2)?,
        kind: parse_col(r, 3)?,
        status: parse_col(r, 4)?,
        branch: r.get(5)?,
        worktree_path: r.get(6)?,
        created_at: r.get(7)?,
        closed_at: r.get(8)?,
    })
}

impl Ledger {
    /// Cria o work item se não existir; sempre retorna o registro atual.
    /// Atômico, e também dentro de uma transação já aberta (o sync, #454).
    pub fn ensure_work_item(
        &self,
        project_id: i64,
        gh_number: i64,
        kind: WorkItemKind,
    ) -> Result<WorkItem> {
        self.atomic(|| self.ensure_work_item_inner(project_id, gh_number, kind))
    }

    fn ensure_work_item_inner(
        &self,
        project_id: i64,
        gh_number: i64,
        kind: WorkItemKind,
    ) -> Result<WorkItem> {
        let inserted = self.conn.execute(
            "INSERT INTO work_item (project_id, gh_number, kind, created_at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (project_id, gh_number) DO NOTHING",
            params![project_id, gh_number, kind.as_str(), now_ms()],
        )?;
        let wi = self.find_work_item(project_id, gh_number)?.ok_or_else(|| {
            LedgerError::NotFound("work_item", format!("{project_id}#{gh_number}"))
        })?;
        if inserted == 1 {
            self.enqueue_work_item(wi.id)?;
            self.append_event(NewEvent {
                project_id,
                work_item_id: Some(wi.id),
                session_id: None,
                kind: "work_item_created",
                payload: json!({ "gh_number": gh_number, "kind": kind }),
            })?;
        }
        Ok(wi)
    }

    pub fn get_work_item(&self, id: i64) -> Result<WorkItem> {
        self.conn
            .query_row(
                &format!("SELECT {COLS} FROM work_item WHERE id = ?1"),
                [id],
                row_to_work_item,
            )
            .optional()?
            .ok_or_else(|| LedgerError::NotFound("work_item", id.to_string()))
    }

    pub fn find_work_item(&self, project_id: i64, gh_number: i64) -> Result<Option<WorkItem>> {
        Ok(self
            .conn
            .query_row(
                &format!("SELECT {COLS} FROM work_item WHERE project_id = ?1 AND gh_number = ?2"),
                params![project_id, gh_number],
                row_to_work_item,
            )
            .optional()?)
    }

    /// Work items do projeto cuja branch registrada é `branch` (a limpeza e
    /// o guarda-corpo partem dela, sem supor o padrão do nome).
    pub fn work_items_by_branch(&self, project_id: i64, branch: &str) -> Result<Vec<WorkItem>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {COLS} FROM work_item WHERE project_id = ?1 AND branch = ?2 ORDER BY id"
        ))?;
        let rows = stmt.query_map(params![project_id, branch], row_to_work_item)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn set_work_item_branch(&self, id: i64, branch: &str, worktree_path: &str) -> Result<()> {
        let wi = self.get_work_item(id)?;
        let tx = self.conn.unchecked_transaction()?;
        self.conn.execute(
            "UPDATE work_item SET branch = ?1, worktree_path = ?2 WHERE id = ?3",
            params![branch, worktree_path, id],
        )?;
        self.enqueue_work_item(id)?;
        self.append_event(NewEvent {
            project_id: wi.project_id,
            work_item_id: Some(id),
            session_id: None,
            kind: "worktree_set",
            payload: json!({ "branch": branch, "worktree_path": worktree_path }),
        })?;
        tx.commit()?;
        Ok(())
    }

    /// Grava a branch de um work item que ainda não tem uma (a sub-issue
    /// cujo PR para a branch do épico apareceu no sync, #454), sem worktree.
    pub(crate) fn set_branch_if_missing(&self, id: i64, branch: &str) -> Result<()> {
        let n = self.conn.execute(
            "UPDATE work_item SET branch = ?2 WHERE id = ?1 AND branch IS NULL",
            params![id, branch],
        )?;
        if n == 1 {
            self.enqueue_work_item(id)?;
        }
        Ok(())
    }

    /// Muda o status e registra evento. Retorna `false` se já estava no status.
    pub fn set_work_item_status(
        &self,
        id: i64,
        status: WorkItemStatus,
        cause: &str,
    ) -> Result<bool> {
        let tx = self.conn.unchecked_transaction()?;
        let changed = self.set_status_tx(id, status, cause)?;
        tx.commit()?;
        Ok(changed)
    }

    /// Como `set_work_item_status`, dentro de uma transação já aberta.
    pub(crate) fn set_status_tx(
        &self,
        id: i64,
        status: WorkItemStatus,
        cause: &str,
    ) -> Result<bool> {
        self.set_status_tx_uid(id, status, cause, None)
    }

    /// `set_status_tx` com `uid` determinístico no evento (gatilhos do GitHub).
    pub(crate) fn set_status_tx_uid(
        &self,
        id: i64,
        status: WorkItemStatus,
        cause: &str,
        uid: Option<&str>,
    ) -> Result<bool> {
        self.set_status_full(id, status, cause, uid, None, None)
    }

    /// Núcleo das mudanças de status: `session_id` e `reason` (opcionais)
    /// vão no evento, o `reason` no payload como no `status_set_manual` do hub.
    pub(crate) fn set_status_full(
        &self,
        id: i64,
        status: WorkItemStatus,
        cause: &str,
        uid: Option<&str>,
        session_id: Option<&str>,
        reason: Option<&str>,
    ) -> Result<bool> {
        let wi = self.get_work_item(id)?;
        if wi.status == status {
            return Ok(false);
        }
        let closed_at = (status == WorkItemStatus::Concluido).then(now_ms);
        let mut payload = json!({ "from": wi.status, "to": status, "cause": cause });
        if let Some(r) = reason {
            payload["reason"] = json!(r);
        }
        self.conn.execute(
            "UPDATE work_item SET status = ?1, closed_at = ?2 WHERE id = ?3",
            params![status.as_str(), closed_at, id],
        )?;
        self.enqueue_work_item(id)?;
        self.append_event_uid(
            NewEvent {
                project_id: wi.project_id,
                work_item_id: Some(id),
                session_id,
                kind: "status_changed",
                payload,
            },
            uid,
        )?;
        Ok(true)
    }

    /// A sessão do épico `epic_id` começou a sub-issue `number` (#458): a
    /// sub-issue vai para em execução (`cause: "mcp_sub_issue"`), criando o
    /// work item (`issue`) se faltar. De `aguardando_epico` ou `concluido`
    /// é retrabalho, uma mudança como outra qualquer. Recusa (sem gravar
    /// nada) se o work item não é épico ou se `number` não é sub-issue dele
    /// no espelho (`issue_cache.parent_number`). Já em execução = nada a
    /// gravar: devolve `(work item, false)`.
    pub fn start_sub_issue(
        &self,
        epic_id: i64,
        number: i64,
        session_id: Option<&str>,
        reason: Option<&str>,
    ) -> Result<(WorkItem, bool)> {
        let epic = self.get_work_item(epic_id)?;
        if epic.kind != WorkItemKind::Epic {
            return Err(LedgerError::Conflict(
                "ledger_start_sub_issue é só para a sessão de um épico".into(),
            ));
        }
        let parent = self
            .get_issue(epic.project_id, number)?
            .and_then(|i| i.parent_number);
        if parent != Some(epic.gh_number) {
            return Err(LedgerError::Conflict(format!(
                "#{number} não é sub-issue do épico #{} (confira as sub-issues em ledger_get_context)",
                epic.gh_number
            )));
        }
        let reason = reason.map(str::trim).filter(|r| !r.is_empty());
        let tx = self.conn.unchecked_transaction()?;
        let wi = self.ensure_work_item_inner(epic.project_id, number, WorkItemKind::Issue)?;
        let changed = self.set_status_full(
            wi.id,
            WorkItemStatus::EmExecucao,
            "mcp_sub_issue",
            None,
            session_id,
            reason,
        )?;
        tx.commit()?;
        Ok((self.get_work_item(wi.id)?, changed))
    }

    pub fn apply_trigger(&self, id: i64, trigger: WorkItemTrigger) -> Result<bool> {
        let wi = self.get_work_item(id)?;
        match next_status(wi.status, trigger) {
            Some(s) => self.set_work_item_status(id, s, trigger.as_str()),
            None => Ok(false),
        }
    }

    /// Work items do projeto, do mais antigo para o mais recente.
    pub fn list_work_items(&self, project_id: i64) -> Result<Vec<WorkItem>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {COLS} FROM work_item WHERE project_id = ?1 ORDER BY id"
        ))?;
        let rows = stmt.query_map([project_id], row_to_work_item)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// O work item e os work items das suas sub-issues (via `issue_cache.parent_number`).
    pub fn epic_work_items(&self, id: i64) -> Result<Vec<WorkItem>> {
        let wi = self.get_work_item(id)?;
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {COLS} FROM work_item w WHERE w.id = ?1
                OR (w.project_id = ?2 AND w.gh_number IN (
                    SELECT gh_number FROM issue_cache
                    WHERE project_id = ?2 AND parent_number = ?3))
             ORDER BY id"
        ))?;
        let rows = stmt.query_map(
            params![wi.id, wi.project_id, wi.gh_number],
            row_to_work_item,
        )?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }
}

#[cfg(test)]
mod tests {
    use crate::test_support::ledger_with_project;
    use ct_core::{WorkItemKind, WorkItemStatus, WorkItemTrigger};

    #[test]
    fn lista_do_projeto_e_do_epico() {
        let (l, p) = ledger_with_project();
        let epic = l.ensure_work_item(p.id, 10, WorkItemKind::Epic).unwrap();
        let filha = l.ensure_work_item(p.id, 11, WorkItemKind::Issue).unwrap();
        let avulsa = l.ensure_work_item(p.id, 20, WorkItemKind::Issue).unwrap();
        let ids = |v: Vec<crate::WorkItem>| v.into_iter().map(|w| w.id).collect::<Vec<_>>();
        assert_eq!(
            ids(l.list_work_items(p.id).unwrap()),
            vec![epic.id, filha.id, avulsa.id]
        );
        assert_eq!(ids(l.epic_work_items(epic.id).unwrap()), vec![epic.id]);
        l.conn
            .execute(
                "INSERT INTO issue_cache (project_id, gh_number, title, state, parent_number, url, updated_at, synced_at)
                 VALUES (?1, 11, 'filha', 'open', 10, 'u', 0, 0)",
                [p.id],
            )
            .unwrap();
        assert_eq!(
            ids(l.epic_work_items(epic.id).unwrap()),
            vec![epic.id, filha.id]
        );
    }

    #[test]
    fn ensure_e_idempotente() {
        let (l, p) = ledger_with_project();
        let a = l.ensure_work_item(p.id, 12, WorkItemKind::Epic).unwrap();
        let b = l.ensure_work_item(p.id, 12, WorkItemKind::Epic).unwrap();
        assert_eq!(a, b);
        assert_eq!(a.status, WorkItemStatus::Backlog);
        let created = l
            .list_events(a.id, 10)
            .unwrap()
            .iter()
            .filter(|e| e.kind == "work_item_created")
            .count();
        assert_eq!(created, 1);
    }

    #[test]
    fn status_muda_e_registra_evento() {
        let (l, p) = ledger_with_project();
        let wi = l.ensure_work_item(p.id, 7, WorkItemKind::Issue).unwrap();
        assert!(l
            .set_work_item_status(wi.id, WorkItemStatus::Pausado, "manual")
            .unwrap());
        assert!(!l
            .set_work_item_status(wi.id, WorkItemStatus::Pausado, "manual")
            .unwrap());
        let ev = &l.list_events(wi.id, 1).unwrap()[0];
        assert_eq!(ev.kind, "status_changed");
        assert_eq!(ev.payload["to"], "pausado");
    }

    #[test]
    fn concluido_registra_fechamento() {
        let (l, p) = ledger_with_project();
        let wi = l.ensure_work_item(p.id, 7, WorkItemKind::Issue).unwrap();
        l.apply_trigger(wi.id, WorkItemTrigger::PrMerged).unwrap();
        let wi = l.get_work_item(wi.id).unwrap();
        assert_eq!(wi.status, WorkItemStatus::Concluido);
        assert!(wi.closed_at.is_some());
        let ev = &l.list_events(wi.id, 1).unwrap()[0];
        assert_eq!(ev.payload["cause"], "pr_merged");
        assert!(!l.apply_trigger(wi.id, WorkItemTrigger::PrOpened).unwrap());
    }

    #[test]
    fn branch_e_worktree() {
        let (l, p) = ledger_with_project();
        let wi = l.ensure_work_item(p.id, 12, WorkItemKind::Epic).unwrap();
        l.set_work_item_branch(wi.id, "wip/12-auth", "/w/demo/wip__12-auth")
            .unwrap();
        let wi = l.get_work_item(wi.id).unwrap();
        assert_eq!(wi.branch.as_deref(), Some("wip/12-auth"));
        assert_eq!(wi.worktree_path.as_deref(), Some("/w/demo/wip__12-auth"));
    }
}
