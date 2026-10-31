//! Servidor MCP do ledger (spec §8): JSON-RPC 2.0 sobre HTTP "streamable",
//! sempre respondendo `application/json` (sem SSE). A sessão vem do cabeçalho
//! `X-Ctower-Session`; o work item é inferido dela.

use crate::hub_sync::{DecisionsQuery, HubSender};
use crate::{bearer_ok, unauthorized, AppState};
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use ct_core::{DecisionOrigin, ProjectKind, WorkItemKind, WorkItemStatus};
use ct_ledger::{DecisionScope, Ledger, NewDecision, Session, WorkItem};
use ct_sessions::Sessions;
use serde_json::{json, Value};

const DEFAULT_PROTOCOL: &str = "2025-06-18";

const INSTRUCTIONS: &str = "Ledger do Control Tower para esta sessão. \
Antes de começar, consulte decisões anteriores com ledger_list_decisions. \
Registre toda decisão relevante com ledger_record_decision: use origin=\"orientada\" \
quando a decisão foi definida pelo humano nesta conversa e origin=\"autonoma\" quando \
você decidiu sozinho (decisões autônomas são revisadas no PR, ou MR no GitLab, para a branch base). \
Cite issues pelo ref de ledger_get_context (owner/repo#n no GitHub, a chave no Jira). \
As regras do projeto e do épico já estão no seu system prompt; releia as vigentes com rules_get. \
Para mudá-las, proponha o texto completo novo com rules_propose: a proposta fica pendente até o \
usuário aprovar, então nunca diga que as regras mudaram antes disso.";

pub(crate) async fn method_not_allowed() -> StatusCode {
    StatusCode::METHOD_NOT_ALLOWED
}

pub(crate) async fn handle(
    State(st): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !bearer_ok(&headers, &st.hook_token) {
        return unauthorized();
    }
    let session_id = headers
        .get("x-ctower-session")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let Ok(req) = serde_json::from_slice::<Value>(&body) else {
        return Json(rpc_error(Value::Null, -32700, "JSON inválido")).into_response();
    };
    // Notificações (sem `id`) não têm resposta.
    let Some(id) = req.get("id").cloned() else {
        return StatusCode::ACCEPTED.into_response();
    };
    let method = req
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let params = req.get("params").cloned().unwrap_or(Value::Null);
    let sessions = st.sessions.clone();
    let hub = st.hub_sender.clone();
    let result = tokio::task::spawn_blocking(move || {
        dispatch(&sessions, hub.as_deref(), &session_id, &method, &params)
    })
    .await;
    let resp = match result {
        Ok(Ok(v)) => json!({ "jsonrpc": "2.0", "id": id, "result": v }),
        Ok(Err((code, msg))) => rpc_error(id, code, &msg),
        Err(e) => rpc_error(id, -32603, &e.to_string()),
    };
    Json(resp).into_response()
}

fn rpc_error(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

type RpcResult = Result<Value, (i64, String)>;

fn dispatch(
    sessions: &Sessions,
    hub: Option<&HubSender>,
    session_id: &str,
    method: &str,
    params: &Value,
) -> RpcResult {
    let session = sessions
        .ledger()
        .get_session(session_id)
        .map_err(|_| (-32001, format!("sessão desconhecida: {session_id:?}")))?;
    if is_gestor(sessions, &session) {
        return dispatch_gestor(sessions, hub, &session, method, params);
    }
    // Escopo épico/projeto com hub: consulta a visão consolidada sem segurar
    // o ledger durante a rede.
    if method == "tools/call"
        && params.get("name").and_then(Value::as_str) == Some("ledger_list_decisions")
    {
        let scope = params
            .pointer("/arguments/scope")
            .and_then(Value::as_str)
            .unwrap_or("work_item");
        if let (Some(hub), Some(sc)) = (hub, hub_scope(scope)) {
            return Ok(tool_result(decisions_via_hub(
                sessions, &session, scope, sc, hub,
            )));
        }
    }
    let l = sessions.ledger();
    match method {
        "initialize" => Ok(json!({
            "protocolVersion": params.get("protocolVersion").and_then(Value::as_str).unwrap_or(DEFAULT_PROTOCOL),
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "ctower-ledger", "version": env!("CARGO_PKG_VERSION") },
            "instructions": INSTRUCTIONS,
        })),
        "ping" => Ok(json!({})),
        "tools/list" => {
            let epic = l
                .get_work_item(session.work_item_id)
                .is_ok_and(|wi| wi.kind == WorkItemKind::Epic);
            Ok(json!({ "tools": tools(epic) }))
        }
        "tools/call" => {
            let name = params
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let args = params.get("arguments").cloned().unwrap_or(json!({}));
            Ok(tool_result(call_tool(&l, &session, name, &args)))
        }
        _ => Err((-32601, format!("método desconhecido: {method}"))),
    }
}

/// A sessão é do gestor (projeto sentinela `_ctower/gestor`).
fn is_gestor(sessions: &Sessions, s: &Session) -> bool {
    let l = sessions.ledger();
    l.get_work_item(s.work_item_id)
        .and_then(|wi| l.get_project(wi.project_id))
        .is_ok_and(|p| p.kind == ProjectKind::Gestor)
}

/// MCP do gestor: instruções e ferramentas próprias (`portfolio_*`).
fn dispatch_gestor(
    sessions: &Sessions,
    hub: Option<&HubSender>,
    session: &Session,
    method: &str,
    params: &Value,
) -> RpcResult {
    match method {
        "initialize" => Ok(json!({
            "protocolVersion": params.get("protocolVersion").and_then(Value::as_str).unwrap_or(DEFAULT_PROTOCOL),
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "ctower-gestor", "version": env!("CARGO_PKG_VERSION") },
            "instructions": crate::mcp_gestor::instructions(),
        })),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": crate::mcp_gestor::tools() })),
        "tools/call" => {
            let name = params
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let args = params.get("arguments").cloned().unwrap_or(json!({}));
            if let Some(r) = crate::mcp_gestor::portfolio(hub, &session.id, name, &args) {
                return Ok(tool_result(r));
            }
            if let Some(r) = crate::mcp_gestor::ledger(sessions, hub, session, name, &args) {
                return Ok(tool_result(r));
            }
            match name {
                "ledger_record_decision" | "ledger_list_decisions" => {
                    let l = sessions.ledger();
                    Ok(tool_result(call_tool(&l, session, name, &args)))
                }
                _ => Ok(tool_result(Err(format!("tool desconhecida: {name}")))),
            }
        }
        _ => Err((-32601, format!("método desconhecido: {method}"))),
    }
}

