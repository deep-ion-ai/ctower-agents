use crate::{new_uid, parse_col, Ledger, LedgerError, NewEvent, Result, SyncChange};
use ct_core::{now_ms, DecisionOrigin};
use rusqlite::types::Value;
use rusqlite::{params, params_from_iter, OptionalExtension, Row};
use serde::Serialize;
use serde_json::json;
use std::collections::BTreeMap;

pub struct NewDecision {
    pub work_item_id: i64,
    pub session_id: Option<String>,
    pub origin: DecisionOrigin,
    pub title: String,
    pub context: String,
    pub decision: String,
    pub consequences: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Decision {
    pub id: i64,
    /// Identidade global (UUIDv7), igual no ambiente e no hub.
    pub uid: String,
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

const COLS: &str = "id, work_item_id, session_id, origin, title, context, decision, consequences, \
                    reviewed_at, reviewed_in_pr, created_at, uid";

fn row_to_decision(r: &Row) -> rusqlite::Result<Decision> {
    Ok(Decision {
        id: r.get(0)?,
        work_item_id: r.get(1)?,
        session_id: r.get(2)?,
        origin: parse_col(r, 3)?,
        title: r.get(4)?,
        context: r.get(5)?,
        decision: r.get(6)?,
        consequences: r.get(7)?,
        reviewed_at: r.get(8)?,
        reviewed_in_pr: r.get(9)?,
        created_at: r.get(10)?,
        uid: r.get(11)?,
    })
}

fn placeholders(n: usize) -> String {
    vec!["?"; n].join(", ")
}

impl Ledger {
    /// Registra decisão. `orientada`/`manual` nascem revisadas; `autonoma` fica pendente.
    pub fn record_decision(&self, d: &NewDecision) -> Result<Decision> {
        let wi = self.get_work_item(d.work_item_id)?;
        let now = now_ms();
        let reviewed_at = (d.origin != DecisionOrigin::Autonoma).then_some(now);
        let uid = new_uid();
        let tx = self.conn.unchecked_transaction()?;
        self.conn.execute(
            "INSERT INTO decision (work_item_id, session_id, origin, title, context, decision,
                                   consequences, reviewed_at, created_at, uid)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                d.work_item_id,
                d.session_id,
                d.origin.as_str(),
                d.title,
                d.context,
                d.decision,
                d.consequences,
                reviewed_at,
                now,
                uid
            ],
        )?;
        let id = self.conn.last_insert_rowid();
        self.enqueue(&self.decision_change(id)?)?;
        self.append_event(NewEvent {
            project_id: wi.project_id,
            work_item_id: Some(wi.id),
            session_id: d.session_id.as_deref(),
            kind: "decision_recorded",
            payload: json!({
                "decision_id": id, "decision_uid": uid, "origin": d.origin, "title": d.title
            }),
        })?;
        tx.commit()?;
        self.get_decision(id)
    }

    pub fn get_decision(&self, id: i64) -> Result<Decision> {
        self.conn
            .query_row(
                &format!("SELECT {COLS} FROM decision WHERE id = ?1"),
                [id],
                row_to_decision,
            )
            .optional()?
            .ok_or_else(|| LedgerError::NotFound("decision", id.to_string()))
    }

