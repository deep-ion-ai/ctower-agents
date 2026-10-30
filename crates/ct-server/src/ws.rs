//! `/ws`: protocolo UI ⇄ daemon (spec §9).
//!
//! - Texto: requisição `{id, method, params}` → `{id, result}` ou `{id, error}`;
//!   push do servidor `{event, data}`.
//! - Binário: `[canal u8][uuid da sessão, 16 bytes][bytes]`; canal 0 = saída
//!   do terminal, canal 1 = entrada. O uuid pode ser o de um shell livre
//!   (#428): o terminal de `session.attach|detach|input|resize` é o da sessão
//!   ou o do shell com aquele id.

use crate::api::{Dashboard, DashboardProject, DashboardWorkItem, EnvInfo, Marked, Peek, Review};
use crate::{bearer_ok, unauthorized, AppState, PROTOCOL_VERSION};
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::Response;
use ct_core::{DecisionOrigin, SessionMode, WorkItemKind, CLAUDE_MODELS};
use ct_http::rpc::{i64_param, invalid, str_param, RpcResult};
use ct_ledger::{Decision, Ledger, Project};
use ct_sessions::transcript_view::TAIL_MAX_ENTRIES;
use ct_sessions::{AgentOptions, GestorScope, PtySize, Sessions, SessionsError, StartRequest};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

pub const CHANNEL_OUTPUT: u8 = 0;
pub const CHANNEL_INPUT: u8 = 1;

/// Janela das sessões com erro no dashboard.
const FAILED_WINDOW_MS: i64 = 24 * 3600 * 1000;

/// Limite de `session.peek` (tiles da Sala de controle).
pub const PEEK_MAX: usize = 4096;

/// Janela padrão de `recovery.archived`.
const ARCHIVED_WINDOW_MS: i64 = 7 * 24 * 3600 * 1000;
const ARCHIVED_LIMIT: u32 = 20;
const ARCHIVED_MAX: u32 = 100;

/// Página padrão e máxima de `events.list` (#422).
const EVENTS_LIMIT: u32 = 20;
const EVENTS_MAX: u32 = 100;

/// Falas devolvidas por `transcript.tail` sem `entries`.
const TAIL_DEFAULT: usize = 30;

pub(crate) async fn upgrade(
    State(st): State<AppState>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    if !bearer_ok(&headers, &st.ui_token) {
        return unauthorized();
    }
    ws.on_upgrade(move |socket| serve(socket, st))
}

/// Frame binário de terminal.
pub fn frame(channel: u8, session_id: &uuid::Uuid, bytes: &[u8]) -> Vec<u8> {
    let mut f = Vec::with_capacity(17 + bytes.len());
    f.push(channel);
    f.extend_from_slice(session_id.as_bytes());
    f.extend_from_slice(bytes);
    f
}

/// Decodifica um frame binário: (canal, sessão, bytes).
pub fn parse_frame(f: &[u8]) -> Option<(u8, uuid::Uuid, &[u8])> {
    if f.len() < 17 {
        return None;
    }
    let id = uuid::Uuid::from_slice(&f[1..17]).ok()?;
    Some((f[0], id, &f[17..]))
}

struct Conn {
    st: AppState,
    out: mpsc::UnboundedSender<Message>,
    /// Sessões anexadas nesta conexão → sinal de cancelamento do encaminhamento.
    attached: HashMap<String, Arc<AtomicBool>>,
    /// A conexão acompanha os pedidos de permissão (`session.permission.watch`).
    watch: Option<crate::permissions::Watch>,
}

async fn serve(socket: WebSocket, st: AppState) {
    let (mut sink, mut stream) = socket.split();
    let (out, mut out_rx) = mpsc::unbounded_channel::<Message>();
    let writer = tokio::spawn(async move {
        while let Some(m) = out_rx.recv().await {
            if sink.send(m).await.is_err() {
                break;
            }
        }
    });

    let pusher = ct_http::rpc::spawn_pusher(st.events.subscribe(), out.clone());

    let mut conn = Conn {
        st,
        out,
        attached: HashMap::new(),
        watch: None,
    };
    while let Some(Ok(msg)) = stream.next().await {
        match msg {
            Message::Text(t) => {
                let reply = conn.handle_text(&t).await;
                let _ = conn.out.send(Message::Text(reply.to_string().into()));
            }
            Message::Binary(b) => conn.handle_input(&b),
            Message::Close(_) => break,
            _ => {}
        }
    }
    for cancel in conn.attached.values() {
        cancel.store(true, Ordering::Relaxed);
    }
    pusher.abort();
    drop(conn);
    let _ = writer.await;
}

/// Despacho de uma chamada que chegou pelo canal reverso do hub
/// (`crate::hub_channel`, spec da PWA §5.2): o mesmo do `/ws`, depois da
/// lista [`ct_http::sync::CHANNEL_METHODS`], que vale mesmo que o hub peça
/// outro método. Cada chamada tem a sua conexão lógica, sem terminal nem
/// acompanhamento de permissões (nenhum dos dois passa pela lista).
pub(crate) async fn channel_call(st: AppState, method: &str, p: &Value) -> RpcResult {
    if !ct_http::sync::channel_method_allowed(method) {
        return Err((
            ct_http::sync::CHANNEL_METHOD_UNAVAILABLE,
            format!("método indisponível pelo canal do hub: {method}"),
        ));
    }
    let (out, _) = mpsc::unbounded_channel();
    let mut conn = Conn {
        st,
        out,
        attached: HashMap::new(),
        watch: None,
    };
    conn.call(method, p).await
}

/// Recusa do Remote Control pelo ambiente da sessão (#197, spec do épico
/// #205 §6.5).
pub const REMOTE_CONTROL_UNAVAILABLE: i64 = -32032;

fn from_sessions(e: SessionsError) -> (i64, String) {
    match e {
        SessionsError::RemoteControl(m) => (REMOTE_CONTROL_UNAVAILABLE, m),
        e => (-32000, e.to_string()),
    }
}

/// `remote_control` (bool, opcional; padrão `false`).
fn remote_control_param(p: &Value) -> Result<bool, (i64, String)> {
    match p.get("remote_control") {
        None | Some(Value::Null) => Ok(false),
        Some(Value::Bool(b)) => Ok(*b),
        Some(_) => Err(invalid("remote_control deve ser true ou false")),
    }
}

/// Dimensão do terminal (`cols`/`rows`): inteiro de 1 a 65535.
fn dim_param(p: &Value, k: &str) -> Result<u16, (i64, String)> {
    i64_param(p, k).and_then(|v| match u16::try_from(v) {
        Ok(d) if d > 0 => Ok(d),
        _ => Err(invalid(format!("{k} inválido"))),
    })
}

/// Tamanho inicial do PTY: `cols` e `rows` juntos, ou nenhum dos dois.
fn size_param(p: &Value) -> Result<Option<PtySize>, (i64, String)> {
    match (p.get("cols"), p.get("rows")) {
        (None, None) => Ok(None),
        _ => Ok(Some(PtySize {
            cols: dim_param(p, "cols")?,
            rows: dim_param(p, "rows")?,
        })),
    }
}

/// Modelo e modo do agente: `model` (alias de [`CLAUDE_MODELS`]) e `mode`
/// (`native` | `subagents`), opcionais. Ausentes: padrão do Claude Code e
/// `native`. Mais o `remote_control` (#197).
fn agent_param(p: &Value) -> Result<AgentOptions, (i64, String)> {
    let model = match p.get("model") {
        None | Some(Value::Null) => None,
        Some(Value::String(m)) if CLAUDE_MODELS.contains(&m.as_str()) => Some(m.clone()),
        Some(m) => {
            return Err(invalid(format!(
                "model inválido: {m} (aceitos: {})",
                CLAUDE_MODELS.join(", ")
            )))
        }
    };
    let mode = match p.get("mode") {
        None | Some(Value::Null) => SessionMode::default(),
        Some(m) => m
            .as_str()
            .and_then(|m| m.parse().ok())
            .ok_or_else(|| invalid(format!("mode inválido: {m} (aceitos: native, subagents)")))?,
    };
    Ok(AgentOptions {
        model,
        mode,
        remote_control: remote_control_param(p)?,
    })
}

/// Executa `f` fora do runtime (ledger e processos são síncronos).
async fn blocking<T: Send + 'static>(
    sessions: &Sessions,
    f: impl FnOnce(&Sessions) -> Result<T, SessionsError> + Send + 'static,
) -> Result<T, (i64, String)> {
    let s = sessions.clone();
    tokio::task::spawn_blocking(move || f(&s))
        .await
        .map_err(|e| (-32603, e.to_string()))?
        .map_err(from_sessions)
}

/// Projetos que este ambiente executa: o cache vindo do hub (e os locais
/// legados, até o primeiro pull). Sem hub, nenhum (spec §7).
fn assigned_projects(l: &Ledger, linked: bool) -> ct_ledger::Result<Vec<Project>> {
    if linked {
        l.list_projects()
    } else {
        Ok(Vec::new())
    }
}

/// Projeto onde iniciar uma sessão, por `project_id` ou por `owner` +
/// `repo` (a UI do hub nunca manda o id local). Recusa sem hub e fora do
/// cache (spec §7).
async fn project_param(sessions: &Sessions, linked: bool, p: &Value) -> Result<i64, (i64, String)> {
    if !linked {
        return Err((-32000, crate::hub_sync::NOT_LINKED.into()));
    }
    let by_id = p.get("project_id").and_then(Value::as_i64);
    let wanted = match by_id {
        Some(_) => None,
        None => Some((
            str_param(p, "owner")
                .map_err(|_| invalid("parâmetro obrigatório: project_id (ou owner e repo)"))?
                .to_string(),
            str_param(p, "repo")?.to_string(),
        )),
    };
    let found = blocking(sessions, {
        let wanted = wanted.clone();
        move |s| {
            Ok(s.ledger()
                .list_projects()?
                .into_iter()
                .find(|x| match (&wanted, by_id) {
                    (Some((o, r)), _) => {
                        x.gh_owner.eq_ignore_ascii_case(o) && x.gh_repo.eq_ignore_ascii_case(r)
                    }
                    (None, id) => Some(x.id) == id,
                })
                .map(|x| x.id))
        }
    })
    .await?;
    found.ok_or_else(|| {
        let what = match &wanted {
            Some((o, r)) => format!("{o}/{r}"),
            None => format!("id {}", by_id.unwrap_or_default()),
        };
        invalid(format!("{NOT_ASSIGNED}: {what}"))
    })
}

/// Recusa de `session.start` para projeto fora do cache (spec §7).
pub const NOT_ASSIGNED: &str = "projeto não atribuído a este ambiente";

impl Conn {
    async fn handle_text(&mut self, text: &str) -> Value {
        match ct_http::rpc::parse(text) {
            Ok(req) => {
                let r = self.call(&req.method, &req.params).await;
                ct_http::rpc::reply(req.id, r)
            }
            Err(reply) => reply,
        }
    }

    /// O ambiente tem `hub_url` e `hub.token` (spec §7).
    fn linked(&self) -> bool {
        self.st.hub_sender.is_some()
    }