fn tool_result(r: Result<Value, String>) -> Value {
    match r {
        Ok(v) => json!({
            "content": [{ "type": "text", "text": v.to_string() }],
            "structuredContent": v,
            "isError": false,
        }),
        Err(msg) => json!({ "content": [{ "type": "text", "text": msg }], "isError": true }),
    }
}

fn hub_scope(scope: &str) -> Option<DecisionScope> {
    match scope {
        "epic" => Some(DecisionScope::Epic),
        "project" => Some(DecisionScope::Project),
        _ => None,
    }
}

/// Work items locais do escopo.
fn scope_ids(l: &Ledger, wi: &WorkItem, scope: &str) -> Result<Vec<i64>, String> {
    let e = |e: ct_ledger::LedgerError| e.to_string();
    Ok(match scope {
        "work_item" => vec![wi.id],
        "epic" => l
            .epic_work_items(wi.id)
            .map_err(e)?
            .iter()
            .map(|w| w.id)
            .collect(),
        "project" => l
            .list_work_items(wi.project_id)
            .map_err(e)?
            .iter()
            .map(|w| w.id)
            .collect(),
        o => return Err(format!("scope inválido: {o}")),
    })
}

/// `ledger_list_decisions` (épico/projeto) pela visão do hub. Se o hub não
/// responder, devolve as decisões locais com `partial: true` e o motivo:
/// nunca trava o agente.
fn decisions_via_hub(
    sessions: &Sessions,
    s: &Session,
    scope: &str,
    hub_scope: DecisionScope,
    hub: &HubSender,
) -> Result<Value, String> {
    let e = |e: ct_ledger::LedgerError| e.to_string();
    let (query, ids) = {
        let l = sessions.ledger();
        let wi = l.get_work_item(s.work_item_id).map_err(e)?;
        let p = l.get_project(wi.project_id).map_err(e)?;
        let ids = scope_ids(&l, &wi, scope)?;
        let q = DecisionsQuery {
            owner: p.gh_owner,
            repo: p.gh_repo,
            number: wi.gh_number,
            scope: hub_scope,
        };
        (q, ids)
    };
    match hub.query_decisions(&query) {
        Ok(decisions) => Ok(json!({ "decisions": decisions, "source": "hub" })),
        Err(reason) => {
            let decisions = sessions.ledger().list_decisions(&ids, false).map_err(e)?;
            Ok(json!({
                "decisions": decisions,
                "source": "local",
                "partial": true,
                "reason": reason,
            }))
        }
    }
}

/// Ferramentas da sessão; `epic` = a sessão é de um épico e ganha
/// `ledger_start_sub_issue` (#458).
fn tools(epic: bool) -> Value {
    let mut tools = json!([
        {
            "name": "ledger_record_decision",
            "description": "Registra uma decisão relevante tomada nesta sessão, vinculada ao work item.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "title": { "type": "string" },
                    "context": { "type": "string", "description": "Por que a decisão foi necessária." },
                    "decision": { "type": "string", "description": "O que foi decidido." },
                    "consequences": { "type": "string" },
                    "origin": { "type": "string", "enum": ["autonoma", "orientada"],
                                "description": "orientada = definida pelo humano na conversa; autonoma = decidida por você." }
                },
                "required": ["title", "context", "decision", "origin"]
            }
        },
        {
            "name": "ledger_list_decisions",
            "description": "Lista decisões já registradas.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "scope": { "type": "string", "enum": ["work_item", "epic", "project"], "default": "work_item" }
                }
            }
        },
        {
            "name": "ledger_set_status",
            "description": "Muda o status do work item (ex.: bloqueado aguardando algo externo).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "status": { "type": "string", "enum": ["bloqueado", "pausado", "em_execucao"] },
                    "reason": { "type": "string" }
                },
                "required": ["status", "reason"]
            }
        },
        {
            "name": "ledger_get_context",
            "description": "Work item, issue e épico pai, branch de trabalho, branches protegidas, repositório e versão das regras do agente desta sessão.",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "rules_get",
            "description": "Regras do agente vigentes desta sessão: as do projeto e as do épico (o da sessão ou o pai da issue), com a versão de cada uma.",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "rules_propose",
            "description": "Propõe o texto completo novo das regras do projeto ou do épico desta sessão. Cria uma proposta pendente: só vale depois que o usuário aprovar na UI do hub. Limite de 16 KiB por escopo.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "scope": { "type": "string", "enum": ["project", "epic"] },
                    "content": { "type": "string", "description": "Texto completo novo do escopo, em Markdown (substitui o vigente inteiro)." },
                    "reason": { "type": "string", "description": "Por que mudar: o que aconteceu nesta sessão." }
                },
                "required": ["scope", "content", "reason"]
            }
        }
    ]);
    if epic {
        tools.as_array_mut().expect("lista").push(json!({
            "name": "ledger_start_sub_issue",
            "description": "Marca a sub-issue #number do épico desta sessão como em execução. Chame ao começar cada sub-issue (também no retrabalho de uma que já aguarda o épico ou está concluída).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "number": { "type": "integer", "description": "Número da sub-issue (uma das sub_issues de ledger_get_context)." },
                    "reason": { "type": "string" }
                },
                "required": ["number"]
            }
        }));
    }
    tools
}

/// Sub-issues do épico no espelho, com o status do work item; sem work
/// item, o mesmo padrão da UI: `concluido` se fechada, senão `backlog`.
fn sub_issues(l: &Ledger, epic: &WorkItem) -> Result<Vec<Value>, ct_ledger::LedgerError> {
    let mut out = Vec::new();
    for i in l.list_issues(epic.project_id)? {
        if i.parent_number != Some(epic.gh_number) {
            continue;
        }
        let status = match l.find_work_item(epic.project_id, i.gh_number)? {
            Some(w) => w.status,
            None if i.state == "closed" => WorkItemStatus::Concluido,
            None => WorkItemStatus::Backlog,
        };
        out.push(json!({
            "number": i.gh_number,
            "title": i.title,
            "state": i.state,
            "status": status,
        }));
    }
    Ok(out)
}

