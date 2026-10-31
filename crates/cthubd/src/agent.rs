//! `/agent/v1/*`: leitura do hub para agentes (issue #217, spec do gestor
//! §6), só com o token de agente (`cta_...`) do ambiente, que o `ctowerd`
//! repassa pelas ferramentas `portfolio_*`. Nenhuma rota daqui aplica
//! mudança: a única escrita é `POST /agent/v1/proposals`, que cria proposta
//! pendente (#218). Aprovar só pela UI do hub (`hub.proposals.decide`).
//!
//! O ambiente manda `X-Ctower-Env-Id`; um `env_id` diferente do preso ao
//! token de sync do mesmo ambiente dá `403`, como no sync.

use crate::HubState;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use ct_http::bearer_token;
use ct_http::sync::ENV_ID_HEADER;
use ct_ledger::{
    AgentDecisionFilter, DecisionScope, HealthFilter, Hub, LedgerError, NewProposal,
    ProposalFilter, RankingFilter, Severity, AGENT_TOKEN_PREFIX, RANKING_LIMIT,
};
use serde::Deserialize;

/// Sessão do gestor que fez a chamada (gravada na proposta).
pub const SESSION_HEADER: &str = "x-ctower-session";

/// Erro de rota: status e mensagem em texto, como o sync.
type Reply = Result<Response, (StatusCode, String)>;

fn bad(msg: impl Into<String>) -> (StatusCode, String) {
    (StatusCode::BAD_REQUEST, msg.into())
}

fn not_found(msg: impl Into<String>) -> (StatusCode, String) {
    (StatusCode::NOT_FOUND, msg.into())
}

/// Autentica o token de agente e confere o `env_id` do cabeçalho, que
/// devolve.
fn authorize(hub: &Hub, headers: &HeaderMap) -> Result<String, (StatusCode, String)> {
    let unauthorized = |m: &str| (StatusCode::UNAUTHORIZED, m.to_string());
    let Some(token) = bearer_token(headers) else {
        return Err(unauthorized("token inválido"));
    };
    if !token.starts_with(AGENT_TOKEN_PREFIX) {
        return Err(unauthorized(
            "token inválido: /agent/v1 pede o token de agente (cta_...), gerado com \
             cthubd agent-token <ambiente>",
        ));
    }
    let env = hub
        .authenticate_agent(&token)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| unauthorized("token inválido"))?;
    let Some(env_id) = headers
        .get(ENV_ID_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|v| !v.is_empty())
    else {
        return Err(bad("falta o cabeçalho X-Ctower-Env-Id"));
    };
    match env.env_id.as_deref() {
        Some(id) if id == env_id => Ok(id.to_string()),
        Some(id) => Err((
            StatusCode::FORBIDDEN,
            format!(
                "o token de agente de {} pertence ao ambiente {id}",
                env.name
            ),
        )),
        None => Err((
            StatusCode::FORBIDDEN,
            format!(
                "o ambiente {} ainda não enviou ao hub com o token de sync; o token de agente \
                 vale depois do primeiro envio",
                env.name
            ),
        )),
    }
}

/// Roda `f` com o `hub.db` travado, fora do runtime, depois de autorizar.
async fn with_hub<F>(st: HubState, headers: HeaderMap, f: F) -> Response
where
    F: FnOnce(&Hub) -> Reply + Send + 'static,
{
    with_env(st, headers, move |h, _| f(h)).await
}