    async fn call(&mut self, method: &str, p: &Value) -> RpcResult {
        let sessions = self.st.sessions.clone();
        match method {
            "env.info" => {
                let sender = self.st.hub_sender.clone();
                let (env_id, hub_sync, hub_config, remote_control, docs) =
                    blocking(&sessions, move |s| {
                        let remote = s.remote_control_status();
                        // Antes do lock do ledger: a montagem caída pode
                        // demorar a responder.
                        let docs = ct_sessions::docs::status(s.config().docs_root.as_deref());
                        let sync = sender.as_ref().map(|h| h.status());
                        let l = s.ledger();
                        let mut config = crate::hub_config::status(
                            sender.as_deref(),
                            &l,
                            s.config().secrets_max_age,
                        );
                        config.agent_token =
                            crate::hub_sync::agent_token(&s.config().dirs).is_some();
                        Ok((l.env_id().to_string(), sync, config, remote, docs))
                    })
                    .await?;
                Ok(json!(EnvInfo {
                    protocol: PROTOCOL_VERSION.into(),
                    version: ct_core::VERSION.into(),
                    commit: ct_core::BUILD_COMMIT.into(),
                    hostname: hostname(),
                    worktrees_root: sessions.config().worktrees_root.display().to_string(),
                    env_id,
                    env_name: self.st.env_name.to_string(),
                    hub_sync,
                    hub_config,
                    update: self.st.updates.as_ref().map(|u| u.status()),
                    trackers: self.st.tracker.as_ref().map(|t| t.readiness()),
                    remote_control,
                    docs,
                }))
            }
            // `env.rollback` (#328): volta à versão anterior, com a mesma
            // recusa por sessões vivas.
            "env.update" | "env.rollback" => {
                let Some(u) = self.st.updates.clone() else {
                    return Err(invalid("atualização desativada neste ambiente"));
                };
                let rollback = method == "env.rollback";
                let force = p.get("force").and_then(Value::as_bool).unwrap_or(false);
                let live = blocking(&sessions, |s| Ok(s.ledger().live_sessions()?.len())).await?;
                if live > 0 && !force {
                    return Err((
                        crate::update::LIVE_SESSIONS,
                        format!(
                            "Recusado: {live} sessão(ões) viva(s) seriam encerradas pelo restart. \
                             Use force para atualizar mesmo assim."
                        ),
                    ));
                }
                u.start(rollback)
                    .await
                    .map(|st| json!(st))
                    .map_err(|e| (-32000, e))
            }
            "env.config.get" | "env.config.set" => {
                let Some(c) = self.st.env_config.clone() else {
                    return Err(invalid("edição do config.toml desativada neste ambiente"));
                };
                if method == "env.config.get" {
                    return tokio::task::spawn_blocking(move || c.get())
                        .await
                        .map_err(|e| (-32603, e.to_string()))?
                        .map(|v| json!(v))
                        .map_err(|e| (-32000, e.to_string()));
                }
                let key = str_param(p, "key")?.to_string();
                let value = p
                    .get("value")
                    .cloned()
                    .ok_or_else(|| invalid("parâmetro obrigatório: value"))?;
                tokio::task::spawn_blocking(move || c.set(&key, &value))
                    .await
                    .map_err(|e| (-32603, e.to_string()))?
                    .map(|v| json!(v))
            }
            "env.check_update" => match &self.st.updates {
                Some(u) => Ok(json!(u.check().await)),
                None => Err(invalid("atualização desativada neste ambiente")),
            },
            "projects.list" => {
                // `unassigned: true`: os desatribuídos no hub (clone e
                // worktrees ficaram no disco para limpar à mão).
                // Sem hub, nenhum projeto (spec §7).
                let unassigned = p.get("unassigned").and_then(Value::as_bool) == Some(true);
                let linked = self.linked();
                let v = blocking(&sessions, move |s| {
                    let l = s.ledger();
                    Ok(if unassigned {
                        l.list_unassigned_projects()?
                    } else {
                        assigned_projects(&l, linked)?
                    })
                })
                .await?;
                Ok(json!(v))
            }
            "dashboard.get" => {
                let linked = self.linked();
                blocking(&sessions, move |s| dashboard(s, linked)).await
            }
            "recovery.list" => {
                let v = blocking(&sessions, |s| s.recovery_list()).await?;
                Ok(json!(v))
            }
            // Descartadas recentemente (#328): padrão, os últimos 7 dias.
            "recovery.archived" => {
                let since = match p.get("since_ms") {
                    None | Some(Value::Null) => ct_core::now_ms() - ARCHIVED_WINDOW_MS,
                    Some(v) => v.as_i64().ok_or_else(|| invalid("since_ms inválido"))?,
                };
                let limit = match p.get("limit") {
                    None | Some(Value::Null) => ARCHIVED_LIMIT,
                    Some(v) => v
                        .as_u64()
                        .filter(|n| (1..=ARCHIVED_MAX as u64).contains(n))
                        .ok_or_else(|| {
                            invalid(format!("limit deve estar entre 1 e {ARCHIVED_MAX}"))
                        })? as u32,
                };
                let v = blocking(&sessions, move |s| s.recovery_archived(since, limit)).await?;
                Ok(json!(v))
            }
            // Fim da conversa de sessão viva ou parada, pelo transcript
            // (o `session.peek` só lê o PTY vivo).
            "transcript.tail" => {
                let id = str_param(p, "session_id")?.to_string();
                let n = match p.get("entries") {
                    None | Some(Value::Null) => TAIL_DEFAULT,
                    Some(v) => v
                        .as_u64()
                        .filter(|n| (1..=TAIL_MAX_ENTRIES as u64).contains(n))
                        .ok_or_else(|| {
                            invalid(format!("entries deve estar entre 1 e {TAIL_MAX_ENTRIES}"))
                        })? as usize,
                };
                let v = blocking(&sessions, move |s| s.transcript_tail(&id, n)).await?;
                Ok(json!(v))
            }
            "session.context" => {
                let id = str_param(p, "session_id")?.to_string();
                let v = blocking(&sessions, move |s| s.context_usage(&id)).await?;
                Ok(json!(v))
            }
            "session.start" => {
                let project_id = project_param(&sessions, self.linked(), p).await?;
                let gh_number = i64_param(p, "gh_number")?;
                let prompt = str_param(p, "prompt")?.to_string();
                let size = size_param(p)?;
                let agent = agent_param(p)?;
                let kind = match p.get("kind").and_then(Value::as_str) {
                    None => None,
                    Some("epic") => Some(WorkItemKind::Epic),
                    Some("issue") => Some(WorkItemKind::Issue),
                    Some(k) => return Err(invalid(format!("kind inválido: {k}"))),
                };
                let title = p.get("title").and_then(Value::as_str).map(String::from);
                let labels: Option<Vec<String>> = p
                    .get("labels")
                    .and_then(|v| serde_json::from_value(v.clone()).ok());
                // Título, labels e tipo ausentes vêm do espelho do GitHub.
                let cached = blocking(&sessions, move |s| {
                    Ok(s.ledger().get_issue(project_id, gh_number)?)
                })
                .await?;
                let title = title
                    .or_else(|| cached.as_ref().map(|i| i.title.clone()))
                    .ok_or_else(|| {
                        invalid("parâmetro obrigatório: title (issue ainda não sincronizada)")
                    })?;
                let req = StartRequest {
                    project_id,
                    gh_number,
                    kind: kind.unwrap_or(match &cached {
                        Some(i) if i.has_sub_issues => WorkItemKind::Epic,
                        _ => WorkItemKind::Issue,
                    }),
                    title,
                    labels: labels
                        .or_else(|| cached.map(|i| i.labels))
                        .unwrap_or_default(),
                    prompt,
                    size,
                    agent,
                };
                let v = blocking(&sessions, move |s| s.start(req)).await?;
                Ok(json!(v))
            }
            "session.start_project" => {
                let project_id = project_param(&sessions, self.linked(), p).await?;
                let prompt = str_param(p, "prompt")?.to_string();
                let size = size_param(p)?;
                let agent = agent_param(p)?;
                let v = blocking(&sessions, move |s| {
                    s.start_project(project_id, &prompt, size, &agent)
                })
                .await?;
                Ok(json!(v))
            }
            "session.start_gestor" => {
                if !self.linked() {
                    return Err((-32000, crate::hub_sync::NOT_LINKED.into()));
                }
                if crate::hub_sync::agent_token(&sessions.config().dirs).is_none() {
                    return Err((-32000, crate::hub_sync::NO_AGENT_TOKEN.into()));
                }
                let scope: GestorScope =
                    serde_json::from_value(p.get("scope").cloned().unwrap_or(Value::Null))
                        .map_err(|_| {
                            invalid(
                        "scope deve ser { kind: \"portfolio\" }, { kind: \"project\", owner, \
                         repo } ou { kind: \"health\" }",
                    )
                        })?;
                let prompt = p
                    .get("prompt")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let size = size_param(p)?;
                let agent = agent_param(p)?;
                let v = blocking(&sessions, move |s| {
                    s.start_gestor(&scope, &prompt, size, &agent)
                })
                .await?;
                Ok(json!(v))
            }
            // Projeto Jira vai ao sync do jira-cli; os demais, ao do GitHub.
            "sync.now" => {
                let gh = self.st.github.clone();
                let tracker = self.st.tracker.clone();
                if gh.is_none() && tracker.is_none() {
                    return Err((-32000, "sincronização com o GitHub desativada".into()));
                }
                let project = p.get("project_id").and_then(Value::as_i64);
                let v = tokio::task::spawn_blocking(move || match project {
                    Some(id) => match (tracker.filter(|t| t.handles(id)), gh) {
                        (Some(t), _) => t.sync_project(id).map(|r| vec![r]),
                        (None, Some(g)) => g.sync_project(id).map(|r| vec![r]),
                        (None, None) => Err(ct_sessions::SessionsError::Invalid(
                            "sincronização com o GitHub desativada".into(),
                        )),
                    },
                    None => {
                        let mut all = match &gh {
                            Some(g) => g.sync_all()?,
                            None => Vec::new(),
                        };
                        if let Some(t) = &tracker {
                            all.extend(t.sync_all()?);
                        }
                        Ok(all)
                    }
                })
                .await
                .map_err(|e| (-32603, e.to_string()))?
                .map_err(from_sessions)?;
                Ok(json!(v))
            }
            "github.status" | "github.set_token" | "github.clear_token" => {
                let gh = self
                    .st
                    .github
                    .clone()
                    .ok_or_else(|| (-32000, "integração com o GitHub desativada".into()))?;
                let token = match method {
                    "github.set_token" => Some(str_param(p, "token")?.to_string()),
                    _ => None,
                };
                let method = method.to_string();
                // O token nunca volta na resposta: só conta e origem.
                tokio::task::spawn_blocking(move || {
                    let c = gh.client();
                    match method.as_str() {
                        "github.set_token" => {
                            c.set_token(&token.unwrap_or_default())?;
                        }
                        "github.clear_token" => c.clear_token()?,
                        _ => {}
                    }
                    Ok::<_, ct_gh::GhError>(c.status())
                })
                .await
                .map_err(|e| (-32603, e.to_string()))?
                .map(|st| json!(st))
                .map_err(|e| (-32000, e.to_string()))
            }
            // Comentário na conversa do PR, com o token do GitHub do
            // ambiente (Revisão, #328).
            "github.pr.comment" => {
                let gh = self
                    .st
                    .github
                    .clone()
                    .ok_or_else(|| (-32000, "integração com o GitHub desativada".into()))?;
                let project_id = project_param(&sessions, self.linked(), p).await?;
                let pr = i64_param(p, "pr")?;
                let body = str_param(p, "body")?.to_string();
                if body.trim().is_empty() {
                    return Err(invalid("body vazio"));
                }
                let project = blocking(&sessions, move |s| {
                    Ok(s.ledger().get_project(project_id)?)
                })
                .await?;
                tokio::task::spawn_blocking(move || {
                    gh.client()
                        .comment_pr(&project.gh_owner, &project.gh_repo, pr, &body)
                })
                .await
                .map_err(|e| (-32603, e.to_string()))?
                .map(|c| json!(c))
                .map_err(|e| (-32000, e.to_string()))
            }
            "tree.get" => {
                let id = i64_param(p, "project_id")?;
                let v = blocking(&sessions, move |s| Ok(s.ledger().project_tree(id)?)).await?;
                Ok(json!(v))
            }
            // "Abrir pasta de docs" (#444): cria o que faltar e devolve o
            // caminho; a UI do desktop abre no gerenciador de arquivos.
            "docs.dir" => {
                let id = i64_param(p, "project_id")?;
                let epic = match p.get("epic") {
                    None | Some(Value::Null) => None,
                    Some(v) => Some(v.as_i64().ok_or_else(|| invalid("epic inválido"))?),
                };
                let path = blocking(&sessions, move |s| s.docs_dir(id, epic)).await?;
                Ok(json!(ct_sessions::docs::DocsDir {
                    path: path.display().to_string()
                }))
            }
            "review.get" => {
                let id = i64_param(p, "work_item_id")?;
                blocking(&sessions, move |s| review_get(s, id)).await
            }
            // Linha do tempo do work item no ledger (#422), do mais novo para
            // o mais antigo; `before` (id do evento) pagina. Acima do máximo,
            // devolve o máximo, para a UI não ter que saber o teto.
            "events.list" => {
                let id = i64_param(p, "work_item_id")?;
                let before = match p.get("before") {
                    None | Some(Value::Null) => None,
                    Some(v) => Some(v.as_i64().ok_or_else(|| invalid("before inválido"))?),
                };
                let limit = match p.get("limit") {
                    None | Some(Value::Null) => EVENTS_LIMIT,
                    Some(v) => v
                        .as_u64()
                        .filter(|n| *n >= 1)
                        .ok_or_else(|| invalid("limit deve ser um inteiro a partir de 1"))?
                        .min(EVENTS_MAX as u64) as u32,
                };
                let v = blocking(&sessions, move |s| {
                    let l = s.ledger();
                    // Item inexistente é erro, não lista vazia.
                    l.get_work_item(id)?;
                    Ok(l.list_events_before(id, before, limit)?)
                })
                .await?;
                Ok(json!(v))
            }
            "review.mark_reviewed" => {
                let id = i64_param(p, "work_item_id")?;
                // Sem PR só no sentinela do gestor, que nunca abre PR (#239).
                let pr = match p.get("pr") {
                    None | Some(Value::Null) => None,
                    Some(_) => Some(i64_param(p, "pr")?),
                };
                // `decision_ids`: só essas, entre as pendentes do item (#328);
                // ausente, todas. Lista inválida é recusada, nunca vira "todas".
                let only: Option<Vec<i64>> = match p.get("decision_ids") {
                    None | Some(Value::Null) => None,
                    Some(v) => Some(serde_json::from_value(v.clone()).map_err(|_| {
                        invalid("decision_ids deve ser uma lista de ids de decisão")
                    })?),
                };
                let marked = blocking(&sessions, move |s| {
                    let l = s.ledger();
                    if pr.is_none() && l.gestor_work_item_id()? != Some(id) {
                        return Ok(None);
                    }
                    let ids: Vec<i64> = pending_review(&l, id)?
                        .iter()
                        .map(|d| d.id)
                        .filter(|i| only.as_ref().is_none_or(|o| o.contains(i)))
                        .collect();
                    Ok(Some(json!(Marked {
                        marked: l.mark_reviewed(&ids, pr)?
                    })))
                })
                .await?;
                marked.ok_or_else(|| invalid("pr é obrigatório fora do work item do gestor"))
            }
            "session.resume" => {
                let id = str_param(p, "session_id")?.to_string();
                let prompt = p.get("prompt").and_then(Value::as_str).map(String::from);
                let rc = remote_control_param(p)?;
                let v = blocking(&sessions, move |s| s.resume(&id, prompt.as_deref(), rc)).await?;
                Ok(json!(v))
            }
            // Liga o Remote Control numa sessão em andamento (#197): escreve
            // `/remote-control <nome>` no PTY; o id chega depois pelo push
            // `session_remote_control`.
            "session.remote_control.enable" => {
                let id = str_param(p, "session_id")?.to_string();
                let v = blocking(&sessions, move |s| s.enable_remote_control(&id)).await?;
                Ok(json!(v))
            }
            // A sessão em qualquer estado: a aba do terminal mostra o fim de
            // uma sessão que já saiu do dashboard (#283).
            "session.get" => {
                let id = str_param(p, "session_id")?.to_string();
                // `work_item_concluded`: a aba não oferece a Recuperação de
                // sessão cuja issue já concluiu (#297).
                let v = blocking(&sessions, move |s| {
                    let l = s.ledger();
                    let sess = l.get_session(&id)?;
                    let concluded = ct_sessions::work_item_concluded(&l, sess.work_item_id)?;
                    Ok(json!({ "session": sess, "work_item_concluded": concluded }))
                })
                .await?;
                let mut out = v["session"].clone();
                out["work_item_concluded"] = v["work_item_concluded"].clone();
                Ok(out)
            }
            "session.archive" => {
                let id = str_param(p, "session_id")?.to_string();
                blocking(&sessions, move |s| s.archive(&id)).await?;
                Ok(json!({}))
            }
            "session.kill" => {
                let id = str_param(p, "session_id")?.to_string();
                blocking(&sessions, move |s| s.kill(&id)).await?;
                Ok(json!({}))
            }
            "session.peek" => {
                let id = str_param(p, "session_id")?;
                let n = match p.get("bytes") {
                    None => PEEK_MAX,
                    Some(v) => v
                        .as_u64()
                        .filter(|n| (1..=PEEK_MAX as u64).contains(n))
                        .ok_or_else(|| invalid(format!("bytes deve estar entre 1 e {PEEK_MAX}")))?
                        as usize,
                };
                let pty = sessions
                    .pty(id)
                    .ok_or_else(|| invalid("sessão não está rodando"))?;
                let data = String::from_utf8_lossy(&pty.tail(n)).into_owned();
                Ok(json!(Peek { data }))
            }
            // Pedidos de permissão do agente esperando a UI (#328).
            // `watch`: a partir daqui o daemon segura os pedidos para esta
            // UI responder (até ela desconectar); devolve os pendentes.
            "session.permission.watch" => {
                if self.watch.is_none() {
                    self.watch = Some(self.st.permissions.watch());
                }
                Ok(json!(self.st.permissions.list()))
            }
            "session.permission.list" => Ok(json!(self.st.permissions.list())),
            "session.permission.respond" => crate::permissions::respond(&self.st, p),
            // Shell livre no worktree (#428): sem ledger, só na memória. O
            // terminal dele é o de `session.attach|input|resize`, pelo id.
            "shell.open" => {
                let wi = i64_param(p, "work_item_id")?;
                let size = size_param(p)?;
                let v = blocking(&sessions, move |s| s.open_shell(wi, size)).await?;
                Ok(json!(v))
            }
            "shell.list" => Ok(json!(sessions.list_shells())),
            "shell.close" => {
                let id = str_param(p, "shell_id")?.to_string();
                blocking(&sessions, move |s| s.close_shell(&id)).await?;
                Ok(json!({}))
            }
            "session.attach" => self.attach(str_param(p, "session_id")?),
            "session.detach" => {
                if let Some(c) = self.attached.remove(str_param(p, "session_id")?) {
                    c.store(true, Ordering::Relaxed);
                }
                Ok(json!({}))
            }
            "session.input" => {
                let id = str_param(p, "session_id")?;
                let data = str_param(p, "data")?;
                let pty = sessions
                    .terminal(id)
                    .ok_or_else(|| invalid("sessão não está rodando"))?;
                pty.write(data.as_bytes())
                    .map_err(|e| (-32000, e.to_string()))?;
                Ok(json!({}))
            }
            "session.resize" => {
                let id = str_param(p, "session_id")?;
                let (cols, rows) = (dim_param(p, "cols")?, dim_param(p, "rows")?);
                let pty = sessions
                    .terminal(id)
                    .ok_or_else(|| invalid("sessão não está rodando"))?;
                pty.resize(cols, rows)
                    .map_err(|e| (-32000, e.to_string()))?;
                Ok(json!({}))
            }
            // Limpeza após a conclusão do work item (issue #287).
            "cleanup.status" | "cleanup.plan" | "cleanup.run" => {
                crate::cleanup::rpc(self.st.cleanup.clone(), method, p).await
            }
            // Envio deste ambiente ao hub: vale em qualquer daemon que envie.
            "hub.sync.now" => crate::hub_sync::run_sync_now(self.st.hub_sender.clone())
                .await
                .map(|r| json!(r))
                .map_err(|e| (-32000, e)),
            _ => Err((-32601, format!("método desconhecido: {method}"))),
        }
    }