fn call_tool(l: &Ledger, s: &Session, name: &str, args: &Value) -> Result<Value, String> {
    let arg = |k: &str| args.get(k).and_then(Value::as_str);
    let req = |k: &str| {
        arg(k)
            .filter(|v| !v.trim().is_empty())
            .ok_or_else(|| format!("parâmetro obrigatório: {k}"))
    };
    let e = |e: ct_ledger::LedgerError| e.to_string();
    let wi = l.get_work_item(s.work_item_id).map_err(e)?;
    match name {
        "ledger_record_decision" => {
            let origin = match req("origin")? {
                "autonoma" => DecisionOrigin::Autonoma,
                "orientada" => DecisionOrigin::Orientada,
                o => return Err(format!("origin inválida: {o} (use autonoma ou orientada)")),
            };
            let d = l
                .record_decision(&NewDecision {
                    work_item_id: wi.id,
                    session_id: Some(s.id.clone()),
                    origin,
                    title: req("title")?.into(),
                    context: req("context")?.into(),
                    decision: req("decision")?.into(),
                    consequences: arg("consequences").map(Into::into),
                })
                .map_err(e)?;
            Ok(json!(d))
        }
        "ledger_list_decisions" => {
            let ids = scope_ids(l, &wi, arg("scope").unwrap_or("work_item"))?;
            Ok(json!({ "decisions": l.list_decisions(&ids, false).map_err(e)? }))
        }
        "ledger_set_status" => {
            let status = match req("status")? {
                "bloqueado" => WorkItemStatus::Bloqueado,
                "pausado" => WorkItemStatus::Pausado,
                "em_execucao" => WorkItemStatus::EmExecucao,
                o => return Err(format!("status inválido: {o}")),
            };
            let reason = req("reason")?;
            l.set_work_item_status(wi.id, status, &format!("mcp: {reason}"))
                .map_err(e)?;
            Ok(json!({ "work_item_id": wi.id, "status": status }))
        }
        "ledger_start_sub_issue" => {
            let number = args
                .get("number")
                .and_then(Value::as_i64)
                .ok_or("parâmetro obrigatório: number (inteiro)")?;
            let (sub, changed) = l
                .start_sub_issue(wi.id, number, Some(&s.id), arg("reason"))
                .map_err(e)?;
            Ok(json!({
                "number": number,
                "work_item_id": sub.id,
                "status": sub.status,
                "changed": changed,
            }))
        }
        "ledger_get_context" => {
            let p = l.get_project(wi.project_id).map_err(e)?;
            let issue = l.get_issue(wi.project_id, wi.gh_number).map_err(e)?;
            let rules = l.session_rules(&p, &wi).map_err(e)?;
            let mut ctx = json!({
                "ref": p.item_ref(wi.gh_number),
                // Rastreador e forja (#351): a sessão Jira usa `ref` como a
                // chave (`PROJ-5450`) e abre MR, não PR.
                "tracker": p.tracker.kind(),
                "forge": p.forge.kind(),
                "change_label": ct_core::change_label(p.forge.kind()),
                "work_item": { "id": wi.id, "number": wi.gh_number, "kind": wi.kind, "status": wi.status },
                "issue": issue.as_ref().map(|i| json!({
                    "title": i.title,
                    // Sem link no espelho, o do rastreador (`/browse/` no Jira).
                    "url": if i.url.is_empty() {
                        ct_core::item_url(&p.tracker_ref(), wi.gh_number)
                    } else {
                        i.url.clone()
                    },
                    "state": i.state,
                })),
                "parent_epic": issue.and_then(|i| i.parent_number),
                "repository": format!("{}/{}", p.gh_owner, p.gh_repo),
                "branch": wi.branch,
                "worktree": wi.worktree_path,
                "base_branch": p.base_branch,
                "protected_branches": p.protected_branches,
                "rules": {
                    "project": rules.project.as_ref().map(|r| json!({ "version": r.version })),
                    "epic": rules.epic_number.map(|n| json!({
                        "number": n,
                        "ref": p.item_ref(n),
                        "version": rules.epic.as_ref().map(|r| r.version),
                    })),
                },
            });
            // Sessão do épico (#458): o andamento das sub-issues.
            if wi.kind == WorkItemKind::Epic {
                ctx["sub_issues"] = json!(sub_issues(l, &wi).map_err(e)?);
            }
            Ok(ctx)
        }
        "rules_get" => {
            let p = l.get_project(wi.project_id).map_err(e)?;
            let rules = l.session_rules(&p, &wi).map_err(e)?;
            Ok(json!({
                "project": {
                    "name": p.name,
                    "version": rules.project.as_ref().map(|r| r.version),
                    "content": rules.project.as_ref().map(|r| r.content.as_str()),
                },
                "epic": rules.epic_number.map(|n| json!({
                    "number": n,
                    "ref": p.item_ref(n),
                    "version": rules.epic.as_ref().map(|r| r.version),
                    "content": rules.epic.as_ref().map(|r| r.content.as_str()),
                })),
                "max_bytes": ct_ledger::RULES_MAX_BYTES,
            }))
        }
        "rules_propose" => {
            let epic = match req("scope")? {
                "project" => 0,
                "epic" => l.rules_epic(&wi).map_err(e)?.ok_or_else(|| {
                    "esta sessão não tem épico: proponha com scope=\"project\"".to_string()
                })?,
                o => return Err(format!("scope inválido: {o} (use project ou epic)")),
            };
            // `change` é o nome da issue #357; a spec usa `content`.
            let content = arg("content")
                .or_else(|| arg("change"))
                .ok_or("parâmetro obrigatório: content (texto completo novo)")?;
            if content.trim().is_empty() {
                return Err("content vazio: proponha o texto completo novo do escopo".into());
            }
            let c = l
                .enqueue_rules_proposal(wi.project_id, epic, content, req("reason")?, Some(&s.id))
                .map_err(e)?;
            Ok(json!({
                "uid": c.uid,
                "state": "pendente",
                "scope": if epic == 0 { "project" } else { "epic" },
                "base_version": c.base_version,
                "message": "Proposta enviada ao hub. Fica pendente até o usuário aprovar na UI do \
                            hub; até lá valem as regras atuais.",
            }))
        }
        _ => Err(format!("tool desconhecida: {name}")),
    }
}

#[cfg(test)]
mod tests {
    use crate::test_util::*;
    use axum::http::StatusCode;
    use ct_core::WorkItemStatus;
    use ct_sessions::test_support::FakeClaude;
    use serde_json::{json, Value};

