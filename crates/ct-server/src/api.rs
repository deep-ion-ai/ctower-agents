//! Respostas do `/ws` montadas pelo servidor (as demais vêm prontas do
//! ledger). Com a feature `ts`, o teste `export_ts` grava os tipos
//! TypeScript da UI em `ui/src/protocol/gen` (spec §14).

use ct_ledger::{Decision, PrRow, Project, Session, SyncState, WorkItem};
use ct_sessions::GestorScope;
use serde::Serialize;
use std::collections::HashMap;

/// Resposta de `env.info`.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct EnvInfo {
    /// Versão do protocolo (`major.minor`); a UI recusa major diferente.
    pub protocol: String,
    pub version: String,
    /// Commit do build; vazio se compilado fora do git.
    pub commit: String,
    pub hostname: String,
    /// Raiz dos worktrees do daemon (prévia do caminho ao iniciar sessão).
    pub worktrees_root: String,
    /// Identidade deste ambiente (`<config_dir>/env_id`).
    pub env_id: String,
    pub env_name: String,
    /// Envio ao hub; `None` = este ambiente não envia.
    pub hub_sync: Option<crate::hub_sync::HubSyncStatus>,
    /// Configuração vinda do hub: ligação, `config_rev` local, idade e
    /// último erro do pull.
    pub hub_config: crate::hub_config::HubConfigStatus,
    /// Atualização do ambiente; `None` = desativada.
    pub update: Option<crate::update::UpdateStatus>,
    /// Prontidão do jira-cli e do glab por projeto Jira atribuído (spec do
    /// Jira e GitLab §8.4), do último ciclo de sync (nada roda na leitura).
    /// Ausente = ambiente anterior à #349, ou sem o sync do Jira.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub trackers: Option<Vec<ct_core::TrackerReadiness>>,
    /// Remote Control do Claude Code (#197): disponível com o ambiente do
    /// processo e os secrets globais do hub; os do projeto só na recusa ao
    /// iniciar.
    pub remote_control: ct_sessions::RemoteControlStatus,
    /// Pasta de docs dos projetos (#440): `docs_root` e se está acessível
    /// agora (a montagem do compartilhamento pode ter caído).
    pub docs: ct_sessions::docs::DocsStatus,
}

#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct DashboardWorkItem {
    #[serde(flatten)]
    pub work_item: WorkItem,
    /// Título da issue no espelho do GitHub, se já sincronizada.
    pub title: Option<String>,
    /// Número do épico pai no espelho do GitHub (árvore do Terminal).
    pub parent_number: Option<i64>,
    /// Decisões autônomas não revisadas; no épico, soma as das sub-issues.
    pub pending_decisions: usize,
}

#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct DashboardProject {
    pub project: Project,
    pub work_items: Vec<DashboardWorkItem>,
}

/// Resposta de `dashboard.get`.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Dashboard {
    pub projects: Vec<DashboardProject>,
    pub live_sessions: Vec<Session>,
    pub interrupted_sessions: Vec<Session>,
    /// Sessões que saíram com código ≠ 0 nas últimas 24 h (faixa de atenção).
    pub failed_sessions: Vec<Session>,
    pub sync: Vec<SyncState>,
    /// Work item das sessões do gestor de projetos (`_ctower/gestor#0`), se
    /// já houve uma neste ambiente: a UI rotula essas sessões.
    pub gestor_work_item_id: Option<i64>,
    /// Escopo de cada sessão viva do gestor (id da sessão → escopo): a tela
    /// Hub identifica as sessões. Sessão sem escopo conhecido fica de fora.
    pub gestor_scopes: HashMap<String, GestorScope>,
}

/// Resposta de `review.get`.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Review {
    pub work_item: WorkItem,
    pub prs: Vec<PrRow>,
    pub decisions: Vec<Decision>,
}

/// Resposta de `session.peek`: o fim do scrollback como texto (UTF-8 com
/// substituição dos bytes inválidos).
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Peek {
    pub data: String,
}

