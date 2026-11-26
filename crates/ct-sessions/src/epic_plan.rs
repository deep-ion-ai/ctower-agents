//! Sessão de plano da execução de épico (#299, épico #295): uma sessão do
//! projeto (#0, no clone principal, sem branch) com o prompt que o hub
//! renderizou, marcada com o evento [`EPIC_PLAN`] e o número do épico. Só
//! ela vê a ferramenta `epic_set_plan`, que grava o plano no hub.

use crate::{Result, Sessions, SessionsError};
use ct_ledger::{NewEvent, Session};
use serde_json::json;

/// Evento que marca a sessão de plano (payload: `session_id`, `epic`).
pub const EPIC_PLAN: &str = "epic_plan";

impl Sessions {
    /// Inicia a sessão de plano do épico `epic` do projeto.
    pub fn start_epic_plan(
        &self,
        project_id: i64,
        epic: i64,
        prompt: &str,
        size: Option<crate::launch::PtySize>,
        agent: &crate::launch::AgentOptions,
    ) -> Result<Session> {
        if epic <= 0 {
            return Err(SessionsError::Invalid(format!("épico inválido: {epic}")));
        }
        if prompt.trim().is_empty() {
            return Err(SessionsError::Invalid(
                "a sessão de plano precisa do prompt".into(),
            ));
        }
        let s = self.start_project(project_id, prompt, size, agent)?;
        self.ledger().append_event(NewEvent {
            project_id,
            work_item_id: Some(s.work_item_id),
            session_id: Some(&s.id),
            kind: EPIC_PLAN,
            payload: json!({ "session_id": s.id, "epic": epic }),
        })?;
        Ok(s)
    }

    /// O épico da sessão de plano; `None` = não é sessão de plano.
    pub fn epic_plan_of(&self, s: &Session) -> Result<Option<i64>> {
        Ok(self
            .ledger()
            .list_events_of_kind(s.work_item_id, EPIC_PLAN)?
            .iter()
            .filter(|e| e.session_id.as_deref() == Some(s.id.as_str()))
            .find_map(|e| e.payload.get("epic").and_then(|v| v.as_i64())))
    }
}

#[cfg(test)]
mod tests {
    use crate::test_support::{FakeClaude, TestEnv};
    use crate::SessionsError;

    #[test]
    fn sessao_de_plano_marcada_com_o_epico() {
        let env = TestEnv::new(FakeClaude::sleep(30));
        let agent = crate::AgentOptions::default();
        let s = env
            .sessions
            .start_epic_plan(env.project_id, 10, "planeje o épico", None, &agent)
            .unwrap();
        assert_eq!(env.sessions.epic_plan_of(&s).unwrap(), Some(10));
        let outra = env
            .sessions
            .start_project(env.project_id, "outra", None, &agent)
            .unwrap();
        assert_eq!(env.sessions.epic_plan_of(&outra).unwrap(), None);
        assert!(matches!(
            env.sessions
                .start_epic_plan(env.project_id, 10, "  ", None, &agent),
            Err(SessionsError::Invalid(_))
        ));
        assert!(matches!(
            env.sessions
                .start_epic_plan(env.project_id, 0, "x", None, &agent),
            Err(SessionsError::Invalid(_))
        ));
        for id in [&s.id, &outra.id] {
            env.sessions.kill(id).ok();
        }
    }
}