    /// Envia o snapshot do scrollback e passa a encaminhar a saída ao vivo
    /// (da sessão ou do shell livre com esse id).
    fn attach(&mut self, session_id: &str) -> RpcResult {
        let uuid = uuid::Uuid::parse_str(session_id).map_err(|_| invalid("session_id inválido"))?;
        let pty = self
            .st
            .sessions
            .terminal(session_id)
            .ok_or_else(|| invalid("sessão não está rodando"))?;
        if let Some(old) = self.attached.remove(session_id) {
            old.store(true, Ordering::Relaxed);
        }
        let (snapshot, rx) = pty.attach();
        let _ = self.out.send(Message::Binary(
            frame(CHANNEL_OUTPUT, &uuid, &snapshot).into(),
        ));
        let cancel = Arc::new(AtomicBool::new(false));
        self.attached.insert(session_id.to_string(), cancel.clone());
        let out = self.out.clone();
        std::thread::spawn(move || {
            while !cancel.load(Ordering::Relaxed) {
                match rx.recv_timeout(Duration::from_millis(200)) {
                    Ok(chunk) => {
                        let f = frame(CHANNEL_OUTPUT, &uuid, &chunk);
                        if out.send(Message::Binary(f.into())).is_err() {
                            return;
                        }
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
                }
            }
        });
        Ok(json!({ "snapshot_bytes": snapshot.len() }))
    }

    fn handle_input(&self, bytes: &[u8]) {
        let Some((CHANNEL_INPUT, id, data)) = parse_frame(bytes) else {
            return;
        };
        if let Some(pty) = self.st.sessions.terminal(&id.to_string()) {
            let _ = pty.write(data);
        }
    }
}

fn dashboard(s: &Sessions, linked: bool) -> Result<Value, SessionsError> {
    let l = s.ledger();
    let mut projects = Vec::new();
    for project in assigned_projects(&l, linked)? {
        let issues: HashMap<i64, ct_ledger::IssueRow> = l
            .list_issues(project.id)?
            .into_iter()
            .map(|i| (i.gh_number, i))
            .collect();
        let mut work_items = Vec::new();
        for wi in l.list_work_items(project.id)? {
            let issue = issues.get(&wi.gh_number);
            work_items.push(DashboardWorkItem {
                title: issue.map(|i| i.title.clone()),
                parent_number: issue.and_then(|i| i.parent_number),
                pending_decisions: pending_review(&l, wi.id)?.len(),
                work_item: wi,
            });
        }
        projects.push(DashboardProject {
            project,
            work_items,
        });
    }
    let live_sessions = l.live_sessions()?;
    let gestor_work_item_id = l.gestor_work_item_id()?;
    let gestor_scopes = match gestor_work_item_id {
        Some(id) => ct_sessions::gestor_scopes(&l, id, &live_sessions)?,
        None => HashMap::new(),
    };
    Ok(json!(Dashboard {
        projects,
        live_sessions,
        // Sem as de work item concluído (#285): não há o que retomar.
        interrupted_sessions: ct_sessions::recoverable_sessions(&l)?,
        failed_sessions: l.failed_sessions_since(ct_core::now_ms() - FAILED_WINDOW_MS)?,
        sync: l.sync_states()?,
        gestor_work_item_id,
        gestor_scopes,
    }))
}

/// Decisões autônomas ainda não revisadas do work item e, se for épico, das
/// sub-issues com work item próprio (spec §7.6).
fn pending_review(l: &Ledger, work_item_id: i64) -> Result<Vec<Decision>, SessionsError> {
    let ids: Vec<i64> = l
        .epic_work_items(work_item_id)?
        .iter()
        .map(|w| w.id)
        .collect();
    Ok(l.list_decisions(&ids, true)?
        .into_iter()
        .filter(|d| d.origin == DecisionOrigin::Autonoma)
        .collect())
}

fn review_get(s: &Sessions, work_item_id: i64) -> Result<Value, SessionsError> {
    let l = s.ledger();
    Ok(json!(Review {
        work_item: l.get_work_item(work_item_id)?,
        prs: l.prs_for_work_item(work_item_id)?,
        decisions: pending_review(&l, work_item_id)?,
    }))
}

fn hostname() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .or_else(|_| std::fs::read_to_string("/etc/hostname"))
        .or_else(|_| {
            // macOS não tem os arquivos acima.
            std::process::Command::new("hostname")
                .output()
                .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        })
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::*;
    use ct_sessions::test_support::FakeClaude;
    use tokio::net::TcpListener;
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    use tokio_tungstenite::tungstenite::Message as WsMsg;
    use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};

    type Client = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;
    const T: Duration = Duration::from_secs(10);

    async fn spawn_server(srv: &Srv) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = crate::router(srv.state.clone());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        format!("ws://{addr}/ws")
    }

    /// Conecta com o token; o erro vem como texto (ex.: "HTTP error: 401 ...").
    async fn connect(url: &str, token: &str) -> Result<Client, String> {
        let mut req = url.into_client_request().unwrap();
        req.headers_mut()
            .insert("authorization", format!("Bearer {token}").parse().unwrap());
        connect_async(req)
            .await
            .map(|(ws, _)| ws)
            .map_err(|e| e.to_string())
    }

    /// Cliente de teste: guarda frames binários e pushes enquanto espera respostas.
    struct Ui {
        ws: Client,
        next: i64,
        binary: Vec<u8>,
        pushes: Vec<Value>,
    }

    impl Ui {
        async fn new(url: &str) -> Self {
            Ui {
                ws: connect(url, UI).await.unwrap(),
                next: 0,
                binary: Vec::new(),
                pushes: Vec::new(),
            }
        }

        async fn recv(&mut self) -> WsMsg {
            tokio::time::timeout(T, self.ws.next())
                .await
                .expect("prazo")
                .unwrap()
                .unwrap()
        }

        fn absorb(&mut self, m: WsMsg) -> Option<Value> {
            match m {
                WsMsg::Text(t) => {
                    let v: Value = serde_json::from_str(&t).unwrap();
                    if v.get("event").is_some() {
                        self.pushes.push(v);
                        None
                    } else {
                        Some(v)
                    }
                }
                WsMsg::Binary(b) => {
                    let (ch, _, data) = parse_frame(&b).unwrap();
                    assert_eq!(ch, CHANNEL_OUTPUT);
                    self.binary.extend_from_slice(data);
                    None
                }
                _ => None,
            }
        }

        async fn call(&mut self, method: &str, params: Value) -> Value {
            self.next += 1;
            let id = self.next;
            let req = json!({ "id": id, "method": method, "params": params });
            self.ws
                .send(WsMsg::Text(req.to_string().into()))
                .await
                .unwrap();
            loop {
                let m = self.recv().await;
                if let Some(v) = self.absorb(m) {
                    if v["id"] == id {
                        return v;
                    }
                }
            }
        }

        async fn wait_output(&mut self, needle: &str) {
            while !String::from_utf8_lossy(&self.binary).contains(needle) {
                let m = self.recv().await;
                self.absorb(m);
            }
        }

        async fn wait_push(&mut self, event: &str) -> Value {
            loop {
                if let Some(p) = self.pushes.iter().find(|p| p["event"] == event) {
                    return p.clone();
                }
                let m = self.recv().await;
                self.absorb(m);
            }
        }
    }

    fn eco() -> FakeClaude {
        // `claude` falso que ecoa o que recebe no terminal.
        FakeClaude::script("echo pronto; while read -r l; do echo \"eco:$l\"; done")
    }

    /// O ambiente não cadastra, edita nem exclui projetos (vêm do hub, #178):
    /// os métodos de escrita saíram do `/ws`, mesmo ligado a um hub.
    #[tokio::test(flavor = "multi_thread")]
    async fn ambiente_sem_metodos_de_escrita_de_projeto() {
        let srv = Srv::linked(FakeClaude::exit(0));
        let url = spawn_server(&srv).await;
        let mut ui = Ui::new(&url).await;
        let pid = srv.env.project_id;
        for (m, params) in [
            (
                "projects.add",
                json!({ "name": "o", "gh_owner": "o", "gh_repo": "r", "repo_path": "/x" }),
            ),
            ("projects.add", json!({ "url": "https://github.com/o/r" })),
            (
                "projects.parse_url",
                json!({ "url": "https://github.com/o/r" }),
            ),
            (
                "projects.update",
                json!({ "id": pid, "base_branch": "main" }),
            ),
            ("projects.delete", json!({ "id": pid })),
        ] {
            let v = ui.call(m, params).await;
            assert_eq!(v["error"]["code"], -32601, "{m}: {v}");
        }
        // O projeto continua lá, intacto, e nada foi para o outbox.
        let v = ui.call("projects.list", json!({})).await;
        assert_eq!(v["result"].as_array().unwrap().len(), 1, "{v}");
        assert_eq!(v["result"][0]["base_branch"], "develop", "{v}");
        let pending = srv.env.sessions.ledger().outbox_pending(1000).unwrap();
        assert!(
            pending
                .iter()
                .all(|e| !matches!(e.change, ct_ledger::SyncChange::ProjectRemoved { .. })),
            "{pending:?}"
        );
    }

    /// Os métodos `hub.*` saíram do ambiente (ficam no `cthubd`), exceto
    /// `hub.sync.now`; `env.info` não tem mais o papel de hub.
    #[tokio::test(flavor = "multi_thread")]
    async fn ambiente_nao_tem_metodos_do_hub() {
        let srv = Srv::new(FakeClaude::exit(0));
        let url = spawn_server(&srv).await;
        let mut ui = Ui::new(&url).await;
        let v = ui.call("env.info", json!({})).await;
        assert!(v["result"].get("hub").is_none(), "{v}");
        // Sem `hub_url`/token: não ligado, e nunca puxou configuração.
        let c = &v["result"]["hub_config"];
        assert_eq!(c["linked"], false, "{v}");
        assert_eq!(c["config_rev"], Value::Null);
        // Os desatribuídos ficam fora de `projects.list`, num pedido à parte.
        let v = ui
            .call("projects.list", json!({ "unassigned": true }))
            .await;
        assert_eq!(v["result"], json!([]), "{v}");
        // Sem hub, sem projetos (o legado do `TestEnv` fica de fora).
        let v = ui.call("projects.list", json!({})).await;
        assert_eq!(v["result"], json!([]), "{v}");
        for m in ["hub.projects.list", "hub.tree.get", "hub.dashboard.get"] {
            let v = ui.call(m, json!({})).await;
            assert_eq!(v["error"]["code"], -32601, "{m}");
        }
    }

    /// `docs.dir` (#444): parâmetros conferidos; docs desligados no projeto
    /// recusam sem criar nada (o caminho feliz está nos testes do
    /// `ct-sessions`).
    #[tokio::test(flavor = "multi_thread")]
    async fn docs_dir_confere_parametros_e_recusa_sem_docs() {
        let srv = Srv::new(FakeClaude::exit(0));
        let url = spawn_server(&srv).await;
        let mut ui = Ui::new(&url).await;
        let v = ui.call("docs.dir", json!({})).await;
        assert_eq!(v["error"]["code"], -32602, "{v}");
        let id = srv.env.project_id;
        let v = ui
            .call("docs.dir", json!({ "project_id": id, "epic": "x" }))
            .await;
        assert_eq!(v["error"]["code"], -32602, "{v}");
        let v = ui
            .call("docs.dir", json!({ "project_id": id, "epic": 7 }))
            .await;
        assert_eq!(v["error"]["code"], -32000, "{v}");
        let m = v["error"]["message"].as_str().unwrap();
        assert!(m.contains("docs desligados"), "{v}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn verificacao_de_atualizacao() {
        use ct_sessions::update::{git, run, Build, Source};
        let mut srv = Srv::new(FakeClaude::exit(0));
        let url = spawn_server(&srv).await;
        let mut ui = Ui::new(&url).await;
        let v = ui.call("env.info", json!({})).await;
        assert_eq!(v["result"]["update"], Value::Null);
        assert!(v["result"]["commit"].is_string());
        let v = ui.call("env.check_update", json!({})).await;
        assert_eq!(v["error"]["code"], -32602);

        let origin = srv.env.tmp.path().join("ct-origin");
        std::fs::create_dir(&origin).unwrap();
        let g = |args: &[&str]| run(git(&origin).args(args)).unwrap();
        g(&["init", "-q", "-b", "main"]);
        std::fs::write(
            origin.join("Cargo.toml"),
            "[workspace.package]\nversion = \"9.9.9\"\n",
        )
        .unwrap();
        g(&["add", "."]);
        g(&[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-qm",
            "v9",
        ]);
        let head = g(&["rev-parse", "HEAD"]);
        let source = Source {
            repo: origin.display().to_string(),
            git_ref: "main".into(),
            dir: srv.env.tmp.path().join("update"),
        };
        let installed = Build {
            version: "0.1.0".into(),
            commit: String::new(),
        };
        srv.state.updates = Some(Arc::new(crate::update::Updates::new(
            source,
            installed,
            srv.state.events.clone(),
        )));
        let url = spawn_server(&srv).await;
        let mut ui = Ui::new(&url).await;
        let v = ui.call("env.info", json!({})).await;
        assert_eq!(v["result"]["update"]["ref"], "main");
        assert_eq!(v["result"]["update"]["available"], Value::Null);
        let v = ui.call("env.check_update", json!({})).await;
        let a = &v["result"]["available"];
        assert_eq!(a["build"]["version"], "9.9.9");
        assert_eq!(a["build"]["commit"], head.as_str());
        assert_eq!(a["relation"], "unknown");
        assert_eq!(a["update_available"], true);
        assert_eq!(v["result"]["check_error"], Value::Null);
        let push = ui.wait_push("update.status").await;
        assert_eq!(push["data"]["available"]["build"]["version"], "9.9.9");
        let v = ui.call("env.info", json!({})).await;
        assert_eq!(
            v["result"]["update"]["available"]["build"]["version"],
            "9.9.9"
        );

        // Falha na verificação não apaga o último resultado.
        std::fs::remove_dir_all(&origin).unwrap();
        let v = ui.call("env.check_update", json!({})).await;
        assert!(v["result"]["check_error"].is_string());
        assert_eq!(v["result"]["available"]["build"]["version"], "9.9.9");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn env_update_recusa_com_sessao_viva_e_dispara_o_atualizador() {
        use std::os::unix::fs::PermissionsExt;
        let mut srv = Srv::new(FakeClaude::sleep(30));
        let tmp = srv.env.tmp.path().to_path_buf();
        let dir = tmp.join("update");
        let exe = |name: &str, body: &str| {
            let p = tmp.join(name);
            std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
            p
        };
        // systemd-run falso: descarta as opções e roda o comando.
        let run = exe(
            "fake-systemd-run",
            "while [ \"${1#--}\" != \"$1\" ]; do shift; done; \"$@\"",
        );
        // ctower falso: grava duas fases no state.json, como o atualizador.
        let state = |phase: &str| {
            format!(
                "now=$(($(date +%s)*1000)); mkdir -p {d}; printf '{{\"kind\":\"update\",\"phase\":\"{phase}\",\"ref\":\"main\",\"from\":{{\"version\":\"0.1.0\",\"commit\":\"a\"}},\"to\":null,\"started_at\":%s,\"updated_at\":%s,\"finished_at\":null,\"message\":null,\"error\":null,\"log_tail\":[]}}' $((now+1000)) $((now+1000)) > {d}/state.json",
                d = dir.display()
            )
        };
        let ctower = exe(
            "fake-ctower",
            &format!(
                "echo \"$@\" > {t}/args\n{}\nsleep 1\n{}",
                state("build"),
                state("done"),
                t = tmp.display()
            ),
        );
        let source = ct_sessions::update::Source {
            repo: "/nao-usado".into(),
            git_ref: "main".into(),
            dir: dir.clone(),
        };
        srv.state.updates = Some(Arc::new(
            crate::update::Updates::new(
                source,
                ct_sessions::update::Build::current(),
                srv.state.events.clone(),
            )
            .with_ctower(ctower)
            .with_systemd_run(run.to_str().unwrap()),
        ));
        let url = spawn_server(&srv).await;
        let mut ui = Ui::new(&url).await;

        let s = srv.env.sessions.start(srv.env.request(1, "A")).unwrap();
        let v = ui.call("env.update", json!({})).await;
        assert_eq!(v["error"]["code"], crate::update::LIVE_SESSIONS);
        assert!(v["error"]["message"].as_str().unwrap().contains("1 sessão"));
        assert!(!tmp.join("args").exists());

        let v = ui.call("env.update", json!({ "force": true })).await;
        assert!(v.get("error").is_none(), "{v}");
        assert_eq!(
            std::fs::read_to_string(tmp.join("args")).unwrap().trim(),
            "update --run"
        );
        let push = loop {
            let p = ui.wait_push("update.status").await;
            if p["data"]["state"]["phase"] == "done" {
                break p;
            }
            ui.pushes.retain(|x| x != &p);
        };
        assert_eq!(push["data"]["state"]["kind"], "update");
        let v = ui.call("env.info", json!({})).await;
        assert_eq!(v["result"]["update"]["state"]["phase"], "done");

        // Rollback: mesma recusa com sessão viva; com force, `--rollback`.
        let v = ui.call("env.rollback", json!({})).await;
        assert_eq!(v["error"]["code"], crate::update::LIVE_SESSIONS, "{v}");
        let v = ui.call("env.rollback", json!({ "force": true })).await;
        assert!(v.get("error").is_none(), "{v}");
        let deadline = std::time::Instant::now() + T;
        let args = loop {
            let a = std::fs::read_to_string(tmp.join("args")).unwrap();
            if a.contains("rollback") || std::time::Instant::now() > deadline {
                break a;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        };
        assert_eq!(args.trim(), "update --run --rollback");
        srv.env.sessions.kill(&s.id).unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn config_editavel_grava_e_aplica_o_ref() {
        let mut srv = Srv::new(FakeClaude::exit(0));
        let url0 = spawn_server(&srv).await;
        let mut ui = Ui::new(&url0).await;
        let v = ui.call("env.config.get", json!({})).await;
        assert_eq!(v["error"]["code"], -32602, "{v}");

        let dir = srv.env.tmp.path().join("upd");
        let updates = Arc::new(crate::update::Updates::new(
            ct_sessions::update::Source {
                repo: "/nao-usado".into(),
                git_ref: "main".into(),
                dir,
            },
            ct_sessions::update::Build::current(),
            srv.state.events.clone(),
        ));
        let path = srv.env.tmp.path().join("config.toml");
        srv.state.updates = Some(Arc::clone(&updates));
        srv.state.env_config = Some(Arc::new(crate::env_config::EnvConfig::new(
            path.clone(),
            180,
            Some(Arc::clone(&updates)),
        )));
        let url = spawn_server(&srv).await;
        let mut ui = Ui::new(&url).await;
        let v = ui.call("env.config.get", json!({})).await;
        assert_eq!(v["result"][0]["key"], "update_ref", "{v}");
        assert_eq!(v["result"][0]["value"], "main");
        assert_eq!(v["result"][1]["value"], 180);
        let v = ui
            .call(
                "env.config.set",
                json!({ "key": "update_ref", "value": "develop" }),
            )
            .await;
        assert_eq!(v["result"]["value"], "develop", "{v}");
        assert_eq!(v["result"]["configured"], true);
        let push = ui.wait_push("update.status").await;
        assert_eq!(push["data"]["ref"], "develop");
        let v = ui.call("env.info", json!({})).await;
        assert_eq!(v["result"]["update"]["ref"], "develop");
        assert!(std::fs::read_to_string(&path)
            .unwrap()
            .contains("update_ref = \"develop\""));
        let v = ui
            .call(
                "env.config.set",
                json!({ "key": "bind", "value": "0.0.0.0" }),
            )
            .await;
        assert_eq!(v["error"]["code"], -32602, "{v}");
        let v = ui
            .call("env.config.set", json!({ "key": "update_ref" }))
            .await;
        assert_eq!(v["error"]["code"], -32602, "{v}");
    }

    /// `POST /hooks/permission` como o `ctower hook`, sem esperar a resposta.
    fn permission_hook(
        srv: &Srv,
        session: &str,
    ) -> tokio::task::JoinHandle<(axum::http::StatusCode, Value)> {
        use axum::body::Body;
        use http_body_util::BodyExt;
        use tower::ServiceExt;
        let app = crate::router(srv.state.clone());
        let body = json!({ "session": session, "event": "PermissionRequest", "ts": 1,
                           "payload": { "tool_name": "Bash",
                                        "tool_input": { "command": "rm -rf target" } } });
        tokio::spawn(async move {
            let req = axum::http::Request::post("/hooks/permission")
                .header("content-type", "application/json")
                .header("authorization", format!("Bearer {HOOK}"))
                .body(Body::from(body.to_string()))
                .unwrap();
            let r = app.oneshot(req).await.unwrap();
            let st = r.status();
            let b = r.into_body().collect().await.unwrap().to_bytes();
            (st, serde_json::from_slice(&b).unwrap_or(Value::Null))
        })
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn permissao_respondida_pela_ui() {
        use ct_core::WorkItemStatus;
        let srv = Srv::new(FakeClaude::sleep(30));
        let s = srv.env.sessions.start(srv.env.request(1, "A")).unwrap();
        // Sem UI acompanhando (nem conectada, ou conectada sem `watch`):
        // não segura o agente.
        let (st, v) = permission_hook(&srv, &s.id).await.unwrap();
        assert_eq!(st, axum::http::StatusCode::OK);
        assert_eq!(v, json!({ "decision": null }));
        let url = spawn_server(&srv).await;
        let mut ui = Ui::new(&url).await;
        let (_, v) = permission_hook(&srv, &s.id).await.unwrap();
        assert_eq!(v, json!({ "decision": null }));
        let v = ui.call("session.permission.watch", json!({})).await;
        assert_eq!(v["result"], json!([]), "{v}");
        let hook = permission_hook(&srv, &s.id);
        let push = ui.wait_push("permission_request").await;
        let d = &push["data"];
        assert_eq!(d["session_id"], s.id.as_str(), "{push}");
        assert_eq!(d["tool_name"], "Bash");
        assert_eq!(d["tool_input"]["command"], "rm -rf target");
        let rid = d["request_id"].as_str().unwrap().to_string();
        let wi = srv
            .env
            .sessions
            .ledger()
            .get_work_item(s.work_item_id)
            .unwrap();
        assert_eq!(wi.status, WorkItemStatus::AguardandoVoce);
        let v = ui.call("session.permission.list", json!({})).await;
        assert_eq!(v["result"][0]["request_id"], rid.as_str(), "{v}");

        let v = ui
            .call(
                "session.permission.respond",
                json!({ "request_id": rid, "decision": "talvez" }),
            )
            .await;
        assert_eq!(v["error"]["code"], -32602, "{v}");
        let v = ui
            .call(
                "session.permission.respond",
                json!({ "request_id": rid, "decision": "deny", "message": "não" }),
            )
            .await;
        assert_eq!(v["result"], json!({}), "{v}");
        let (_, v) = hook.await.unwrap();
        assert_eq!(v, json!({ "decision": "deny", "message": "não" }));
        let p = ui.wait_push("permission_resolved").await;
        assert_eq!(p["data"]["decision"], "deny");
        let v = ui.call("session.permission.list", json!({})).await;
        assert_eq!(v["result"], json!([]));
        let v = ui
            .call(
                "session.permission.respond",
                json!({ "request_id": rid, "decision": "allow" }),
            )
            .await;
        assert_eq!(v["error"]["code"], -32000, "{v}");
        // Respondeu: o agente segue.
        let deadline = std::time::Instant::now() + T;
        loop {
            let wi = srv
                .env
                .sessions
                .ledger()
                .get_work_item(s.work_item_id)
                .unwrap();
            if wi.status == WorkItemStatus::EmExecucao {
                break;
            }
            assert!(std::time::Instant::now() < deadline, "{:?}", wi.status);
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        // "Responder no terminal": sem decisão para o hook.
        ui.pushes.clear();
        let hook = permission_hook(&srv, &s.id);
        let push = ui.wait_push("permission_request").await;
        let rid = push["data"]["request_id"].as_str().unwrap().to_string();
        ui.call(
            "session.permission.respond",
            json!({ "request_id": rid, "decision": "ask" }),
        )
        .await;
        let (_, v) = hook.await.unwrap();
        assert_eq!(v, json!({ "decision": null }));

        // Sessão desconhecida: sem decisão, nada pendente.
        let (_, v) = permission_hook(&srv, "nao-existe").await.unwrap();
        assert_eq!(v, json!({ "decision": null }));

        // A UI que acompanhava caiu: volta a não segurar.
        drop(ui);
        let deadline = std::time::Instant::now() + T;
        while srv.state.permissions.watched() {
            assert!(std::time::Instant::now() < deadline);
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let (_, v) = permission_hook(&srv, &s.id).await.unwrap();
        assert_eq!(v, json!({ "decision": null }));
        srv.env.sessions.kill(&s.id).unwrap();
    }

    #[test]
    fn frame_ida_e_volta() {
        let id = uuid::Uuid::new_v4();
        let f = frame(CHANNEL_INPUT, &id, b"abc");
        assert_eq!(parse_frame(&f), Some((CHANNEL_INPUT, id, &b"abc"[..])));
        assert_eq!(parse_frame(&f[..10]), None);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn handshake_exige_token_de_ui() {
        let srv = Srv::new(FakeClaude::exit(0));
        let url = spawn_server(&srv).await;
        for t in ["", "errado", HOOK] {
            let err = connect(&url, t).await.unwrap_err();
            assert!(err.to_string().contains("401"), "{t}: {err}");
        }
        assert!(connect(&url, UI).await.is_ok());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn metodos_de_controle() {
        let srv = Srv::new(FakeClaude::exit(0));
        let url = spawn_server(&srv).await;
        let mut ui = Ui::new(&url).await;
        let v = ui.call("env.info", json!({})).await;
        assert_eq!(v["result"]["protocol"], PROTOCOL_VERSION);
        assert_eq!(
            v["result"]["worktrees_root"],
            srv.env.cfg.worktrees_root.display().to_string()
        );
        assert_eq!(
            v["result"]["env_id"],
            srv.env.sessions.ledger().env_id().to_string()
        );
        assert_eq!(v["result"]["env_name"], "teste");
        assert!(v["result"]["hub_sync"].is_null());
        let v = ui.call("hub.sync.now", json!({})).await;
        assert_eq!(v["error"]["code"], -32000);
        assert!(v["error"]["message"]
            .as_str()
            .unwrap()
            .contains("ambiente não ligado a um hub"));
        let v = ui.call("dashboard.get", json!({})).await;
        assert!(v["result"]["live_sessions"].is_array());
        let v = ui.call("recovery.list", json!({})).await;
        assert_eq!(v["result"], json!([]));
        let v = ui.call("recovery.archived", json!({})).await;
        assert_eq!(v["result"], json!([]), "{v}");
        let v = ui.call("recovery.archived", json!({ "limit": 0 })).await;
        assert_eq!(v["error"]["code"], -32602);
        let v = ui
            .call("transcript.tail", json!({ "session_id": "nao-existe" }))
            .await;
        assert_eq!(v["error"]["code"], -32000, "{v}");
        let v = ui
            .call(
                "transcript.tail",
                json!({ "session_id": "x", "entries": 0 }),
            )
            .await;
        assert_eq!(v["error"]["code"], -32602);
        let v = ui
            .call("session.context", json!({ "session_id": "nao-existe" }))
            .await;
        assert_eq!(v["error"]["code"], -32000, "{v}");
        let v = ui.call("nada", json!({})).await;
        assert_eq!(v["error"]["code"], -32601);
        // Sem hub, nem chega a validar os parâmetros (spec §7).
        let v = ui.call("session.start", json!({ "project_id": 1 })).await;
        assert_eq!(v["error"]["code"], -32000);
        assert_eq!(
            v_info_rc(&mut ui).await,
            json!({ "available": true, "reason": null })
        );
    }

    async fn v_info_rc(ui: &mut Ui) -> Value {
        ui.call("env.info", json!({})).await["result"]["remote_control"].clone()
    }

    /// Remote Control pelo `/ws` (#197): a opção no início, a recusa com
    /// `-32032` por um secret do projeto, o `env.info` e o
    /// `session.remote_control.enable`.
    #[tokio::test(flavor = "multi_thread")]
    async fn remote_control_pelo_ws() {
        let srv = Srv::linked(FakeClaude::script(
            "read cmd; echo 'https://claude.ai/code/session_01Ws'; sleep 30",
        ));
        let url = spawn_server(&srv).await;
        let mut ui = Ui::new(&url).await;
        let pid = srv.env.project_id;
        let v = ui
            .call(
                "session.start",
                json!({ "project_id": pid, "gh_number": 1, "title": "Um", "prompt": "oi",
                        "remote_control": true }),
            )
            .await;
        let cmd: Vec<String> = serde_json::from_value(v["result"]["launch_cmd"].clone()).unwrap();
        let i = cmd.iter().position(|a| a == "--remote-control").unwrap();
        assert_eq!(cmd[i + 1], "demo#1 Um");
        let v = ui
            .call(
                "session.start",
                json!({ "project_id": pid, "gh_number": 2, "title": "Dois", "prompt": "oi",
                        "remote_control": "sim" }),
            )
            .await;
        assert_eq!(v["error"]["code"], -32602, "{v}");

        let v = ui
            .call(
                "session.start",
                json!({ "project_id": pid, "gh_number": 3, "title": "Três", "prompt": "oi" }),
            )
            .await;
        let sid = v["result"]["id"].as_str().unwrap().to_string();
        srv.env.wait_fake_output(&sid);
        let v = ui
            .call(
                "session.remote_control.enable",
                json!({ "session_id": sid }),
            )
            .await;
        assert_eq!(
            v["result"],
            json!({ "requested": true, "rc_session_id": null }),
            "{v}"
        );
        let push = ui.wait_push("session_remote_control").await;
        assert_eq!(push["data"]["rc_session_id"], "session_01Ws");
        let v = ui.call("session.get", json!({ "session_id": sid })).await;
        assert_eq!(v["result"]["rc_session_id"], "session_01Ws");

        srv.env.write_secrets_cache(&[(
            Some("deep-ion-ai/demo"),
            "ANTHROPIC_API_KEY",
            "sk-ant-segredo",
        )]);
        let v = ui
            .call(
                "session.start_project",
                json!({ "project_id": pid, "prompt": "oi", "remote_control": true }),
            )
            .await;
        assert_eq!(v["error"]["code"], REMOTE_CONTROL_UNAVAILABLE, "{v}");
        let msg = v["error"]["message"].as_str().unwrap();
        assert!(
            msg.contains("ANTHROPIC_API_KEY") && !msg.contains("sk-ant"),
            "{msg}"
        );
        // Só os globais contam no `env.info`.
        assert_eq!(v_info_rc(&mut ui).await["available"], true);
        srv.env
            .write_secrets_cache(&[(None, "ANTHROPIC_API_KEY", "sk-ant-segredo")]);
        let rc = v_info_rc(&mut ui).await;
        assert_eq!(rc["available"], false);
        assert!(rc["reason"]
            .as_str()
            .unwrap()
            .contains("secret global do hub"));
        let v = ui
            .call(
                "session.remote_control.enable",
                json!({ "session_id": "nada" }),
            )
            .await;
        assert_eq!(v["error"]["code"], -32000);
        for s in srv.env.sessions.ledger().live_sessions().unwrap() {
            srv.env.sessions.kill(&s.id).ok();
        }
    }

    /// Sessão interrompida de issue concluída sai da Recuperação e do
    /// painel (#285); a linha continua no ledger.
    #[tokio::test(flavor = "multi_thread")]
    async fn recuperacao_sem_sessoes_de_issue_concluida() {
        let srv = Srv::new(FakeClaude::sleep(30));
        let url = spawn_server(&srv).await;
        let mut ui = Ui::new(&url).await;
        let env = &srv.env;
        let mut ids = Vec::new();
        for n in [1, 2] {
            let s = env.sessions.start(env.request(n, "A")).unwrap();
            env.wait_fake_output(&s.id);
            env.sessions
                .ledger()
                .transition_session(&s.id, ct_core::SessionState::Interrupted, None)
                .unwrap();
            env.sessions.kill(&s.id).unwrap();
            ids.push((s.id, s.work_item_id));
        }
        env.sessions
            .ledger()
            .set_work_item_status(ids[0].1, ct_core::WorkItemStatus::Concluido, "teste")
            .unwrap();
        let v = ui.call("recovery.list", json!({})).await;
        let got: Vec<&str> = v["result"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["session"]["id"].as_str().unwrap())
            .collect();
        assert_eq!(got, vec![ids[1].0.as_str()]);
        let v = ui.call("dashboard.get", json!({})).await;
        let got: Vec<&str> = v["result"]["interrupted_sessions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["id"].as_str().unwrap())
            .collect();
        assert_eq!(got, vec![ids[1].0.as_str()]);
        assert_eq!(
            env.sessions.ledger().get_session(&ids[0].0).unwrap().state,
            ct_core::SessionState::Interrupted
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn sessao_com_terminal_para_dois_clientes() {
        let srv = Srv::linked(eco());
        let url = spawn_server(&srv).await;
        let mut a = Ui::new(&url).await;
        let mut b = Ui::new(&url).await;
        let v = a
            .call(
                "session.start",
                json!({ "project_id": srv.env.project_id, "gh_number": 1, "title": "A", "prompt": "oi" }),
            )
            .await;
        let sid = v["result"]["id"].as_str().unwrap().to_string();
        a.wait_push("session_started").await;

        a.call("session.attach", json!({ "session_id": sid })).await;
        b.call("session.attach", json!({ "session_id": sid })).await;
        a.wait_output("pronto").await;
        b.wait_output("pronto").await;

        // Entrada por frame binário e pelo método session.input.
        let uuid = uuid::Uuid::parse_str(&sid).unwrap();
        a.ws.send(WsMsg::Binary(frame(CHANNEL_INPUT, &uuid, b"um\n").into()))
            .await
            .unwrap();
        a.wait_output("eco:um").await;
        b.wait_output("eco:um").await;
        b.call(
            "session.input",
            json!({ "session_id": sid, "data": "dois\n" }),
        )
        .await;
        a.wait_output("eco:dois").await;
        // Peek: fim do scrollback sem anexar, com limite.
        let v = b.call("session.peek", json!({ "session_id": sid })).await;
        let data = v["result"]["data"].as_str().unwrap();
        assert!(data.contains("eco:dois"), "{v}");
        assert!(data.len() <= PEEK_MAX);
        let v = b
            .call("session.peek", json!({ "session_id": sid, "bytes": 3 }))
            .await;
        assert_eq!(v["result"]["data"].as_str().unwrap().len(), 3, "{v}");
        for bytes in [json!(0), json!(PEEK_MAX + 1), json!("x")] {
            let v = b
                .call("session.peek", json!({ "session_id": sid, "bytes": bytes }))
                .await;
            assert_eq!(v["error"]["code"], -32602, "{bytes}");
        }
        let v = b
            .call(
                "session.peek",
                json!({ "session_id": "00000000-0000-0000-0000-000000000000" }),
            )
            .await;
        assert_eq!(v["error"]["code"], -32602);
        let v = a
            .call(
                "session.resize",
                json!({ "session_id": sid, "cols": 100, "rows": 30 }),
            )
            .await;
        assert!(v.get("result").is_some(), "{v}");

        a.call("session.detach", json!({ "session_id": sid })).await;
        a.call("session.kill", json!({ "session_id": sid })).await;
        let ex = b.wait_push("session_exited").await;
        assert_eq!(ex["data"]["state"], "exited", "{ex}");
        // Encerrada, sai do attach, mas segue legível para a aba (#283).
        let v = b.call("session.attach", json!({ "session_id": sid })).await;
        assert_eq!(v["error"]["message"], "sessão não está rodando");
        let v = b.call("session.get", json!({ "session_id": sid })).await;
        assert_eq!(v["result"]["state"], "exited", "{v}");
        assert_eq!(v["result"]["work_item_concluded"], false, "{v}");
        let v = b.call("session.get", json!({ "session_id": "nada" })).await;
        assert!(v.get("error").is_some(), "{v}");
    }

    /// Shell livre (#428): abre no worktree do item, usa o terminal das
    /// sessões pelo id (attach, frame binário, input e resize), aparece em
    /// `shell.list` e avisa `shell_exited` com o código.
    #[tokio::test(flavor = "multi_thread")]
    async fn shell_livre_pelo_terminal_das_sessoes() {
        let srv = Srv::linked(FakeClaude::exit(0));
        let url = spawn_server(&srv).await;
        let mut ui = Ui::new(&url).await;
        let v = ui
            .call(
                "session.start",
                json!({ "project_id": srv.env.project_id, "gh_number": 2, "title": "B", "prompt": "oi" }),
            )
            .await;
        let wi = v["result"]["work_item_id"].as_i64().unwrap();
        let v = ui
            .call(
                "shell.open",
                json!({ "work_item_id": wi, "cols": 100, "rows": 30 }),
            )
            .await;
        let sh = v["result"].clone();
        let id = sh["id"].as_str().unwrap().to_string();
        assert_eq!(sh["work_item_id"], wi, "{v}");
        assert_eq!(sh["busy"], false, "{v}");
        assert!(sh["cwd"].as_str().unwrap().contains("feat__2-b"), "{v}");
        let v = ui.call("shell.list", json!({})).await;
        assert_eq!(v["result"][0]["id"], id.as_str(), "{v}");

        ui.call("session.attach", json!({ "session_id": id })).await;
        ui.wait_output("FIM-ENV").await;
        assert!(String::from_utf8_lossy(&ui.binary).contains("SIZE=30 100"));
        let uuid = uuid::Uuid::parse_str(&id).unwrap();
        ui.ws
            .send(WsMsg::Binary(
                frame(CHANNEL_INPUT, &uuid, b"echo um-$((0+1))\n").into(),
            ))
            .await
            .unwrap();
        ui.wait_output("um-1").await;
        let v = ui
            .call(
                "session.input",
                json!({ "session_id": id, "data": "stty size; echo dois-$((1+1))\n" }),
            )
            .await;
        assert!(v.get("result").is_some(), "{v}");
        ui.wait_output("dois-2").await;
        let v = ui
            .call(
                "session.resize",
                json!({ "session_id": id, "cols": 70, "rows": 20 }),
            )
            .await;
        assert!(v.get("result").is_some(), "{v}");
        ui.call(
            "session.input",
            json!({ "session_id": id, "data": "stty size\n" }),
        )
        .await;
        ui.wait_output("20 70").await;
        ui.call(
            "session.input",
            json!({ "session_id": id, "data": "exit 3\n" }),
        )
        .await;
        let ex = ui.wait_push("shell_exited").await;
        assert_eq!(ex["data"], json!({ "shell_id": id, "code": 3 }), "{ex}");
        let v = ui.call("session.attach", json!({ "session_id": id })).await;
        assert_eq!(v["error"]["message"], "sessão não está rodando");
        let v = ui.call("shell.list", json!({})).await;
        assert_eq!(v["result"], json!([]), "{v}");
        let v = ui.call("shell.close", json!({ "shell_id": id })).await;
        assert_eq!(v["error"]["code"], -32000, "{v}");

        // Recusas: item sem worktree e parâmetros faltando.
        let semwt = srv
            .env
            .sessions
            .ledger()
            .ensure_work_item(srv.env.project_id, 50, WorkItemKind::Issue)
            .unwrap();
        let v = ui
            .call("shell.open", json!({ "work_item_id": semwt.id }))
            .await;
        assert_eq!(v["error"]["code"], -32000, "{v}");
        assert!(v["error"]["message"]
            .as_str()
            .unwrap()
            .contains("não tem worktree"));
        let v = ui.call("shell.open", json!({})).await;
        assert_eq!(v["error"]["code"], -32602, "{v}");
        let v = ui.call("shell.close", json!({})).await;
        assert_eq!(v["error"]["code"], -32602, "{v}");
    }

    /// Os métodos `shell.*` ficam fora do canal do hub (PWA, #205).
    #[tokio::test(flavor = "multi_thread")]
    async fn shell_fora_do_canal_do_hub() {
        let srv = Srv::linked(FakeClaude::exit(0));
        for m in ["shell.open", "shell.list", "shell.close"] {
            let r = channel_call(srv.state.clone(), m, &json!({ "work_item_id": 1 })).await;
            assert_eq!(
                r.unwrap_err().0,
                ct_http::sync::CHANNEL_METHOD_UNAVAILABLE,
                "{m}"
            );
        }
        assert!(srv.env.sessions.list_shells().is_empty());
    }

    fn gh_issue(n: i64, parent: Option<i64>, subs: i64, label: &str) -> Value {
        json!({
            "number": n, "title": format!("Issue {n}"), "state": "open",
            "labels": [{ "name": label }],
            "html_url": format!("https://github.com/deep-ion-ai/demo/issues/{n}"),
            "updated_at": "2026-10-14T10:00:00Z",
            "parent_issue_url": parent.map(|p| format!("https://api.github.com/repos/deep-ion-ai/demo/issues/{p}")),
            "sub_issues_summary": { "total": subs },
        })
    }

    /// Projeto Jira (#349): o `sync.now` vai ao jira-cli (o GitHub não o
    /// busca) e o `env.info` mostra a prontidão do último ciclo.
    #[tokio::test(flavor = "multi_thread")]
    async fn sync_now_do_projeto_jira_e_prontidao_no_env_info() {
        use ct_jira::fake::{api_issue, FakeJira, FAKE_SERVER};
        let (mut srv, gh) = Srv::with_github(FakeClaude::exit(0));
        let fake = FakeJira::new(srv.env.tmp.path(), "senha-de-teste");
        let repo_path = srv.env.repo.display().to_string();
        srv.env
            .sessions
            .ledger()
            .apply_hub_config(
                &ct_ledger::ConfigResponse {
                    config_rev: 1,
                    config_epoch: 1,
                    changed: true,
                    projects: Some(vec![ct_ledger::ConfigProject {
                        docs: Default::default(),
                        owner: "deep-ion-ai".into(),
                        repo: "demo".into(),
                        name: "demo".into(),
                        base_branch: "develop".into(),
                        protected_branches: vec![],
                        repo_path,
                        rev: 1,
                        tracker: ct_core::TrackerConfig::Jira(ct_core::JiraConfig {
                            url: FAKE_SERVER.into(),
                            project_key: "PROJ".into(),
                            credential_cmd: "printf 'senha-de-teste\\n'".into(),
                            config_file: fake.config_file().display().to_string(),
                            type_map: ct_core::default_type_map(),
                            ..Default::default()
                        }),
                        forge: ct_core::ForgeConfig::Github,
                        branch_pattern: None,
                        attribution: true,
                        sync: false,
                        rules: Default::default(),
                    }]),
                    removed: Some(vec![]),
                },
                |p| p.repo_path.clone(),
            )
            .unwrap();
        fake.set_issues(&[
            api_issue("PROJ-1", "Épico", "new"),
            api_issue("PROJ-2", "Tarefa", "done"),
        ]);
        srv.state.tracker = Some(Arc::new(ct_sessions::JiraGitlabSync::new(
            srv.env.sessions.clone(),
            ct_sessions::TrackerSyncConfig::new(fake.bin().to_str().unwrap()),
        )));
        let url = spawn_server(&srv).await;
        let mut ui = Ui::new(&url).await;

        let v = ui.call("env.info", json!({})).await;
        assert_eq!(v["result"]["trackers"], json!([]), "{v}");
        gh.clear_requests();
        let v = ui.call("sync.now", json!({})).await;
        let r = v["result"].as_array().unwrap();
        assert_eq!(r.len(), 1, "{v}");
        assert_eq!(
            (r[0]["ok"].clone(), r[0]["issues"].clone()),
            (json!(true), json!(2))
        );
        assert!(gh.requests().is_empty(), "o GitHub não busca projeto Jira");
        let v = ui
            .call("sync.now", json!({ "project_id": srv.env.project_id }))
            .await;
        assert_eq!(v["result"][0]["ok"], true, "{v}");

        let v = ui.call("env.info", json!({})).await;
        let t = &v["result"]["trackers"][0];
        assert_eq!(t["owner"], "deep-ion-ai");
        assert_eq!(t["jira"]["cli"], "1.7.0");
        assert_eq!(t["jira"]["server_version"], "8.5.2");
        assert_eq!(t["jira"]["credential_ok"], true);
        assert!(!v.to_string().contains("senha-de-teste"));

        let v = ui
            .call("tree.get", json!({ "project_id": srv.env.project_id }))
            .await;
        let roots = v["result"]["roots"].as_array().unwrap();
        assert_eq!(roots.len(), 2, "{v}");
        assert_eq!(roots[1]["issue"]["state"], "closed");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn sync_arvore_e_inicio_pelo_cache() {
        use ct_gh::sync::{issues_path, sub_issues_path};
        let (mut srv, gh) = Srv::with_github(FakeClaude::sleep(30));
        srv.link();
        gh.set(
            &issues_path("deep-ion-ai", "demo", "open", None),
            json!([gh_issue(1, None, 1, "epic"), gh_issue(2, Some(1), 0, "bug")]),
        );
        gh.set(
            &sub_issues_path("deep-ion-ai", "demo", 1),
            json!([gh_issue(2, Some(1), 0, "bug")]),
        );
        let url = spawn_server(&srv).await;
        let mut ui = Ui::new(&url).await;

        let v = ui.call("sync.now", json!({})).await;
        assert_eq!(v["result"][0]["ok"], true, "{v}");
        assert_eq!(v["result"][0]["issues"], 2);
        ui.wait_push("sync_status").await;
        let v = ui
            .call("sync.now", json!({ "project_id": srv.env.project_id }))
            .await;
        assert_eq!(v["result"].as_array().unwrap().len(), 1);

        let v = ui
            .call("tree.get", json!({ "project_id": srv.env.project_id }))
            .await;
        let roots = &v["result"]["roots"];
        assert_eq!(roots.as_array().unwrap().len(), 1, "{v}");
        assert_eq!(roots[0]["issue"]["gh_number"], 1);
        assert_eq!(roots[0]["children"][0]["issue"]["gh_number"], 2);
        assert!(v["result"]["sync"]["last_ok_at"].is_i64());

        let v = ui.call("dashboard.get", json!({})).await;
        assert_eq!(v["result"]["sync"].as_array().unwrap().len(), 1);

        // Só número e prompt: título, labels (bug → fix/) e tipo vêm do cache.
        let v = ui
            .call(
                "session.start",
                json!({ "project_id": srv.env.project_id, "gh_number": 2, "prompt": "oi" }),
            )
            .await;
        let sid = v["result"]["id"]
            .as_str()
            .expect("session.start")
            .to_string();
        let wi = srv
            .env
            .sessions
            .ledger()
            .find_work_item(srv.env.project_id, 2)
            .unwrap()
            .unwrap();
        assert_eq!(wi.kind, WorkItemKind::Issue);
        assert_eq!(wi.branch.as_deref(), Some("fix/2-issue-2"));
        // O épico vira `epic` (wip/) pelo has_sub_issues.
        let v = ui
            .call(
                "session.start",
                json!({ "project_id": srv.env.project_id, "gh_number": 1, "prompt": "oi" }),
            )
            .await;
        let sid_epic = v["result"]["id"].as_str().unwrap().to_string();
        let epic = srv
            .env
            .sessions
            .ledger()
            .find_work_item(srv.env.project_id, 1)
            .unwrap()
            .unwrap();
        assert_eq!(epic.kind, WorkItemKind::Epic);
        assert!(epic.branch.unwrap().starts_with("wip/1-"));
        // Fora do cache e sem título: parâmetro inválido.
        let v = ui
            .call(
                "session.start",
                json!({ "project_id": srv.env.project_id, "gh_number": 99, "prompt": "oi" }),
            )
            .await;
        assert_eq!(v["error"]["code"], -32602);
        for s in [sid, sid_epic] {
            ui.call("session.kill", json!({ "session_id": s })).await;
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn sessao_que_saiu_com_erro_fica_interrompida_no_dashboard() {
        let srv = Srv::linked(FakeClaude::exit(3));
        let url = spawn_server(&srv).await;
        let mut ui = Ui::new(&url).await;
        let v = ui
            .call(
                "session.start",
                json!({ "project_id": srv.env.project_id, "gh_number": 1, "title": "A", "prompt": "oi" }),
            )
            .await;
        let sid = v["result"]["id"].clone();
        ui.wait_push("session_exited").await;
        // Saída anormal entra na recuperação, com o código (#283).
        let v = ui.call("dashboard.get", json!({})).await;
        let int = v["result"]["interrupted_sessions"].as_array().unwrap();
        assert_eq!(int.len(), 1, "{v}");
        assert_eq!(int[0]["id"], sid);
        assert_eq!(int[0]["exit_code"], 3);
        assert_eq!(v["result"]["failed_sessions"], json!([]));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn gestor_exige_hub_e_token_de_agente() {
        let mut srv = Srv::new(FakeClaude::sleep(30));
        let start = json!({ "scope": { "kind": "portfolio" }, "prompt": "oi" });
        {
            let url = spawn_server(&srv).await;
            let mut ui = Ui::new(&url).await;
            let v = ui.call("session.start_gestor", start.clone()).await;
            assert_eq!(v["error"]["code"], -32000);
            assert!(v["error"]["message"]
                .as_str()
                .unwrap()
                .contains("não ligado a um hub"));
        }
        srv.link();
        let url = spawn_server(&srv).await;
        let mut ui = Ui::new(&url).await;
        let v = ui.call("env.info", json!({})).await;
        assert_eq!(v["result"]["protocol"], PROTOCOL_VERSION);
        assert_eq!(v["result"]["hub_config"]["agent_token"], false);
        let v = ui.call("session.start_gestor", start.clone()).await;
        assert!(
            v["error"]["message"]
                .as_str()
                .unwrap()
                .contains("set-agent-token"),
            "{v}"
        );
        std::fs::write(srv.env.cfg.dirs.hub_agent_token_file(), "cta_x\n").unwrap();
        let v = ui.call("env.info", json!({})).await;
        assert_eq!(v["result"]["hub_config"]["agent_token"], true);
        for bad in [
            json!({ "prompt": "x" }),
            json!({ "scope": { "kind": "project" } }),
        ] {
            let v = ui.call("session.start_gestor", bad.clone()).await;
            assert_eq!(v["error"]["code"], -32602, "{bad}");
        }
        let g = ui
            .call(
                "session.start_gestor",
                json!({ "scope": { "kind": "project", "owner": "deep-ion-ai", "repo": "demo" },
                        "prompt": "oi", "model": "opus", "cols": 100, "rows": 30 }),
            )
            .await;
        assert_eq!(
            g["result"]["cwd"],
            srv.env.cfg.gestor_dir.to_string_lossy().as_ref(),
            "{g}"
        );
        assert_eq!(g["result"]["model"], "opus");
        // O sentinela não aparece em projects.list nem em dashboard.get, mas
        // a sessão viva do gestor sim.
        let v = ui.call("projects.list", json!({})).await;
        assert!(v["result"]
            .as_array()
            .unwrap()
            .iter()
            .all(|p| p["gh_owner"] != "_ctower"));
        let d = ui.call("dashboard.get", json!({})).await;
        assert!(d["result"]["projects"]
            .as_array()
            .unwrap()
            .iter()
            .all(|p| p["project"]["gh_owner"] != "_ctower"));
        assert!(d["result"]["live_sessions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["id"] == g["result"]["id"]));
        assert_eq!(
            d["result"]["gestor_work_item_id"],
            g["result"]["work_item_id"]
        );
        // Uma segunda sessão com a primeira viva: as duas no dashboard, cada
        // uma com o seu escopo.
        let g2 = ui
            .call(
                "session.start_gestor",
                json!({ "scope": { "kind": "health" }, "prompt": "oi" }),
            )
            .await;
        assert_eq!(g2["result"]["work_item_id"], g["result"]["work_item_id"]);
        assert_ne!(g2["result"]["id"], g["result"]["id"]);
        let d = ui.call("dashboard.get", json!({})).await;
        let ids = [&g, &g2].map(|x| x["result"]["id"].as_str().unwrap().to_string());
        let live = d["result"]["live_sessions"].as_array().unwrap();
        assert!(ids
            .iter()
            .all(|id| live.iter().any(|s| s["id"] == *id.as_str())));
        assert_eq!(
            d["result"]["gestor_scopes"],
            json!({
                ids[0].clone(): { "kind": "project", "owner": "deep-ion-ai", "repo": "demo" },
                ids[1].clone(): { "kind": "health" },
            })
        );
        // Revisão do gestor sem PR (#239): uma a uma, depois o resto.
        let gwi = g["result"]["work_item_id"].as_i64().unwrap();
        let first = {
            let l = srv.env.sessions.ledger();
            let mut ids = vec![];
            for t in ["a", "b", "c"] {
                ids.push(
                    l.record_decision(&ct_ledger::NewDecision {
                        work_item_id: gwi,
                        session_id: None,
                        origin: ct_core::DecisionOrigin::Autonoma,
                        title: t.into(),
                        context: "c".into(),
                        decision: "d".into(),
                        consequences: None,
                    })
                    .unwrap()
                    .id,
                );
            }
            ids[0]
        };
        let v = ui
            .call(
                "review.mark_reviewed",
                json!({ "work_item_id": gwi, "decision_ids": [first] }),
            )
            .await;
        assert_eq!(v["result"]["marked"], 1, "{v}");
        let d1 = srv.env.sessions.ledger().get_decision(first).unwrap();
        assert!(d1.reviewed_at.is_some() && d1.reviewed_in_pr.is_none());
        let v = ui
            .call("review.mark_reviewed", json!({ "work_item_id": gwi }))
            .await;
        assert_eq!(v["result"]["marked"], 2, "{v}");
        let v = ui.call("review.get", json!({ "work_item_id": gwi })).await;
        assert_eq!(v["result"]["decisions"], json!([]));
        for id in &ids {
            srv.env.sessions.kill(id).ok();
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn token_do_github_configuravel_sem_expor_o_token() {
        let (srv, gh) = Srv::with_github(FakeClaude::exit(0));
        gh.set("/user", json!({ "login": "octocat" }));
        let url = spawn_server(&srv).await;
        let mut ui = Ui::new(&url).await;
        let v = ui.call("github.status", json!({})).await;
        assert_eq!(v["result"]["source"], "gh", "{v}");
        assert_eq!(v["result"]["login"], "octocat");
        let v = ui
            .call("github.set_token", json!({ "token": "invalido" }))
            .await;
        assert_eq!(v["error"]["code"], -32000, "{v}");
        let v = ui.call("github.set_token", json!({ "token": "tok" })).await;
        assert_eq!(v["result"]["source"], "token", "{v}");
        assert!(!v.to_string().contains("\"tok\""), "{v}");
        let file = srv.env.tmp.path().join("github.token");
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "tok");
        let v = ui.call("github.clear_token", json!({})).await;
        assert_eq!(v["result"]["source"], "gh", "{v}");
        assert!(!file.exists());
        let v = ui.call("github.set_token", json!({})).await;
        assert_eq!(v["error"]["code"], -32602);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn comenta_no_pr_pelo_projeto() {
        let (mut srv, gh) = Srv::with_github(FakeClaude::exit(0));
        srv.link();
        let path = "/repos/deep-ion-ai/demo/issues/40/comments";
        gh.set(path, json!({ "id": 7, "html_url": "https://x/pull/40#c7" }));
        let url = spawn_server(&srv).await;
        let mut ui = Ui::new(&url).await;
        let v = ui
            .call(
                "github.pr.comment",
                json!({ "owner": "deep-ion-ai", "repo": "demo", "pr": 40, "body": "ok" }),
            )
            .await;
        assert_eq!(
            v["result"],
            json!({ "id": 7, "url": "https://x/pull/40#c7" }),
            "{v}"
        );
        assert_eq!(
            gh.posted(),
            vec![(path.to_string(), json!({ "body": "ok" }))]
        );
        let v = ui
            .call(
                "github.pr.comment",
                json!({ "project_id": srv.env.project_id, "pr": 40, "body": "  " }),
            )
            .await;
        assert_eq!(v["error"]["code"], -32602, "{v}");
        let v = ui
            .call(
                "github.pr.comment",
                json!({ "owner": "deep-ion-ai", "repo": "outro", "pr": 1, "body": "x" }),
            )
            .await;
        assert_eq!(v["error"]["code"], -32602, "{v}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn sessao_do_projeto_sem_issue() {
        let srv = Srv::linked(FakeClaude::exit(0));
        let url = spawn_server(&srv).await;
        let mut ui = Ui::new(&url).await;
        let v = ui
            .call(
                "session.start_project",
                json!({ "project_id": srv.env.project_id, "prompt": "criar o épico" }),
            )
            .await;
        assert_eq!(
            v["result"]["cwd"],
            srv.env.repo.to_string_lossy().as_ref(),
            "{v}"
        );
        let v = ui.call("dashboard.get", json!({})).await;
        let items = v["result"]["projects"][0]["work_items"].as_array().unwrap();
        assert_eq!(items[0]["gh_number"], 0);
        let v = ui
            .call(
                "session.start",
                json!({ "project_id": srv.env.project_id, "gh_number": 0, "title": "x", "prompt": "x" }),
            )
            .await;
        assert_eq!(v["error"]["code"], -32000);
        let v = ui
            .call(
                "session.start_project",
                json!({ "project_id": 999, "prompt": "x" }),
            )
            .await;
        assert_eq!(v["error"]["code"], -32602);
        assert!(v["error"]["message"]
            .as_str()
            .unwrap()
            .contains(NOT_ASSIGNED));
        // Pela chave natural (visão do hub), sem diferenciar maiúsculas.
        let v = ui
            .call(
                "session.start",
                json!({ "owner": "Deep-Ion-AI", "repo": "demo", "gh_number": 7, "title": "sete", "prompt": "x" }),
            )
            .await;
        assert!(v["result"]["id"].is_string(), "{v}");
        let v = ui
            .call(
                "session.start",
                json!({ "owner": "outro", "repo": "demo", "gh_number": 7, "title": "x", "prompt": "x" }),
            )
            .await;
        assert_eq!(v["error"]["code"], -32602);
        assert_eq!(
            v["error"]["message"],
            format!("{NOT_ASSIGNED}: outro/demo"),
            "{v}"
        );
    }

    /// Projeto desatribuído no hub (clone ainda no disco) não inicia sessão.
    #[tokio::test(flavor = "multi_thread")]
    async fn projeto_nao_atribuido_e_recusado() {
        let srv = Srv::linked(FakeClaude::exit(0));
        let url = spawn_server(&srv).await;
        let mut ui = Ui::new(&url).await;
        // Primeiro pull de um hub que não atribuiu o projeto legado.
        srv.env
            .sessions
            .ledger()
            .apply_hub_config(
                &ct_ledger::ConfigResponse {
                    config_rev: 1,
                    config_epoch: 1,
                    changed: true,
                    projects: Some(vec![]),
                    removed: None,
                },
                |_| String::new(),
            )
            .unwrap();
        let v = ui.call("projects.list", json!({})).await;
        assert_eq!(v["result"], json!([]), "{v}");
        let pid = srv.env.project_id;
        for (method, params) in [
            (
                "session.start",
                json!({ "project_id": pid, "gh_number": 1, "title": "t", "prompt": "x" }),
            ),
            (
                "session.start",
                json!({ "owner": "deep-ion-ai", "repo": "demo", "gh_number": 1, "title": "t", "prompt": "x" }),
            ),
            (
                "session.start_project",
                json!({ "project_id": pid, "prompt": "x" }),
            ),
        ] {
            let v = ui.call(method, params).await;
            assert_eq!(v["error"]["code"], -32602, "{method}: {v}");
            assert!(
                v["error"]["message"]
                    .as_str()
                    .unwrap()
                    .starts_with(NOT_ASSIGNED),
                "{method}: {v}"
            );
        }
    }

    /// Sem `hub_url`/`hub.token`: sem projetos e sem iniciar sessão, mas a
    /// sessão que já existia continua podendo ser retomada (spec §7).
    #[tokio::test(flavor = "multi_thread")]
    async fn sem_hub_recusa_iniciar_e_retoma() {
        let srv = Srv::new(FakeClaude::sleep(30));
        // Sessão de antes (ex.: o `hub_url` saiu do config na migração).
        let s = srv.env.sessions.start(srv.env.request(1, "A")).unwrap();
        let transcript = srv.env.tmp.path().join("t.jsonl");
        std::fs::write(&transcript, "{}\n").unwrap();
        // Hooks continuam sem hub: o SessionStart grava o transcript.
        let (st, _) = srv
            .post(
                "/hooks",
                Some(HOOK),
                &[],
                json!({ "session": s.id, "event": "SessionStart", "ts": 1,
                        "payload": { "session_id": s.tool_session_id,
                                     "transcript_path": transcript } }),
            )
            .await;
        assert_eq!(st, axum::http::StatusCode::ACCEPTED);
        let gravada = srv.env.sessions.ledger().get_session(&s.id).unwrap();
        assert_eq!(
            gravada.transcript_path.as_deref(),
            Some(transcript.to_str().unwrap())
        );
        srv.env.sessions.kill(&s.id).unwrap();
        let url = spawn_server(&srv).await;
        let mut ui = Ui::new(&url).await;
        // Sessão parada: o fim do transcript sai sem PTY.
        let v = ui
            .call("transcript.tail", json!({ "session_id": s.id }))
            .await;
        assert_eq!(
            v["result"],
            json!({ "source": "original", "entries": [] }),
            "{v}"
        );
        let v = ui
            .call("session.context", json!({ "session_id": s.id }))
            .await;
        assert_eq!(v["result"]["tokens"], Value::Null, "{v}");
        let v = ui.call("env.info", json!({})).await;
        assert_eq!(v["result"]["hub_config"]["linked"], false, "{v}");
        let v = ui.call("projects.list", json!({})).await;
        assert_eq!(v["result"], json!([]), "{v}");
        let v = ui.call("dashboard.get", json!({})).await;
        assert_eq!(v["result"]["projects"], json!([]), "{v}");
        let pid = srv.env.project_id;
        for (method, params) in [
            (
                "session.start",
                json!({ "project_id": pid, "gh_number": 2, "title": "t", "prompt": "x" }),
            ),
            (
                "session.start",
                json!({ "owner": "deep-ion-ai", "repo": "demo", "gh_number": 2, "prompt": "x" }),
            ),
            (
                "session.start_project",
                json!({ "project_id": pid, "prompt": "x" }),
            ),
        ] {
            let v = ui.call(method, params).await;
            assert_eq!(v["error"]["code"], -32000, "{method}: {v}");
            assert_eq!(
                v["error"]["message"],
                crate::hub_sync::NOT_LINKED,
                "{method}: {v}"
            );
        }
        assert_eq!(
            crate::hub_sync::NOT_LINKED,
            "ambiente não ligado a um hub: configure hub_url e grave o token com \
             \"ctower hub set-token\""
        );
        // Retomar a sessão existente continua.
        let deadline = std::time::Instant::now() + T;
        while srv.env.sessions.ledger().get_session(&s.id).unwrap().state
            != ct_core::SessionState::Exited
        {
            assert!(std::time::Instant::now() < deadline, "sessão não saiu");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let v = ui
            .call("session.resume", json!({ "session_id": s.id }))
            .await;
        let new = v["result"]["id"]
            .as_str()
            .expect("session.resume")
            .to_string();
        assert_ne!(new, s.id);
        ui.call("session.kill", json!({ "session_id": new })).await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn inicio_aceita_tamanho_do_terminal() {
        let srv = Srv::linked(FakeClaude::exit(0));
        let url = spawn_server(&srv).await;
        let mut ui = Ui::new(&url).await;
        let pid = srv.env.project_id;
        let cases = [
            (
                "session.start",
                json!({ "project_id": pid, "gh_number": 3, "title": "t", "prompt": "x", "cols": 90, "rows": 25 }),
                "SIZE=25 90\n",
            ),
            (
                "session.start",
                json!({ "project_id": pid, "gh_number": 4, "title": "t", "prompt": "x" }),
                "SIZE=40 120\n",
            ),
            (
                "session.start_project",
                json!({ "project_id": pid, "prompt": "x", "cols": 150, "rows": 45 }),
                "SIZE=45 150\n",
            ),
            (
                "session.start_project",
                json!({ "project_id": pid, "prompt": "x" }),
                "SIZE=40 120\n",
            ),
        ];
        for (method, params, size) in cases {
            let v = ui.call(method, params).await;
            let sid = v["result"]["id"]
                .as_str()
                .expect("id da sessão")
                .to_string();
            let out = srv.env.wait_fake_output(&sid);
            assert!(out.contains(size), "{method}: {out}");
        }
        for bad in [
            json!({ "project_id": pid, "prompt": "x", "cols": 90 }),
            json!({ "project_id": pid, "prompt": "x", "cols": 0, "rows": 20 }),
            json!({ "project_id": pid, "prompt": "x", "cols": 70000, "rows": 20 }),
        ] {
            let v = ui.call("session.start_project", bad).await;
            assert_eq!(v["error"]["code"], -32602, "{v}");
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn inicio_aceita_modelo_e_modo() {
        let srv = Srv::linked(FakeClaude::exit(0));
        let url = spawn_server(&srv).await;
        let mut ui = Ui::new(&url).await;
        let pid = srv.env.project_id;
        let v = ui
            .call(
                "session.start",
                json!({ "project_id": pid, "gh_number": 5, "title": "t", "prompt": "x",
                        "model": "haiku", "mode": "subagents" }),
            )
            .await;
        assert_eq!(v["result"]["model"], "haiku", "{v}");
        assert_eq!(v["result"]["mode"], "subagents", "{v}");
        let out = srv
            .env
            .wait_fake_output(v["result"]["id"].as_str().unwrap());
        assert!(out.contains("--model\nhaiku\n"), "{out}");
        let v = ui
            .call(
                "session.start_project",
                json!({ "project_id": pid, "prompt": "x", "model": "opus", "mode": "native" }),
            )
            .await;
        assert_eq!(v["result"]["model"], "opus", "{v}");
        assert_eq!(v["result"]["mode"], "native", "{v}");
        let v = ui
            .call(
                "session.start_project",
                json!({ "project_id": pid, "prompt": "x" }),
            )
            .await;
        assert!(v["result"]["model"].is_null(), "{v}");
        assert_eq!(v["result"]["mode"], "native", "{v}");
        for (method, bad) in [
            (
                "session.start",
                json!({ "gh_number": 6, "title": "t", "model": "gpt-4" }),
            ),
            (
                "session.start",
                json!({ "gh_number": 6, "title": "t", "mode": "turbo" }),
            ),
            ("session.start_project", json!({ "model": 3 })),
            ("session.start_project", json!({ "mode": "" })),
        ] {
            let mut params = bad.clone();
            params["project_id"] = json!(pid);
            params["prompt"] = json!("x");
            let v = ui.call(method, params).await;
            assert_eq!(v["error"]["code"], -32602, "{bad}: {v}");
        }
        assert!(srv
            .env
            .sessions
            .ledger()
            .find_work_item(pid, 6)
            .unwrap()
            .is_none());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn sync_now_sem_github() {
        let srv = Srv::new(FakeClaude::exit(0));
        let url = spawn_server(&srv).await;
        let mut ui = Ui::new(&url).await;
        let v = ui.call("sync.now", json!({})).await;
        assert_eq!(v["error"]["code"], -32000);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn revisao_lista_e_marca_decisoes() {
        use ct_core::PrState;
        use ct_ledger::{GithubSnapshot, IssueRow, NewDecision, PrRow};
        let srv = Srv::linked(FakeClaude::exit(0));
        let pid = srv.env.project_id;
        let (epic, outro) = {
            let l = srv.env.sessions.ledger();
            let epic = l.ensure_work_item(pid, 1, WorkItemKind::Epic).unwrap();
            let filha = l.ensure_work_item(pid, 2, WorkItemKind::Issue).unwrap();
            let outro = l.ensure_work_item(pid, 3, WorkItemKind::Issue).unwrap();
            let issue = |n, parent| IssueRow {
                gh_number: n,
                title: format!("Issue {n}"),
                state: "open".into(),
                labels: vec![],
                parent_number: parent,
                has_sub_issues: parent.is_none(),
                url: String::new(),
                updated_at: 0,
                sprints: Vec::new(),
            };
            l.apply_github(&GithubSnapshot {
                project_id: pid,
                issues: vec![issue(1, None), issue(2, Some(1))],
                prs: vec![PrRow {
                    number: 40,
                    work_item_id: epic.id,
                    head: "wip/1-x".into(),
                    base: "develop".into(),
                    state: PrState::Open,
                    url: String::new(),
                    updated_at: 0,
                    head_sha: None,
                }],
                etags: vec![],
                synced_at: 1,
            })
            .unwrap();
            let dec = |wi: i64, origin| {
                l.record_decision(&NewDecision {
                    work_item_id: wi,
                    session_id: None,
                    origin,
                    title: "t".into(),
                    context: "c".into(),
                    decision: "d".into(),
                    consequences: None,
                })
                .unwrap()
            };
            dec(epic.id, DecisionOrigin::Autonoma);
            dec(filha.id, DecisionOrigin::Autonoma);
            dec(filha.id, DecisionOrigin::Orientada);
            dec(outro.id, DecisionOrigin::Autonoma);
            (epic.id, outro.id)
        };
        let url = spawn_server(&srv).await;
        let mut ui = Ui::new(&url).await;
        // Dashboard: título do cache e decisões pendentes (épico soma a filha).
        let v = ui.call("dashboard.get", json!({})).await;
        let items = v["result"]["projects"][0]["work_items"].as_array().unwrap();
        let item = |n: i64| items.iter().find(|w| w["gh_number"] == n).unwrap();
        assert_eq!(item(1)["title"], "Issue 1", "{v}");
        assert_eq!(item(1)["pending_decisions"], 2);
        assert_eq!(item(1)["id"], epic);
        assert_eq!(item(2)["pending_decisions"], 1);
        assert_eq!(item(1)["parent_number"], Value::Null);
        assert_eq!(item(2)["parent_number"], 1);
        assert_eq!(item(3)["title"], Value::Null);
        assert_eq!(item(3)["pending_decisions"], 1);
        let v = ui.call("review.get", json!({ "work_item_id": epic })).await;
        assert_eq!(v["result"]["decisions"].as_array().unwrap().len(), 2, "{v}");
        assert_eq!(v["result"]["prs"][0]["number"], 40);
        assert_eq!(v["result"]["work_item"]["id"], epic);
        // Uma decisão só; ids fora das pendentes do item são ignorados.
        let first = v["result"]["decisions"][0]["id"].as_i64().unwrap();
        let alheia = {
            let l = srv.env.sessions.ledger();
            l.list_decisions(&[outro], true).unwrap()[0].id
        };
        let v = ui
            .call(
                "review.mark_reviewed",
                json!({ "work_item_id": epic, "pr": 40, "decision_ids": [first, alheia] }),
            )
            .await;
        assert_eq!(v["result"]["marked"], 1, "{v}");
        let v = ui.call("review.get", json!({ "work_item_id": epic })).await;
        assert_eq!(v["result"]["decisions"].as_array().unwrap().len(), 1, "{v}");
        let v = ui
            .call(
                "review.mark_reviewed",
                json!({ "work_item_id": epic, "pr": 40, "decision_ids": "x" }),
            )
            .await;
        assert_eq!(v["error"]["code"], -32602, "{v}");
        let v = ui
            .call(
                "review.mark_reviewed",
                json!({ "work_item_id": epic, "pr": 40 }),
            )
            .await;
        assert_eq!(v["result"]["marked"], 1);
        let v = ui.call("review.get", json!({ "work_item_id": epic })).await;
        assert_eq!(v["result"]["decisions"], json!([]));
        // Decisões de outro work item ficam pendentes.
        let v = ui
            .call("review.get", json!({ "work_item_id": outro }))
            .await;
        assert_eq!(v["result"]["decisions"].as_array().unwrap().len(), 1);
        let v = ui
            .call("review.mark_reviewed", json!({ "work_item_id": epic }))
            .await;
        assert_eq!(v["error"]["code"], -32602);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn events_list_pagina_do_mais_novo() {
        use ct_ledger::NewEvent;
        let srv = Srv::linked(FakeClaude::exit(0));
        let pid = srv.env.project_id;
        let wi = {
            let l = srv.env.sessions.ledger();
            let wi = l.ensure_work_item(pid, 1, WorkItemKind::Issue).unwrap();
            // 120 eventos além do work_item_created: 121 no total.
            for n in 0..120 {
                l.append_event(NewEvent {
                    project_id: pid,
                    work_item_id: Some(wi.id),
                    session_id: None,
                    kind: "teste",
                    payload: json!({ "n": n }),
                })
                .unwrap();
            }
            wi.id
        };
        let url = spawn_server(&srv).await;
        let mut ui = Ui::new(&url).await;
        let ids = |v: &Value| -> Vec<i64> {
            v["result"]
                .as_array()
                .unwrap_or_else(|| panic!("{v}"))
                .iter()
                .map(|e| e["id"].as_i64().unwrap())
                .collect()
        };

        // Padrão: 20, do mais novo para o mais antigo.
        let v = ui.call("events.list", json!({ "work_item_id": wi })).await;
        let p1 = ids(&v);
        assert_eq!(p1.len(), 20, "{v}");
        assert!(p1.windows(2).all(|w| w[0] > w[1]), "{p1:?}");
        assert_eq!(v["result"][0]["payload"]["n"], 119);

        // `before` continua de onde a página parou, sem repetir.
        let v = ui
            .call(
                "events.list",
                json!({ "work_item_id": wi, "before": p1[19], "limit": 5 }),
            )
            .await;
        let p2 = ids(&v);
        assert_eq!(p2.len(), 5);
        assert!(p2[0] < p1[19]);
        assert_eq!(v["result"][0]["payload"]["n"], 99);

        // Acima do máximo vira 100.
        let v = ui
            .call("events.list", json!({ "work_item_id": wi, "limit": 500 }))
            .await;
        let all = ids(&v);
        assert_eq!(all.len(), 100);
        // A última página traz o resto, terminando no work_item_created.
        let v = ui
            .call(
                "events.list",
                json!({ "work_item_id": wi, "before": all[99], "limit": 100 }),
            )
            .await;
        assert_eq!(ids(&v).len(), 21);
        assert_eq!(v["result"][20]["kind"], "work_item_created");

        // Parâmetros inválidos e item inexistente.
        for p in [
            json!({ "work_item_id": wi, "limit": 0 }),
            json!({ "work_item_id": wi, "limit": "x" }),
            json!({ "work_item_id": wi, "before": "x" }),
            json!({}),
        ] {
            let v = ui.call("events.list", p).await;
            assert_eq!(v["error"]["code"], -32602, "{v}");
        }
        let v = ui
            .call("events.list", json!({ "work_item_id": 999_999 }))
            .await;
        assert_eq!(v["error"]["code"], -32000, "{v}");
        let msg = v["error"]["message"].as_str().unwrap();
        assert!(msg.contains("work_item não encontrado"), "{msg}");
    }
}
