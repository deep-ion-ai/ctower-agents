//! Limite de trabalho em andamento no início de sessão pelo hub (issue
//! #126, épico #178). O uso e o limite vêm de `ct_ledger::Hub::project_wip`;
//! aqui fica só a recusa do início automático.
//!
//! - `session.start` com `autonomous: true` (o agendador do piloto
//!   automático, #183, ou "Sessão autônoma" na UI) num projeto no limite é
//!   recusado com [`WIP_LIMIT_REACHED`] e o motivo ("limite de trabalho em
//!   andamento (N de M ...)"). Sem `owner` e `repo`, o hub não sabe o
//!   projeto e também recusa.
//! - O início manual nunca é recusado aqui: a UI pede confirmação com o
//!   `HubProjectView.wip` (o app desktop fala direto com o ambiente).

use ct_ledger::Hub;
use serde_json::Value;

/// Erro do hub: o projeto está no limite de trabalho em andamento e o
/// início automático foi recusado (#126). Estável: o agendador o reconhece.
pub const WIP_LIMIT_REACHED: i64 = -32051;

/// Recusa de `method` com `params` pelo limite do projeto; `None` = segue.
/// Só `session.start` automático passa por aqui.
pub fn start_refusal(hub: &Hub, method: &str, params: &Value) -> Option<(i64, String)> {
    if method != "session.start" || params.get("autonomous") != Some(&Value::Bool(true)) {
        return None;
    }
    let field = |k: &str| {
        params
            .get(k)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
    };
    let (Some(owner), Some(repo)) = (field("owner"), field("repo")) else {
        return Some((
            WIP_LIMIT_REACHED,
            "início automático pelo hub precisa de owner e repo para conferir o limite de \
             trabalho em andamento"
                .into(),
        ));
    };
    match hub.wip_usage(owner, repo) {
        Ok(Some(u)) => u.reason().map(|r| {
            (
                WIP_LIMIT_REACHED,
                format!("{owner}/{repo}: {r}; o início automático espera uma vaga"),
            )
        }),
        // Projeto fora do hub: o ambiente responde como sempre.
        Ok(None) => None,
        Err(e) => Some((-32000, e.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ct_core::WipLimits;
    use ct_ledger::{HubProjectPatch, NewHubProject};
    use serde_json::json;

    fn hub_no_limite() -> Hub {
        let hub = Hub::open_in_memory().unwrap();
        hub.add_project(&NewHubProject {
            owner: "acme".into(),
            repo: "acme-web".into(),
            name: "acme-web".into(),
            base_branch: "develop".into(),
            protected_branches: vec!["develop".into()],
        })
        .unwrap();
        hub
    }

    fn limitar(hub: &Hub, sessions: u32) {
        hub.update_project(
            "acme",
            "acme-web",
            &HubProjectPatch {
                wip_limits: Some(WipLimits {
                    max_sessions: Some(sessions),
                    max_prs: None,
                }),
                ..Default::default()
            },
        )
        .unwrap();
    }

    #[test]
    fn recusa_so_o_inicio_automatico_no_limite() {
        let hub = hub_no_limite();
        let auto = json!({ "owner": "acme", "repo": "acme-web", "gh_number": 123,
                           "autonomous": true });
        let manual = json!({ "owner": "acme", "repo": "acme-web", "gh_number": 123 });
        // Sem limite: segue.
        assert_eq!(start_refusal(&hub, "session.start", &auto), None);
        // Com vaga: segue.
        limitar(&hub, 1);
        assert_eq!(start_refusal(&hub, "session.start", &auto), None);

        // Uma sessão viva noutro ambiente enche o limite de 1.
        let push: ct_ledger::OutboxEntry = serde_json::from_value(json!({
            "seq": 1, "created_at": 1,
            "change": { "kind": "session", "data": {
                "id": "s-acme-1", "owner": "acme", "repo": "acme-web", "gh_number": 124,
                "tool": "claude", "tool_session_id": null, "transcript_path": null,
                "cwd": "/tmp/acme-web", "state": "running", "exit_code": null, "pid": null,
                "started_at": 1, "last_event_at": 1, "ended_at": null
            }}
        }))
        .unwrap();
        hub.ingest("env-b", "env-b", &[push]).unwrap();
        let (code, msg) = start_refusal(&hub, "session.start", &auto).unwrap();
        assert_eq!(code, WIP_LIMIT_REACHED);
        assert_eq!(
            msg,
            "acme/acme-web: limite de trabalho em andamento (1 de 1 sessões); o início \
             automático espera uma vaga"
        );
        // O manual segue: a UI pede confirmação.
        assert_eq!(start_refusal(&hub, "session.start", &manual), None);
        assert_eq!(start_refusal(&hub, "env.info", &auto), None);
        let sem_repo = json!({ "project_id": 1, "gh_number": 123, "autonomous": true });
        let (code, msg) = start_refusal(&hub, "session.start", &sem_repo).unwrap();
        assert_eq!(code, WIP_LIMIT_REACHED);
        assert!(msg.contains("owner e repo"), "{msg}");
        let outro = json!({ "owner": "acme", "repo": "outro", "autonomous": true });
        assert_eq!(start_refusal(&hub, "session.start", &outro), None);
    }
}
