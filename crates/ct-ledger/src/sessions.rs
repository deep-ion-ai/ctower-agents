use crate::{json_col, parse_col, Ledger, LedgerError, NewEvent, Result};
use ct_core::{now_ms, SessionMode, SessionState, Tool};
use rusqlite::{params, OptionalExtension, Row};
use serde::Serialize;
use serde_json::json;

pub struct NewSession {
    pub work_item_id: i64,
    pub tool: Tool,
    pub cwd: String,
    pub launch_cmd: Vec<String>,
    /// Modelo pedido (`--model`); `None` = padrão da ferramenta.
    pub model: Option<String>,
    pub mode: SessionMode,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Session {
    pub id: String,
    pub work_item_id: i64,
    pub tool: Tool,
    pub tool_session_id: Option<String>,
    pub transcript_path: Option<String>,
    pub backup_path: Option<String>,
    pub cwd: String,
    pub launch_cmd: Vec<String>,
    pub state: SessionState,
    pub exit_code: Option<i32>,
    pub pid: Option<i64>,
    pub started_at: i64,
    pub last_event_at: i64,
    pub ended_at: Option<i64>,
    /// Modelo pedido ao iniciar; `None` = padrão da ferramenta.
    pub model: Option<String>,
    pub mode: SessionMode,
    /// Id da sessão no Remote Control (`session_…`, link
    /// `claude.ai/code/<id>`); `None` = nunca ligou (issue #197).
    pub rc_session_id: Option<String>,
}

const COLS: &str = "id, work_item_id, tool, tool_session_id, transcript_path, backup_path, cwd, \
                    launch_cmd, state, exit_code, pid, started_at, last_event_at, ended_at, \
                    model, mode, rc_session_id";

fn row_to_session(r: &Row) -> rusqlite::Result<Session> {
    Ok(Session {
        id: r.get(0)?,
        work_item_id: r.get(1)?,
        tool: parse_col(r, 2)?,
        tool_session_id: r.get(3)?,
        transcript_path: r.get(4)?,
        backup_path: r.get(5)?,
        cwd: r.get(6)?,
        launch_cmd: json_col(r, 7)?,
        state: parse_col(r, 8)?,
        exit_code: r.get(9)?,
        pid: r.get(10)?,
        started_at: r.get(11)?,
        last_event_at: r.get(12)?,
        ended_at: r.get(13)?,
        model: r.get(14)?,
        mode: parse_col(r, 15)?,
        rc_session_id: r.get(16)?,
    })
}

impl Ledger {
    pub fn create_session(&self, s: &NewSession) -> Result<Session> {
        let wi = self.get_work_item(s.work_item_id)?;
        let id = uuid::Uuid::new_v4().to_string();
        let launch_cmd = serde_json::to_string(&s.launch_cmd).expect("Vec<String> serializa");
        let tx = self.conn.unchecked_transaction()?;
        self.conn.execute(
            "INSERT INTO session (id, work_item_id, tool, cwd, launch_cmd, state, started_at, last_event_at,
                                  model, mode)
             VALUES (?1, ?2, ?3, ?4, ?5, 'starting', ?6, ?6, ?7, ?8)",
            params![
                id,
                s.work_item_id,
                s.tool.as_str(),
                s.cwd,
                launch_cmd,
                now_ms(),
                s.model,
                s.mode.as_str()
            ],
        )?;
        self.enqueue_session(&id)?;
        self.append_event(NewEvent {
            project_id: wi.project_id,
            work_item_id: Some(wi.id),
            session_id: Some(&id),
            kind: "session_created",
            payload: json!({ "tool": s.tool, "cwd": s.cwd, "model": s.model, "mode": s.mode }),
        })?;
        tx.commit()?;
        self.get_session(&id)
    }

    pub fn get_session(&self, id: &str) -> Result<Session> {
        self.conn
            .query_row(
                &format!("SELECT {COLS} FROM session WHERE id = ?1"),
                [id],
                row_to_session,
            )
            .optional()?
            .ok_or_else(|| LedgerError::NotFound("session", id.to_string()))
    }

    fn update_session(&self, id: &str, sql: &str, value: &dyn rusqlite::ToSql) -> Result<()> {
        self.update_session_with(id, sql, params![value, id])
    }

