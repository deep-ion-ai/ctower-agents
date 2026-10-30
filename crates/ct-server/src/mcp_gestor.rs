//! MCP das sessões do gestor de projetos (spec do gestor §2.4 e §7): as
//! ferramentas `portfolio_*`, que o ambiente repassa a `/agent/v1/*` do hub
//! com o token de agente (o agente nunca o vê), e o `ledger_*` restrito ao
//! work item do gestor (`_ctower/gestor#0`). Nenhuma ferramenta aprova
//! proposta: só a UI do hub decide.

use crate::hub_sync::HubSender;
use ct_ledger::{Ledger, Session};
use ct_sessions::Sessions;
use serde_json::{json, Value};

/// Instruções do `initialize` no gestor: os mesmos limites fixos do system
/// prompt e o uso das ferramentas.
pub(crate) fn instructions() -> String {
    format!(
        "{}\n\nFerramentas: portfolio_overview, portfolio_project, portfolio_work_item, \
         portfolio_ranking, portfolio_health, portfolio_list_decisions e \
         portfolio_list_proposals leem o hub inteiro; portfolio_propose cria uma proposta \
         pendente de prioridade ou de status, com justificativa. ledger_record_decision e \
         ledger_list_decisions tratam das decisões deste gestor; ao registrar uma decisão sobre \
         um projeto ou issue, cite o alvo no texto pelo ref (owner/repo#n no GitHub, PROJ-n \
         no Jira). Cada projeto de portfolio_overview traz tracker e issue_view, o comando para \
         ler uma issue (no Jira, com o comando de credencial, sem a senha).",
        ct_sessions::GESTOR_LIMITS
    )
}

fn owner_repo() -> Value {
    json!({
        "owner": { "type": "string" },
        "repo": { "type": "string" }
    })
}

fn with(mut base: Value, extra: Value) -> Value {
    if let (Some(b), Some(e)) = (base.as_object_mut(), extra.as_object()) {
        b.extend(e.clone());
    }
    base
}

fn tool(name: &str, description: &str, properties: Value, required: &[&str]) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": { "type": "object", "properties": properties, "required": required },
    })
}