/// Resposta de `review.mark_reviewed`.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Marked {
    pub marked: usize,
}

#[cfg(all(test, feature = "ts"))]
mod export {
    use super::*;
    use std::fs;
    use std::path::Path;
    use ts_rs::TS;

    /// Regenera `ui/src/protocol/gen`. O CI roda este teste e falha se o
    /// resultado diferir do que está commitado.
    ///
    /// Inteiros de 64 bits viram `number`: o JSON chega à UI por
    /// `JSON.parse`, e ids e timestamps em ms cabem em 2^53.
    #[test]
    fn export_ts() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ui/src/protocol/gen");
        let _ = fs::remove_dir_all(&dir);
        EnvInfo::export_all_to(&dir).unwrap();
        crate::hub_sync::HubSyncStatus::export_all_to(&dir).unwrap();
        crate::update::UpdateStatus::export_all_to(&dir).unwrap();
        crate::env_config::ConfigItem::export_all_to(&dir).unwrap();
        crate::permissions::PermissionRequest::export_all_to(&dir).unwrap();
        crate::hub_sync::HubSyncResult::export_all_to(&dir).unwrap();
        crate::cleanup::CleanupStatus::export_all_to(&dir).unwrap();
        ct_sessions::cleanup::CleanupRun::export_all_to(&dir).unwrap();
        // Os tipos do /ws do hub ficam em ui/src/protocol/hub-gen (cthubd).
        Dashboard::export_all_to(&dir).unwrap();
        Review::export_all_to(&dir).unwrap();
        Peek::export_all_to(&dir).unwrap();
        Marked::export_all_to(&dir).unwrap();
        ct_ledger::ProjectTree::export_all_to(&dir).unwrap();
        // `events.list` (#422).
        ct_ledger::Event::export_all_to(&dir).unwrap();
        ct_sessions::RecoveryItem::export_all_to(&dir).unwrap();
        ct_sessions::ArchivedItem::export_all_to(&dir).unwrap();
        ct_sessions::TranscriptTail::export_all_to(&dir).unwrap();
        ct_sessions::ContextUsage::export_all_to(&dir).unwrap();
        ct_sessions::ProjectSync::export_all_to(&dir).unwrap();
        ct_sessions::GestorScope::export_all_to(&dir).unwrap();
        ct_sessions::RemoteControlEnable::export_all_to(&dir).unwrap();
        ct_sessions::Shell::export_all_to(&dir).unwrap();
        ct_sessions::docs::DocsDir::export_all_to(&dir).unwrap();
        ct_gh::TokenStatus::export_all_to(&dir).unwrap();
        ct_gh::PrComment::export_all_to(&dir).unwrap();
        // Rastreador e forja (#347): os tipos soltos, além dos de `Project`.
        ct_core::TrackerKind::export_all_to(&dir).unwrap();
        ct_core::ForgeKind::export_all_to(&dir).unwrap();

        // (nome do tipo, caminho relativo sem extensão), ex.: serde_json/JsonValue.
        fn walk(dir: &Path, rel: &str, out: &mut Vec<(String, String)>) {
            for entry in fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                let name = path.file_stem().unwrap().to_string_lossy().to_string();
                if path.is_dir() {
                    walk(&path, &format!("{rel}{name}/"), out);
                    continue;
                }
                let ts = fs::read_to_string(&path).unwrap();
                fs::write(&path, ts.replace("bigint", "number")).unwrap();
                out.push((name.clone(), format!("{rel}{name}")));
            }
        }
        let mut names = Vec::new();
        walk(&dir, "", &mut names);
        names.sort();
        let index: String = names
            .iter()
            .map(|(n, p)| format!("export type {{ {n} }} from \"./{p}\";\n"))
            .collect();
        fs::write(
            dir.join("index.ts"),
            format!("// Gerado por `cargo test -p ct-server --features ts export_ts`.\n{index}"),
        )
        .unwrap();
    }
}