    /// Executa o UPDATE da sessão e põe a fotografia nova no outbox, juntos.
    fn update_session_with(&self, id: &str, sql: &str, args: impl rusqlite::Params) -> Result<()> {
        self.atomic(|| {
            let n = self.conn.execute(sql, args)?;
            if n == 0 {
                return Err(LedgerError::NotFound("session", id.to_string()));
            }
            self.enqueue_session(id)
        })
    }

    pub fn set_session_pid(&self, id: &str, pid: i64) -> Result<()> {
        self.update_session(id, "UPDATE session SET pid = ?1 WHERE id = ?2", &pid)
    }

    pub fn set_session_backup_path(&self, id: &str, path: &str) -> Result<()> {
        self.update_session(
            id,
            "UPDATE session SET backup_path = ?1 WHERE id = ?2",
            &path,
        )
    }

    pub fn set_session_tool_info(
        &self,
        id: &str,
        tool_session_id: &str,
        transcript_path: &str,
    ) -> Result<()> {
        self.update_session_with(
            id,
            "UPDATE session SET tool_session_id = ?1, transcript_path = ?2 WHERE id = ?3",
            params![tool_session_id, transcript_path, id],
        )
    }

    /// Grava o id da sessão no Remote Control (issue #197) e põe a
    /// fotografia no outbox. Devolve `false` sem escrever se já era esse.
    pub fn set_session_rc_id(&self, id: &str, rc_session_id: &str) -> Result<bool> {
        let s = self.get_session(id)?;
        if s.rc_session_id.as_deref() == Some(rc_session_id) {
            return Ok(false);
        }
        self.update_session(
            id,
            "UPDATE session SET rc_session_id = ?1 WHERE id = ?2",
            &rc_session_id,
        )?;
        Ok(true)
    }

    /// Grava o comando de lançamento definitivo e o id que a ferramenta vai
    /// usar (`claude --session-id`), conhecidos só depois de criar a sessão.
    pub fn set_session_launch(
        &self,
        id: &str,
        launch_cmd: &[String],
        tool_session_id: Option<&str>,
    ) -> Result<()> {
        let launch_cmd = serde_json::to_string(launch_cmd).expect("Vec<String> serializa");
        self.update_session_with(
            id,
            "UPDATE session SET launch_cmd = ?1,
                    tool_session_id = COALESCE(?2, tool_session_id)
             WHERE id = ?3",
            params![launch_cmd, tool_session_id, id],
        )
    }

    /// Sessões de um work item, da mais antiga para a mais recente.
    pub fn list_sessions(&self, work_item_id: i64) -> Result<Vec<Session>> {
        self.query_sessions("work_item_id = ?1", &[&work_item_id])
    }

    /// Leva a sessão ao estado `to`, gravando um evento `session_state`.
    ///
    /// Retorna `Ok(true)` se o estado mudou e `Ok(false)` se nada mudou (a
    /// sessão já estava em `to`, inclusive quando outro processo a levou até
    /// lá entre a leitura e a escrita). Se o estado atual não pode ir para
    /// `to`, retorna `LedgerError::InvalidTransition`.
    pub fn transition_session(
        &self,
        id: &str,
        to: SessionState,
        exit_code: Option<i32>,
    ) -> Result<bool> {
        let s = self.get_session(id)?;
        self.apply_transition(s, to, exit_code)
    }

    /// Arquiva a sessão (`interrupted`/`exited` → `archived`). Com `resumed_as`,
    /// grava na mesma transação o evento `resumed_as` apontando a sessão nova.
    pub fn archive_session(&self, id: &str, resumed_as: Option<&str>) -> Result<bool> {
        let s = self.get_session(id)?;
        let extra = resumed_as.map(|new| ("resumed_as", json!({ "new_session_id": new })));
        self.apply_transition_with(s, SessionState::Archived, None, extra)
    }

    /// Aplica a transição a partir de `s`, uma leitura da sessão que pode já
    /// estar velha: se outro processo mudou o estado depois dela, a sessão é
    /// relida e a decisão é refeita sobre o estado atual.
    fn apply_transition(
        &self,
        s: Session,
        to: SessionState,
        exit_code: Option<i32>,
    ) -> Result<bool> {
        self.apply_transition_with(s, to, exit_code, None)
    }

