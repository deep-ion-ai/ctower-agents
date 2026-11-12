//! `hub.release.status { owner, repo }` (#204): o que o botão "Criar
//! release" mostra antes de iniciar a sessão — a configuração do projeto,
//! se o secret do token exclusivo existe (nunca o valor) e o estado da
//! release no GitHub (diferença de conteúdo, PRs abertos do fluxo, runs
//! pendentes), lido com o token do hub, só leitura.

use crate::HubState;
use ct_core::release::ReleaseStatus;
use ct_core::{ForgeKind, TrackerKind};
use ct_http::rpc::{str_param, RpcResult};
use ct_secrets::Scope;
use serde::Serialize;
use serde_json::{json, Value};

/// Resposta de `hub.release.status`.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct HubReleaseStatus {
    /// `release.enabled` do projeto.
    pub enabled: bool,
    /// Nome do secret do token exclusivo (`release.token_secret`).
    pub token_secret: String,
    /// O secret existe no escopo do projeto.
    pub secret_present: bool,
    /// Estado da release no GitHub; `None` com `error` quando não deu para
    /// ler (GitHub fora do ar, sem a integração no hub).
    pub status: Option<ReleaseStatus>,
    pub error: Option<String>,
}

pub(crate) async fn status(st: &HubState, p: Value) -> RpcResult {
    let (owner, repo) = (
        str_param(&p, "owner")?.to_string(),
        str_param(&p, "repo")?.to_string(),
    );
    let hub = st.hub.clone();
    let (o, r) = (owner.clone(), repo.clone());
    let project = tokio::task::spawn_blocking(move || hub.lock().find_project(&o, &r))
        .await
        .map_err(|e| (-32603, e.to_string()))?
        .map_err(|e| (-32000, e.to_string()))?
        .ok_or_else(|| (-32000, format!("projeto {owner}/{repo} não está no hub")))?;
    if project.tracker.kind() != TrackerKind::Github || project.forge.kind() != ForgeKind::Github {
        return Err((
            -32000,
            "release: só em projetos GitHub + GitHub".to_string(),
        ));
    }
    let cfg = project.release.normalized();
    // O store de secrets antes do hub, nunca os dois locks juntos.
    let secret_present = match &st.secrets {
        Some(s) => Scope::project(&project.gh_owner, &project.gh_repo)
            .map(|scope| s.with_store(|store| store.get(&scope, &cfg.token_secret).is_some()))
            .unwrap_or(false),
        None => false,
    };
    let (status, error) = match st.github.clone() {
        None => (
            None,
            Some("integração com o GitHub desativada no hub".to_string()),
        ),
        Some(gh) => {
            let (o, r, c) = (
                project.gh_owner.clone(),
                project.gh_repo.clone(),
                cfg.clone(),
            );
            let res = tokio::task::spawn_blocking(move || {
                ct_gh::release::Release::new(gh.client(), &o, &r, &c).status()
            })
            .await
            .map_err(|e| (-32603, e.to_string()))?;
            match res {
                Ok(s) => (Some(s), None),
                Err(e) => (None, Some(format!("GitHub: {e}"))),
            }
        }
    };
    Ok(json!(HubReleaseStatus {
        enabled: cfg.enabled,
        token_secret: cfg.token_secret,
        secret_present,
        status,
        error,
    }))
}