/// `tools/list` do gestor. `ledger_set_status` fica de fora: o gestor não
/// tem status próprio.
pub(crate) fn tools() -> Value {
    let limit = json!({ "type": "integer", "minimum": 1, "maximum": 200, "default": 50 });
    let priority = json!({ "type": "string", "enum": ["P0", "P1", "P2", "P3"] });
    json!([
        tool(
            "ledger_record_decision",
            "Registra uma decisão deste gestor. Sobre um projeto ou issue, cite o alvo pelo ref (owner/repo#n ou a chave do Jira) no texto.",
            json!({
                "title": { "type": "string" },
                "context": { "type": "string", "description": "Por que a decisão foi necessária." },
                "decision": { "type": "string", "description": "O que foi decidido." },
                "consequences": { "type": "string" },
                "origin": { "type": "string", "enum": ["autonoma", "orientada"],
                            "description": "orientada = recomendação aceita pelo usuário na conversa; autonoma = escolha sua." }
            }),
            &["title", "context", "decision", "origin"],
        ),
        tool(
            "ledger_list_decisions",
            "Lista as decisões deste gestor. Para decisões dos projetos, use portfolio_list_decisions.",
            json!({ "scope": { "type": "string", "enum": ["work_item"], "default": "work_item" } }),
            &[],
        ),
        tool(
            "ledger_get_context",
            "Ambiente, pasta, escopo desta sessão do gestor e se o hub responde.",
            json!({}),
            &[],
        ),
        tool(
            "portfolio_overview",
            "Projetos do hub (na ordem entre projetos) com rastreador (github ou jira), comando para ler uma issue (issue_view), contagem por status e prioridade, PRs/MRs abertos, decisões pendentes, sinais de saúde e atribuições; ambientes.",
            json!({}),
            &[],
        ),
        tool(
            "portfolio_project",
            "Árvore de um projeto (épicos e sub-issues) com status, prioridade, ordem, labels, PR, atividade e decisões pendentes. Cada item traz o ref (owner/repo#n ou a chave do Jira). O corpo das issues fica de fora: use o issue_view do projeto (gh issue view <n> -R owner/repo no GitHub; jira issue view no Jira).",
            with(owner_repo(), json!({
                "include_closed": { "type": "boolean", "default": false },
                "limit": limit
            })),
            &["owner", "repo"],
        ),
        tool(
            "portfolio_work_item",
            "Um item: épico pai, sub-issues, sessões, PRs, últimos eventos, decisões e propostas.",
            with(owner_repo(), json!({
                "number": { "type": "integer" },
                "events": { "type": "integer", "minimum": 1, "maximum": 200, "default": 30 }
            })),
            &["owner", "repo", "number"],
        ),
        tool(
            "portfolio_ranking",
            "Ranking do portfólio: a prioridade primeiro (P0 de qualquer projeto antes de P1), depois a ordem entre projetos e a do item; não triados no fim.",
            with(owner_repo(), json!({ "priority": priority, "limit": limit })),
            &[],
        ),
        tool(
            "portfolio_health",
            "Sinais de saúde e risco: itens parados, bloqueados, PRs sem andamento, decisões sem revisão, sessões falhas, P0 sem andamento, ambientes mudos.",
            with(owner_repo(), json!({
                "min_severity": { "type": "string", "enum": ["info", "aviso", "critico"], "default": "aviso" }
            })),
            &[],
        ),
        tool(
            "portfolio_list_decisions",
            "Decisões dos projetos, da mais nova para a mais velha. Com number, o scope parte desse work item.",
            with(owner_repo(), json!({
                "number": { "type": "integer" },
                "scope": { "type": "string", "enum": ["work_item", "epic", "project"] },
                "origin": { "type": "string", "enum": ["autonoma", "orientada", "manual"] },
                "unreviewed": { "type": "boolean" },
                "since": { "type": "integer", "description": "created_at mínimo, em ms." },
                "limit": limit
            })),
            &[],
        ),
        tool(
            "portfolio_list_proposals",
            "Propostas do gestor e o estado de cada uma (pendente, aprovada, rejeitada, obsoleta).",
            with(owner_repo(), json!({
                "state": { "type": "string", "enum": ["pendente", "aprovada", "rejeitada", "obsoleta", "todos"], "default": "pendente" },
                "limit": limit
            })),
            &[],
        ),
        tool(
            "portfolio_propose",
            "Propõe uma mudança de prioridade ({\"priority\": \"P1\" | null, \"before\"?: n, \"after\"?: n}) ou de status ({\"status\": \"backlog\" | \"bloqueado\" | \"pausado\" | \"concluido\"}). Fica pendente até o usuário aprovar na tela Hub; nada é aplicado agora.",
            with(owner_repo(), json!({
                "kind": { "type": "string", "enum": ["priority", "status"] },
                "number": { "type": "integer" },
                "proposed": { "type": "object" },
                "justification": { "type": "string", "maxLength": 2000 }
            })),
            &["kind", "owner", "repo", "number", "proposed", "justification"],
        ),
    ])
}