/// Como `with_hub`, com o `env_id` autorizado.
async fn with_env<F>(st: HubState, headers: HeaderMap, f: F) -> Response
where
    F: FnOnce(&Hub, &str) -> Reply + Send + 'static,
{
    let r = tokio::task::spawn_blocking(move || {
        let hub = st.hub.lock();
        let env_id = authorize(&hub, &headers)?;
        f(&hub, &env_id)
    })
    .await;
    match r {
        Ok(Ok(resp)) => resp,
        Ok(Err((code, msg))) => (code, msg).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

fn internal(e: LedgerError) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}

/// `owner` e `repo` vão juntos.
fn project(
    owner: Option<String>,
    repo: Option<String>,
) -> Result<Option<(String, String)>, (StatusCode, String)> {
    match (owner, repo) {
        (Some(o), Some(r)) => Ok(Some((o, r))),
        (None, None) => Ok(None),
        _ => Err(bad("owner e repo vão juntos")),
    }
}

/// `GET /agent/v1/overview`.
pub(crate) async fn overview(State(st): State<HubState>, headers: HeaderMap) -> Response {
    let (t, gh) = (st.health.clone(), st.gh_sync_interval_secs);
    with_hub(st, headers, move |h| {
        let v = h
            .agent_overview(&t, gh, ct_core::now_ms())
            .map_err(internal)?;
        Ok(Json(v).into_response())
    })
    .await
}

#[derive(Debug, Deserialize)]
pub(crate) struct ProjectParams {
    include_closed: Option<bool>,
    limit: Option<u32>,
}

/// `GET /agent/v1/projects/{owner}/{repo}`.
pub(crate) async fn project_tree(
    State(st): State<HubState>,
    headers: HeaderMap,
    Path((owner, repo)): Path<(String, String)>,
    Query(q): Query<ProjectParams>,
) -> Response {
    with_hub(st, headers, move |h| {
        let v = h
            .agent_project(
                &owner,
                &repo,
                q.include_closed.unwrap_or(false),
                q.limit.unwrap_or(RANKING_LIMIT),
            )
            .map_err(internal)?
            .ok_or_else(|| not_found(format!("projeto {owner}/{repo} não está no hub")))?;
        Ok(Json(v).into_response())
    })
    .await
}

#[derive(Debug, Deserialize)]
pub(crate) struct WorkItemParams {
    events: Option<u32>,
}

/// `GET /agent/v1/work-items/{owner}/{repo}/{number}`.
pub(crate) async fn work_item(
    State(st): State<HubState>,
    headers: HeaderMap,
    Path((owner, repo, number)): Path<(String, String, i64)>,
    Query(q): Query<WorkItemParams>,
) -> Response {
    with_hub(st, headers, move |h| {
        let v = h
            .agent_work_item(&owner, &repo, number, q.events.unwrap_or(30))
            .map_err(internal)?
            .ok_or_else(|| not_found(format!("{owner}/{repo}#{number} não está no hub")))?;
        Ok(Json(v).into_response())
    })
    .await
}

#[derive(Debug, Deserialize)]
pub(crate) struct RankingParams {
    owner: Option<String>,
    repo: Option<String>,
    priority: Option<String>,
    limit: Option<u32>,
}

/// `GET /agent/v1/ranking`.
pub(crate) async fn ranking(
    State(st): State<HubState>,
    headers: HeaderMap,
    Query(q): Query<RankingParams>,
) -> Response {
    with_hub(st, headers, move |h| {
        let priority = q
            .priority
            .map(|p| p.parse::<ct_core::Priority>())
            .transpose()
            .map_err(|_| bad("priority deve ser P0, P1, P2 ou P3"))?;
        let f = RankingFilter {
            project: project(q.owner, q.repo)?,
            priority,
        };
        let v = h
            .ranking(&f, q.limit.unwrap_or(RANKING_LIMIT))
            .map_err(internal)?;
        Ok(Json(v).into_response())
    })
    .await
}

#[derive(Debug, Deserialize)]
pub(crate) struct HealthParams {
    owner: Option<String>,
    repo: Option<String>,
    min_severity: Option<String>,
}

/// `GET /agent/v1/health`: padrão `min_severity = aviso` (spec §7).
pub(crate) async fn health(
    State(st): State<HubState>,
    headers: HeaderMap,
    Query(q): Query<HealthParams>,
) -> Response {
    let (t, gh) = (st.health.clone(), st.gh_sync_interval_secs);
    with_hub(st, headers, move |h| {
        let min_severity = q
            .min_severity
            .map(|s| s.parse::<Severity>())
            .transpose()
            .map_err(bad)?
            .unwrap_or(Severity::Aviso);
        let f = HealthFilter {
            project: project(q.owner, q.repo)?,
            min_severity,
        };
        let v = h.health(&t, gh, &f, ct_core::now_ms()).map_err(internal)?;
        Ok(Json(v).into_response())
    })
    .await
}

#[derive(Debug, Deserialize)]
pub(crate) struct DecisionParams {
    owner: Option<String>,
    repo: Option<String>,
    number: Option<i64>,
    scope: Option<DecisionScope>,
    origin: Option<String>,
    unreviewed: Option<bool>,
    since: Option<i64>,
    limit: Option<u32>,
}

/// `GET /agent/v1/decisions`.
pub(crate) async fn decisions(
    State(st): State<HubState>,
    headers: HeaderMap,
    Query(q): Query<DecisionParams>,
) -> Response {
    with_hub(st, headers, move |h| {
        let project = project(q.owner, q.repo)?;
        if project.is_none() && (q.number.is_some() || q.scope.is_some()) {
            return Err(bad("number e scope pedem owner e repo"));
        }
        if q.number.is_none() && q.scope.is_some() {
            return Err(bad("scope pede number (o work item de partida)"));
        }
        let origin = q
            .origin
            .map(|o| o.parse::<ct_core::DecisionOrigin>())
            .transpose()
            .map_err(|_| bad("origin deve ser autonoma, orientada ou manual"))?;
        let f = AgentDecisionFilter {
            project,
            number: q.number,
            scope: q.scope,
            origin,
            unreviewed: q.unreviewed.unwrap_or(false),
            since: q.since,
        };
        let v = h
            .agent_decisions(&f, q.limit.unwrap_or(RANKING_LIMIT))
            .map_err(internal)?
            .ok_or_else(|| not_found("projeto ou work item não está no hub"))?;
        Ok(Json(v).into_response())
    })
    .await
}

/// `ProposalState` de um parâmetro: ausente = `pendente`, `todos` = todos.
pub(crate) fn proposal_state(s: Option<&str>) -> Result<Option<ct_core::ProposalState>, String> {
    match s.map(str::trim).filter(|s| !s.is_empty()) {
        None => Ok(Some(ct_core::ProposalState::Pendente)),
        Some("todos") => Ok(None),
        Some(s) => s.parse().map(Some).map_err(|_| {
            "state deve ser pendente, aprovada, rejeitada, obsoleta ou todos".to_string()
        }),
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct ProposalParams {
    state: Option<String>,
    owner: Option<String>,
    repo: Option<String>,
    limit: Option<u32>,
}

/// `GET /agent/v1/proposals`: padrão `state = pendente`.
pub(crate) async fn list_proposals(
    State(st): State<HubState>,
    headers: HeaderMap,
    Query(q): Query<ProposalParams>,
) -> Response {
    with_hub(st, headers, move |h| {
        let f = ProposalFilter {
            state: proposal_state(q.state.as_deref()).map_err(bad)?,
            project: project(q.owner, q.repo)?,
            number: None,
        };
        let v = h
            .list_proposals(&f, q.limit.unwrap_or(RANKING_LIMIT), ct_core::now_ms())
            .map_err(internal)?;
        Ok(Json(v).into_response())
    })
    .await
}

/// `POST /agent/v1/proposals`: cria proposta pendente; nada é aplicado.
pub(crate) async fn create_proposal(
    State(st): State<HubState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let session = headers
        .get(SESSION_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string);
    let notify = st.clone();
    let resp = with_env(st, headers, move |h, env_id| {
        let np: NewProposal =
            serde_json::from_slice(&body).map_err(|e| bad(format!("proposta inválida: {e}")))?;
        let p = h
            .create_proposal(&np, env_id, session.as_deref(), ct_core::now_ms())
            .map_err(|e| match e {
                LedgerError::Conflict(m) => (StatusCode::CONFLICT, m),
                LedgerError::NotFound(_, m) => not_found(m),
                e => internal(e),
            })?;
        Ok((StatusCode::CREATED, Json(p)).into_response())
    })
    .await;
    if resp.status() == StatusCode::CREATED {
        notify.proposals_changed();
    }
    resp
}

#[cfg(test)]
mod tests {
    use crate::{router, HubState};
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use ct_core::{DecisionOrigin, WorkItemKind};
    use ct_ledger::{GithubSnapshot, Hub, IssueRow, Ledger, NewDecision, NewProject, SharedHub};
    use http_body_util::BodyExt;
    use serde_json::{json, Value};
    use tower::ServiceExt;

    const UI: &str = "ui-secreto";

    struct Fx {
        st: HubState,
        sync: String,
        agent: String,
        env_id: String,
    }

    fn issue(n: i64, parent: Option<i64>, subs: bool, state: &str) -> IssueRow {
        IssueRow {
            gh_number: n,
            title: format!("Issue {n}"),
            state: state.into(),
            labels: vec!["bug".into()],
            parent_number: parent,
            has_sub_issues: subs,
            url: "u".into(),
            updated_at: 1,
            ..Default::default()
        }
    }

    /// Hub com `deep-ion-ai/demo` (épico 10 com as filhas 11 e 12, a 12
    /// fechada; issue 20), o work item 11 com decisão autônoma enviado pelo
    /// ambiente "wsl", e os tokens de sync e de agente dele.
    fn fx() -> Fx {
        let st = HubState::new(
            SharedHub::new(Hub::open_in_memory().unwrap()),
            UI,
            "hub",
            "hub-id",
        );
        let l = Ledger::open_in_memory().unwrap();
        let (sync, agent) = {
            let h = st.hub.lock();
            h.add_project(&ct_ledger::NewHubProject {
                owner: "deep-ion-ai".into(),
                repo: "demo".into(),
                name: "demo".into(),
                base_branch: "develop".into(),
                protected_branches: vec![],
            })
            .unwrap();
            let pid = h.find_project("deep-ion-ai", "demo").unwrap().unwrap().id;
            h.apply_github(
                &GithubSnapshot {
                    project_id: pid,
                    issues: vec![
                        issue(10, None, true, "open"),
                        issue(11, Some(10), false, "open"),
                        issue(12, Some(10), false, "closed"),
                        issue(20, None, false, "open"),
                    ],
                    prs: vec![],
                    etags: vec![],
                    synced_at: 1,
                },
                "hub-id",
            )
            .unwrap();
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
            let wi = l.ensure_work_item(p.id, 11, WorkItemKind::Issue).unwrap();
            l.record_decision(&NewDecision {
                work_item_id: wi.id,
                session_id: None,
                origin: DecisionOrigin::Autonoma,
                title: "escolha".into(),
                context: "c".into(),
                decision: "d".into(),
                consequences: None,
            })
            .unwrap();
            let sync = h.add_legacy_env_token("wsl").unwrap();
            h.bind_env_token("wsl", l.env_id()).unwrap();
            h.ingest(l.env_id(), "wsl", &l.outbox_pending(1000).unwrap())
                .unwrap();
            h.set_priority(
                "deep-ion-ai",
                "demo",
                20,
                Some(ct_core::Priority::P1),
                None,
                None,
                "hub-id",
            )
            .unwrap();
            (sync, h.add_agent_token("wsl").unwrap())
        };
        Fx {
            st,
            sync,
            agent,
            env_id: l.env_id().to_string(),
        }
    }

    async fn get(
        st: &HubState,
        path: &str,
        token: Option<&str>,
        env: Option<&str>,
    ) -> (StatusCode, Value) {
        let mut req = Request::get(path);
        if let Some(t) = token {
            req = req.header("authorization", format!("Bearer {t}"));
        }
        if let Some(e) = env {
            req = req.header("x-ctower-env-id", e);
        }
        let resp = router(st.clone())
            .oneshot(req.body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let v = serde_json::from_slice(&bytes)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into()));
        (status, v)
    }

    const ROUTES: &[&str] = &[
        "/agent/v1/overview",
        "/agent/v1/projects/deep-ion-ai/demo",
        "/agent/v1/work-items/deep-ion-ai/demo/11",
        "/agent/v1/ranking",
        "/agent/v1/health",
        "/agent/v1/decisions",
    ];

    #[tokio::test(flavor = "multi_thread")]
    async fn cada_rota_so_aceita_o_token_de_agente() {
        let f = fx();
        let env = Some(f.env_id.as_str());
        for r in ROUTES {
            let (s, v) = get(&f.st, r, Some(&f.agent), env).await;
            assert_eq!(s, StatusCode::OK, "{r}: {v}");
            for t in [None, Some(f.sync.as_str()), Some(UI), Some("cta_inventado")] {
                let (s, v) = get(&f.st, r, t, env).await;
                assert_eq!(s, StatusCode::UNAUTHORIZED, "{r} com {t:?}: {v}");
            }
            let (_, v) = get(&f.st, r, Some(&f.sync), env).await;
            assert!(v.as_str().unwrap().contains("token de agente"), "{v}");
            let (s, _) = get(&f.st, r, Some(&f.agent), None).await;
            assert_eq!(s, StatusCode::BAD_REQUEST, "{r} sem env_id");
            let (s, _) = get(&f.st, r, Some(&f.agent), Some("outro")).await;
            assert_eq!(s, StatusCode::FORBIDDEN, "{r} com outro env_id");
            // Só leitura: nenhum outro método.
            let resp = router(f.st.clone())
                .oneshot(
                    Request::post(*r)
                        .header("authorization", format!("Bearer {}", f.agent))
                        .header("x-ctower-env-id", &f.env_id)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::METHOD_NOT_ALLOWED, "{r}");
        }
        // Nenhuma rota mudou prioridade nem status.
        let h = f.st.hub.lock();
        let w11 = h
            .find_work_item("deep-ion-ai", "demo", 11)
            .unwrap()
            .unwrap();
        let w20 = h
            .find_work_item("deep-ion-ai", "demo", 20)
            .unwrap()
            .unwrap();
        assert_eq!(
            (w11.status, w11.priority),
            (ct_core::WorkItemStatus::Backlog, None)
        );
        assert_eq!(w20.priority, Some(ct_core::Priority::P1));
    }

    async fn post(
        st: &HubState,
        path: &str,
        token: &str,
        env: &str,
        body: Value,
    ) -> (StatusCode, Value) {
        let resp = router(st.clone())
            .oneshot(
                Request::post(path)
                    .header("authorization", format!("Bearer {token}"))
                    .header("x-ctower-env-id", env)
                    .header("x-ctower-session", "s-gestor")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let v = serde_json::from_slice(&bytes)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into()));
        (status, v)
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn propor_cria_pendente_sem_aplicar() {
        let f = fx();
        let mut rx = f.st.events.subscribe();
        let body = |n: i64, p: Value| {
            json!({ "kind": "priority", "owner": "deep-ion-ai", "repo": "demo", "number": n,
                    "proposed": p, "justification": "é o mais urgente" })
        };
        let path = "/agent/v1/proposals";
        let (s, v) = post(
            &f.st,
            path,
            &f.agent,
            &f.env_id,
            body(11, json!({ "priority": "P0" })),
        )
        .await;
        assert_eq!(s, StatusCode::CREATED, "{v}");
        assert_eq!(v["state"], "pendente");
        assert_eq!(v["current"], json!({ "priority": null }));
        assert_eq!(v["session_id"], "s-gestor");
        assert_eq!(v["env_name"], "wsl");
        assert_eq!(rx.try_recv().unwrap()["event"], "hub.proposals.changed");
        // Nada aplicado.
        let w =
            f.st.hub
                .lock()
                .find_work_item("deep-ion-ai", "demo", 11)
                .unwrap()
                .unwrap();
        assert_eq!(w.priority, None);
        // Recusas: nada a mudar (409), alvo inexistente (404), corpo ruim (400),
        // token de sync (401).
        let (s, _) = post(
            &f.st,
            path,
            &f.agent,
            &f.env_id,
            body(20, json!({ "priority": "P1" })),
        )
        .await;
        assert_eq!(s, StatusCode::CONFLICT);
        let (s, _) = post(
            &f.st,
            path,
            &f.agent,
            &f.env_id,
            body(99, json!({ "priority": "P1" })),
        )
        .await;
        assert_eq!(s, StatusCode::NOT_FOUND);
        let (s, _) = post(&f.st, path, &f.agent, &f.env_id, json!({ "kind": "x" })).await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        let (s, _) = post(
            &f.st,
            path,
            &f.sync,
            &f.env_id,
            body(11, json!({ "priority": "P1" })),
        )
        .await;
        assert_eq!(s, StatusCode::UNAUTHORIZED);

        let a = Some(f.agent.as_str());
        let env = Some(f.env_id.as_str());
        let (_, v) = get(&f.st, path, a, env).await;
        assert_eq!(v["proposals"].as_array().unwrap().len(), 1);
        let (_, v) = get(&f.st, "/agent/v1/proposals?state=aprovada", a, env).await;
        assert_eq!(v["proposals"].as_array().unwrap().len(), 0);
        let (s, _) = get(&f.st, "/agent/v1/proposals?state=x", a, env).await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        let (_, v) = get(&f.st, "/agent/v1/work-items/deep-ion-ai/demo/11", a, env).await;
        assert_eq!(v["proposals"][0]["state"], "pendente");
        // O agente não tem rota para decidir.
        let id = v["proposals"][0]["id"].as_i64().unwrap();
        for p in [
            format!("/agent/v1/proposals/{id}"),
            format!("/agent/v1/proposals/{id}/decide"),
        ] {
            let (s, _) = post(&f.st, &p, &f.agent, &f.env_id, json!({ "approve": true })).await;
            assert!(
                matches!(s, StatusCode::NOT_FOUND | StatusCode::METHOD_NOT_ALLOWED),
                "{p}: {s}"
            );
        }
        assert_eq!(
            f.st.hub.lock().proposal(id).unwrap().unwrap().state,
            ct_core::ProposalState::Pendente
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn token_de_agente_nao_vale_no_sync() {
        let f = fx();
        let req = |path: &str| {
            Request::post(path)
                .header("authorization", format!("Bearer {}", f.agent))
                .header("x-ctower-env-id", &f.env_id)
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({ "env_id": f.env_id, "entries": [], "owner": "deep-ion-ai",
                            "repo": "demo", "number": 11, "scope": "work_item" })
                    .to_string(),
                ))
                .unwrap()
        };
        for path in ["/sync/push", "/sync/query/decisions"] {
            let resp = router(f.st.clone()).oneshot(req(path)).await.unwrap();
            assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "{path}");
            let body = resp.into_body().collect().await.unwrap().to_bytes();
            assert!(
                String::from_utf8_lossy(&body).contains("/agent/v1"),
                "{path}"
            );
        }
        let (s, _) = get(&f.st, "/sync/config", Some(&f.agent), Some(&f.env_id)).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn rotacao_e_revogacao() {
        let f = fx();
        let env = Some(f.env_id.as_str());
        let novo = f.st.hub.lock().add_agent_token("wsl").unwrap();
        let (s, _) = get(&f.st, "/agent/v1/overview", Some(&f.agent), env).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED, "o anterior deixa de valer");
        let (s, _) = get(&f.st, "/agent/v1/overview", Some(&novo), env).await;
        assert_eq!(s, StatusCode::OK);
        assert!(f.st.hub.lock().revoke_agent_token("wsl").unwrap());
        let (s, _) = get(&f.st, "/agent/v1/overview", Some(&novo), env).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED);
        // O sync continua.
        assert!(f.st.hub.lock().authenticate(&f.sync).unwrap().is_some());
        // revoke-env apaga os dois.
        let outro = f.st.hub.lock().add_agent_token("wsl").unwrap();
        assert!(f.st.hub.lock().revoke_env_token("wsl").unwrap());
        let (s, _) = get(&f.st, "/agent/v1/overview", Some(&outro), env).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED);
        assert!(
            f.st.hub.lock().add_agent_token("wsl").is_err(),
            "sem token de sync"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn ambiente_que_ainda_nao_enviou() {
        let f = fx();
        let t = {
            let h = f.st.hub.lock();
            h.add_legacy_env_token("mac").unwrap();
            h.add_agent_token("mac").unwrap()
        };
        let (s, v) = get(&f.st, "/agent/v1/overview", Some(&t), Some("qualquer")).await;
        assert_eq!(s, StatusCode::FORBIDDEN);
        assert!(v.as_str().unwrap().contains("primeiro envio"), "{v}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn respostas_e_parametros() {
        let f = fx();
        let env = Some(f.env_id.as_str());
        let a = Some(f.agent.as_str());
        let (_, v) = get(&f.st, "/agent/v1/overview", a, env).await;
        let p = &v["projects"][0];
        assert_eq!(p["repo"], "demo");
        assert_eq!(p["by_status"]["backlog"], 3);
        assert_eq!(p["by_priority"]["P1"], 1);
        assert_eq!(p["by_priority"]["nao_triado"], 2);
        assert_eq!(p["pending_decisions"], 1);
        assert_eq!(v["envs"][0]["name"], "wsl");

        let (_, v) = get(&f.st, "/agent/v1/projects/deep-ion-ai/demo", a, env).await;
        let items = v["items"].as_array().unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0]["number"], 10);
        assert_eq!(items[0]["kind"], "epic");
        // A filha fechada fica de fora sem include_closed.
        assert_eq!(items[0]["children"].as_array().unwrap().len(), 1);
        assert_eq!(items[0]["children"][0]["pending_decisions"], 1);
        assert_eq!(items[1]["priority"], "P1");
        assert!(items[0].get("body").is_none());
        let (_, v) = get(
            &f.st,
            "/agent/v1/projects/deep-ion-ai/demo?include_closed=true&limit=2",
            a,
            env,
        )
        .await;
        assert_eq!(v["truncated"], true);
        assert_eq!(v["items"][0]["children"].as_array().unwrap().len(), 1);
        let (s, _) = get(&f.st, "/agent/v1/projects/x/y", a, env).await;
        assert_eq!(s, StatusCode::NOT_FOUND);

        let (_, v) = get(&f.st, "/agent/v1/work-items/deep-ion-ai/demo/11", a, env).await;
        assert_eq!(v["parent"]["number"], 10);
        assert_eq!(v["decisions"][0]["title"], "escolha");
        assert!(!v["events"].as_array().unwrap().is_empty());
        let (_, v) = get(&f.st, "/agent/v1/work-items/deep-ion-ai/demo/10", a, env).await;
        assert_eq!(v["status"], "backlog");
        assert_eq!(v["children"].as_array().unwrap().len(), 2);
        let (s, _) = get(&f.st, "/agent/v1/work-items/deep-ion-ai/demo/99", a, env).await;
        assert_eq!(s, StatusCode::NOT_FOUND);

        let (_, v) = get(&f.st, "/agent/v1/ranking?limit=1", a, env).await;
        assert_eq!(v["items"][0]["number"], 20);
        assert_eq!(v["truncated"], true);
        let (s, _) = get(&f.st, "/agent/v1/ranking?priority=P7", a, env).await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        let (s, _) = get(&f.st, "/agent/v1/ranking?owner=x", a, env).await;
        assert_eq!(s, StatusCode::BAD_REQUEST);

        // Saúde: padrão aviso (o nao_triado é info e fica de fora).
        let (_, v) = get(&f.st, "/agent/v1/health", a, env).await;
        assert_eq!(v["counts"]["info"], 0);
        let (s, _) = get(&f.st, "/agent/v1/health?min_severity=grave", a, env).await;
        assert_eq!(s, StatusCode::BAD_REQUEST);

        let (_, v) = get(
            &f.st,
            "/agent/v1/decisions?origin=autonoma&unreviewed=true",
            a,
            env,
        )
        .await;
        assert_eq!(v["decisions"][0]["number"], 11);
        // Work item de partida que não está no hub.
        let (s, _) = get(
            &f.st,
            "/agent/v1/decisions?owner=deep-ion-ai&repo=demo&number=10&scope=epic",
            a,
            env,
        )
        .await;
        assert_eq!(s, StatusCode::NOT_FOUND);
        let (_, v) = get(
            &f.st,
            "/agent/v1/decisions?owner=deep-ion-ai&repo=demo&number=11",
            a,
            env,
        )
        .await;
        assert_eq!(v["decisions"].as_array().unwrap().len(), 1);
        let (_, v) = get(&f.st, "/agent/v1/decisions?origin=orientada", a, env).await;
        assert_eq!(v["decisions"].as_array().unwrap().len(), 0);
        let (s, _) = get(&f.st, "/agent/v1/decisions?number=11", a, env).await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        let (s, _) = get(&f.st, "/agent/v1/decisions?origin=x", a, env).await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
    }
}