    pub fn list_decisions(
        &self,
        work_item_ids: &[i64],
        only_unreviewed: bool,
    ) -> Result<Vec<Decision>> {
        if work_item_ids.is_empty() {
            return Ok(Vec::new());
        }
        let mut sql = format!(
            "SELECT {COLS} FROM decision WHERE work_item_id IN ({})",
            placeholders(work_item_ids.len())
        );
        if only_unreviewed {
            sql.push_str(" AND reviewed_at IS NULL");
        }
        sql.push_str(" ORDER BY id");
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(work_item_ids.iter()), row_to_decision)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Marca como revisadas as decisões ainda pendentes entre `ids`. Retorna quantas mudaram.
    ///
    /// Na mesma transação, grava um evento `decisions_reviewed` por work item
    /// afetado, com `{ "pr": <pr ou null>, "ids": [...] }` (só os ids que mudaram).
    /// Se nada mudou, nenhum evento é gravado.
    pub fn mark_reviewed(&self, ids: &[i64], pr_number: Option<i64>) -> Result<usize> {
        if ids.is_empty() {
            return Ok(0);
        }
        let sql = format!(
            "UPDATE decision SET reviewed_at = ?, reviewed_in_pr = ?
             WHERE reviewed_at IS NULL AND id IN ({})
             RETURNING id, work_item_id, uid, reviewed_at",
            placeholders(ids.len())
        );
        let now = now_ms();
        let mut values = vec![
            Value::Integer(now),
            pr_number.map_or(Value::Null, Value::Integer),
        ];
        values.extend(ids.iter().map(|i| Value::Integer(*i)));
        let tx = self.conn.unchecked_transaction()?;
        let changed: Vec<(i64, i64, String)> = {
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(params_from_iter(values), |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        let mut by_work_item: BTreeMap<i64, Vec<(i64, String)>> = BTreeMap::new();
        for (id, work_item_id, uid) in &changed {
            self.enqueue(&SyncChange::DecisionReview {
                uid: uid.clone(),
                reviewed_at: now,
                reviewed_in_pr: pr_number,
            })?;
            by_work_item
                .entry(*work_item_id)
                .or_default()
                .push((*id, uid.clone()));
        }
        for (work_item_id, mut revisadas) in by_work_item {
            revisadas.sort_unstable();
            let (ids, uids): (Vec<i64>, Vec<String>) = revisadas.into_iter().unzip();
            let wi = self.get_work_item(work_item_id)?;
            self.append_event(NewEvent {
                project_id: wi.project_id,
                work_item_id: Some(wi.id),
                session_id: None,
                kind: "decisions_reviewed",
                payload: json!({ "pr": pr_number, "ids": ids, "uids": uids }),
            })?;
        }
        tx.commit()?;
        Ok(changed.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::ledger_with_project;
    use ct_core::{DecisionOrigin, WorkItemKind};

    fn nd(work_item_id: i64, origin: DecisionOrigin, title: &str) -> NewDecision {
        NewDecision {
            work_item_id,
            session_id: None,
            origin,
            title: title.into(),
            context: "ctx".into(),
            decision: "dec".into(),
            consequences: None,
        }
    }

    #[test]
    fn autonoma_nasce_pendente_e_orientada_revisada() {
        let (l, p) = ledger_with_project();
        let wi = l.ensure_work_item(p.id, 12, WorkItemKind::Epic).unwrap();
        let a = l
            .record_decision(&nd(wi.id, DecisionOrigin::Autonoma, "a"))
            .unwrap();
        let o = l
            .record_decision(&nd(wi.id, DecisionOrigin::Orientada, "o"))
            .unwrap();
        assert!(a.reviewed_at.is_none());
        assert_eq!(o.reviewed_at, Some(o.created_at));
        let pend = l.list_decisions(&[wi.id], true).unwrap();
        assert_eq!(
            pend.iter().map(|d| d.title.as_str()).collect::<Vec<_>>(),
            vec!["a"]
        );
        assert_eq!(l.list_decisions(&[wi.id], false).unwrap().len(), 2);
    }

    #[test]
    fn lista_varios_work_items() {
        let (l, p) = ledger_with_project();
        let w1 = l.ensure_work_item(p.id, 12, WorkItemKind::Epic).unwrap();
        let w2 = l.ensure_work_item(p.id, 13, WorkItemKind::Issue).unwrap();
        l.record_decision(&nd(w1.id, DecisionOrigin::Autonoma, "1"))
            .unwrap();
        l.record_decision(&nd(w2.id, DecisionOrigin::Autonoma, "2"))
            .unwrap();
        assert_eq!(l.list_decisions(&[w1.id, w2.id], true).unwrap().len(), 2);
        assert!(l.list_decisions(&[], true).unwrap().is_empty());
    }

    #[test]
    fn marca_revisadas_sem_pr() {
        let (l, p) = ledger_with_project();
        let wi = l.ensure_work_item(p.id, 12, WorkItemKind::Epic).unwrap();
        let a = l
            .record_decision(&nd(wi.id, DecisionOrigin::Autonoma, "a"))
            .unwrap();
        assert_eq!(l.mark_reviewed(&[a.id], None).unwrap(), 1);
        let d = l.get_decision(a.id).unwrap();
        assert!(d.reviewed_at.is_some());
        assert_eq!(d.reviewed_in_pr, None);
        let ev = l.list_events_of_kind(wi.id, "decisions_reviewed").unwrap();
        assert_eq!(ev[0].payload["pr"], serde_json::Value::Null);
    }

    #[test]
    fn marca_revisadas_uma_vez() {
        let (l, p) = ledger_with_project();
        let wi = l.ensure_work_item(p.id, 12, WorkItemKind::Epic).unwrap();
        let a = l
            .record_decision(&nd(wi.id, DecisionOrigin::Autonoma, "a"))
            .unwrap();
        let o = l
            .record_decision(&nd(wi.id, DecisionOrigin::Orientada, "o"))
            .unwrap();
        assert_eq!(l.mark_reviewed(&[a.id, o.id], Some(31)).unwrap(), 1);
        assert_eq!(l.mark_reviewed(&[a.id], Some(32)).unwrap(), 0);
        assert_eq!(l.get_decision(a.id).unwrap().reviewed_in_pr, Some(31));
        assert!(l.mark_reviewed(&[], Some(1)).unwrap() == 0);
    }

    fn eventos_de_revisao(l: &Ledger, work_item_id: i64) -> Vec<serde_json::Value> {
        l.list_events(work_item_id, 100)
            .unwrap()
            .into_iter()
            .filter(|e| e.kind == "decisions_reviewed")
            .map(|e| e.payload)
            .collect()
    }

    #[test]
    fn revisao_gera_um_evento_por_work_item() {
        let (l, p) = ledger_with_project();
        let w1 = l.ensure_work_item(p.id, 12, WorkItemKind::Epic).unwrap();
        let w2 = l.ensure_work_item(p.id, 13, WorkItemKind::Issue).unwrap();
        let a1 = l
            .record_decision(&nd(w1.id, DecisionOrigin::Autonoma, "a1"))
            .unwrap();
        let b1 = l
            .record_decision(&nd(w1.id, DecisionOrigin::Autonoma, "b1"))
            .unwrap();
        let o1 = l
            .record_decision(&nd(w1.id, DecisionOrigin::Orientada, "o1"))
            .unwrap();
        let a2 = l
            .record_decision(&nd(w2.id, DecisionOrigin::Autonoma, "a2"))
            .unwrap();

        assert_eq!(
            l.mark_reviewed(&[b1.id, a2.id, o1.id, a1.id], Some(31))
                .unwrap(),
            3
        );
        let ev = eventos_de_revisao(&l, w1.id);
        assert_eq!(
            ev,
            vec![json!({ "pr": 31, "ids": [a1.id, b1.id], "uids": [a1.uid, b1.uid] })]
        );
        let ev = eventos_de_revisao(&l, w2.id);
        assert_eq!(
            ev,
            vec![json!({ "pr": 31, "ids": [a2.id], "uids": [a2.uid] })]
        );
        let e = &l.list_events(w1.id, 1).unwrap()[0];
        assert_eq!(e.project_id, p.id);
        assert_eq!(e.work_item_id, Some(w1.id));
    }

    #[test]
    fn revisao_sem_mudanca_nao_gera_evento() {
        let (l, p) = ledger_with_project();
        let wi = l.ensure_work_item(p.id, 12, WorkItemKind::Epic).unwrap();
        let a = l
            .record_decision(&nd(wi.id, DecisionOrigin::Autonoma, "a"))
            .unwrap();
        let o = l
            .record_decision(&nd(wi.id, DecisionOrigin::Orientada, "o"))
            .unwrap();
        l.mark_reviewed(&[a.id], Some(31)).unwrap();
        assert_eq!(l.mark_reviewed(&[a.id, o.id], Some(32)).unwrap(), 0);
        assert_eq!(l.mark_reviewed(&[9999], Some(33)).unwrap(), 0);
        assert_eq!(eventos_de_revisao(&l, wi.id).len(), 1);
    }

    #[test]
    fn conteudo_da_decisao_e_imutavel() {
        let (l, p) = ledger_with_project();
        let wi = l.ensure_work_item(p.id, 12, WorkItemKind::Epic).unwrap();
        let a = l
            .record_decision(&nd(wi.id, DecisionOrigin::Autonoma, "a"))
            .unwrap();
        assert!(l
            .conn
            .execute("UPDATE decision SET title = 'x' WHERE id = ?1", [a.id])
            .is_err());
        assert!(l
            .conn
            .execute("DELETE FROM decision WHERE id = ?1", [a.id])
            .is_err());
    }
}
