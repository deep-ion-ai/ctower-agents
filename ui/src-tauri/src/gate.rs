//! Allowlist dos métodos que a WebView chama pela ponte (#231).
//!
//! `daemon_call` e `hub_call` são o "call genérico": aceitam um nome de
//! método vindo do JavaScript. Por isso só passam o que está aqui:
//!
//! - ambiente: os métodos do registro (`ct_http::methods`) com o escopo
//!   [`Scope::Ui`], menos os [`SENSITIVE_ENV`], que só chegam pelo comando
//!   dedicado da tabela;
//! - hub: [`HUB_METHODS`], lista de hoje (o hub ainda não tem registro
//!   tipado; ver o corpo do PR de #231).
//!
//! Fora da lista a ponte devolve [`METHOD_NOT_FOUND`], o mesmo código que o
//! daemon dá para método desconhecido, e o método nem chega ao cliente.

use crate::bridge::{Bridge, BridgeError};
use ct_http::methods::{allowed_in, Scope};
use serde_json::Value;

/// `-32601`: método fora do que a ponte aceita (como o daemon, para desconhecido).
pub const METHOD_NOT_FOUND: i64 = -32601;

/// Métodos do ambiente que nunca passam pelo `daemon_call`: segredo, shell,
/// configuração, instalação, limpeza, aprovação de permissão ou escrita no
/// GitHub. Cada um tem um comando Tauri dedicado (nome à direita), que chama
/// o método com o nome fixo. Testes em `tests` conferem a tabela com o
/// registro e com `lib.rs`.
pub const SENSITIVE_ENV: &[(&str, &str)] = &[
    ("github.pr.comment", "github_pr_comment"),
    ("env.update", "env_update"),
    ("env.rollback", "env_rollback"),
    ("env.config.set", "env_config_set"),
    ("env.connections.set", "env_connections_set"),
    ("shell.open", "shell_open"),
    ("shell.close", "shell_close"),
    ("session.input", "session_input"),
    ("session.permission.respond", "session_permission_respond"),
    ("cleanup.run", "cleanup_run"),
];

/// Métodos do hub que a UI desktop chama, achados nas chamadas `hubCall` de
/// `ui/src` e conferidos contra os despachos do `cthubd`. Excluídos de
/// propósito: `hub.env.call` (leva qualquer método de ambiente sem passar
/// por esta allowlist) e os nomes de evento (`*.changed`, `hub.logs.lines`).
/// Sem sensíveis separados ainda: ver o corpo do PR.
pub const HUB_METHODS: &[&str] = &[
    "hub.assignments.add",
    "hub.assignments.remove",
    "hub.autopilot.deps.set",
    "hub.autopilot.digest",
    "hub.autopilot.pause",
    "hub.autopilot.policy.get",
    "hub.autopilot.policy.preview",
    "hub.autopilot.policy.set",
    "hub.autopilot.resume",
    "hub.autopilot.set",
    "hub.autopilot.status",
    "hub.autopilot.wip",
    "hub.backup.get",
    "hub.backup.run",
    "hub.backup.set",
    "hub.backup.test",
    "hub.dashboard.get",
    "hub.envs.list",
    "hub.envs.set_autonomous",
    "hub.health.get",
    "hub.info",
    "hub.logs.follow",
    "hub.logs.tail",
    "hub.logs.unfollow",
    "hub.plan.accounts",
    "hub.projects.add",
    "hub.projects.delete",
    "hub.projects.impact",
    "hub.projects.list",
    "hub.projects.move",
    "hub.projects.update",
    "hub.proposals.decide",
    "hub.proposals.list",
    "hub.push.list",
    "hub.push.remove",
    "hub.push.subscribe",
    "hub.push.test",
    "hub.push.update",
    "hub.push.vapid_key",
    "hub.quarantine.discard",
    "hub.ranking.get",
    "hub.recovery.generate",
    "hub.recovery.status",
    "hub.recovery.verify",
    "hub.release.status",
    "hub.rules.get",
    "hub.rules.history",
    "hub.rules.import",
    "hub.rules.proposals.decide",
    "hub.rules.proposals.list",
    "hub.rules.set",
    "hub.secrets.delete",
    "hub.secrets.list",
    "hub.secrets.rotate_pending_dismiss",
    "hub.secrets.set",
    "hub.status",
    "hub.sync.now",
    "hub.tracker.add_issue",
    "hub.tree.get",
    "hub.update.check",
    "hub.update.get",
    "hub.update.rollback",
    "hub.update.start",
    "hub.update.status",
    "hub.web.sessions.list",
    "hub.web.sessions.revoke",
    "hub.workitem.body",
    "hub.workitem.history",
    "hub.workitem.move",
    "hub.workitem.set_model",
    "hub.workitem.set_priority",
    "hub.workitem.set_status",
];

fn refused(message: String) -> BridgeError {
    BridgeError {
        code: METHOD_NOT_FOUND,
        message,
        data: None,
    }
}

/// Comando dedicado de um método sensível do ambiente, se ele for um.
pub fn sensitive_command(method: &str) -> Option<&'static str> {
    SENSITIVE_ENV
        .iter()
        .find(|(m, _)| *m == method)
        .map(|(_, cmd)| *cmd)
}

