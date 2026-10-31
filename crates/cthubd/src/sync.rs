//! Rotas do sync entre ambiente e hub (spec do ledger centralizado §5 e §8;
//! spec do hub como plano de controle §4), só com o token de sync do
//! ambiente: `POST /sync/push` aplica o lote de forma idempotente e devolve o
//! último `seq` aplicado; `GET /sync/config` entrega a configuração dos
//! projetos atribuídos ao ambiente; `POST /sync/query/decisions` atende o MCP
//! `ledger_list_decisions` dos ambientes.

use crate::secrets::HubSecrets;
use crate::HubState;
use axum::body::Bytes;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use ct_http::sync::{
    ConfigReply, DecisionsQuery, DecisionsReply, PushRequest, PushResponse, ENV_ID_HEADER,
    ENV_PUBKEY_HEADER,
};
use ct_http::{bearer_token, unauthorized};
use ct_ledger::{project_key, IngestResult, LedgerError, AGENT_TOKEN_PREFIX};
use ct_secrets::{Entry, Scope};
use std::collections::HashSet;

/// Token de agente usado no sync: `401` com a explicação, para quem trocou
/// os arquivos de token.
pub(crate) fn agent_token_here(token: &str) -> Option<Response> {
    token.starts_with(AGENT_TOKEN_PREFIX).then(|| {
        (
            StatusCode::UNAUTHORIZED,
            "token inválido: o token de agente (cta_...) só vale em /agent/v1; o sync pede o \
             token de sync (hub.token)",
        )
            .into_response()
    })
}