    /// `apply_transition` com um evento extra gravado na mesma transação.
    fn apply_transition_with(
        &self,
        mut s: Session,
        to: SessionState,
        exit_code: Option<i32>,
        extra: Option<(&str, serde_json::Value)>,
    ) -> Result<bool> {
        loop {
            // Estritamente crescente: duas mudanças no mesmo ms empatariam o
            // `last_event_at` e o hub aceitaria a cópia velha sobre a nova.
            let now = now_ms().max(s.last_event_at + 1);
            if s.state == to {
                // Só sessões vivas recebem sinal; em `exited`/`interrupted`/`archived`
                // o `last_event_at` guarda quando a sessão parou e não pode mudar.
                if to.is_live() {
                    self.atomic(|| {
                        let n = self.conn.execute(
                            "UPDATE session SET last_event_at = ?1 WHERE id = ?2 AND state = ?3",
                            params![now, s.id, s.state.as_str()],
                        )?;
                        if n > 0 {
                            self.enqueue_session(&s.id)?;
                        }
                        Ok(())
                    })?;
                }
                return Ok(false);
            }
            if !s.state.can_transition_to(to) {
                return Err(LedgerError::InvalidTransition { from: s.state, to });
            }
            let (last_event_at, ended_at) = match to {
                SessionState::Interrupted => (s.last_event_at, Some(s.last_event_at)),
                SessionState::Exited => (now, Some(now)),
                _ => (now, s.ended_at),
            };
            let wi = self.get_work_item(s.work_item_id)?;
            let tx = self.conn.unchecked_transaction()?;
            // `state = ?6` protege contra outro processo ter mudado a sessão
            // depois da leitura: nesse caso nada é gravado e a sessão é relida.
            let n = self.conn.execute(
                "UPDATE session SET state = ?1, exit_code = COALESCE(?2, exit_code),
                        last_event_at = ?3, ended_at = ?4
                 WHERE id = ?5 AND state = ?6",
                params![
                    to.as_str(),
                    exit_code,
                    last_event_at,
                    ended_at,
                    s.id,
                    s.state.as_str()
                ],
            )?;
            if n == 0 {
                drop(tx);
                s = self.get_session(&s.id)?;
                continue;
            }
            self.enqueue_session(&s.id)?;
            let mut payload = json!({ "from": s.state, "to": to, "exit_code": exit_code });
            if to == SessionState::Interrupted {
                payload["last_event_at"] = json!(s.last_event_at);
            }
            self.append_event(NewEvent {
                project_id: wi.project_id,
                work_item_id: Some(wi.id),
                session_id: Some(&s.id),
                kind: "session_state",
                payload,
            })?;
            if let Some((kind, payload)) = extra {
                self.append_event(NewEvent {
                    project_id: wi.project_id,
                    work_item_id: Some(wi.id),
                    session_id: Some(&s.id),
                    kind,
                    payload,
                })?;
            }
            tx.commit()?;
            return Ok(true);
        }
    }

    pub fn live_sessions(&self) -> Result<Vec<Session>> {
        self.query_sessions("state IN ('starting', 'running', 'idle')", &[])
    }

    /// Sessões de qualquer work item com o `cwd` dado (as de um worktree),
    /// da mais antiga para a mais recente.
    pub fn sessions_in_cwd(&self, cwd: &str) -> Result<Vec<Session>> {
        self.query_sessions("cwd = ?1", &[&cwd])
    }

    pub fn sessions_in_state(&self, state: SessionState) -> Result<Vec<Session>> {
        self.query_sessions("state = ?1", &[&state.as_str()])
    }

    /// Sessões que terminaram com código ≠ 0 a partir de `since_ms` (morte por
    /// sinal, como `session.kill`, não conta).
    pub fn failed_sessions_since(&self, since_ms: i64) -> Result<Vec<Session>> {
        self.query_sessions(
            "state = 'exited' AND exit_code IS NOT NULL AND exit_code <> 0 AND ended_at >= ?1",
            &[&since_ms],
        )
    }