/// `Ok` se o `daemon_call` pode levar `method` ao ambiente.
pub fn check_env(method: &str) -> Result<(), BridgeError> {
    if let Some(cmd) = sensitive_command(method) {
        return Err(refused(format!(
            "método sensível: use o comando dedicado `{cmd}`"
        )));
    }
    if !allowed_in(method, Scope::Ui) {
        return Err(refused(format!("método fora da allowlist da UI: {method}")));
    }
    Ok(())
}

/// `Ok` se o `hub_call` pode levar `method` ao hub.
pub fn check_hub(method: &str) -> Result<(), BridgeError> {
    if HUB_METHODS.contains(&method) {
        Ok(())
    } else {
        Err(refused(format!(
            "método fora da allowlist do hub: {method}"
        )))
    }
}

/// O `daemon_call`: confere a allowlist e só então chama o ambiente.
pub async fn env_generic(
    bridge: &Bridge,
    env: &str,
    method: &str,
    params: Value,
) -> Result<Value, BridgeError> {
    check_env(method)?;
    bridge.call(env, method, params).await
}

/// O `hub_call`: confere a allowlist e só então chama o hub.
pub async fn hub_generic(
    bridge: &Bridge,
    hub_id: &str,
    method: &str,
    params: Value,
) -> Result<Value, BridgeError> {
    check_hub(method)?;
    bridge.call(hub_id, method, params).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::tests::Collect;
    use crate::bridge::Resolver;
    use crate::envs::UiConfig;
    use serde_json::json;
    use std::collections::BTreeSet;
    use std::sync::Arc;

    const LIB_RS: &str = include_str!("lib.rs");

    fn bridge() -> Arc<Bridge> {
        let resolve: Resolver = Arc::new(|_| Err("sem token".into()));
        Bridge::new(
            Arc::new(Collect::default()),
            resolve,
            UiConfig::default().notify,
        )
    }

    /// Ambiente que não existe: se a chamada chegar à ponte, a resposta é
    /// `INVALID_PARAMS` ("ambiente desconhecido"). Recusa = `METHOD_NOT_FOUND`.
    const NO_ENV: &str = "nao-existe";

    #[tokio::test]
    async fn metodo_fora_da_allowlist_e_recusado_na_ponte() {
        let b = bridge();
        for m in ["metodo.inexistente", "hub.env.call", "hub.secrets.list"] {
            let e = env_generic(&b, NO_ENV, m, json!({})).await.unwrap_err();
            assert_eq!(e.code, METHOD_NOT_FOUND, "{m}: {e:?}");
        }
    }

    #[tokio::test]
    async fn metodo_permitido_chega_ao_ambiente() {
        let b = bridge();
        let e = env_generic(&b, NO_ENV, "env.info", json!({}))
            .await
            .unwrap_err();
        assert_eq!(e.code, crate::bridge::BRIDGE_INVALID_PARAMS, "{e:?}");
    }

    #[tokio::test]
    async fn metodos_sensiveis_nao_passam_pelo_generico() {
        let b = bridge();
        for (m, cmd) in SENSITIVE_ENV {
            let e = env_generic(&b, NO_ENV, m, json!({})).await.unwrap_err();
            assert_eq!(e.code, METHOD_NOT_FOUND, "{m} passou pelo genérico");
            assert!(e.message.contains(cmd), "{m}: {}", e.message);
        }
    }

    #[tokio::test]
    async fn hub_fora_da_lista_e_recusado_e_a_da_lista_passa() {
        let b = bridge();
        let e = hub_generic(&b, "hub", "hub.env.call", json!({}))
            .await
            .unwrap_err();
        assert_eq!(e.code, METHOD_NOT_FOUND);
        let e = hub_generic(&b, "hub", "hub.secrets.list", json!({}))
            .await
            .unwrap_err();
        assert_eq!(e.code, crate::bridge::BRIDGE_INVALID_PARAMS, "{e:?}");
    }

    #[test]
    fn sensiveis_sao_do_registro_e_tem_comando_registrado() {
        let mut seen = BTreeSet::new();
        for (m, cmd) in SENSITIVE_ENV {
            assert!(seen.insert(*m), "repetido: {m}");
            assert!(
                ct_http::methods::find(m).is_some_and(|s| s.scopes.contains(&Scope::Ui)),
                "{m} não é método de UI do registro"
            );
            assert!(
                LIB_RS.contains(&format!("fn {cmd}(")),
                "{cmd} não é comando em lib.rs"
            );
            assert!(
                LIB_RS.contains(&format!("            {cmd},\n")),
                "{cmd} não está em generate_handler!"
            );
        }
    }

    #[test]
    fn escopo_de_ui_vale_no_generico_exceto_sensiveis() {
        assert!(check_env("projects.list").is_ok());
        assert!(check_env("env.info").is_ok());
        assert!(check_env("sync.now").is_ok());
        assert!(check_env("shell.list").is_ok());
        assert_eq!(check_env("env.update").unwrap_err().code, METHOD_NOT_FOUND);
    }

    #[test]
    fn lista_do_hub_sem_repetidos_bypass_nem_eventos() {
        let mut seen = BTreeSet::new();
        for m in HUB_METHODS {
            assert!(seen.insert(*m), "repetido: {m}");
            assert!(m.starts_with("hub."), "{m}");
            assert!(!m.ends_with(".changed"), "evento na lista: {m}");
        }
        assert!(!HUB_METHODS.contains(&"hub.env.call"));
        assert!(!HUB_METHODS.contains(&"hub.logs.lines"));
    }
}