/// `POST /sync/push`.
pub(crate) async fn push(State(st): State<HubState>, headers: HeaderMap, body: Bytes) -> Response {
    let Some(token) = bearer_token(&headers) else {
        return unauthorized();
    };
    if let Some(r) = agent_token_here(&token) {
        return r;
    }
    let req: PushRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => return (StatusCode::BAD_REQUEST, format!("lote inválido: {e}")).into_response(),
    };
    let hub = st.hub.clone();
    let r = tokio::task::spawn_blocking(move || -> Result<Option<IngestResult>, LedgerError> {
        let hub = hub.lock();
        let Some(env) = hub.authenticate(&token)? else {
            return Ok(None);
        };
        if env.env_id.is_none() {
            tracing::warn!(
                "o token de {} não nasceu preso a um env_id (instalação antiga): preso agora ao \
                 primeiro uso; gire-o com \"cthubd add-env {}\"",
                env.name,
                env.name
            );
        }
        hub.bind_env_token(&env.name, &req.env_id)?;
        let r = hub.ingest(&req.env_id, &env.name, &req.entries)?;
        hub.set_env_pending(&req.env_id, req.pending)?;
        if let Some(c) = &req.cleanup {
            hub.set_env_cleanup(&req.env_id, c)?;
        }
        if r.quarantined > 0 {
            tracing::warn!(
                "push de {}: {} linha(s) em quarentena de projeto(s) não cadastrado(s) no hub: {} \
                 (cadastre com hub.projects.add ou descarte com hub.quarantine.discard)",
                env.name,
                r.quarantined,
                r.quarantined_projects.join(", ")
            );
        }
        if r.discarded > 0 {
            tracing::warn!(
                "push de {}: {} linha(s) descartada(s) de projeto(s) excluído(s) do hub: {}",
                env.name,
                r.discarded,
                r.discarded_projects.join(", ")
            );
        }
        if r.mirror_rejected > 0 {
            tracing::warn!(
                "push de {}: {} linha(s) do espelho do rastreador descartada(s): o ambiente não é \
                 o ambiente de sync de {}",
                env.name,
                r.mirror_rejected,
                r.mirror_rejected_projects.join(", ")
            );
        }
        Ok(Some(r))
    })
    .await;
    if matches!(r, Ok(Ok(Some(_)))) {
        // Sessões e eventos novos podem virar Web Push (#203).
        st.push_kick();
    }
    match r {
        Ok(Ok(Some(r))) => {
            if r.rules_proposals > 0 {
                // Proposta de regras nova (#357): a UI relê as pendentes.
                st.rules_changed();
            }
            axum::Json(PushResponse {
                last_seq: r.last_seq,
            })
            .into_response()
        }
        Ok(Ok(None)) => unauthorized(),
        Ok(Err(LedgerError::Conflict(m))) => (StatusCode::FORBIDDEN, m).into_response(),
        Ok(Err(e)) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

#[derive(Debug, serde::Deserialize)]
pub(crate) struct ConfigParams {
    since: Option<i64>,
    /// Época do último pull do ambiente; diferente da atual = conjunto completo.
    epoch: Option<i64>,
}

/// `GET /sync/config?since=<rev>&epoch=<época>`: só os projetos atribuídos ao `env_id`
/// preso ao token. O ambiente manda o `env_id` em `X-Ctower-Env-Id`; token
/// ainda sem `env_id` fica preso a ele (como no primeiro push), e divergência
/// dá `403`.
///
/// Entrega de secrets (#184): o ambiente manda a chave pública age em
/// `X-Ctower-Env-Pubkey`, que o hub guarda pelo `env_id` (só o token preso a
/// ele chega aqui). Quando a resposta é de mudança (`changed`) — inclusive
/// porque a chave mudou desde a última entrega —, ela leva em `secrets` o
/// conjunto completo das entradas globais e dos projetos atribuídos, cifrado
/// só para essa chave. Sem o cabeçalho, nada de secrets.
pub(crate) async fn config(
    State(st): State<HubState>,
    headers: HeaderMap,
    Query(q): Query<ConfigParams>,
) -> Response {
    let Some(token) = bearer_token(&headers) else {
        return unauthorized();
    };
    if let Some(r) = agent_token_here(&token) {
        return r;
    }
    let Some(env_id) = headers
        .get(ENV_ID_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string)
    else {
        return (StatusCode::BAD_REQUEST, "falta o cabeçalho X-Ctower-Env-Id").into_response();
    };
    let pubkey = match headers.get(ENV_PUBKEY_HEADER) {
        None => None,
        Some(v) => match v
            .to_str()
            .ok()
            .and_then(|v| v.trim().parse::<ct_secrets::Recipient>().ok())
        {
            Some(r) => Some(r),
            None => {
                return (
                    StatusCode::BAD_REQUEST,
                    "chave pública inválida em X-Ctower-Env-Pubkey (esperado age1...)",
                )
                    .into_response()
            }
        },
    };
    let hub = st.hub.clone();
    let secrets = st.secrets.clone();
    let r = tokio::task::spawn_blocking(move || -> Result<Option<ConfigReply>, ConfigFail> {
        // O lock do hub sai antes do store: `HubSecrets::set` pega o store e
        // depois o hub. Lido depois do `config_rev`, o store está pelo menos
        // tão novo quanto ele.
        let (config, deliver_to) = {
            let hub = hub.lock();
            let Some(env) = hub.authenticate(&token)? else {
                return Ok(None);
            };
            if env.env_id.is_none() {
                tracing::warn!(
                    "o token de {} não nasceu preso a um env_id (instalação antiga): preso agora ao \
                     primeiro uso; gire-o com \"cthubd add-env {}\"",
                    env.name,
                    env.name
                );
            }
            hub.bind_env_token(&env.name, &env_id)?;
            let key = match &pubkey {
                Some(pk) => {
                    hub.set_env_pubkey(&env_id, &pk.to_string())?;
                    // Antes da entrega desta resposta: o `since` confirma
                    // só entregas anteriores à mesma chave (#165).
                    hub.record_env_applied(&env_id, q.since, q.epoch)?;
                    hub.env_pubkey(&env_id)?
                }
                None => None,
            };
            let force = key.as_ref().is_some_and(|k| k.needs_delivery());
            let config = hub.env_config_with(&env_id, q.since, q.epoch, force)?;
            let deliver_to = pubkey.filter(|_| config.changed);
            (config, deliver_to)
        };
        let secrets = match deliver_to {
            None => None,
            Some(pk) => {
                let b64 = seal_for_env(secrets.as_deref(), &config, &pk)?;
                hub.lock()
                    .mark_secrets_delivered(&env_id, &pk.to_string())?;
                Some(b64)
            }
        };
        Ok(Some(ConfigReply { config, secrets }))
    })
    .await;
    match r {
        Ok(Ok(Some(c))) => axum::Json(c).into_response(),
        Ok(Ok(None)) => unauthorized(),
        Ok(Err(ConfigFail::Ledger(LedgerError::Conflict(m)))) => {
            (StatusCode::FORBIDDEN, m).into_response()
        }
        Ok(Err(e)) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// `GET /sync/done` (#298): os work items que o hub deu como `concluido` nos
/// projetos atribuídos ao `env_id` preso ao token (conjunto completo). Mesma
/// autenticação de `/sync/config`; o status do hub não move o `config_rev`,
/// por isso não vai naquela rota.
pub(crate) async fn done(State(st): State<HubState>, headers: HeaderMap) -> Response {
    let Some(token) = bearer_token(&headers) else {
        return unauthorized();
    };
    if let Some(r) = agent_token_here(&token) {
        return r;
    }
    let Some(env_id) = headers
        .get(ENV_ID_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string)
    else {
        return (StatusCode::BAD_REQUEST, "falta o cabeçalho X-Ctower-Env-Id").into_response();
    };
    let hub = st.hub.clone();
    let r = tokio::task::spawn_blocking(move || -> Result<Option<_>, LedgerError> {
        let hub = hub.lock();
        let Some(env) = hub.authenticate(&token)? else {
            return Ok(None);
        };
        hub.bind_env_token(&env.name, &env_id)?;
        hub.env_done(&env_id).map(Some)
    })
    .await;
    match r {
        Ok(Ok(Some(d))) => axum::Json(d).into_response(),
        Ok(Ok(None)) => unauthorized(),
        Ok(Err(LedgerError::Conflict(m))) => (StatusCode::FORBIDDEN, m).into_response(),
        Ok(Err(e)) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

#[derive(Debug, thiserror::Error)]
enum ConfigFail {
    #[error(transparent)]
    Ledger(#[from] LedgerError),
    #[error("entrega de secrets: {0}")]
    Secrets(#[from] ct_secrets::SecretsError),
}

/// Cifra para `to` as entradas globais e as dos projetos de `config` (o
/// conjunto completo atribuído ao ambiente). Projetos não atribuídos nunca
/// entram. Hub sem store (testes): conjunto vazio, coerente com "não há
/// secrets".
fn seal_for_env(
    secrets: Option<&HubSecrets>,
    config: &ct_http::sync::ConfigResponse,
    to: &ct_secrets::Recipient,
) -> Result<String, ct_secrets::SecretsError> {
    let assigned: HashSet<String> = config
        .projects
        .iter()
        .flatten()
        .map(|p| project_key(&p.owner, &p.repo))
        .collect();
    let entries: Vec<Entry> = secrets
        .map(|s| {
            s.with_store(|store| {
                store
                    .entries()
                    .iter()
                    .filter(|e| !ct_secrets::is_hub_only(&e.name))
                    .filter(|e| match &e.scope {
                        Scope::Global => true,
                        Scope::Project(k) => assigned.contains(k),
                    })
                    .cloned()
                    .collect()
            })
        })
        .unwrap_or_default();
    ct_secrets::seal_delivery(config.config_rev, entries, to)
}

/// `POST /sync/query/decisions` (token de sync de qualquer ambiente).
pub(crate) async fn query_decisions(
    State(st): State<HubState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let Some(token) = bearer_token(&headers) else {
        return unauthorized();
    };
    if let Some(r) = agent_token_here(&token) {
        return r;
    }
    let q: DecisionsQuery = match serde_json::from_slice(&body) {
        Ok(q) => q,
        Err(e) => {
            return (StatusCode::BAD_REQUEST, format!("consulta inválida: {e}")).into_response()
        }
    };
    let hub = st.hub.clone();
    let r = tokio::task::spawn_blocking(move || {
        let hub = hub.lock();
        if hub.authenticate(&token)?.is_none() {
            return Ok(None);
        }
        hub.scoped_decisions(&q.owner, &q.repo, q.number, q.scope)
            .map(|d| Some(d.ok_or(())))
    })
    .await;
    match r {
        Ok(Ok(Some(Ok(decisions)))) => axum::Json(DecisionsReply { decisions }).into_response(),
        Ok(Ok(Some(Err(())))) => {
            (StatusCode::NOT_FOUND, "work item não está no hub").into_response()
        }
        Ok(Ok(None)) => unauthorized(),
        Ok(Err::<_, LedgerError>(e)) => {
            (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

#[cfg(test)]
mod tests {
    use crate::{router, HubState};
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use ct_core::{DecisionOrigin, WorkItemKind};
    use ct_ledger::{Hub, Ledger, NewDecision, NewProject, SharedHub};
    use http_body_util::BodyExt;
    use serde_json::{json, Value};
    use tower::ServiceExt;

    const UI: &str = "ui-secreto";

    fn state() -> HubState {
        HubState::new(
            SharedHub::new(Hub::open_in_memory().unwrap()),
            UI,
            "hub",
            "hub-id",
        )
    }

    async fn post(
        st: &HubState,
        path: &str,
        token: Option<&str>,
        body: Value,
    ) -> (StatusCode, Value) {
        let mut req = Request::post(path).header("content-type", "application/json");
        if let Some(t) = token {
            req = req.header("authorization", format!("Bearer {t}"));
        }
        let resp = router(st.clone())
            .oneshot(req.body(Body::from(body.to_string())).unwrap())
            .await
            .unwrap();
        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn push_exige_token_de_sync() {
        let st = state();
        let tok = st.hub.lock().add_legacy_env_token("a").unwrap();
        let body = json!({ "env_id": "e", "entries": [] });
        // O token de UI do hub não serve para o sync.
        for t in [UI, "x"] {
            let (s, _) = post(&st, "/sync/push", Some(t), body.clone()).await;
            assert_eq!(s, StatusCode::UNAUTHORIZED, "{t}");
        }
        let (s, _) = post(&st, "/sync/push", None, body.clone()).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED);
        let (s, v) = post(&st, "/sync/push", Some(&tok), body.clone()).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(v["last_seq"], 0);
        // O token fica preso ao primeiro env_id.
        let outro = json!({ "env_id": "f", "entries": [] });
        let (s, _) = post(&st, "/sync/push", Some(&tok), outro).await;
        assert_eq!(s, StatusCode::FORBIDDEN);
        let (s, _) = post(&st, "/sync/push", Some(&tok), json!({ "x": 1 })).await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        // Revogado: corta o envio.
        assert!(st.hub.lock().revoke_env_token("a").unwrap());
        let (s, _) = post(&st, "/sync/push", Some(&tok), body).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED);
    }

    /// #195: o token nasce preso ao `env_id`. Nome novo: só funciona com o
    /// `env_id` devolvido junto; ambiente existente: girar o token mantém o
    /// `env_id` (segue sincronizando) e quem tem o token novo não o prende a
    /// outro `env_id`, nem no push nem no pull de configuração.
    #[tokio::test(flavor = "multi_thread")]
    async fn token_nasce_preso_ao_env_id_e_girar_mantem() {
        let st = state();
        let novo = st.hub.lock().issue_env_token("a").unwrap();
        let body = |id: &str| json!({ "env_id": id, "entries": [] });
        let (s, _) = post(&st, "/sync/push", Some(&novo.token), body("outro")).await;
        assert_eq!(s, StatusCode::FORBIDDEN, "primeiro uso com env_id alheio");
        let (s, _) = get(&st, "/sync/config", Some(&novo.token), Some("outro")).await;
        assert_eq!(s, StatusCode::FORBIDDEN, "pull com env_id alheio");
        let (s, _) = post(&st, "/sync/push", Some(&novo.token), body(&novo.env_id)).await;
        assert_eq!(s, StatusCode::OK);

        // Gira o token: o env_id é o mesmo e o ambiente segue sincronizando.
        let girado = st.hub.lock().issue_env_token("a").unwrap();
        assert_eq!(girado.env_id, novo.env_id);
        let (s, _) = post(&st, "/sync/push", Some(&novo.token), body(&novo.env_id)).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED, "token antigo");
        let (s, _) = post(&st, "/sync/push", Some(&girado.token), body("outro")).await;
        assert_eq!(
            s,
            StatusCode::FORBIDDEN,
            "token girado não prende a outro env_id"
        );
        let (s, _) = get(&st, "/sync/config", Some(&girado.token), Some("outro")).await;
        assert_eq!(s, StatusCode::FORBIDDEN);
        let (s, _) = post(&st, "/sync/push", Some(&girado.token), body(&girado.env_id)).await;
        assert_eq!(s, StatusCode::OK);
        let (s, _) = get(
            &st,
            "/sync/config",
            Some(&girado.token),
            Some(&girado.env_id),
        )
        .await;
        assert_eq!(s, StatusCode::OK);
    }

    /// Um lote de verdade chega ao `hub.db` e a consulta de decisões responde
    /// pelo mesmo token.
    #[tokio::test(flavor = "multi_thread")]
    async fn push_ingere_e_consulta_decisoes() {
        let st = state();
        let tok = st.hub.lock().add_legacy_env_token("wsl").unwrap();
        let l = Ledger::open_in_memory().unwrap();
        let p = l
            .add_project(&NewProject {
                name: "demo".into(),
                gh_owner: "deep-ion-ai".into(),
                gh_repo: "demo".into(),
                repo_path: "/x".into(),
                base_branch: "develop".into(),
                protected_branches: vec![],
            })
            .unwrap();
        let wi = l.ensure_work_item(p.id, 7, WorkItemKind::Issue).unwrap();
        l.record_decision(&NewDecision {
            work_item_id: wi.id,
            session_id: None,
            origin: DecisionOrigin::Autonoma,
            title: "t".into(),
            context: "c".into(),
            decision: "d".into(),
            consequences: None,
        })
        .unwrap();
        let entries = l.outbox_pending(1000).unwrap();
        let last = entries.last().unwrap().seq;
        let entries_len = entries.len() as i64;
        let body = json!({ "env_id": l.env_id(), "entries": entries, "pending": 3 });
        let (s, v) = post(&st, "/sync/push", Some(&tok), body).await;
        assert_eq!(s, StatusCode::OK, "{v}");
        // Projeto ainda não cadastrado no hub: ack do lote, linhas em quarentena.
        assert_eq!(v["last_seq"], last);
        {
            let h = st.hub.lock();
            assert!(h
                .find_work_item("deep-ion-ai", "demo", 7)
                .unwrap()
                .is_none());
            assert_eq!(
                h.quarantine_count("deep-ion-ai", "demo").unwrap(),
                entries_len
            );
            h.add_project(&ct_ledger::NewHubProject {
                owner: "deep-ion-ai".into(),
                repo: "demo".into(),
                name: "demo".into(),
                base_branch: "develop".into(),
                protected_branches: vec![],
            })
            .unwrap();
        }
        {
            let h = st.hub.lock();
            assert!(h
                .find_work_item("deep-ion-ai", "demo", 7)
                .unwrap()
                .is_some());
            let envs = h.list_envs().unwrap();
            assert_eq!((envs[0].name.as_str(), envs[0].pending), ("wsl", 3));
            // Sem `cleanup` no lote (ambiente antigo): nada informado.
            assert_eq!(envs[0].cleanup, None);
        }

        // Resumo das limpezas pendentes (issue #287): guardado a cada envio;
        // um lote sem o campo, ou com um resumo mais velho, não apaga o atual.
        let resumo = |checked_at: i64, items: i64| {
            json!({ "env_id": l.env_id(), "entries": [], "pending": 0,
                    "cleanup": { "checked_at": checked_at, "items": items, "bytes": 5_000_000_000i64 } })
        };
        let (s, v) = post(&st, "/sync/push", Some(&tok), resumo(2_000, 4)).await;
        assert_eq!(s, StatusCode::OK, "{v}");
        let (s, _) = post(&st, "/sync/push", Some(&tok), resumo(1_000, 9)).await;
        assert_eq!(s, StatusCode::OK);
        let sem = json!({ "env_id": l.env_id(), "entries": [], "pending": 0 });
        let (s, _) = post(&st, "/sync/push", Some(&tok), sem).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(
            st.hub.lock().list_envs().unwrap()[0].cleanup,
            Some(ct_ledger::CleanupSummary {
                checked_at: 2_000,
                items: 4,
                bytes: 5_000_000_000
            })
        );

        let q = |n: i64| json!({ "owner": "deep-ion-ai", "repo": "demo", "number": n, "scope": "project" });
        let (s, v) = post(&st, "/sync/query/decisions", Some(&tok), q(7)).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(v["decisions"].as_array().unwrap().len(), 1);
        let (s, _) = post(&st, "/sync/query/decisions", Some(&tok), q(999)).await;
        assert_eq!(s, StatusCode::NOT_FOUND);
        let (s, _) = post(&st, "/sync/query/decisions", Some(UI), q(7)).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED);
        let (s, _) = post(&st, "/sync/query/decisions", Some(&tok), json!({})).await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
    }

    async fn get(
        st: &HubState,
        path: &str,
        token: Option<&str>,
        env_id: Option<&str>,
    ) -> (StatusCode, Value) {
        let mut req = Request::get(path);
        if let Some(t) = token {
            req = req.header("authorization", format!("Bearer {t}"));
        }
        if let Some(e) = env_id {
            req = req.header("X-Ctower-Env-Id", e);
        }
        let resp = router(st.clone())
            .oneshot(req.body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    /// `GET /sync/done` (#298): só os `concluido` dos projetos atribuídos ao
    /// ambiente do token.
    #[tokio::test(flavor = "multi_thread")]
    async fn done_so_dos_projetos_do_ambiente() {
        let st = state();
        let tok = {
            let h = st.hub.lock();
            for repo in ["demo", "outro"] {
                h.add_project(&ct_ledger::NewHubProject {
                    owner: "deep-ion-ai".into(),
                    repo: repo.into(),
                    name: repo.into(),
                    base_branch: "develop".into(),
                    protected_branches: vec!["develop".into()],
                })
                .unwrap();
                let pid = h.find_project("deep-ion-ai", repo).unwrap().unwrap().id;
                let issue = |n: i64| ct_ledger::IssueRow {
                    gh_number: n,
                    title: format!("Issue {n}"),
                    state: "open".into(),
                    labels: vec![],
                    parent_number: None,
                    has_sub_issues: false,
                    url: "u".into(),
                    updated_at: 1,
                    ..Default::default()
                };
                h.apply_github(
                    &ct_ledger::GithubSnapshot {
                        project_id: pid,
                        issues: vec![issue(7), issue(8)],
                        prs: vec![],
                        etags: vec![],
                        synced_at: 1,
                    },
                    "hub",
                )
                .unwrap();
            }
            for repo in ["demo", "outro"] {
                h.set_status_manual(
                    "deep-ion-ai",
                    repo,
                    7,
                    ct_core::WorkItemStatus::Concluido,
                    None,
                    "hub",
                )
                .unwrap();
            }
            h.set_status_manual(
                "deep-ion-ai",
                "demo",
                8,
                ct_core::WorkItemStatus::Backlog,
                None,
                "hub",
            )
            .unwrap();
            h.add_legacy_env_token("env-a").unwrap()
        };
        // O primeiro pedido prende o token ao `env_id`; só então dá para atribuir.
        let (s, v) = get(&st, "/sync/done", Some(&tok), Some("env-a")).await;
        assert_eq!((s, &v["items"]), (StatusCode::OK, &json!([])));
        st.hub
            .lock()
            .add_assignment("deep-ion-ai", "demo", "env-a", None)
            .unwrap();
        let (s, _) = get(&st, "/sync/done", None, Some("env-a")).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED);
        let (s, _) = get(&st, "/sync/done", Some(&tok), None).await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        let (s, v) = get(&st, "/sync/done", Some(&tok), Some("env-a")).await;
        assert_eq!(s, StatusCode::OK, "{v}");
        let items = v["items"].as_array().unwrap();
        assert_eq!(items.len(), 1, "{v}");
        assert_eq!(
            (&items[0]["repo"], &items[0]["number"]),
            (&json!("demo"), &json!(7))
        );
    }

    /// O token de sync só lê a configuração do seu `env_id`, e só dos
    /// projetos atribuídos a ele.
    #[tokio::test(flavor = "multi_thread")]
    async fn config_so_do_ambiente_do_token() {
        let st = state();
        let (tok_a, tok_b) = {
            let h = st.hub.lock();
            let a = h.add_legacy_env_token("a").unwrap();
            let b = h.add_legacy_env_token("b").unwrap();
            for repo in ["demo", "outro"] {
                h.add_project(&ct_ledger::NewHubProject {
                    owner: "deep-ion-ai".into(),
                    repo: repo.into(),
                    name: repo.into(),
                    base_branch: "develop".into(),
                    protected_branches: vec!["develop".into()],
                })
                .unwrap();
            }
            (a, b)
        };
        // Sem token, com o token de UI, sem cabeçalho.
        let (s, _) = get(&st, "/sync/config", None, Some("env-a")).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED);
        let (s, _) = get(&st, "/sync/config", Some(UI), Some("env-a")).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED);
        let (s, _) = get(&st, "/sync/config", Some(&tok_a), None).await;
        assert_eq!(s, StatusCode::BAD_REQUEST);

        // O primeiro pull prende o token ao `env_id` do cabeçalho.
        let (s, v) = get(&st, "/sync/config", Some(&tok_a), Some("env-a")).await;
        assert_eq!(s, StatusCode::OK, "{v}");
        assert_eq!(v["changed"], true);
        assert_eq!(v["projects"], json!([]));
        let (s, _) = get(&st, "/sync/config", Some(&tok_b), Some("env-b")).await;
        assert_eq!(s, StatusCode::OK);
        // Divergência: 403, como no push.
        let (s, _) = get(&st, "/sync/config", Some(&tok_a), Some("env-b")).await;
        assert_eq!(s, StatusCode::FORBIDDEN);

        {
            let h = st.hub.lock();
            h.add_assignment("deep-ion-ai", "demo", "env-a", None)
                .unwrap();
            h.add_assignment("deep-ion-ai", "outro", "env-b", Some("/b/outro"))
                .unwrap();
        }
        let (s, v) = get(&st, "/sync/config?since=0", Some(&tok_a), Some("env-a")).await;
        assert_eq!(s, StatusCode::OK);
        let rev = v["config_rev"].as_i64().unwrap();
        assert_eq!(v["projects"].as_array().unwrap().len(), 1, "{v}");
        assert_eq!(v["projects"][0]["repo"], "demo");
        assert_eq!(v["projects"][0]["repo_path"], "");
        assert_eq!(v["projects"][0]["base_branch"], "develop");
        let epoch = v["config_epoch"].as_i64().unwrap();
        let (_, v) = get(
            &st,
            &format!("/sync/config?since={rev}&epoch={epoch}"),
            Some(&tok_a),
            Some("env-a"),
        )
        .await;
        assert_eq!(
            v,
            json!({ "config_rev": rev, "config_epoch": epoch, "changed": false })
        );
        // Outra época (hub restaurado), mesmo `config_rev`: conjunto completo.
        let (_, v) = get(
            &st,
            &format!("/sync/config?since={rev}&epoch={}", epoch + 1),
            Some(&tok_a),
            Some("env-a"),
        )
        .await;
        assert_eq!(v["changed"], true);
        assert_eq!(v["projects"].as_array().unwrap().len(), 1);
        let (_, v) = get(&st, "/sync/config", Some(&tok_b), Some("env-b")).await;
        assert_eq!(v["projects"][0]["repo"], "outro");
        assert_eq!(v["projects"][0]["repo_path"], "/b/outro");
        // `since` inválido.
        let (s, _) = get(&st, "/sync/config?since=x", Some(&tok_a), Some("env-a")).await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        // Revogado: corta o pull.
        assert!(st.hub.lock().revoke_env_token("a").unwrap());
        let (s, _) = get(&st, "/sync/config", Some(&tok_a), Some("env-a")).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED);
    }

    async fn get_with(
        st: &HubState,
        path: &str,
        token: &str,
        env_id: &str,
        pubkey: Option<&str>,
    ) -> (StatusCode, Value) {
        let mut req = Request::get(path)
            .header("authorization", format!("Bearer {token}"))
            .header("X-Ctower-Env-Id", env_id);
        if let Some(k) = pubkey {
            req = req.header("X-Ctower-Env-Pubkey", k);
        }
        let resp = router(st.clone())
            .oneshot(req.body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    /// Nomes e valores da entrega em `v["secrets"]`, decifrada com `id`.
    fn entregues(v: &Value, id: &ct_secrets::Identity) -> Vec<(String, String, String)> {
        let blob =
            ct_secrets::decode_delivery(v["secrets"].as_str().expect("sem secrets")).unwrap();
        let s = ct_secrets::EnvSecrets::open(&blob, id).unwrap();
        s.entries()
            .iter()
            .map(|e| {
                let scope = match &e.scope {
                    ct_secrets::Scope::Global => "global".to_string(),
                    ct_secrets::Scope::Project(k) => k.clone(),
                };
                (scope, e.name.clone(), e.value.expose().to_string())
            })
            .collect()
    }

    /// O ambiente recebe só os globais e os dos projetos atribuídos, cifrados
    /// para a chave dele; a troca de chave força reentregar com `since` em
    /// dia; desatribuir tira os secrets do projeto; revogar corta tudo.
    #[tokio::test(flavor = "multi_thread")]
    async fn config_entrega_secrets_so_do_ambiente() {
        use ct_secrets::{Identity, Kind, Scope, SecretValue};
        let d = tempfile::tempdir().unwrap();
        let dirs = crate::config::Dirs {
            config: d.path().join("config"),
            data: d.path().join("data"),
        };
        dirs.ensure().unwrap();
        let mut st = state();
        let secrets = crate::secrets::HubSecrets::open(&dirs, st.hub.clone()).unwrap();
        let tok = {
            let h = st.hub.lock();
            for repo in ["demo", "outro"] {
                h.add_project(&ct_ledger::NewHubProject {
                    owner: "deep-ion-ai".into(),
                    repo: repo.into(),
                    name: repo.into(),
                    base_branch: "develop".into(),
                    protected_branches: vec![],
                })
                .unwrap();
            }
            h.add_legacy_env_token("a").unwrap()
        };
        let demo = Scope::parse_project("deep-ion-ai/demo").unwrap();
        let outro = Scope::parse_project("deep-ion-ai/outro").unwrap();
        secrets
            .set(Scope::Global, "G", Kind::Variable, SecretValue::new("g"))
            .unwrap();
        secrets
            .set(
                demo.clone(),
                "D",
                Kind::Secret,
                SecretValue::new("d-secreto"),
            )
            .unwrap();
        secrets
            .set(outro, "O", Kind::Secret, SecretValue::new("o-secreto"))
            .unwrap();
        // Credencial do backup (#263): vive no mesmo store, mas só do hub.
        // A igualdade da entrega abaixo prova que ela não sai daqui.
        secrets
            .set_hub_only("CTHUB_BACKUP_S3_SECRET_KEY", "segredo-do-bucket")
            .unwrap();
        st.secrets = Some(std::sync::Arc::new(secrets));

        let a = Identity::generate();
        let pk_a = a.to_public().to_string();
        // Chave inválida: 400, sem registrar nada.
        let (s, _) = get_with(&st, "/sync/config", &tok, "env-a", Some("age1lixo")).await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        // Ambiente antigo (sem cabeçalho): configuração sem secrets.
        let (s, v) = get_with(&st, "/sync/config", &tok, "env-a", None).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(v["changed"], true);
        assert!(v.get("secrets").is_none(), "{v}");
        st.hub
            .lock()
            .add_assignment("deep-ion-ai", "demo", "env-a", None)
            .unwrap();

        let (s, v) = get_with(&st, "/sync/config", &tok, "env-a", Some(&pk_a)).await;
        assert_eq!(s, StatusCode::OK);
        let rev = v["config_rev"].as_i64().unwrap();
        let epoch = v["config_epoch"].as_i64().unwrap();
        assert_eq!(
            entregues(&v, &a),
            [
                ("global".into(), "G".into(), "g".into()),
                ("deep-ion-ai/demo".into(), "D".into(), "d-secreto".into()),
            ]
        );
        assert!(!v.to_string().contains("o-secreto"));
        assert!(!v.to_string().contains("d-secreto"), "só cifrado");
        // Outra chave não abre a entrega.
        let blob = ct_secrets::decode_delivery(v["secrets"].as_str().unwrap()).unwrap();
        assert!(ct_secrets::EnvSecrets::open(&blob, &Identity::generate()).is_err());
        // Entregue, mas ainda sem a confirmação (o `since` do próximo pull).
        use crate::secrets::DeliveryState;
        let estado = |st: &HubState| {
            let l = st.secrets.as_ref().unwrap().list().unwrap();
            (l.delivery[0].state, l.delivery[0].applied_rev)
        };
        assert_eq!(estado(&st), (DeliveryState::Pending, None));

        // Em dia, mesma chave: nada, e o `since` confirma a entrega.
        let em_dia = format!("/sync/config?since={rev}&epoch={epoch}");
        let (_, v) = get_with(&st, &em_dia, &tok, "env-a", Some(&pk_a)).await;
        assert_eq!(v["changed"], false);
        assert!(v.get("secrets").is_none());
        assert_eq!(estado(&st), (DeliveryState::Delivered, Some(rev)));
        // Chave nova (rotação): reentrega com `since` em dia.
        let b = Identity::generate();
        let pk_b = b.to_public().to_string();
        let (_, v) = get_with(&st, &em_dia, &tok, "env-a", Some(&pk_b)).await;
        assert_eq!(v["changed"], true);
        assert_eq!(v["config_rev"], rev);
        assert_eq!(v["removed"], json!([]));
        assert_eq!(entregues(&v, &b).len(), 2);
        let blob = ct_secrets::decode_delivery(v["secrets"].as_str().unwrap()).unwrap();
        assert!(
            ct_secrets::EnvSecrets::open(&blob, &a).is_err(),
            "só a chave nova"
        );
        let (_, v) = get_with(&st, &em_dia, &tok, "env-a", Some(&pk_b)).await;
        assert_eq!(v["changed"], false);

        // Desatribuir: a entrega seguinte não tem mais os do projeto.
        st.hub
            .lock()
            .remove_assignment("deep-ion-ai", "demo", "env-a")
            .unwrap();
        assert_eq!(estado(&st).0, DeliveryState::Pending);
        let (_, v) = get_with(&st, &em_dia, &tok, "env-a", Some(&pk_b)).await;
        assert_eq!(v["changed"], true);
        assert_eq!(
            entregues(&v, &b),
            [("global".into(), "G".into(), "g".into())]
        );
        let novo = v["config_rev"].as_i64().unwrap();
        assert_eq!(estado(&st), (DeliveryState::Pending, Some(rev)));
        let em_dia = format!("/sync/config?since={novo}&epoch={epoch}");
        get_with(&st, &em_dia, &tok, "env-a", Some(&pk_b)).await;
        assert_eq!(estado(&st), (DeliveryState::Delivered, Some(novo)));

        // Revogar: 401 e a chave some do hub.
        assert!(st.hub.lock().revoke_env_token("a").unwrap());
        let (s, v) = get_with(&st, "/sync/config", &tok, "env-a", Some(&pk_b)).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED);
        assert!(v.get("secrets").is_none());
        assert!(st.hub.lock().env_pubkey("env-a").unwrap().is_none());
    }

    /// Hub sem store (`secrets: None`): entrega um conjunto vazio.
    #[tokio::test(flavor = "multi_thread")]
    async fn config_sem_store_entrega_vazio() {
        let st = state();
        let tok = st.hub.lock().add_legacy_env_token("a").unwrap();
        let id = ct_secrets::Identity::generate();
        let pk = id.to_public().to_string();
        let (s, v) = get_with(&st, "/sync/config", &tok, "env-a", Some(&pk)).await;
        assert_eq!(s, StatusCode::OK);
        assert!(entregues(&v, &id).is_empty());
    }

    #[tokio::test]
    async fn healthz_sem_token() {
        let resp = router(state())
            .oneshot(Request::get("/healthz").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let v: Value =
            serde_json::from_slice(&resp.into_body().collect().await.unwrap().to_bytes()).unwrap();
        assert_eq!(v["role"], "hub");
        assert_eq!(v["protocol"], crate::PROTOCOL_VERSION);
    }
}
