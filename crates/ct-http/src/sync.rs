//! Contrato do sync entre ambiente e hub (spec do ledger centralizado §5 e
//! §8; spec do hub como plano de controle §4): o `ctowerd` é o cliente
//! (`ct-server::hub_sync`), o `cthubd` o servidor.

use ct_ledger::{CleanupSummary, DecisionScope, HubDecision, OutboxEntry};
pub use ct_ledger::{ConfigDone, ConfigProject, ConfigRemoved, ConfigResponse, DoneResponse};

use serde::{Deserialize, Serialize};

/// Cabeçalho com o `env_id` do ambiente em `GET /sync/config`. Divergir do
/// `env_id` preso ao token dá `403`, como no push.
pub const ENV_ID_HEADER: &str = "x-ctower-env-id";

/// Cabeçalho com a chave pública age do ambiente (`age1...`) em
/// `GET /sync/config` (issue #184). O hub a guarda pelo `env_id` preso ao
/// token e cifra a entrega de secrets para ela. Sem o cabeçalho (ambiente
/// antigo), o hub não entrega secrets.
pub const ENV_PUBKEY_HEADER: &str = "x-ctower-env-pubkey";

/// Resposta de `GET /sync/config`: a configuração ([`ConfigResponse`]) e,
/// quando ela mudou (ou a chave pública do ambiente mudou desde a última
/// entrega), os secrets e variáveis globais e dos projetos atribuídos.
/// Hub antigo não manda `secrets`; ambiente antigo ignora o campo.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConfigReply {
    #[serde(flatten)]
    pub config: ConfigResponse,
    /// Arquivo age em base64 (`ct_secrets::seal_delivery`), cifrado só para
    /// a chave do ambiente, com o conjunto **completo**. Ausente = o
    /// ambiente mantém o cache que tem.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secrets: Option<String>,
}

/// Tamanho máximo de um lote em `POST /sync/push`.
pub const SYNC_BODY_LIMIT: usize = 32 * 1024 * 1024;

/// Corpo de `POST /sync/push`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PushRequest {
    pub env_id: String,
    pub entries: Vec<OutboxEntry>,
    /// Mudanças que ficam no outbox depois deste lote (`hub.envs.list`).
    #[serde(default)]
    pub pending: i64,
    /// Resumo do último plano de limpeza do ambiente (issue #287). Ausente =
    /// ambiente antigo, ou o job de verificação ainda não rodou; o hub mantém
    /// o que tinha.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cleanup: Option<CleanupSummary>,
}

/// Resposta de `POST /sync/push`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PushResponse {
    pub last_seq: i64,
}

/// Corpo de `POST /sync/query/decisions` (MCP local → hub).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecisionsQuery {
    pub owner: String,
    pub repo: String,
    pub number: i64,
    pub scope: DecisionScope,
}

/// Resposta de `POST /sync/query/decisions`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecisionsReply {
    pub decisions: Vec<HubDecision>,
}

/// Canal reverso `GET /sync/channel` (spec da PWA §5, #198): WebSocket que o
/// `ctowerd` abre com o hub, com o token de sync e o [`ENV_ID_HEADER`]. O
/// hub manda `{id, method, params}`; o ambiente responde `{id, result}` ou
/// `{id, error}` ([`crate::rpc`]) e manda os pushes `{event, data}` de
/// [`CHANNEL_PUSHES`]. Só frames de texto.
pub const CHANNEL_PATH: &str = "/sync/channel";

/// Métodos do `/ws` do ambiente que o canal aceita (spec da PWA §5.2). Os
/// dois lados aplicam a lista: o hub antes de mandar, o ambiente antes de
/// despachar. Fora: terminal (`session.attach|detach|input|resize|peek`),
/// o shell livre (`shell.*`, #428), `session.permission.*`, `env.update|rollback|check_update`,
/// `env.config.*`, `cleanup.*`, `github.*` e `sync.now`.
pub const CHANNEL_METHODS: &[&str] = &[
    "env.info",
    "projects.list",
    "dashboard.get",
    "tree.get",
    "review.get",
    "events.list",
    "review.mark_reviewed",
    "recovery.list",
    "recovery.archived",
    "transcript.tail",
    "session.context",
    "session.get",
    "session.start",
    "session.start_project",
    "session.start_gestor",
    "session.resume",
    "session.kill",
    "session.archive",
    "session.remote_control.enable",
    "hub.sync.now",
];

/// Pushes do ambiente repassados pelo canal (spec da PWA §5.3). Fora:
/// `permission_request` (traz o `tool_input`), `update.status`,
/// `cleanup.status` e `shell_exited` (#428).
pub const CHANNEL_PUSHES: &[&str] = &[
    "session_started",
    "session_state",
    "session_exited",
    "session_archived",
    "sessions_interrupted",
    "needs_human",
    "work_item_status",
    "projects_changed",
    "alert",
];

/// Erro de método fora de [`CHANNEL_METHODS`], nos dois lados do canal.
pub const CHANNEL_METHOD_UNAVAILABLE: i64 = -32601;
/// Erro do hub: o ambiente não tem canal aberto.
pub const CHANNEL_ENV_OFFLINE: i64 = -32030;
/// Erro do hub: o ambiente não respondeu no prazo.
pub const CHANNEL_TIMEOUT: i64 = -32031;

/// O canal aceita `method`?
pub fn channel_method_allowed(method: &str) -> bool {
    CHANNEL_METHODS.contains(&method)
}

/// O canal repassa o push `event`?
pub fn channel_push_allowed(event: &str) -> bool {
    CHANNEL_PUSHES.contains(&event)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lista_do_canal_sem_terminal_nem_metodos_sensiveis() {
        for m in [
            "session.attach",
            "session.detach",
            "session.input",
            "session.resize",
            "session.peek",
            // Shell livre (#428): só no app desktop.
            "shell.open",
            "shell.list",
            "shell.close",
            "session.permission.watch",
            "session.permission.list",
            "session.permission.respond",
            "env.update",
            "env.rollback",
            "env.check_update",
            "env.config.get",
            "env.config.set",
            "cleanup.status",
            "cleanup.plan",
            "cleanup.run",
            "github.status",
            "github.set_token",
            "github.clear_token",
            "github.pr.comment",
            "sync.now",
        ] {
            assert!(!channel_method_allowed(m), "{m}");
        }
        assert!(channel_method_allowed("session.start"));
        // Linha do tempo do work item (#422): só leitura, vai para a web.
        assert!(channel_method_allowed("events.list"));
        assert!(!channel_push_allowed("permission_request"));
        assert!(!channel_push_allowed("update.status"));
        assert!(!channel_push_allowed("cleanup.status"));
        assert!(!channel_push_allowed("shell_exited"));
        assert!(channel_push_allowed("needs_human"));
    }
}
