use crate::{json_col, new_uid, Ledger, Result};
use ct_core::now_ms;
use rusqlite::{params, OptionalExtension, Row};
use serde::Serialize;

pub struct NewEvent<'a> {
    pub project_id: i64,
    pub work_item_id: Option<i64>,
    pub session_id: Option<&'a str>,
    pub kind: &'a str,
    pub payload: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Event {
    pub id: i64,
    /// Identidade global (UUIDv7 ou determinística, `gh:...`).
    pub uid: String,
    pub ts: i64,
    pub project_id: i64,
    pub work_item_id: Option<i64>,
    pub session_id: Option<String>,
    pub kind: String,
    pub payload: serde_json::Value,
}

fn row_to_event(r: &Row) -> rusqlite::Result<Event> {
    Ok(Event {
        id: r.get(0)?,
        ts: r.get(1)?,
        project_id: r.get(2)?,
        work_item_id: r.get(3)?,
        session_id: r.get(4)?,
        kind: r.get(5)?,
        payload: json_col(r, 6)?,
        uid: r.get(7)?,
    })
}

impl Ledger {
    /// Grava o evento com `uid`, `env_id` e HLC, e o põe no outbox na mesma transação.
    pub fn append_event(&self, e: NewEvent<'_>) -> Result<i64> {
        self.append_event_uid(e, None)
    }

    /// Como `append_event`, com `uid` determinístico (eventos derivados do
    /// GitHub, para o hub guardar um só quando vários ambientes percebem a
    /// mesma mudança). Se o uid já existe aqui (a mesma transição de novo,
    /// ex.: PR reaberto), recebe o sufixo `:2`, `:3`...
    pub(crate) fn append_event_uid(&self, e: NewEvent<'_>, uid: Option<&str>) -> Result<i64> {
        let uid = match uid {
            None => new_uid(),
            Some(base) => self.free_uid(base)?,
        };
        self.atomic(|| {
            let hlc = self.tick_hlc()?;
            self.conn.execute(
                "INSERT INTO event (ts, project_id, work_item_id, session_id, type, payload,
                                    uid, env_id, hlc)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    now_ms(),
                    e.project_id,
                    e.work_item_id,
                    e.session_id,
                    e.kind,
                    e.payload.to_string(),
                    uid,
                    self.env_id,
                    hlc
                ],
            )?;
            let id = self.conn.last_insert_rowid();
            self.enqueue(&self.event_change(id)?)?;
            Ok(id)
        })
    }

    /// Primeiro `uid` livre para `base`: aqui e entre os já podados
    /// (`pruned_uid`), cujo maior sufixo o hub já recebeu.
    fn free_uid(&self, base: &str) -> Result<String> {
        let pruned: Option<i64> = self
            .conn
            .query_row(
                "SELECT max_n FROM pruned_uid WHERE base = ?1",
                [base],
                |r| r.get(0),
            )
            .optional()?;
        let mut k = pruned.map_or(1, |n| n + 1);
        let mut uid = if k == 1 {
            base.to_string()
        } else {
            format!("{base}:{k}")
        };
        while self.conn.query_row(
            "SELECT EXISTS (SELECT 1 FROM event WHERE uid = ?1)",
            [&uid],
            |r| r.get::<_, bool>(0),
        )? {
            k += 1;
            uid = format!("{base}:{k}");
        }
        Ok(uid)
    }

    /// Eventos do work item, do mais recente para o mais antigo.
    pub fn list_events(&self, work_item_id: i64, limit: u32) -> Result<Vec<Event>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, ts, project_id, work_item_id, session_id, type, payload, uid
             FROM event WHERE work_item_id = ?1 ORDER BY id DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![work_item_id, limit], row_to_event)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Página da linha do tempo do work item (`events.list`): do mais recente
    /// para o mais antigo, só os de id menor que `before` quando há. O id
    /// cresce com a inserção, então ele serve de cursor estável.
    pub fn list_events_before(
        &self,
        work_item_id: i64,
        before: Option<i64>,
        limit: u32,
    ) -> Result<Vec<Event>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, ts, project_id, work_item_id, session_id, type, payload, uid
             FROM event WHERE work_item_id = ?1 AND (?2 IS NULL OR id < ?2)
             ORDER BY id DESC LIMIT ?3",
        )?;
        let rows = stmt.query_map(params![work_item_id, before, limit], row_to_event)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Eventos de um tipo no work item, do mais recente para o mais antigo.
    pub fn list_events_of_kind(&self, work_item_id: i64, kind: &str) -> Result<Vec<Event>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, ts, project_id, work_item_id, session_id, type, payload, uid
             FROM event WHERE work_item_id = ?1 AND type = ?2 ORDER BY id DESC",
        )?;
        let rows = stmt.query_map(params![work_item_id, kind], row_to_event)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::ledger_with_project;
    use ct_core::WorkItemKind;
    use serde_json::json;

    #[test]
    fn append_e_lista_do_mais_recente() {
        let (l, p) = ledger_with_project();
        let wi = l.ensure_work_item(p.id, 12, WorkItemKind::Epic).unwrap();
        l.append_event(NewEvent {
            project_id: p.id,
            work_item_id: Some(wi.id),
            session_id: None,
            kind: "a",
            payload: json!({"n": 1}),
        })
        .unwrap();
        l.append_event(NewEvent {
            project_id: p.id,
            work_item_id: Some(wi.id),
            session_id: None,
            kind: "b",
            payload: json!({}),
        })
        .unwrap();
        let evs = l.list_events(wi.id, 10).unwrap();
        assert_eq!(evs[0].kind, "b");
        assert_eq!(evs[1].kind, "a");
        assert_eq!(evs[1].payload, json!({"n": 1}));
        // +1 do evento work_item_created
        assert_eq!(evs.len(), 3);
        let a = l.list_events_of_kind(wi.id, "a").unwrap();
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].payload, json!({"n": 1}));
    }

    #[test]
    fn pagina_por_before() {
        let (l, p) = ledger_with_project();
        let wi = l.ensure_work_item(p.id, 12, WorkItemKind::Issue).unwrap();
        let outro = l.ensure_work_item(p.id, 13, WorkItemKind::Issue).unwrap();
        for (w, k) in [(wi.id, "a"), (outro.id, "x"), (wi.id, "b"), (wi.id, "c")] {
            l.append_event(NewEvent {
                project_id: p.id,
                work_item_id: Some(w),
                session_id: None,
                kind: k,
                payload: json!({}),
            })
            .unwrap();
        }
        let kinds = |evs: &[Event]| evs.iter().map(|e| e.kind.clone()).collect::<Vec<_>>();
        let p1 = l.list_events_before(wi.id, None, 2).unwrap();
        assert_eq!(kinds(&p1), ["c", "b"]);
        let p2 = l.list_events_before(wi.id, Some(p1[1].id), 2).unwrap();
        // O evento do outro item fica de fora; o último é o work_item_created.
        assert_eq!(kinds(&p2), ["a", "work_item_created"]);
        let p3 = l.list_events_before(wi.id, Some(p2[1].id), 2).unwrap();
        assert!(p3.is_empty());
    }
}
