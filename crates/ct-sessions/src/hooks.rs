//! Aplicação dos hooks do Claude Code ao ledger (spec §7.2) e spool offline.

use crate::transcripts::is_volatile;
use crate::{Result, Sessions};
use ct_core::{SessionState, WorkItemTrigger};
use ct_ledger::{LedgerError, NewEvent, Session};
use serde::Deserialize;
use serde_json::{json, Value};
use std::fs;
use std::path::Path;

/// Corpo enviado por `ctower hook` (POST em `/hooks` ou arquivo no spool).
#[derive(Debug, Clone, Deserialize)]
pub struct HookEnvelope {
    pub session: String,
    pub event: String,
    #[serde(default)]
    pub payload: Value,
    #[serde(default)]
    pub ts: i64,
    /// `CLAUDE_CODE_BRIDGE_SESSION_ID` do processo do agente: id da sessão
    /// no Remote Control (#197), fora do `payload`. Ausente = desligado ou
    /// `ctower` anterior.
    #[serde(default)]
    pub rc_session_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookOutcome {
    Applied,
    /// Sessão não existe neste daemon.
    UnknownSession,
    /// Evento desconhecido ou fora de ordem (ex.: hook atrasado de sessão encerrada).
    Ignored,
}

impl Sessions {
    pub fn apply_hook(&self, h: &HookEnvelope) -> Result<HookOutcome> {
        let s = match self.ledger().get_session(&h.session) {
            Ok(s) => s,
            Err(LedgerError::NotFound(..)) => return Ok(HookOutcome::UnknownSession),
            Err(e) => return Err(e.into()),
        };
        self.hook_rc_id(&s, h)?;
        let str_field = |k: &str| h.payload.get(k).and_then(Value::as_str);
        let outcome = match h.event.as_str() {
            "SessionStart" => {
                if let Some(transcript) = str_field("transcript_path") {
                    let tool_id = str_field("session_id")
                        .or(s.tool_session_id.as_deref())
                        .unwrap_or_default()
                        .to_string();
                    self.ledger()
                        .set_session_tool_info(&s.id, &tool_id, transcript)?;
                    if is_volatile(Path::new(transcript)) {
                        self.event(&s, "transcript_volatile", json!({ "path": transcript }))?;
                        self.notify(
                            "alert",
                            json!({ "kind": "transcript_volatile", "session_id": s.id, "path": transcript }),
                        );
                    }
                }
                self.transition(&s, SessionState::Running)?
            }
            "UserPromptSubmit" => {
                let o = self.transition(&s, SessionState::Running)?;
                if o == HookOutcome::Applied {
                    self.trigger(&s, WorkItemTrigger::SessionActive)?;
                }
                o
            }
            "Notification" => {
                let message = str_field("message").unwrap_or_default();
                // Mesmo estado: só registra a hora do pedido em `last_event_at`.
                self.transition(&s, s.state)?;
                self.event(&s, "needs_human", json!({ "message": message }))?;
                self.trigger(&s, WorkItemTrigger::NeedsHuman)?;
                self.notify(
                    "needs_human",
                    json!({ "session_id": s.id, "work_item_id": s.work_item_id, "message": message }),
                );
                HookOutcome::Applied
            }
            "Stop" => {
                let o = self.transition(&s, SessionState::Idle)?;
                if o == HookOutcome::Applied {
                    let s = self.ledger().get_session(&s.id)?;
                    if s.transcript_path.is_some() {
                        if let Err(e) = self.backup_session(&s.id) {
                            self.backup_failed(&s, &e)?;
                        }
                    }
                }
                o
            }
            "SessionEnd" => self.transition(&s, SessionState::Exited)?,
            "GuardBlocked" => {
                self.event(
                    &s,
                    "guard_blocked",
                    json!({ "command": h.payload.get("command"), "reason": h.payload.get("reason") }),
                )?;
                HookOutcome::Applied
            }
            _ => HookOutcome::Ignored,
        };
        Ok(outcome)
    }