    /// Sessões descartadas (arquivadas sem retomada) a partir de
    /// `since_ms`, da mais recente para a mais antiga, até `limit`. Na
    /// sessão arquivada, `last_event_at` é a hora do arquivamento.
    pub fn discarded_sessions_since(&self, since_ms: i64, limit: u32) -> Result<Vec<Session>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {COLS} FROM session s
             WHERE state = 'archived' AND last_event_at >= ?1
               AND NOT EXISTS (SELECT 1 FROM event e
                               WHERE e.session_id = s.id AND e.type = 'resumed_as')
             ORDER BY last_event_at DESC LIMIT ?2"
        ))?;
        let rows = stmt.query_map(params![since_ms, limit], row_to_session)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    fn query_sessions(&self, filter: &str, args: &[&dyn rusqlite::ToSql]) -> Result<Vec<Session>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {COLS} FROM session WHERE {filter} ORDER BY started_at"
        ))?;
        let rows = stmt.query_map(args, row_to_session)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Marca como `interrupted` toda sessão viva cujo PID não passa em
    /// `is_alive` (ou que nunca registrou PID). Retorna os ids afetados. Usado
    /// no boot do daemon, com os PIDs dos PTYs que ele mesmo tem.
    pub fn mark_interrupted(&self, is_alive: &dyn Fn(i64) -> bool) -> Result<Vec<String>> {
        let mut ids = Vec::new();
        for s in self.live_sessions()? {
            if !s.pid.is_some_and(is_alive)
                && self.transition_session(&s.id, SessionState::Interrupted, None)?
            {
                ids.push(s.id);
            }
        }
        Ok(ids)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::ledger_with_project;
    use crate::LedgerError;
    use ct_core::{SessionState, Tool, WorkItemKind};

    fn new_session(l: &Ledger, project_id: i64) -> Session {
        let wi = l
            .ensure_work_item(project_id, 12, WorkItemKind::Epic)
            .unwrap();
        l.create_session(&NewSession {
            work_item_id: wi.id,
            tool: Tool::Claude,
            cwd: "/w/demo/wip__12-auth".into(),
            launch_cmd: vec!["claude".into(), "prompt".into()],
            model: None,
            mode: ct_core::SessionMode::Native,
        })
        .unwrap()
    }

    #[test]
    fn cria_sessao_em_starting() {
        let (l, p) = ledger_with_project();
        let s = new_session(&l, p.id);
        assert_eq!(s.state, SessionState::Starting);
        assert_eq!(s.launch_cmd, vec!["claude", "prompt"]);
        assert_eq!(l.get_session(&s.id).unwrap(), s);
    }

    #[test]
    fn arquivar_com_resumed_as_grava_os_dois_eventos() {
        let (l, p) = ledger_with_project();
        let s = new_session(&l, p.id);
        // starting não vai direto para archived
        assert!(matches!(
            l.archive_session(&s.id, None),
            Err(LedgerError::InvalidTransition { .. })
        ));
        l.mark_interrupted(&|_| false).unwrap();
        assert!(l.archive_session(&s.id, Some("nova")).unwrap());
        assert_eq!(l.get_session(&s.id).unwrap().state, SessionState::Archived);
        let evs = l.list_events(s.work_item_id, 10).unwrap();
        assert_eq!(evs[0].kind, "resumed_as");
        assert_eq!(evs[0].payload["new_session_id"], "nova");
        assert_eq!(evs[1].kind, "session_state");
        assert_eq!(evs[1].payload["to"], "archived");
        // Repetir não muda nada nem duplica eventos.
        assert!(!l.archive_session(&s.id, Some("nova")).unwrap());
        assert_eq!(l.list_events(s.work_item_id, 10).unwrap().len(), evs.len());
    }

    #[test]
    fn grava_lancamento_e_lista_por_work_item() {
        let (l, p) = ledger_with_project();
        let s = new_session(&l, p.id);
        let cmd = vec!["claude".to_string(), "--session-id".into(), "u-1".into()];
        l.set_session_launch(&s.id, &cmd, Some("u-1")).unwrap();
        let got = l.get_session(&s.id).unwrap();
        assert_eq!(got.launch_cmd, cmd);
        assert_eq!(got.tool_session_id.as_deref(), Some("u-1"));
        // None não apaga o id já gravado
        l.set_session_launch(&s.id, &cmd, None).unwrap();
        assert_eq!(
            l.get_session(&s.id).unwrap().tool_session_id.as_deref(),
            Some("u-1")
        );
        let s2 = new_session(&l, p.id);
        let ids: Vec<_> = l
            .list_sessions(s.work_item_id)
            .unwrap()
            .into_iter()
            .map(|s| s.id)
            .collect();
        assert_eq!(ids, vec![s.id.clone(), s2.id]);
        assert!(matches!(
            l.set_session_launch("nao-existe", &cmd, None),
            Err(LedgerError::NotFound(..))
        ));
    }

    #[test]
    fn transicoes_e_info_da_ferramenta() {
        let (l, p) = ledger_with_project();
        let s = new_session(&l, p.id);
        l.set_session_pid(&s.id, 4242).unwrap();
        l.set_session_tool_info(&s.id, "abc-123", "/home/u/.claude/projects/x/abc-123.jsonl")
            .unwrap();
        assert!(l
            .transition_session(&s.id, SessionState::Running, None)
            .unwrap());
        assert!(!l
            .transition_session(&s.id, SessionState::Running, None)
            .unwrap());
        assert!(l
            .transition_session(&s.id, SessionState::Exited, Some(1))
            .unwrap());
        let s = l.get_session(&s.id).unwrap();
        assert_eq!(s.pid, Some(4242));
        assert_eq!(s.tool_session_id.as_deref(), Some("abc-123"));
        assert_eq!(s.exit_code, Some(1));
        assert!(s.ended_at.is_some());
    }

    #[test]
    fn sessoes_com_erro_desde() {
        let (l, p) = ledger_with_project();
        let sai = |code: Option<i32>| {
            let s = new_session(&l, p.id);
            l.transition_session(&s.id, SessionState::Exited, code)
                .unwrap();
            s.id
        };
        let erro = sai(Some(1));
        sai(Some(0));
        sai(None);
        new_session(&l, p.id);
        let ids: Vec<_> = l
            .failed_sessions_since(0)
            .unwrap()
            .into_iter()
            .map(|s| s.id)
            .collect();
        assert_eq!(ids, vec![erro]);
        assert!(l.failed_sessions_since(i64::MAX).unwrap().is_empty());
    }

    #[test]
    fn transicao_invalida_e_erro() {
        let (l, p) = ledger_with_project();
        let s = new_session(&l, p.id);
        l.transition_session(&s.id, SessionState::Exited, Some(0))
            .unwrap();
        let err = l
            .transition_session(&s.id, SessionState::Running, None)
            .unwrap_err();
        assert!(matches!(err, LedgerError::InvalidTransition { .. }));
    }

    #[test]
    fn mark_interrupted_so_afeta_vivas_sem_processo() {
        let (l, p) = ledger_with_project();
        let viva = new_session(&l, p.id);
        l.set_session_pid(&viva.id, 1).unwrap();
        l.transition_session(&viva.id, SessionState::Running, None)
            .unwrap();
        let morta = new_session(&l, p.id);
        l.set_session_pid(&morta.id, 2).unwrap();
        l.transition_session(&morta.id, SessionState::Idle, None)
            .unwrap();
        let sem_pid = new_session(&l, p.id);
        let encerrada = new_session(&l, p.id);
        l.transition_session(&encerrada.id, SessionState::Exited, Some(0))
            .unwrap();

        let antes = l.get_session(&morta.id).unwrap().last_event_at;
        let mut ids = l.mark_interrupted(&|pid| pid == 1).unwrap();
        ids.sort();
        let mut esperado = vec![morta.id.clone(), sem_pid.id.clone()];
        esperado.sort();
        assert_eq!(ids, esperado);

        let m = l.get_session(&morta.id).unwrap();
        assert_eq!(m.state, SessionState::Interrupted);
        assert_eq!(m.last_event_at, antes);
        assert_eq!(m.ended_at, Some(antes));
        assert_eq!(
            l.get_session(&viva.id).unwrap().state,
            SessionState::Running
        );
        assert_eq!(
            l.get_session(&encerrada.id).unwrap().state,
            SessionState::Exited
        );
    }

    #[test]
    fn mark_interrupted_repetido_nao_duplica() {
        let (l, p) = ledger_with_project();
        let s = new_session(&l, p.id);
        assert_eq!(l.mark_interrupted(&|_| false).unwrap().len(), 1);
        assert!(l.mark_interrupted(&|_| false).unwrap().is_empty());
        let wi = l.get_session(&s.id).unwrap().work_item_id;
        let n = l
            .list_events(wi, 100)
            .unwrap()
            .iter()
            .filter(|e| e.kind == "session_state" && e.payload["to"] == "interrupted")
            .count();
        assert_eq!(n, 1);
        assert_eq!(
            l.sessions_in_state(SessionState::Interrupted)
                .unwrap()
                .len(),
            1
        );
    }

    /// Simula uma sessão cujo último sinal foi há muito tempo, para que
    /// "preservar o horário" não passe por coincidência de relógio.
    fn envelhece(l: &Ledger, id: &str, ts: i64) {
        l.conn
            .execute(
                "UPDATE session SET last_event_at = ?1 WHERE id = ?2",
                params![ts, id],
            )
            .unwrap();
    }

    fn eventos_de_estado(l: &Ledger, s: &Session) -> usize {
        l.list_events(s.work_item_id, 1000)
            .unwrap()
            .iter()
            .filter(|e| e.kind == "session_state" && e.session_id.as_deref() == Some(&s.id))
            .count()
    }

    #[test]
    fn mark_interrupted_repetido_preserva_ultimo_sinal() {
        let (l, p) = ledger_with_project();
        let s = new_session(&l, p.id);
        l.transition_session(&s.id, SessionState::Running, None)
            .unwrap();
        envelhece(&l, &s.id, 1_000);
        assert_eq!(l.mark_interrupted(&|_| false).unwrap(), vec![s.id.clone()]);
        assert!(l.mark_interrupted(&|_| false).unwrap().is_empty());
        let s2 = l.get_session(&s.id).unwrap();
        assert_eq!(s2.state, SessionState::Interrupted);
        assert_eq!(s2.last_event_at, 1_000);
        assert_eq!(s2.ended_at, Some(1_000));
    }

    #[test]
    fn reinterromper_nao_altera_horario_da_parada() {
        let (l, p) = ledger_with_project();
        let s = new_session(&l, p.id);
        envelhece(&l, &s.id, 1_000);
        assert!(l
            .transition_session(&s.id, SessionState::Interrupted, None)
            .unwrap());
        assert!(!l
            .transition_session(&s.id, SessionState::Interrupted, None)
            .unwrap());
        let s2 = l.get_session(&s.id).unwrap();
        assert_eq!(s2.last_event_at, 1_000);
        assert_eq!(s2.ended_at, Some(1_000));
        assert_eq!(eventos_de_estado(&l, &s), 1);
    }

    #[test]
    fn evento_de_interrupcao_traz_ultimo_sinal() {
        let (l, p) = ledger_with_project();
        let s = new_session(&l, p.id);
        l.transition_session(&s.id, SessionState::Running, None)
            .unwrap();
        envelhece(&l, &s.id, 1_000);
        l.transition_session(&s.id, SessionState::Interrupted, None)
            .unwrap();
        let ev = &l.list_events(s.work_item_id, 1).unwrap()[0];
        assert_eq!(ev.kind, "session_state");
        assert_eq!(ev.payload["to"], "interrupted");
        assert_eq!(ev.payload["last_event_at"], 1_000);
    }

    /// `snapshot` é a leitura feita antes de outro processo mudar a sessão.
    fn muda_por_fora(l: &Ledger, s: &Session, para: SessionState) -> Session {
        let snapshot = l.get_session(&s.id).unwrap();
        l.transition_session(&s.id, para, Some(0)).unwrap();
        snapshot
    }

    #[test]
    fn corrida_para_o_mesmo_estado_nao_muda_nada() {
        let (l, p) = ledger_with_project();
        let s = new_session(&l, p.id);
        let velho = muda_por_fora(&l, &s, SessionState::Interrupted);
        let antes = l.get_session(&s.id).unwrap();
        assert!(!l
            .apply_transition(velho, SessionState::Interrupted, None)
            .unwrap());
        assert_eq!(l.get_session(&s.id).unwrap(), antes);
        assert_eq!(eventos_de_estado(&l, &s), 1);
    }

    #[test]
    fn corrida_para_estado_inalcancavel_e_erro() {
        let (l, p) = ledger_with_project();
        let s = new_session(&l, p.id);
        let velho = muda_por_fora(&l, &s, SessionState::Exited);
        let err = l
            .apply_transition(velho, SessionState::Running, None)
            .unwrap_err();
        assert!(matches!(
            err,
            LedgerError::InvalidTransition {
                from: SessionState::Exited,
                to: SessionState::Running
            }
        ));
        assert_eq!(eventos_de_estado(&l, &s), 1);
    }

    #[test]
    fn corrida_para_estado_ainda_alcancavel_aplica_a_partir_do_atual() {
        let (l, p) = ledger_with_project();
        let s = new_session(&l, p.id);
        let velho = muda_por_fora(&l, &s, SessionState::Running);
        assert!(l.apply_transition(velho, SessionState::Idle, None).unwrap());
        assert_eq!(l.get_session(&s.id).unwrap().state, SessionState::Idle);
        let ev = &l.list_events(s.work_item_id, 1).unwrap()[0];
        assert_eq!(ev.payload["from"], "running");
        assert_eq!(ev.payload["to"], "idle");
    }

    #[test]
    fn mark_interrupted_nao_toca_exited_nem_archived() {
        let (l, p) = ledger_with_project();
        let exited = new_session(&l, p.id);
        l.transition_session(&exited.id, SessionState::Exited, Some(0))
            .unwrap();
        let archived = new_session(&l, p.id);
        l.transition_session(&archived.id, SessionState::Interrupted, None)
            .unwrap();
        l.transition_session(&archived.id, SessionState::Archived, None)
            .unwrap();
        let antes_e = l.get_session(&exited.id).unwrap();
        let antes_a = l.get_session(&archived.id).unwrap();
        let ev_e = eventos_de_estado(&l, &exited);
        let ev_a = eventos_de_estado(&l, &archived);

        assert!(l.mark_interrupted(&|_| false).unwrap().is_empty());
        assert!(l.mark_interrupted(&|_| false).unwrap().is_empty());

        assert_eq!(l.get_session(&exited.id).unwrap(), antes_e);
        assert_eq!(l.get_session(&archived.id).unwrap(), antes_a);
        assert_eq!(eventos_de_estado(&l, &exited), ev_e);
        assert_eq!(eventos_de_estado(&l, &archived), ev_a);
    }

    /// Dois processos fazendo a recuperação ao mesmo tempo (ex.: daemon
    /// reiniciado duas vezes em sequência rápida): a sessão é interrompida uma
    /// vez só, com um único evento, e só quem a interrompeu a reporta.
    #[test]
    fn mark_interrupted_concorrente_nao_duplica() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ledger.db");
        let l1 = Ledger::open(&path, "env-a").unwrap();
        let p = l1
            .add_project(&crate::NewProject {
                name: "demo".into(),
                gh_owner: "o".into(),
                gh_repo: "r".into(),
                repo_path: "/x".into(),
                base_branch: "develop".into(),
                protected_branches: vec!["develop".into()],
            })
            .unwrap();
        let s = new_session(&l1, p.id);
        l1.set_session_pid(&s.id, 7).unwrap();
        envelhece(&l1, &s.id, 1_000);
        let l2 = Ledger::open(&path, "env-a").unwrap();

        // l1 já listou as sessões vivas; durante a checagem do PID, l2 faz a
        // própria recuperação e interrompe a mesma sessão antes de l1.
        let de_l2 = std::cell::RefCell::new(Vec::new());
        let de_l1 = l1
            .mark_interrupted(&|_| {
                de_l2
                    .borrow_mut()
                    .extend(l2.mark_interrupted(&|_| false).unwrap());
                false
            })
            .unwrap();

        assert_eq!(de_l2.into_inner(), vec![s.id.clone()]);
        assert!(de_l1.is_empty());
        let s2 = l1.get_session(&s.id).unwrap();
        assert_eq!(s2.state, SessionState::Interrupted);
        assert_eq!(s2.last_event_at, 1_000);
        assert_eq!(eventos_de_estado(&l1, &s), 1);
    }
}