/// Segmento de caminho com os caracteres fora de `[A-Za-z0-9._-]` codificados.
fn seg(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'.' | b'_' | b'-' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// Repassa uma ferramenta `portfolio_*` ao hub. `None` se `name` não é dela.
pub(crate) fn portfolio(
    hub: Option<&HubSender>,
    session_id: &str,
    name: &str,
    args: &Value,
) -> Option<Result<Value, String>> {
    if !name.starts_with("portfolio_") {
        return None;
    }
    Some(portfolio_call(hub, session_id, name, args))
}

fn portfolio_call(
    hub: Option<&HubSender>,
    session_id: &str,
    name: &str,
    args: &Value,
) -> Result<Value, String> {
    let hub = hub.ok_or_else(|| format!("hub indisponível: {}", crate::hub_sync::NOT_LINKED))?;
    let s = |k: &str| {
        args.get(k)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|v| !v.is_empty())
    };
    let req = |k: &str| s(k).ok_or_else(|| format!("parâmetro obrigatório: {k}"));
    let num = |k: &str| args.get(k).and_then(Value::as_i64);
    // Parâmetros de consulta presentes, como texto.
    fn query<'k>(args: &Value, keys: &[&'k str]) -> Vec<(&'k str, String)> {
        keys.iter()
            .filter_map(|&k| match args.get(k) {
                None | Some(Value::Null) => None,
                Some(Value::String(v)) => Some((k, v.clone())),
                Some(v) => Some((k, v.to_string())),
            })
            .collect()
    }
    let get =
        |path: String, q: Vec<(&str, String)>| hub.agent_call("GET", &path, &q, None, session_id);
    match name {
        "portfolio_overview" => get("overview".into(), vec![]),
        "portfolio_project" => get(
            format!("projects/{}/{}", seg(req("owner")?), seg(req("repo")?)),
            query(args, &["include_closed", "limit"]),
        ),
        "portfolio_work_item" => {
            let n = num("number").ok_or("parâmetro obrigatório: number")?;
            get(
                format!(
                    "work-items/{}/{}/{n}",
                    seg(req("owner")?),
                    seg(req("repo")?)
                ),
                query(args, &["events"]),
            )
        }
        "portfolio_ranking" => get(
            "ranking".into(),
            query(args, &["owner", "repo", "priority", "limit"]),
        ),
        "portfolio_health" => get(
            "health".into(),
            query(args, &["owner", "repo", "min_severity"]),
        ),
        "portfolio_list_decisions" => get(
            "decisions".into(),
            query(
                args,
                &[
                    "owner",
                    "repo",
                    "number",
                    "scope",
                    "origin",
                    "unreviewed",
                    "since",
                    "limit",
                ],
            ),
        ),
        "portfolio_list_proposals" => get(
            "proposals".into(),
            query(args, &["state", "owner", "repo", "limit"]),
        ),
        "portfolio_propose" => {
            let body = json!({
                "kind": req("kind")?,
                "owner": req("owner")?,
                "repo": req("repo")?,
                "number": num("number").ok_or("parâmetro obrigatório: number")?,
                "proposed": args.get("proposed").cloned().ok_or("parâmetro obrigatório: proposed")?,
                "justification": req("justification")?,
            });
            hub.agent_call("POST", "proposals", &[], Some(&body), session_id)
        }
        _ => Err(format!("tool desconhecida: {name}")),
    }
}

/// `ledger_*` com as regras do gestor; `None` = segue o caminho comum
/// (`ledger_record_decision`).
pub(crate) fn ledger(
    sessions: &Sessions,
    hub: Option<&HubSender>,
    s: &Session,
    name: &str,
    args: &Value,
) -> Option<Result<Value, String>> {
    match name {
        "ledger_set_status" => Some(Err(
            "o gestor não tem status próprio; proponha com portfolio_propose".into(),
        )),
        "ledger_list_decisions" => {
            match args
                .get("scope")
                .and_then(Value::as_str)
                .unwrap_or("work_item")
            {
                "work_item" => None,
                _ => Some(Err(
                    "no gestor, ledger_list_decisions só tem o escopo work_item (as decisões \
                     deste gestor); para as dos projetos, use portfolio_list_decisions"
                        .into(),
                )),
            }
        }
        "ledger_get_context" => Some(context(sessions, hub, s)),
        _ => None,
    }
}

fn context(sessions: &Sessions, hub: Option<&HubSender>, s: &Session) -> Result<Value, String> {
    let e = |e: ct_ledger::LedgerError| e.to_string();
    let (env_id, scope) = {
        let l: std::sync::MutexGuard<'_, Ledger> = sessions.ledger();
        let scope = l
            .list_events(s.work_item_id, 500)
            .map_err(e)?
            .into_iter()
            .find(|ev| ev.kind == "gestor_started" && ev.ts <= s.started_at)
            .map(|ev| ev.payload["scope"].clone())
            .unwrap_or(Value::Null);
        (l.env_id().to_string(), scope)
    };
    Ok(json!({
        "kind": "gestor",
        "env": { "id": env_id },
        "gestor_dir": s.cwd,
        "scope": scope,
        "hub_url": hub.map(|h| h.target.url.clone()),
        "hub_reachable": hub.is_some_and(HubSender::reachable),
    }))
}