    /// Pedido de permissão do Claude Code (hook `PermissionRequest`, #328):
    /// registra o evento e põe o work item em `aguardando_voce`. Devolve a
    /// sessão e o `tool_input` com os secrets da sessão mascarados, para a
    /// UI mostrar; `None` se a sessão não existe ou já não está viva.
    pub fn permission_requested(&self, h: &HookEnvelope) -> Result<Option<(Session, Value)>> {
        let s = match self.ledger().get_session(&h.session) {
            Ok(s) if s.state.is_live() => s,
            Ok(_) | Err(LedgerError::NotFound(..)) => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        self.hook_rc_id(&s, h)?;
        let tool = h.payload.get("tool_name").cloned().unwrap_or(Value::Null);
        self.transition(&s, s.state)?;
        self.event(&s, "permission_request", json!({ "tool_name": tool }))?;
        self.trigger(&s, WorkItemTrigger::NeedsHuman)?;
        let input = h.payload.get("tool_input").cloned().unwrap_or(Value::Null);
        let masked = self.transcript_mask(&s).apply(input.to_string().as_bytes());
        let input = serde_json::from_slice(&masked).unwrap_or(Value::Null);
        Ok(Some((s, input)))
    }

    /// Fim do pedido de permissão: `decision` é a resposta da UI (`allow`,
    /// `allow_session`, `deny`, `ask`) ou `expired`. Com resposta de
    /// permitir ou negar o agente segue, e o work item volta a
    /// `em_execucao`; com `ask`/`expired` o pedido continua no terminal.
    pub fn permission_resolved(&self, session_id: &str, decision: &str) -> Result<()> {
        let s = match self.ledger().get_session(session_id) {
            Ok(s) => s,
            Err(LedgerError::NotFound(..)) => return Ok(()),
            Err(e) => return Err(e.into()),
        };
        self.event(&s, "permission_resolved", json!({ "decision": decision }))?;
        if matches!(decision, "allow" | "allow_session" | "deny") && s.state.is_live() {
            self.trigger(&s, WorkItemTrigger::SessionActive)?;
        }
        Ok(())
    }

    /// Qualquer evento com `rc_session_id` grava o id quando ele mudou.
    fn hook_rc_id(&self, s: &Session, h: &HookEnvelope) -> Result<()> {
        match h.rc_session_id.as_deref().map(str::trim) {
            Some(id) if !id.is_empty() => self.rc_id_from_hook(s, id),
            _ => Ok(()),
        }
    }

    fn transition(&self, s: &Session, to: SessionState) -> Result<HookOutcome> {
        match self.ledger().transition_session(&s.id, to, None) {
            Ok(true) => {
                self.notify(
                    "session_state",
                    json!({ "session_id": s.id, "work_item_id": s.work_item_id, "state": to }),
                );
                Ok(HookOutcome::Applied)
            }
            Ok(false) => Ok(HookOutcome::Applied),
            Err(LedgerError::InvalidTransition { .. }) => Ok(HookOutcome::Ignored),
            Err(e) => Err(e.into()),
        }
    }

    fn trigger(&self, s: &Session, t: WorkItemTrigger) -> Result<()> {
        let changed = self.ledger().apply_trigger(s.work_item_id, t)?;
        if changed {
            let wi = self.ledger().get_work_item(s.work_item_id)?;
            self.notify(
                "work_item_status",
                json!({ "work_item_id": wi.id, "status": wi.status }),
            );
        }
        Ok(())
    }

    fn event(&self, s: &Session, kind: &str, payload: Value) -> Result<()> {
        let l = self.ledger();
        let wi = l.get_work_item(s.work_item_id)?;
        l.append_event(NewEvent {
            project_id: wi.project_id,
            work_item_id: Some(wi.id),
            session_id: Some(&s.id),
            kind,
            payload,
        })?;
        Ok(())
    }

    /// Processa o spool de hooks (gravado quando o daemon estava fora do ar),
    /// em ordem de nome (= ordem de gravação). Arquivos ilegíveis vão para
    /// `hooks-spool/bad/`. Retorna quantos foram aplicados ou descartados.
    pub fn drain_spool(&self) -> Result<usize> {
        let dir = self.config().dirs.spool();
        let mut files: Vec<_> = match fs::read_dir(&dir) {
            Ok(rd) => rd
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| {
                    p.is_file()
                        && p.extension().is_some_and(|e| e == "json")
                        && !p
                            .file_name()
                            .is_some_and(|n| n.to_string_lossy().starts_with('.'))
                })
                .collect(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
            Err(e) => return Err(e.into()),
        };
        files.sort();
        let mut n = 0;
        for f in files {
            let parsed = fs::read_to_string(&f)
                .ok()
                .and_then(|s| serde_json::from_str::<HookEnvelope>(&s).ok());
            match parsed {
                Some(h) => {
                    self.apply_hook(&h)?;
                    fs::remove_file(&f)?;
                }
                None => {
                    let bad = dir.join("bad");
                    fs::create_dir_all(&bad)?;
                    fs::rename(&f, bad.join(f.file_name().unwrap()))?;
                }
            }
            n += 1;
        }
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::*;
    use ct_core::WorkItemStatus;

    fn hook(session: &str, event: &str, payload: Value) -> HookEnvelope {
        HookEnvelope {
            session: session.into(),
            event: event.into(),
            payload,
            ts: 0,
            rc_session_id: None,
        }
    }

    fn started(env: &TestEnv) -> Session {
        env.sessions.start(env.request(1, "A")).unwrap()
    }

    #[test]
    fn ciclo_completo_de_hooks() {
        let env = TestEnv::new(FakeClaude::sleep(30));
        let s = started(&env);
        let transcript = env.tmp.path().join("t.jsonl");
        std::fs::write(&transcript, "x\n").unwrap();
        let ap = |e: &str, p: Value| env.sessions.apply_hook(&hook(&s.id, e, p)).unwrap();

        assert_eq!(
            ap(
                "SessionStart",
                json!({ "session_id": "uuid-9", "transcript_path": transcript })
            ),
            HookOutcome::Applied
        );
        let got = env.sessions.ledger().get_session(&s.id).unwrap();
        assert_eq!(got.state, SessionState::Running);
        assert_eq!(got.tool_session_id.as_deref(), Some("uuid-9"));

        // O pedido marca o último sinal (a UI mostra "aguardando há N min").
        let antes = env.sessions.ledger().get_session(&s.id).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(5));
        ap("Notification", json!({ "message": "Permissão para Bash" }));
        let depois = env.sessions.ledger().get_session(&s.id).unwrap();
        assert!(depois.last_event_at > antes.last_event_at);
        assert_eq!(depois.state, SessionState::Running);
        let wi = |env: &TestEnv| {
            env.sessions
                .ledger()
                .get_work_item(s.work_item_id)
                .unwrap()
                .status
        };
        assert_eq!(wi(&env), WorkItemStatus::AguardandoVoce);

        ap("Stop", json!({}));
        let got = env.sessions.ledger().get_session(&s.id).unwrap();
        assert_eq!(got.state, SessionState::Idle);
        assert!(got.backup_path.is_some(), "Stop faz backup");

        ap("UserPromptSubmit", json!({ "prompt": "continue" }));
        assert_eq!(
            env.sessions.ledger().get_session(&s.id).unwrap().state,
            SessionState::Running
        );
        assert_eq!(wi(&env), WorkItemStatus::EmExecucao);

        ap(
            "GuardBlocked",
            json!({ "command": "git push origin develop", "reason": "push para develop" }),
        );
        ap("SessionEnd", json!({ "reason": "exit" }));
        assert_eq!(
            env.sessions.ledger().get_session(&s.id).unwrap().state,
            SessionState::Exited
        );
        // Hook atrasado depois do fim é ignorado, sem erro.
        assert_eq!(ap("Stop", json!({})), HookOutcome::Ignored);

        let kinds: Vec<_> = env
            .sessions
            .ledger()
            .list_events(s.work_item_id, 200)
            .unwrap()
            .into_iter()
            .map(|e| e.kind)
            .collect();
        assert!(kinds.contains(&"needs_human".to_string()));
        assert!(kinds.contains(&"guard_blocked".to_string()));
        let names: Vec<_> = env.notices().into_iter().map(|(n, _)| n).collect();
        assert!(names.contains(&"needs_human".to_string()));
        assert!(names.contains(&"session_state".to_string()));
        env.sessions.kill(&s.id).unwrap();
    }

    /// O id do Remote Control vem no envelope de qualquer hook (#197): grava
    /// na sessão (e no outbox, para o hub), avisa a UI só quando muda e
    /// vence o que o PTY tinha gravado.
    #[test]
    fn hook_grava_o_rc_session_id() {
        let env = TestEnv::new(FakeClaude::sleep(30));
        let s = started(&env);
        env.sessions
            .ledger()
            .set_session_rc_id(&s.id, "session_doPty")
            .unwrap();
        let mut h = hook(&s.id, "Stop", json!({}));
        h.rc_session_id = Some("session_01Hook".into());
        env.sessions.apply_hook(&h).unwrap();
        let got = env.sessions.ledger().get_session(&s.id).unwrap();
        assert_eq!(got.rc_session_id.as_deref(), Some("session_01Hook"));
        let outbox = env.sessions.ledger().outbox_pending(10_000).unwrap();
        assert!(outbox.iter().any(|e| matches!(&e.change,
            ct_ledger::SyncChange::Session(c) if c.rc_session_id.as_deref() == Some("session_01Hook"))));
        h.event = "UserPromptSubmit".into();
        env.sessions.apply_hook(&h).unwrap();
        // Envelope do `ctower` antigo, sem o campo: o id fica.
        env.sessions
            .apply_hook(&hook(&s.id, "Stop", json!({})))
            .unwrap();
        let got = env.sessions.ledger().get_session(&s.id).unwrap();
        assert_eq!(got.rc_session_id.as_deref(), Some("session_01Hook"));
        let avisos = env
            .notices()
            .into_iter()
            .filter(|(n, _)| n == "session_remote_control")
            .count();
        assert_eq!(avisos, 1);
        let json: HookEnvelope = serde_json::from_value(
            json!({ "session": s.id, "event": "Stop", "rc_session_id": "session_01J" }),
        )
        .unwrap();
        assert_eq!(json.rc_session_id.as_deref(), Some("session_01J"));
        env.sessions.kill(&s.id).unwrap();
    }

    #[test]
    fn transcript_em_tmp_gera_alerta() {
        let env = TestEnv::new(FakeClaude::sleep(30));
        let s = started(&env);
        env.sessions
            .apply_hook(&hook(
                &s.id,
                "SessionStart",
                json!({ "session_id": "u", "transcript_path": "/tmp/x/t.jsonl" }),
            ))
            .unwrap();
        assert!(env
            .notices()
            .iter()
            .any(|(n, d)| n == "alert" && d["kind"] == "transcript_volatile"));
        env.sessions.kill(&s.id).unwrap();
    }

    #[test]
    fn sessao_desconhecida_e_evento_desconhecido() {
        let env = TestEnv::new(FakeClaude::sleep(30));
        assert_eq!(
            env.sessions
                .apply_hook(&hook("nao-existe", "Stop", json!({})))
                .unwrap(),
            HookOutcome::UnknownSession
        );
        let s = started(&env);
        assert_eq!(
            env.sessions
                .apply_hook(&hook(&s.id, "Inventado", json!({})))
                .unwrap(),
            HookOutcome::Ignored
        );
        env.sessions.kill(&s.id).unwrap();
    }

    #[test]
    fn spool_aplica_em_ordem_e_separa_invalidos() {
        let env = TestEnv::new(FakeClaude::sleep(30));
        let s = started(&env);
        let spool = env.cfg.dirs.spool();
        let w = |name: &str, body: String| std::fs::write(spool.join(name), body).unwrap();
        w(
            "100-1.json",
            json!({ "session": s.id, "event": "SessionStart", "payload": {}, "ts": 100 })
                .to_string(),
        );
        w(
            "200-1.json",
            json!({ "session": s.id, "event": "Stop", "payload": {}, "ts": 200 }).to_string(),
        );
        w("150-1.json", "{ quebrado".into());
        w(".300-1.json.tmp", "em escrita".into());

        assert_eq!(env.sessions.drain_spool().unwrap(), 3);
        assert_eq!(
            env.sessions.ledger().get_session(&s.id).unwrap().state,
            SessionState::Idle
        );
        assert!(spool.join("bad/150-1.json").is_file());
        assert!(spool.join(".300-1.json.tmp").is_file());
        assert!(!spool.join("100-1.json").exists());
        assert_eq!(env.sessions.drain_spool().unwrap(), 0);
        env.sessions.kill(&s.id).unwrap();
    }
}
