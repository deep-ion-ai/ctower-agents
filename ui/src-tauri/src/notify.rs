//! Notificações nativas (spec §10): só aguardando você, saída com erro e
//! ambiente desconectado, cada uma com a sua preferência. Disparadas no lado
//! Rust para funcionar com a janela minimizada.

use crate::envs::NotifyPrefs;
use ct_client::ClientEvent;
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub title: String,
    pub body: String,
}

fn notice(title: &str, body: String) -> Option<Notice> {
    Some(Notice {
        title: title.into(),
        body,
    })
}

/// Notificação para o evento, se houver. `was_connected` = estado anterior
/// do ambiente (`None` antes da primeira conexão: falhar ao abrir o app não
/// notifica, a janela já mostra).
pub fn rule(
    prefs: &NotifyPrefs,
    env_name: &str,
    ev: &ClientEvent,
    was_connected: Option<bool>,
) -> Option<Notice> {
    match ev {
        ClientEvent::Push { event, data } if event == "needs_human" && prefs.needs_human => {
            let msg = data
                .get("message")
                .and_then(Value::as_str)
                .filter(|m| !m.trim().is_empty())
                .unwrap_or("Permissão ou pergunta pendente no terminal");
            notice("Sessão aguardando você", format!("{env_name}: {msg}"))
        }
        ClientEvent::Push { event, data } if event == "session_exited" && prefs.session_error => {
            match data.get("exit_code").and_then(Value::as_i64) {
                Some(code) if code != 0 => notice(
                    "Sessão saiu com erro",
                    format!("{env_name}: o processo terminou com código {code}"),
                ),
                _ => None,
            }
        }
        ClientEvent::Status {
            connected: false,
            error,
        } if prefs.env_down && was_connected == Some(true) => notice(
            "Ambiente desconectado",
            match error {
                Some(e) => format!("{env_name}: {e}. Reconectando…"),
                None => format!("{env_name}: reconectando…"),
            },
        ),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn all() -> NotifyPrefs {
        NotifyPrefs {
            needs_human: true,
            session_error: true,
            env_down: true,
            muted_envs: Vec::new(),
        }
    }

    fn push(event: &str, data: Value) -> ClientEvent {
        ClientEvent::Push {
            event: event.into(),
            data,
        }
    }

    fn down() -> ClientEvent {
        ClientEvent::Status {
            connected: false,
            error: Some("conexão perdida".into()),
        }
    }

    #[test]
    fn aguardando_voce_com_a_mensagem_do_claude() {
        let n = rule(
            &all(),
            "WSL",
            &push(
                "needs_human",
                json!({ "message": "Claude needs your permission to use Bash" }),
            ),
            Some(true),
        )
        .unwrap();
        assert_eq!(n.title, "Sessão aguardando você");
        assert_eq!(n.body, "WSL: Claude needs your permission to use Bash");
        let n = rule(&all(), "WSL", &push("needs_human", json!({})), Some(true)).unwrap();
        assert!(n.body.contains("pendente"));
    }

    #[test]
    fn saida_so_com_codigo_diferente_de_zero() {
        let n = |code: Value| {
            rule(
                &all(),
                "WSL",
                &push("session_exited", json!({ "exit_code": code })),
                Some(true),
            )
        };
        assert!(n(json!(1)).unwrap().body.contains("código 1"));
        assert_eq!(n(json!(0)), None);
        assert_eq!(
            n(Value::Null),
            None,
            "morte por sinal (encerrar) não notifica"
        );
    }

    #[test]
    fn queda_so_depois_de_ter_conectado() {
        assert!(rule(&all(), "mac", &down(), Some(true)).is_some());
        assert_eq!(rule(&all(), "mac", &down(), None), None);
        assert_eq!(rule(&all(), "mac", &down(), Some(false)), None);
        let up = ClientEvent::Status {
            connected: true,
            error: None,
        };
        assert_eq!(rule(&all(), "mac", &up, Some(false)), None);
    }

    #[test]
    fn preferencias_desligam_cada_tipo() {
        let off = NotifyPrefs {
            needs_human: false,
            session_error: false,
            env_down: false,
            muted_envs: Vec::new(),
        };
        assert_eq!(
            rule(&off, "x", &push("needs_human", json!({})), Some(true)),
            None
        );
        assert_eq!(
            rule(
                &off,
                "x",
                &push("session_exited", json!({ "exit_code": 2 })),
                Some(true)
            ),
            None
        );
        assert_eq!(rule(&off, "x", &down(), Some(true)), None);
    }

    #[test]
    fn outros_eventos_nao_notificam() {
        assert_eq!(
            rule(&all(), "x", &push("session_state", json!({})), Some(true)),
            None
        );
        let out = ClientEvent::Output {
            session: "s".into(),
            bytes: vec![],
            snapshot: false,
        };
        assert_eq!(rule(&all(), "x", &out, Some(true)), None);
    }
}