    async fn rpc(srv: &Srv, session: &str, method: &str, params: Value) -> Value {
        let (st, v) = srv
            .post(
                "/mcp",
                Some(HOOK),
                &[("x-ctower-session", session)],
                json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params }),
            )
            .await;
        assert_eq!(st, StatusCode::OK);
        v
    }

    async fn call(srv: &Srv, session: &str, tool: &str, args: Value) -> Value {
        let v = rpc(
            srv,
            session,
            "tools/call",
            json!({ "name": tool, "arguments": args }),
        )
        .await;
        v["result"].clone()
    }

    #[tokio::test]
    async fn autenticacao_e_sessao() {
        let srv = Srv::new(FakeClaude::exit(0));
        let body = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" });
        for t in [None, Some(UI), Some("x")] {
            let (st, _) = srv.post("/mcp", t, &[], body.clone()).await;
            assert_eq!(st, StatusCode::UNAUTHORIZED);
        }
        let v = rpc(&srv, "nao-existe", "tools/list", json!({})).await;
        assert_eq!(v["error"]["code"], -32001);
    }

    #[tokio::test]
    async fn initialize_lista_e_notificacao() {
        let srv = Srv::new(FakeClaude::sleep(30));
        let s = srv.env.sessions.start(srv.env.request(1, "A")).unwrap();
        let v = rpc(
            &srv,
            &s.id,
            "initialize",
            json!({ "protocolVersion": "2025-03-26" }),
        )
        .await;
        assert_eq!(v["result"]["protocolVersion"], "2025-03-26");
        assert!(v["result"]["instructions"]
            .as_str()
            .unwrap()
            .contains("orientada"));
        let v = rpc(&srv, &s.id, "tools/list", json!({})).await;
        let names: Vec<_> = v["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(
            names,
            [
                "ledger_record_decision",
                "ledger_list_decisions",
                "ledger_set_status",
                "ledger_get_context",
                "rules_get",
                "rules_propose"
            ]
        );
        let (st, _) = srv
            .post(
                "/mcp",
                Some(HOOK),
                &[("x-ctower-session", &s.id)],
                json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
            )
            .await;
        assert_eq!(st, StatusCode::ACCEPTED);
        let v = rpc(&srv, &s.id, "inventado", json!({})).await;
        assert_eq!(v["error"]["code"], -32601);
        srv.env.sessions.kill(&s.id).unwrap();
    }

    /// Escopo épico/projeto consulta o hub; hub fora = decisões locais com
    /// `partial: true`, sem travar.
    #[tokio::test(flavor = "multi_thread")]
    async fn list_decisions_via_hub_com_fallback_parcial() {
        use crate::hub_sync::{HubSender, HubTarget};
        use ct_ledger::{Hub, Ledger, NewDecision, NewProject, SharedHub};
        use std::sync::Arc;
        use std::time::Duration;
        let mut srv = Srv::new(FakeClaude::sleep(30));
        let s = srv.env.sessions.start(srv.env.request(1, "A")).unwrap();
        call(
            &srv,
            &s.id,
            "ledger_record_decision",
            json!({ "title": "local", "context": "c", "decision": "d", "origin": "autonoma" }),
        )
        .await;
        let dead = crate::test_util::DeadHub::new();
        srv.state.hub_sender = Some(Arc::new(HubSender::new(
            srv.env.sessions.clone(),
            HubTarget {
                url: dead.url.clone(),
                token: "t".into(),
            },
            Duration::from_secs(5),
        )));
        let r = call(
            &srv,
            &s.id,
            "ledger_list_decisions",
            json!({ "scope": "epic" }),
        )
        .await;
        let v = &r["structuredContent"];
        assert_eq!(v["partial"], true, "{v}");
        assert_eq!(v["source"], "local");
        assert!(v["reason"].as_str().unwrap().contains("hub inacessível"));
        assert_eq!(v["decisions"].as_array().unwrap().len(), 1);
        // Escopo do work item continua só local, sem marcação.
        let r = call(&srv, &s.id, "ledger_list_decisions", json!({})).await;
        assert!(r["structuredContent"]["partial"].is_null());

        // Hub (`cthubd`) no ar com uma decisão de outro ambiente.
        let hub = SharedHub::new(Hub::open_in_memory().unwrap());
        hub.lock()
            .add_project(&ct_ledger::NewHubProject {
                owner: "deep-ion-ai".into(),
                repo: "demo".into(),
                name: "demo".into(),
                base_branch: "develop".into(),
                protected_branches: vec![],
            })
            .unwrap();
        let outro = Ledger::open_in_memory().unwrap();
        let p = outro
            .add_project(&NewProject {
                name: "demo".into(),
                gh_owner: "deep-ion-ai".into(),
                gh_repo: "demo".into(),
                repo_path: "/longe".into(),
                base_branch: "develop".into(),
                protected_branches: vec![],
            })
            .unwrap();
        let wi = outro
            .ensure_work_item(p.id, 1, ct_core::WorkItemKind::Issue)
            .unwrap();
        outro
            .record_decision(&NewDecision {
                work_item_id: wi.id,
                session_id: None,
                origin: ct_core::DecisionOrigin::Autonoma,
                title: "remota".into(),
                context: "c".into(),
                decision: "d".into(),
                consequences: None,
            })
            .unwrap();
        hub.lock()
            .ingest(
                outro.env_id(),
                "outro",
                &outro.outbox_pending(1000).unwrap(),
            )
            .unwrap();
        let token = hub.lock().add_legacy_env_token("a").unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        crate::test_util::serve_hub(hub.clone(), listener).await;
        let sender = Arc::new(HubSender::new(
            srv.env.sessions.clone(),
            HubTarget { url, token },
            Duration::from_secs(5),
        ));
        {
            let s = Arc::clone(&sender);
            tokio::task::spawn_blocking(move || s.push_pending())
                .await
                .unwrap()
                .unwrap();
        }
        srv.state.hub_sender = Some(sender);
        let r = call(
            &srv,
            &s.id,
            "ledger_list_decisions",
            json!({ "scope": "project" }),
        )
        .await;
        let v = &r["structuredContent"];
        assert_eq!(v["source"], "hub", "{v}");
        let mut titulos: Vec<&str> = v["decisions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|d| d["title"].as_str().unwrap())
            .collect();
        titulos.sort();
        assert_eq!(titulos, vec!["local", "remota"]);
        srv.env.sessions.kill(&s.id).unwrap();
    }

    /// Sessão do gestor: `tools/list` próprio, `portfolio_*` repassadas ao
    /// hub (`cthubd` de verdade) com o token de agente, `ledger_*` restrito,
    /// e as decisões do gestor chegam ao sentinela do hub.
    #[tokio::test(flavor = "multi_thread")]
    async fn gestor_tem_portfolio_e_ledger_restrito() {
        use crate::hub_sync::{HubSender, HubTarget};
        use ct_ledger::{GithubSnapshot, Hub, IssueRow, SharedHub};
        use std::sync::Arc;
        use std::time::Duration;
        let mut srv = Srv::new(FakeClaude::sleep(30));
        let comum = srv.env.sessions.start(srv.env.request(1, "A")).unwrap();
        let g = srv
            .env
            .sessions
            .start_gestor(
                &ct_sessions::GestorScope::Portfolio,
                "oi",
                None,
                &Default::default(),
            )
            .unwrap();
        let names = |v: &Value| -> Vec<String> {
            v["result"]["tools"]
                .as_array()
                .unwrap()
                .iter()
                .map(|t| t["name"].as_str().unwrap().to_string())
                .collect()
        };
        let lista = names(&rpc(&srv, &g.id, "tools/list", json!({})).await);
        for t in [
            "portfolio_overview",
            "portfolio_project",
            "portfolio_work_item",
            "portfolio_ranking",
            "portfolio_health",
            "portfolio_list_decisions",
            "portfolio_list_proposals",
            "portfolio_propose",
            "ledger_record_decision",
            "ledger_list_decisions",
            "ledger_get_context",
        ] {
            assert!(lista.iter().any(|n| n == t), "{t}: {lista:?}");
        }
        assert!(!lista.iter().any(|n| n == "ledger_set_status"));
        assert!(!lista.iter().any(|n| n == "ledger_start_sub_issue"));
        let comuns = names(&rpc(&srv, &comum.id, "tools/list", json!({})).await);
        assert!(
            comuns.iter().all(|n| !n.starts_with("portfolio_")),
            "{comuns:?}"
        );
        let r = call(&srv, &comum.id, "portfolio_overview", json!({})).await;
        assert_eq!(r["isError"], true, "fora do gestor não existe");
        let init = rpc(&srv, &g.id, "initialize", json!({})).await;
        assert!(init["result"]["instructions"]
            .as_str()
            .unwrap()
            .contains("portfolio_propose"));

        // Sem hub: erro claro, sem resposta parcial.
        let r = call(&srv, &g.id, "portfolio_overview", json!({})).await;
        assert_eq!(r["isError"], true);
        assert!(r["content"][0]["text"]
            .as_str()
            .unwrap()
            .starts_with("hub indisponível"));

        // ledger_* do gestor.
        let r = call(
            &srv,
            &g.id,
            "ledger_set_status",
            json!({ "status": "pausado", "reason": "x" }),
        )
        .await;
        assert!(r["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("portfolio_propose"));
        let r = call(
            &srv,
            &g.id,
            "ledger_list_decisions",
            json!({ "scope": "epic" }),
        )
        .await;
        assert_eq!(r["isError"], true);
        let r = call(
            &srv,
            &g.id,
            "ledger_record_decision",
            json!({ "title": "P0 em deep-ion-ai/demo#7", "context": "c", "decision": "d", "origin": "orientada" }),
        )
        .await;
        assert_eq!(r["isError"], false, "{r}");
        let r = call(&srv, &g.id, "ledger_list_decisions", json!({})).await;
        assert_eq!(
            r["structuredContent"]["decisions"]
                .as_array()
                .unwrap()
                .len(),
            1
        );

        // Hub no ar, com o token de agente gravado no ambiente.
        let hub = SharedHub::new(Hub::open_in_memory().unwrap());
        let env_id = srv.env.sessions.ledger().env_id().to_string();
        let (sync, agent) = {
            let h = hub.lock();
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
                    issues: vec![IssueRow {
                        gh_number: 7,
                        title: "Issue 7".into(),
                        state: "open".into(),
                        labels: vec![],
                        parent_number: None,
                        has_sub_issues: false,
                        url: "u".into(),
                        updated_at: 1,
                        ..Default::default()
                    }],
                    prs: vec![],
                    etags: vec![],
                    synced_at: 1,
                    gone: Vec::new(),
                },
                "hub-id",
            )
            .unwrap();
            let sync = h.add_legacy_env_token("teste").unwrap();
            h.bind_env_token("teste", &env_id).unwrap();
            (sync, h.add_agent_token("teste").unwrap())
        };
        std::fs::write(srv.env.cfg.dirs.hub_agent_token_file(), &agent).unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        crate::test_util::serve_hub(hub.clone(), listener).await;
        let sender = Arc::new(HubSender::new(
            srv.env.sessions.clone(),
            HubTarget { url, token: sync },
            Duration::from_secs(5),
        ));
        srv.state.hub_sender = Some(Arc::clone(&sender));

        let r = call(&srv, &g.id, "portfolio_ranking", json!({ "limit": 5 })).await;
        assert_eq!(r["isError"], false, "{r}");
        assert_eq!(r["structuredContent"]["items"][0]["number"], 7);
        let r = call(&srv, &g.id, "portfolio_overview", json!({})).await;
        assert_eq!(r["structuredContent"]["projects"][0]["repo"], "demo", "{r}");
        let r = call(
            &srv,
            &g.id,
            "portfolio_work_item",
            json!({ "owner": "deep-ion-ai", "repo": "demo", "number": 7 }),
        )
        .await;
        assert_eq!(r["structuredContent"]["title"], "Issue 7", "{r}");
        let r = call(
            &srv,
            &g.id,
            "portfolio_propose",
            json!({ "kind": "priority", "owner": "deep-ion-ai", "repo": "demo", "number": 7,
                    "proposed": { "priority": "P0" }, "justification": "cliente esperando" }),
        )
        .await;
        assert_eq!(r["structuredContent"]["state"], "pendente", "{r}");
        assert_eq!(r["structuredContent"]["session_id"], g.id.as_str());
        let r = call(&srv, &g.id, "portfolio_list_proposals", json!({})).await;
        assert_eq!(
            r["structuredContent"]["proposals"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        // Recusa do hub vira erro da ferramenta.
        let r = call(
            &srv,
            &g.id,
            "portfolio_propose",
            json!({ "kind": "status", "owner": "deep-ion-ai", "repo": "demo", "number": 7,
                    "proposed": { "status": "backlog" }, "justification": "x" }),
        )
        .await;
        assert_eq!(r["isError"], true);
        assert!(
            r["content"][0]["text"].as_str().unwrap().contains("409"),
            "{r}"
        );
        // Nada foi aplicado.
        assert_eq!(
            hub.lock()
                .find_work_item("deep-ion-ai", "demo", 7)
                .unwrap()
                .map(|w| w.priority),
            None
        );
        let r = call(&srv, &g.id, "ledger_get_context", json!({})).await;
        let c = &r["structuredContent"];
        assert_eq!(c["kind"], "gestor");
        assert_eq!(c["scope"]["kind"], "portfolio");
        assert_eq!(c["hub_reachable"], true);
        assert_eq!(c["gestor_dir"], g.cwd.as_str());

        // As decisões do gestor chegam ao sentinela do hub, sem quarentena.
        {
            let s = Arc::clone(&sender);
            tokio::task::spawn_blocking(move || s.push_pending())
                .await
                .unwrap()
                .unwrap();
        }
        {
            let h = hub.lock();
            assert_eq!(h.quarantine_count("_ctower", "gestor").unwrap(), 0);
            let wi = h.find_work_item("_ctower", "gestor", 0).unwrap().unwrap();
            let decs = h.list_decisions(&[wi.id], false).unwrap();
            assert_eq!(decs[0].title, "P0 em deep-ion-ai/demo#7");
            assert!(h
                .list_projects()
                .unwrap()
                .iter()
                .all(|p| p.gh_owner != "_ctower"));
        }

        // Sem o arquivo do token de agente: erro claro.
        std::fs::remove_file(srv.env.cfg.dirs.hub_agent_token_file()).unwrap();
        let r = call(&srv, &g.id, "portfolio_overview", json!({})).await;
        assert!(r["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("set-agent-token"));
        for id in [&g.id, &comum.id] {
            srv.env.sessions.kill(id).ok();
        }
    }

    #[tokio::test]
    async fn tools_do_ledger() {
        let srv = Srv::new(FakeClaude::sleep(30));
        let s = srv.env.sessions.start(srv.env.request(1, "A")).unwrap();

        let r = call(
            &srv,
            &s.id,
            "ledger_record_decision",
            json!({
                "title": "Usar SQLite", "context": "c", "decision": "d", "origin": "orientada"
            }),
        )
        .await;
        assert_eq!(r["isError"], false);
        assert!(
            r["structuredContent"]["reviewed_at"].is_i64(),
            "orientada nasce revisada"
        );
        let r = call(
            &srv,
            &s.id,
            "ledger_record_decision",
            json!({
                "title": "Cache", "context": "c", "decision": "d", "origin": "autonoma"
            }),
        )
        .await;
        assert!(r["structuredContent"]["reviewed_at"].is_null());
        let r = call(
            &srv,
            &s.id,
            "ledger_record_decision",
            json!({ "title": "x", "origin": "autonoma" }),
        )
        .await;
        assert_eq!(r["isError"], true);
        let r = call(
            &srv,
            &s.id,
            "ledger_record_decision",
            json!({
                "title": "x", "context": "c", "decision": "d", "origin": "manual"
            }),
        )
        .await;
        assert_eq!(r["isError"], true, "agente não registra decisão manual");

        let r = call(&srv, &s.id, "ledger_list_decisions", json!({})).await;
        assert_eq!(
            r["structuredContent"]["decisions"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        let r = call(
            &srv,
            &s.id,
            "ledger_list_decisions",
            json!({ "scope": "project" }),
        )
        .await;
        assert_eq!(
            r["structuredContent"]["decisions"]
                .as_array()
                .unwrap()
                .len(),
            2
        );

        let r = call(
            &srv,
            &s.id,
            "ledger_set_status",
            json!({ "status": "bloqueado", "reason": "aguardando API" }),
        )
        .await;
        assert_eq!(r["isError"], false);
        let wi = srv
            .env
            .sessions
            .ledger()
            .get_work_item(s.work_item_id)
            .unwrap();
        assert_eq!(wi.status, WorkItemStatus::Bloqueado);
        let r = call(
            &srv,
            &s.id,
            "ledger_set_status",
            json!({ "status": "concluido", "reason": "x" }),
        )
        .await;
        assert_eq!(r["isError"], true);

        let r = call(&srv, &s.id, "ledger_get_context", json!({})).await;
        let c = &r["structuredContent"];
        assert_eq!(c["branch"], "feat/1-a");
        assert_eq!(c["ref"], "deep-ion-ai/demo#1");
        assert_eq!(c["repository"], "deep-ion-ai/demo");
        assert_eq!(c["protected_branches"], json!(["develop", "main"]));
        // Sem espelho do GitHub: sem issue nem épico.
        assert!(c["issue"].is_null() && c["parent_epic"].is_null());
        srv.env.sessions.kill(&s.id).unwrap();
    }

    #[tokio::test]
    async fn contexto_de_sub_issue_informa_o_epico() {
        let srv = Srv::new(FakeClaude::sleep(30));
        srv.env
            .sessions
            .ledger()
            .apply_github(&ct_ledger::GithubSnapshot {
                project_id: srv.env.project_id,
                issues: vec![ct_ledger::IssueRow {
                    gh_number: 1,
                    title: "Sub-issue".into(),
                    state: "open".into(),
                    labels: vec![],
                    parent_number: Some(9),
                    has_sub_issues: false,
                    url: "https://github.com/deep-ion-ai/demo/issues/1".into(),
                    updated_at: 0,
                    ..Default::default()
                }],
                prs: vec![],
                etags: vec![],
                synced_at: 1,
                gone: Vec::new(),
            })
            .unwrap();
        let s = srv.env.sessions.start(srv.env.request(1, "A")).unwrap();
        let r = call(&srv, &s.id, "ledger_get_context", json!({})).await;
        let c = &r["structuredContent"];
        assert_eq!(c["parent_epic"], 9);
        assert_eq!(c["issue"]["title"], "Sub-issue");
        assert_eq!(c["issue"]["state"], "open");
        srv.env.sessions.kill(&s.id).unwrap();
    }

    /// Regras do agente (#357): `rules_get` devolve os dois escopos da
    /// sessão, `ledger_get_context` as versões, e `rules_propose` só põe a
    /// proposta no outbox (as vigentes não mudam). Projeto Jira, épico
    /// `PROJ-5014`.
    #[tokio::test]
    async fn rules_get_e_rules_propose() {
        let srv = Srv::new(FakeClaude::sleep(30));
        let rules = ct_ledger::ConfigRules {
            project: Some(ct_ledger::RuleText {
                version: 7,
                content: "Commit [KEY] título.".into(),
            }),
            epics: vec![ct_ledger::EpicRuleText {
                number: 5014,
                version: 3,
                content: "Só o openapi.yaml.".into(),
            }],
        };
        srv.env.assign_with_rules(
            ct_core::TrackerConfig::Jira(ct_core::JiraConfig {
                project_key: "PROJ".into(),
                ..Default::default()
            }),
            ct_core::ForgeConfig::Github,
            false,
            1,
            rules,
        );
        srv.env.mirror_issues(&[
            (5014, None, true),
            (5450, Some(5014), false),
            (77, None, false),
        ]);
        let s = srv.env.sessions.start(srv.env.request(5450, "A")).unwrap();

        let r = call(&srv, &s.id, "rules_get", json!({})).await;
        let c = &r["structuredContent"];
        assert_eq!(c["project"]["version"], 7);
        assert_eq!(c["project"]["content"], "Commit [KEY] título.");
        assert_eq!(c["epic"]["ref"], "PROJ-5014");
        assert_eq!(c["epic"]["version"], 3);
        assert_eq!(c["epic"]["content"], "Só o openapi.yaml.");

        let r = call(&srv, &s.id, "ledger_get_context", json!({})).await;
        // Projeto Jira (#351): o ref é a chave, e o link vai ao /browse/.
        let ctx = &r["structuredContent"];
        assert_eq!(ctx["ref"], "PROJ-5450");
        assert_eq!(ctx["tracker"], "jira");
        assert_eq!(ctx["issue"]["url"], "/browse/PROJ-5450");
        let c = &r["structuredContent"]["rules"];
        assert_eq!(c["project"]["version"], 7);
        assert_eq!(
            (c["epic"]["number"].clone(), c["epic"]["version"].clone()),
            (json!(5014), json!(3))
        );

        let r = call(
            &srv,
            &s.id,
            "rules_propose",
            json!({ "scope": "epic", "content": "Só o openapi.yaml.\nE o CHANGELOG.", "reason": "o bot pediu" }),
        )
        .await;
        assert_eq!(r["isError"], false, "{r}");
        assert_eq!(r["structuredContent"]["state"], "pendente");
        assert_eq!(r["structuredContent"]["base_version"], 3);
        let out = srv.env.sessions.ledger().outbox_pending(100).unwrap();
        let props: Vec<_> = out
            .iter()
            .filter_map(|e| match &e.change {
                ct_ledger::SyncChange::RulesProposal(p) => Some(p.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(props.len(), 1);
        assert_eq!(
            (props[0].epic_number, props[0].session_id.as_deref()),
            (5014, Some(s.id.as_str()))
        );
        // Pendente: as vigentes continuam as mesmas.
        let r = call(&srv, &s.id, "rules_get", json!({})).await;
        assert_eq!(r["structuredContent"]["epic"]["version"], 3);

        // Acima do limite: erro claro, nada no outbox.
        let big = "a".repeat(ct_ledger::RULES_MAX_BYTES + 1);
        let r = call(
            &srv,
            &s.id,
            "rules_propose",
            json!({ "scope": "project", "content": big, "reason": "x" }),
        )
        .await;
        assert_eq!(r["isError"], true);
        assert!(
            r["content"][0]["text"].as_str().unwrap().contains("16 KiB"),
            "{r}"
        );

        // Issue sem épico: só as do projeto; propor no épico é recusado.
        let solta = srv.env.sessions.start(srv.env.request(77, "B")).unwrap();
        let r = call(&srv, &solta.id, "rules_get", json!({})).await;
        assert_eq!(r["structuredContent"]["epic"], Value::Null);
        assert_eq!(r["structuredContent"]["project"]["version"], 7);
        let r = call(
            &srv,
            &solta.id,
            "rules_propose",
            json!({ "scope": "epic", "content": "x", "reason": "y" }),
        )
        .await;
        assert_eq!(r["isError"], true);
        for id in [&s.id, &solta.id] {
            srv.env.sessions.kill(id).ok();
        }
    }

    fn tool_names(v: &Value) -> Vec<String> {
        v["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap().to_string())
            .collect()
    }

    /// Sessão do épico #10 com as sub-issues 11 e 12 (abertas) e 13
    /// (fechada) no espelho, e a issue 20 fora do épico.
    fn epico(srv: &Srv) -> ct_ledger::Session {
        let issue = |n: i64, parent: Option<i64>, state: &str| ct_ledger::IssueRow {
            gh_number: n,
            title: format!("Issue {n}"),
            state: state.into(),
            labels: vec![],
            parent_number: parent,
            has_sub_issues: n == 10,
            url: String::new(),
            updated_at: 0,
            ..Default::default()
        };
        srv.env
            .sessions
            .ledger()
            .apply_github(&ct_ledger::GithubSnapshot {
                project_id: srv.env.project_id,
                issues: vec![
                    issue(10, None, "open"),
                    issue(11, Some(10), "open"),
                    issue(12, Some(10), "open"),
                    issue(13, Some(10), "closed"),
                    issue(20, None, "open"),
                ],
                ..Default::default()
            })
            .unwrap();
        let mut req = srv.env.request(10, "Épico");
        req.kind = ct_core::WorkItemKind::Epic;
        srv.env.sessions.start(req).unwrap()
    }

    /// `ledger_start_sub_issue` (#458): só no `tools/list` do épico; a
    /// sub-issue vai para em execução com `cause: "mcp_sub_issue"` (criando
    /// o work item), e o mesmo evento chega ao hub pelo outbox.
    #[tokio::test]
    async fn start_sub_issue_na_sessao_do_epico() {
        use ct_ledger::{Hub, SharedHub};
        let srv = Srv::new(FakeClaude::sleep(30));
        let ep = epico(&srv);
        let lista = tool_names(&rpc(&srv, &ep.id, "tools/list", json!({})).await);
        assert_eq!(lista.last().unwrap(), "ledger_start_sub_issue", "{lista:?}");
        let pid = srv.env.project_id;
        assert!(srv
            .env
            .sessions
            .ledger()
            .find_work_item(pid, 11)
            .unwrap()
            .is_none());

        let r = call(
            &srv,
            &ep.id,
            "ledger_start_sub_issue",
            json!({ "number": 11, "reason": "começando pela API" }),
        )
        .await;
        assert_eq!(r["isError"], false, "{r}");
        assert_eq!(r["structuredContent"]["status"], "em_execucao");
        assert_eq!(r["structuredContent"]["changed"], true);
        let (wi, ev) = {
            let l = srv.env.sessions.ledger();
            let wi = l.find_work_item(pid, 11).unwrap().unwrap();
            let ev = l.list_events_of_kind(wi.id, "status_changed").unwrap();
            (wi, ev)
        };
        assert_eq!(
            (wi.kind, wi.status),
            (ct_core::WorkItemKind::Issue, WorkItemStatus::EmExecucao)
        );
        assert_eq!(ev.len(), 1);
        assert_eq!(
            ev[0].payload,
            json!({ "from": "backlog", "to": "em_execucao", "cause": "mcp_sub_issue",
                    "reason": "começando pela API" })
        );
        assert_eq!(ev[0].session_id.as_deref(), Some(ep.id.as_str()));

        // Já em execução: nada novo.
        let r = call(
            &srv,
            &ep.id,
            "ledger_start_sub_issue",
            json!({ "number": 11 }),
        )
        .await;
        assert_eq!(r["isError"], false, "{r}");
        assert_eq!(r["structuredContent"]["changed"], false);
        assert_eq!(
            srv.env
                .sessions
                .ledger()
                .list_events_of_kind(wi.id, "status_changed")
                .unwrap()
                .len(),
            1
        );

        // O hub recebe pelo outbox: cria o work item e aplica o status.
        let hub = SharedHub::new(Hub::open_in_memory().unwrap());
        hub.lock()
            .add_project(&ct_ledger::NewHubProject {
                owner: "deep-ion-ai".into(),
                repo: "demo".into(),
                name: "demo".into(),
                base_branch: "develop".into(),
                protected_branches: vec![],
            })
            .unwrap();
        let (env_id, out) = {
            let l = srv.env.sessions.ledger();
            (l.env_id().to_string(), l.outbox_pending(1000).unwrap())
        };
        hub.lock().ingest(&env_id, "env", &out).unwrap();
        let h = hub.lock();
        let hw = h
            .find_work_item("deep-ion-ai", "demo", 11)
            .unwrap()
            .unwrap();
        assert_eq!(hw.status, WorkItemStatus::EmExecucao);
        let hev = h.list_events(hw.id, 10).unwrap();
        assert!(
            hev.iter()
                .any(|e| e.kind == "status_changed" && e.payload["cause"] == "mcp_sub_issue"),
            "{hev:?}"
        );
        drop(h);
        srv.env.sessions.kill(&ep.id).unwrap();
    }

    /// Recusas: fora do épico, sem número, sessão de issue (mesmo sem a
    /// ferramenta listada). Nenhuma cria work item.
    #[tokio::test]
    async fn start_sub_issue_recusa() {
        let srv = Srv::new(FakeClaude::sleep(30));
        let ep = epico(&srv);
        let pid = srv.env.project_id;
        for (args, msg) in [
            (json!({ "number": 20 }), "não é sub-issue do épico #10"),
            (json!({ "number": 99 }), "não é sub-issue do épico #10"),
            (json!({}), "parâmetro obrigatório: number"),
        ] {
            let r = call(&srv, &ep.id, "ledger_start_sub_issue", args).await;
            assert_eq!(r["isError"], true, "{r}");
            assert!(
                r["content"][0]["text"].as_str().unwrap().contains(msg),
                "{r}"
            );
        }
        let sub = srv.env.sessions.start(srv.env.request(12, "Sub")).unwrap();
        let lista = tool_names(&rpc(&srv, &sub.id, "tools/list", json!({})).await);
        assert!(!lista.iter().any(|n| n == "ledger_start_sub_issue"));
        let r = call(
            &srv,
            &sub.id,
            "ledger_start_sub_issue",
            json!({ "number": 11 }),
        )
        .await;
        assert_eq!(r["isError"], true, "{r}");
        assert!(r["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("só para a sessão de um épico"));
        // Sessão de issue também não ganha `sub_issues`.
        let r = call(&srv, &sub.id, "ledger_get_context", json!({})).await;
        assert!(r["structuredContent"].get("sub_issues").is_none(), "{r}");
        {
            let l = srv.env.sessions.ledger();
            for n in [11, 20, 99] {
                assert!(l.find_work_item(pid, n).unwrap().is_none(), "#{n}");
            }
        }
        for id in [&ep.id, &sub.id] {
            srv.env.sessions.kill(id).ok();
        }
    }

    /// Retrabalho: de `aguardando_epico` e de `concluido` volta a em execução.
    #[tokio::test]
    async fn start_sub_issue_retrabalho() {
        let srv = Srv::new(FakeClaude::sleep(30));
        let ep = epico(&srv);
        let pid = srv.env.project_id;
        for (n, de) in [
            (11, WorkItemStatus::AguardandoEpico),
            (13, WorkItemStatus::Concluido),
        ] {
            let id = {
                let l = srv.env.sessions.ledger();
                let wi = l
                    .ensure_work_item(pid, n, ct_core::WorkItemKind::Issue)
                    .unwrap();
                l.set_work_item_status(wi.id, de, "teste").unwrap();
                wi.id
            };
            let r = call(
                &srv,
                &ep.id,
                "ledger_start_sub_issue",
                json!({ "number": n }),
            )
            .await;
            assert_eq!(r["structuredContent"]["changed"], true, "{r}");
            let l = srv.env.sessions.ledger();
            let wi = l.get_work_item(id).unwrap();
            assert_eq!(wi.status, WorkItemStatus::EmExecucao);
            assert!(wi.closed_at.is_none());
            let ev = &l.list_events_of_kind(id, "status_changed").unwrap()[0];
            assert_eq!(ev.payload["from"], json!(de));
            assert_eq!(ev.payload["cause"], "mcp_sub_issue");
            assert!(ev.payload.get("reason").is_none());
        }
        srv.env.sessions.kill(&ep.id).unwrap();
    }

    /// `ledger_get_context` do épico lista as sub-issues com o status do
    /// work item, ou `concluido`/`backlog` pelo estado da issue sem work item.
    #[tokio::test]
    async fn contexto_do_epico_lista_sub_issues() {
        let srv = Srv::new(FakeClaude::sleep(30));
        let ep = epico(&srv);
        call(
            &srv,
            &ep.id,
            "ledger_start_sub_issue",
            json!({ "number": 12 }),
        )
        .await;
        let r = call(&srv, &ep.id, "ledger_get_context", json!({})).await;
        assert_eq!(
            r["structuredContent"]["sub_issues"],
            json!([
                { "number": 11, "title": "Issue 11", "state": "open", "status": "backlog" },
                { "number": 12, "title": "Issue 12", "state": "open", "status": "em_execucao" },
                { "number": 13, "title": "Issue 13", "state": "closed", "status": "concluido" },
            ]),
            "{r}"
        );
        srv.env.sessions.kill(&ep.id).unwrap();
    }

    #[tokio::test]
    async fn get_retorna_405() {
        use axum::body::Body;
        use axum::http::Request;
        use tower::ServiceExt;
        let srv = Srv::new(FakeClaude::exit(0));
        let r = crate::router(srv.state.clone())
            .oneshot(Request::get("/mcp").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::METHOD_NOT_ALLOWED);
    }
}
