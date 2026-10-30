//! Lançamento de sessões (spec §7.1): work item → branch → worktree →
//! sessão `starting` → arquivos da sessão → spawn em PTY.

use crate::paths::{private_dir, write_private};
use crate::{Result, Sessions, SessionsError};
use ct_core::branch::{branch_from_pattern, worktree_dir_name};
use ct_core::{
    ProjectKind, SessionMode, SessionState, Tool, WorkItemKind, WorkItemTrigger, CLAUDE_MODELS,
};
use ct_git::{ensure_worktree, WorktreeSpec};
use ct_ledger::{Ledger, NewEvent, NewSession, Project, Session, WorkItem};
use ct_pty::{PtySession, SpawnSpec};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub struct StartRequest {
    pub project_id: i64,
    pub gh_number: i64,
    pub kind: WorkItemKind,
    pub title: String,
    pub labels: Vec<String>,
    pub prompt: String,
    /// Tamanho inicial do PTY; `None` = [`PtySize::DEFAULT`].
    pub size: Option<PtySize>,
    pub agent: AgentOptions,
}

/// Modelo e modo de execução do agente, escolhidos ao iniciar a sessão.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentOptions {
    /// Alias de [`CLAUDE_MODELS`]; `None` = padrão do Claude Code.
    pub model: Option<String>,
    pub mode: SessionMode,
    /// Liga o Remote Control do Claude Code (`--remote-control "<nome>"`,
    /// issue #197). Recusado se o ambiente efetivo da sessão o impede.
    pub remote_control: bool,
}

impl AgentOptions {
    fn validate(&self) -> Result<()> {
        match &self.model {
            Some(m) if !CLAUDE_MODELS.contains(&m.as_str()) => {
                Err(SessionsError::Invalid(format!(
                    "modelo não suportado: {m} (aceitos: {})",
                    CLAUDE_MODELS.join(", ")
                )))
            }
            _ => Ok(()),
        }
    }

    /// Argumentos do `claude` para o modelo e o modo. `gestor` soma os
    /// limites fixos do gestor ao system prompt, `rules` as regras do
    /// agente do projeto e do épico (#357) e `docs` a linha da pasta de docs
    /// (#441; um só `--append-system-prompt`, com o do modo `subagents` por
    /// último, se houver).
    fn args(&self, gestor: bool, rules: Option<&str>, docs: Option<&str>) -> Vec<String> {
        let mut args = Vec::new();
        if let Some(m) = &self.model {
            args.extend(["--model".to_string(), m.clone()]);
        }
        let mut append = Vec::new();
        if gestor {
            append.push(GESTOR_LIMITS);
        }
        if let Some(r) = rules {
            append.push(r);
        }
        if let Some(d) = docs {
            append.push(d);
        }
        if self.mode == SessionMode::Subagents {
            append.push(SUBAGENTS_PROMPT);
        }
        if !append.is_empty() {
            args.extend(["--append-system-prompt".to_string(), append.join("\n\n")]);
        }
        args
    }
}

/// Instrução do modo `subagents`, somada ao system prompt do Claude Code.
const SUBAGENTS_PROMPT: &str = "Modo subagents (Control Tower): delegue cada tarefa do trabalho \
a um subagente (ferramenta Agent), com contexto suficiente para executá-la sozinho, em paralelo \
quando as tarefas forem independentes. Nesta sessão, planeje, coordene, revise o que os \
subagentes entregarem e integre o resultado.";

/// Limites fixos do gestor de projetos (spec do gestor §10.2). Vão no
/// system prompt de toda sessão do gestor, inclusive na retomada, e o
/// usuário não os edita.
pub const GESTOR_LIMITS: &str = "Gestor de projetos (Control Tower): limites fixos desta sessão.
1. Leia o hub só pelas ferramentas portfolio_*. Não leia arquivos de token nem de \
secrets (~/.config/ctower/*.token, ~/.config/ctower/env.key, secrets-cache.age, ~/.config/cthub/, \
snapshots secrets-*.age) nem chame o hub ou o ambiente por HTTP.
2. Prioridade e status mudam só por portfolio_propose, que cria uma proposta pendente; o usuário \
aprova na tela Hub. Nunca diga que uma mudança foi aplicada antes da aprovação; confira com \
portfolio_list_proposals.
3. Crie issue (gh issue create -R owner/repo; nos projetos Jira, jira issue create) só depois do \
ok explícito do usuário nesta conversa. Não edite, feche, reabra, comente nem mude o status de \
issues (no Jira, nada de jira issue move, edit ou comment); não crie, edite, revise nem mescle PRs \
ou MRs; não mexa em labels, milestones e repositórios.
4. Não faça commits nem push, não clone repositórios para trabalhar neles, não inicie sessões em \
ambientes.
5. Registre com ledger_record_decision as recomendações aceitas pelo usuário (orientada) e as suas \
escolhas relevantes (autonoma).
Esta sessão não tem repositório no cwd: use -R owner/repo em todo comando gh. Nos projetos Jira, \
leia a issue pelo comando issue_view do projeto em portfolio_overview, que já traz o comando de \
credencial (nunca a senha).";

/// Escopo escolhido ao abrir o gestor (spec do gestor §3). Só orienta o
/// prompt inicial: as ferramentas leem o hub inteiro.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum GestorScope {
    Portfolio,
    Project { owner: String, repo: String },
    Health,
}

/// Escopo de cada sessão do gestor em `sessions` (id da sessão → escopo),
/// pelo `tool_session_id` gravado no evento `gestor_started`. Sessões de
/// outros work items, ou cujo evento já foi podado, ficam de fora.
pub fn gestor_scopes(
    l: &Ledger,
    gestor_work_item_id: i64,
    sessions: &[Session],
) -> Result<HashMap<String, GestorScope>> {
    let mut by_tool: HashMap<String, GestorScope> = HashMap::new();
    for e in l.list_events_of_kind(gestor_work_item_id, "gestor_started")? {
        let (Some(t), Ok(scope)) = (
            e.payload["tool_session_id"].as_str(),
            serde_json::from_value::<GestorScope>(e.payload["scope"].clone()),
        ) else {
            continue;
        };
        // Do mais recente para o mais antigo: fica o primeiro visto.
        by_tool.entry(t.to_string()).or_insert(scope);
    }
    Ok(sessions
        .iter()
        .filter(|s| s.work_item_id == gestor_work_item_id)
        .filter_map(|s| {
            let scope = by_tool.get(s.tool_session_id.as_deref()?)?;
            Some((s.id.clone(), scope.clone()))
        })
        .collect())
}

/// Tamanho do PTY em colunas e linhas.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PtySize {
    pub cols: u16,
    pub rows: u16,
}

impl PtySize {
    /// Tamanho quando a UI não informa o da área do terminal.
    pub const DEFAULT: PtySize = PtySize {
        cols: 120,
        rows: 40,
    };
}

/// Como o agente é chamado, com o id da conversa na ferramenta: conversa
/// nova com id fixo, ou retomada.
pub(crate) enum LaunchMode<'a> {
    New(&'a str),
    Resume(&'a str),
}

/// O que muda entre iniciar e retomar: onde, como chamar o agente, prompt,
/// tamanho do PTY e modelo/modo.
pub(crate) struct LaunchSpec<'a> {
    pub cwd: &'a str,
    pub mode: LaunchMode<'a>,
    pub prompt: &'a str,
    pub size: PtySize,
    pub agent: &'a AgentOptions,
    /// Nome da sessão no Remote Control; `None` = sem `--remote-control`.
    pub remote_name: Option<String>,
    /// Título do pedido, para a pasta do épico ainda fora do espelho
    /// (#440); vazio na retomada (a pasta já existe e é achada por `<n>-*`).
    pub title: &'a str,
}

impl LaunchMode<'_> {
    fn tool_session_id(&self) -> &str {
        match self {
            LaunchMode::New(id) | LaunchMode::Resume(id) => id,
        }
    }
}

/// Eventos de hook instalados por sessão (`--settings`), com o argumento do
/// `ctower hook`.
const HOOKS: &[(&str, &str)] = &[
    ("SessionStart", "session-start"),
    ("UserPromptSubmit", "user-prompt-submit"),
    ("Notification", "notification"),
    ("Stop", "stop"),
    ("SessionEnd", "session-end"),
    ("PreToolUse", "pre-tool-use"),
    ("PermissionRequest", "permission-request"),
];

/// Prazo do hook `PermissionRequest` no Claude Code (s): acima da espera
/// do daemon pela resposta da UI (`ctower hook` desiste antes).
pub const PERMISSION_HOOK_TIMEOUT_SECS: u64 = 180;

/// Número do work item das sessões do projeto (sem issue: planejar, criar
/// issues e épicos). Não tem branch nem worktree; roda no clone principal.
pub const PROJECT_WORK_ITEM: i64 = 0;

impl Sessions {
    /// Sessão do projeto, sem issue: roda no clone principal (`repo_path`),
    /// sem criar branch nem worktree. O guarda-corpo das branches protegidas
    /// continua valendo.
    pub fn start_project(
        &self,
        project_id: i64,
        prompt: &str,
        size: Option<PtySize>,
        agent: &AgentOptions,
    ) -> Result<Session> {
        agent.validate()?;
        let project = self.ledger().get_project(project_id)?;
        if agent.remote_control {
            self.check_remote_control(&project)?;
        }
        let wi =
            self.ledger()
                .ensure_work_item(project_id, PROJECT_WORK_ITEM, WorkItemKind::Issue)?;
        if !Path::new(&project.repo_path).is_dir() {
            return Err(SessionsError::Invalid(format!(
                "clone do projeto não encontrado em {}",
                project.repo_path
            )));
        }
        let tool_session_id = uuid::Uuid::new_v4().to_string();
        let remote_name = agent
            .remote_control
            .then(|| self.remote_name(&project, &wi, None));
        self.launch(
            &project,
            &wi,
            LaunchSpec {
                cwd: &project.repo_path,
                mode: LaunchMode::New(&tool_session_id),
                prompt,
                size: size.unwrap_or(PtySize::DEFAULT),
                agent,
                remote_name,
                title: "",
            },
        )
    }

    /// Sessão do gestor de projetos (spec do gestor §2): projeto sentinela
    /// `_ctower/gestor`, work item `#0`, na pasta `gestor_dir` (criada com
    /// 0700 se faltar). Quem chama confere antes se o ambiente está ligado
    /// a um hub e tem token de agente.
    pub fn start_gestor(
        &self,
        scope: &GestorScope,
        prompt: &str,
        size: Option<PtySize>,
        agent: &AgentOptions,
    ) -> Result<Session> {
        agent.validate()?;
        let dir = self.config().gestor_dir.clone();
        private_dir(&dir)?;
        let dir_str = dir.to_string_lossy().to_string();
        // O id da conversa vai no evento: liga o escopo à sessão, inclusive
        // depois da retomada (que reusa o id), para a UI identificar cada
        // sessão do gestor viva (`gestor_scopes`).
        let tool_session_id = uuid::Uuid::new_v4().to_string();
        let (project, wi) = self.ledger().ensure_gestor_project(&dir_str)?;
        if agent.remote_control {
            self.check_remote_control(&project)?;
        }
        {
            let l = self.ledger();
            l.append_event(NewEvent {
                project_id: project.id,
                work_item_id: Some(wi.id),
                session_id: None,
                kind: "gestor_started",
                payload: json!({ "scope": scope, "tool_session_id": tool_session_id }),
            })?;
        }
        let remote_name = agent
            .remote_control
            .then(|| self.remote_name(&project, &wi, None));
        self.launch(
            &project,
            &wi,
            LaunchSpec {
                cwd: &dir_str,
                mode: LaunchMode::New(&tool_session_id),
                prompt,
                size: size.unwrap_or(PtySize::DEFAULT),
                agent,
                remote_name,
                title: "",
            },
        )
    }

    /// Inicia uma sessão nova para o work item, criando worktree e branch se preciso.
    pub fn start(&self, req: StartRequest) -> Result<Session> {
        if req.gh_number <= PROJECT_WORK_ITEM {
            return Err(SessionsError::Invalid(format!(
                "número de issue inválido: {} (sessão sem issue: start_project)",
                req.gh_number
            )));
        }
        req.agent.validate()?;
        let project = self.ledger().get_project(req.project_id)?;
        // Antes de criar work item, branch e worktree: a recusa não deixa rastro.
        if req.agent.remote_control {
            self.check_remote_control(&project)?;
        }
        let wi = self
            .ledger()
            .ensure_work_item(req.project_id, req.gh_number, req.kind)?;
        // A branch registrada no work item vale para sempre: mudar o padrão
        // do projeto não renomeia branch existente (spec do Jira e GitLab §4).
        let branch = match wi.branch.clone() {
            Some(b) => b,
            None => branch_from_pattern(
                project.effective_branch_pattern(),
                wi.kind,
                req.gh_number,
                &ct_core::item_key(&project.tracker_ref(), req.gh_number),
                &req.title,
                &req.labels,
            )
            .map_err(|e| SessionsError::Invalid(e.to_string()))?,
        };
        let worktree = wi
            .worktree_path
            .clone()
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                self.config()
                    .worktrees_root
                    .join(&project.name)
                    .join(worktree_dir_name(&branch))
            });

        // Gravar antes de agir: o pedido de worktree fica no ledger mesmo se o git falhar.
        self.ledger().append_event(NewEvent {
            project_id: project.id,
            work_item_id: Some(wi.id),
            session_id: None,
            kind: "worktree_requested",
            payload: json!({ "branch": branch, "worktree_path": worktree }),
        })?;
        if let Some(parent) = worktree.parent() {
            std::fs::create_dir_all(parent)?;
        }
        ensure_worktree(&WorktreeSpec {
            repo_path: Path::new(&project.repo_path),
            branch: &branch,
            base: &project.base_branch,
            worktree_path: &worktree,
        })?;
        let worktree_str = worktree.to_string_lossy().to_string();
        if wi.branch.as_deref() != Some(&branch)
            || wi.worktree_path.as_deref() != Some(&worktree_str)
        {
            self.ledger()
                .set_work_item_branch(wi.id, &branch, &worktree_str)?;
        }
        let tool_session_id = uuid::Uuid::new_v4().to_string();
        let remote_name = req
            .agent
            .remote_control
            .then(|| self.remote_name(&project, &wi, Some(&req.title)));
        self.launch(
            &project,
            &wi,
            LaunchSpec {
                cwd: &worktree_str,
                mode: LaunchMode::New(&tool_session_id),
                prompt: &req.prompt,
                size: req.size.unwrap_or(PtySize::DEFAULT),
                agent: &req.agent,
                remote_name,
                title: &req.title,
            },
        )
    }

    /// Cria a sessão no ledger e faz o spawn. Comum a iniciar e retomar.
    pub(crate) fn launch(
        &self,
        project: &Project,
        wi: &WorkItem,
        spec: LaunchSpec,
    ) -> Result<Session> {
        let LaunchSpec {
            cwd,
            mode,
            prompt,
            size,
            agent,
            remote_name,
            title,
        } = spec;
        let cfg = self.config().clone();
        let gestor = project.kind == ProjectKind::Gestor;
        // Pastas de docs do projeto e do épico (o próprio ou o pai da
        // sub-issue; #440, #441), recalculadas a cada início e retomada. O
        // gestor não tem docs; a sessão de planejamento (#0) só a do projeto.
        let docs = if gestor {
            crate::docs::SessionDocs::default()
        } else {
            let item = (wi.gh_number != PROJECT_WORK_ITEM).then_some(wi);
            self.ensure_docs(project, item, title)
        };
        let session = self.ledger().create_session(&NewSession {
            work_item_id: wi.id,
            tool: Tool::Claude,
            cwd: cwd.to_string(),
            launch_cmd: vec![cfg.claude_bin.clone()],
            model: agent.model.clone(),
            mode: agent.mode,
        })?;
        let dir = cfg.dirs.session_dir(&session.id);
        private_dir(&dir)?;
        let mcp_path = dir.join("mcp.json");
        let settings_path = dir.join("settings.json");
        write_private(
            &mcp_path,
            mcp_config(cfg.port, &cfg.hook_token, &session.id)
                .to_string()
                .as_bytes(),
        )?;
        write_private(
            &settings_path,
            session_settings(&cfg.ctower_bin, project.attribution)
                .to_string()
                .as_bytes(),
        )?;
        if let Some(claude_json) = &cfg.claude_json {
            self.pre_trust(claude_json, project, wi, &session.id, cwd)?;
        }

        let tool_session_id = mode.tool_session_id();
        let mut args = match mode {
            LaunchMode::New(id) => vec!["--session-id".to_string(), id.to_string()],
            LaunchMode::Resume(id) => vec!["--resume".to_string(), id.to_string()],
        };
        // Antes de outra opção: `--add-dir` aceita vários valores e
        // engoliria o prompt posicional.
        if let Some(dir) = &docs.project {
            args.extend(["--add-dir".to_string(), dir.to_string_lossy().to_string()]);
        }
        args.extend([
            "--mcp-config".to_string(),
            mcp_path.to_string_lossy().to_string(),
            "--settings".to_string(),
            settings_path.to_string_lossy().to_string(),
        ]);
        if let Some(name) = &remote_name {
            args.extend(["--remote-control".to_string(), name.clone()]);
        }
        // Regras do agente vigentes no cache do pull, relidas a cada início
        // e retomada (spec do Jira e GitLab §10.1).
        let rules = {
            let l = self.ledger();
            l.session_rules(project, wi)?.system_prompt(project)
        };
        let docs_line = docs.instruction();
        args.extend(agent.args(gestor, rules.as_deref(), docs_line.as_deref()));
        if !prompt.is_empty() {
            args.push(prompt.to_string());
        }
        let mut argv = vec![cfg.claude_bin.clone()];
        argv.extend(args.iter().cloned());
        self.ledger()
            .set_session_launch(&session.id, &argv, Some(tool_session_id))?;

        // Secrets e variáveis do hub primeiro (só no ambiente do processo,
        // nunca no `launch_cmd` nem no ledger); as `CTOWER_*` vêm depois e
        // sempre vencem.
        let secrets = self.session_secrets(project, &session.id);
        let pty_mask = secrets.mask.pty.clone();
        self.set_mask(&session.id, secrets.mask);
        let mut env = secrets.env;
        env.extend([
            ("CTOWER_SESSION".to_string(), session.id.clone()),
            (
                "CTOWER_URL".to_string(),
                format!("http://127.0.0.1:{}", cfg.port),
            ),
            ("CTOWER_TOKEN".to_string(), cfg.hook_token.clone()),
            (
                "CTOWER_PROTECTED".to_string(),
                project.protected_branches.join(","),
            ),
            (
                "CTOWER_SPOOL_DIR".to_string(),
                cfg.dirs.spool().to_string_lossy().to_string(),
            ),
        ]);
        if gestor {
            // Perfil `gestor` do guarda-corpo no `ctower hook` (§10.2).
            env.push(("CTOWER_SESSION_KIND".to_string(), "gestor".to_string()));
        }
        env.extend(provider_env(project, wi, gestor));
        env.extend(docs.env());
        if docs.unreachable {
            self.docs_unreachable(project, wi, &session.id)?;
        }
        let spawned = PtySession::spawn(
            SpawnSpec {
                program: cfg.claude_bin.clone(),
                args,
                cwd: PathBuf::from(cwd),
                env,
                env_remove: Vec::new(),
                cols: size.cols,
                rows: size.rows,
                mask: pty_mask,
            },
            cfg.scrollback,
        );
        let pty = match spawned {
            Ok(p) => p,
            Err(e) => {
                self.forget_mask(&session.id);
                let l = self.ledger();
                l.transition_session(&session.id, SessionState::Exited, None)?;
                l.append_event(NewEvent {
                    project_id: project.id,
                    work_item_id: Some(wi.id),
                    session_id: Some(&session.id),
                    kind: "session_spawn_failed",
                    payload: json!({ "error": e.to_string() }),
                })?;
                return Err(e.into());
            }
        };
        {
            let l = self.ledger();
            if let Some(pid) = pty.pid() {
                l.set_session_pid(&session.id, i64::from(pid))?;
            }
            l.apply_trigger(wi.id, WorkItemTrigger::SessionActive)?;
            if let Some(name) = &remote_name {
                l.append_event(NewEvent {
                    project_id: project.id,
                    work_item_id: Some(wi.id),
                    session_id: Some(&session.id),
                    kind: "remote_control_requested",
                    payload: json!({ "name": name, "via": "flag" }),
                })?;
            }
        }
        self.inner
            .live
            .lock()
            .unwrap()
            .insert(session.id.clone(), pty.clone());
        if remote_name.is_some() {
            // Confirmação inicial e link na saída (§6.4, §6.6).
            self.watch_remote_control(&session.id, &pty, true);
        }

        let this = self.clone();
        let sid = session.id.clone();
        let (project_id, wi_id) = (project.id, wi.id);
        pty.on_exit(move |code| this.on_process_exit(project_id, wi_id, &sid, code));

        let session = self.ledger().get_session(&session.id)?;
        self.notify("session_started", json!(session));
        Ok(session)
    }

    /// Sessão de projeto com docs que subiu sem a pasta (`docs_root`
    /// inacessível, #441): alerta `docs_unreachable` para a UI e evento no
    /// log do work item. O aviso no log do daemon sai do `ensure_docs`.
    fn docs_unreachable(&self, project: &Project, wi: &WorkItem, session_id: &str) -> Result<()> {
        let root = self
            .config()
            .docs_root
            .as_deref()
            .map(|r| r.display().to_string())
            .unwrap_or_default();
        let error =
            format!("docs_root {root} inacessível: a sessão subiu sem CT_DOCS_DIR e sem --add-dir");
        self.notify(
            "alert",
            json!({ "kind": crate::docs::DOCS_UNREACHABLE, "session_id": session_id,
                    "error": error, "root": root }),
        );
        self.ledger().append_event(NewEvent {
            project_id: project.id,
            work_item_id: Some(wi.id),
            session_id: Some(session_id),
            kind: crate::docs::DOCS_UNREACHABLE,
            payload: json!({ "root": root }),
        })?;
        Ok(())
    }

    /// Pré-aceita a confiança na raiz do repositório e no worktree. Falha
    /// vira alerta (a sessão segue; o humano responde ao diálogo).
    fn pre_trust(
        &self,
        claude_json: &Path,
        project: &Project,
        wi: &WorkItem,
        session_id: &str,
        cwd: &str,
    ) -> Result<()> {
        let (kind, payload) = match crate::trust::ensure_trusted(
            claude_json,
            &[project.repo_path.as_str(), cwd],
        ) {
            Ok(changed) if changed.is_empty() => return Ok(()),
            Ok(changed) => ("trust_granted", json!({ "paths": changed })),
            Err(e) => {
                self.notify(
                    "alert",
                    json!({ "kind": "trust_failed", "session_id": session_id, "error": e.to_string() }),
                );
                ("trust_failed", json!({ "error": e.to_string() }))
            }
        };
        self.ledger().append_event(NewEvent {
            project_id: project.id,
            work_item_id: Some(wi.id),
            session_id: Some(session_id),
            kind,
            payload,
        })?;
        Ok(())
    }

    /// Saída limpa (código 0 ou `kill` pedido) vira `exited`; qualquer outra
    /// (código ≠ 0 ou sinal) sem `SessionEnd` antes vira `interrupted` e entra
    /// na recuperação (#283).
    fn on_process_exit(&self, project_id: i64, wi_id: i64, sid: &str, code: Option<i32>) {
        self.inner.live.lock().unwrap().remove(sid);
        let killed = self.inner.killed.lock().unwrap().remove(sid);
        let clean = code == Some(0) || killed;
        let to = if clean {
            SessionState::Exited
        } else {
            SessionState::Interrupted
        };
        let state = {
            let l = self.ledger();
            // Pode já estar `exited` (hook SessionEnd): nada muda. Outros
            // estados finais não aceitam a transição; nada a fazer.
            let _ = l.transition_session(sid, to, code);
            if !clean {
                let _ = l.append_event(NewEvent {
                    project_id,
                    work_item_id: Some(wi_id),
                    session_id: Some(sid),
                    kind: "session_exit_error",
                    payload: json!({ "exit_code": code }),
                });
            }
            l.get_session(sid).map(|s| s.state).unwrap_or(to)
        };
        self.notify(
            "session_exited",
            json!({ "session_id": sid, "exit_code": code, "state": state }),
        );
        if state == SessionState::Interrupted {
            self.notify("sessions_interrupted", json!({ "session_ids": [sid] }));
        }
    }
}

/// `mcp.json` da sessão: servidor HTTP do ledger com os cabeçalhos de autenticação.
pub(crate) fn mcp_config(port: u16, hook_token: &str, session_id: &str) -> Value {
    json!({
        "mcpServers": {
            "ctower": {
                "type": "http",
                "url": format!("http://127.0.0.1:{port}/mcp"),
                "headers": {
                    "Authorization": format!("Bearer {hook_token}"),
                    "X-Ctower-Session": session_id,
                }
            }
        }
    })
}

/// Variáveis da sessão que dependem do rastreador e da forja (spec do Jira e
/// GitLab §7.5). Nenhuma é segredo: o token nunca entra no ambiente da
/// sessão, e o agente roda o comando de credencial dentro do próprio
/// comando (§8.2).
///
/// - GitHub: `CTOWER_ISSUE` na sessão de issue, para o `ctower hook` exigir
///   `Closes #n` no `gh pr create` (#375).
/// - Jira: `CTOWER_TRACKER=jira` (perfil Jira do hook) e, na sessão de issue
///   ou de épico, `CTOWER_ISSUE_KEY` (`PROJ-5450`); `JIRA_CONFIG_FILE` se
///   configurado. Sem `CTOWER_ISSUE`: o `Closes #n` é do GitHub.
/// - GitLab: `GITLAB_HOST`.
fn provider_env(project: &Project, wi: &WorkItem, gestor: bool) -> Vec<(String, String)> {
    let mut env = Vec::new();
    if gestor {
        return env;
    }
    let item = wi.gh_number != PROJECT_WORK_ITEM;
    match &project.tracker {
        ct_core::TrackerConfig::Github => {
            if item && wi.kind == WorkItemKind::Issue {
                env.push(("CTOWER_ISSUE".to_string(), wi.gh_number.to_string()));
            }
        }
        ct_core::TrackerConfig::Jira(j) => {
            env.push(("CTOWER_TRACKER".to_string(), "jira".to_string()));
            if item {
                env.push((
                    "CTOWER_ISSUE_KEY".to_string(),
                    ct_core::item_key(&project.tracker_ref(), wi.gh_number),
                ));
            }
            if !j.config_file.trim().is_empty() {
                env.push((
                    "JIRA_CONFIG_FILE".to_string(),
                    j.config_file.trim().to_string(),
                ));
            }
        }
    }
    if let Some(g) = project.forge.gitlab() {
        if !g.host.trim().is_empty() {
            env.push(("GITLAB_HOST".to_string(), g.host.trim().to_string()));
        }
    }
    env
}

/// `settings.json` da sessão: os hooks e, com a atribuição desligada no
/// projeto (spec do Jira e GitLab §9.3), as chaves do Claude Code que tiram o
/// `Co-Authored-By` dos commits e o rodapé dos PRs/MRs. `attribution` com
/// texto vazio esconde a atribuição (Claude Code 2.1); `includeCoAuthoredBy`
/// é a chave antiga, ainda lida, para versões anteriores.
pub(crate) fn session_settings(ctower_bin: &str, attribution: bool) -> Value {
    let mut s = hook_settings(ctower_bin);
    if !attribution {
        s["attribution"] = json!({ "commit": "", "pr": "" });
        s["includeCoAuthoredBy"] = json!(false);
    }
    s
}

/// `settings.json` da sessão com os hooks do Control Tower (passado em `--settings`).
pub(crate) fn hook_settings(ctower_bin: &str) -> Value {
    let mut hooks = serde_json::Map::new();
    for (event, arg) in HOOKS {
        let mut entry = json!({
            "hooks": [{ "type": "command", "command": format!("{ctower_bin} hook {arg}") }]
        });
        if *event == "PreToolUse" {
            entry["matcher"] = json!("Bash");
        }
        if *event == "PermissionRequest" {
            entry["hooks"][0]["timeout"] = json!(PERMISSION_HOOK_TIMEOUT_SECS);
        }
        hooks.insert((*event).to_string(), json!([entry]));
    }
    json!({ "hooks": hooks })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::*;
    use ct_core::{SessionState, WorkItemStatus};
    use std::os::unix::fs::PermissionsExt;
    use std::time::Duration;

    #[test]
    fn settings_tem_os_sete_hooks_e_matcher_bash() {
        let s = hook_settings("/usr/local/bin/ctower");
        let h = s["hooks"].as_object().unwrap();
        assert_eq!(h.len(), 7);
        assert_eq!(
            h["PermissionRequest"][0]["hooks"][0]["timeout"],
            PERMISSION_HOOK_TIMEOUT_SECS
        );
        assert!(h["PermissionRequest"][0].get("matcher").is_none());
        assert_eq!(
            h["Stop"][0]["hooks"][0]["command"],
            "/usr/local/bin/ctower hook stop"
        );
        assert_eq!(h["PreToolUse"][0]["matcher"], "Bash");
        assert!(h["Stop"][0].get("matcher").is_none());
    }

    /// Projeto Jira (#347): a branch sai do padrão do projeto, com a chave
    /// derivada; sem padrão, o do Jira. A branch registrada continua valendo
    /// depois de o padrão mudar.
    #[test]
    fn branch_pelo_padrao_do_projeto_jira() {
        let env = TestEnv::new(FakeClaude::exit(0));
        let pull = |pattern: Option<&str>, rev: i64| {
            let l = env.sessions.ledger();
            let p = l.get_project(env.project_id).unwrap();
            let cfg = ct_ledger::ConfigResponse {
                config_rev: rev,
                config_epoch: 1,
                changed: true,
                projects: Some(vec![ct_ledger::ConfigProject {
                    docs: Default::default(),
                    owner: p.gh_owner.clone(),
                    repo: p.gh_repo.clone(),
                    name: p.name.clone(),
                    base_branch: p.base_branch.clone(),
                    protected_branches: p.protected_branches.clone(),
                    repo_path: p.repo_path.clone(),
                    rev,
                    tracker: ct_core::TrackerConfig::Jira(ct_core::JiraConfig {
                        project_key: "PROJ".into(),
                        ..Default::default()
                    }),
                    forge: ct_core::ForgeConfig::Github,
                    branch_pattern: pattern.map(String::from),
                    attribution: true,
                    sync: false,
                    rules: Default::default(),
                }]),
                removed: None,
            };
            l.apply_hub_config(&cfg, |c| c.repo_path.clone()).unwrap();
        };
        pull(None, 1);
        let s = env.sessions.start(env.request(12, "Autenticação")).unwrap();
        let wi = env.sessions.ledger().get_work_item(s.work_item_id).unwrap();
        assert_eq!(wi.branch.as_deref(), Some("feat/PROJ-12-autenticacao"));

        pull(Some("{key}"), 2);
        let s2 = env.sessions.start(env.request(13, "Outra")).unwrap();
        let wi2 = env
            .sessions
            .ledger()
            .get_work_item(s2.work_item_id)
            .unwrap();
        assert_eq!(wi2.branch.as_deref(), Some("PROJ-13"));
        assert!(wi2
            .worktree_path
            .as_deref()
            .unwrap()
            .ends_with("demo/PROJ-13"));
        let wi = env.sessions.ledger().get_work_item(wi.id).unwrap();
        assert_eq!(wi.branch.as_deref(), Some("feat/PROJ-12-autenticacao"));
    }

    #[test]
    fn settings_sem_atribuicao_desliga_co_author_e_rodape() {
        let com = session_settings("/bin/ctower", true);
        assert_eq!(com, hook_settings("/bin/ctower"));
        assert!(com.get("attribution").is_none());
        assert!(com.get("includeCoAuthoredBy").is_none());
        let sem = session_settings("/bin/ctower", false);
        assert_eq!(sem["attribution"], json!({ "commit": "", "pr": "" }));
        assert_eq!(sem["includeCoAuthoredBy"], false);
        assert_eq!(sem["hooks"], com["hooks"]);
    }

    /// Projeto Jira + GitLab do exemplo (#351): o comando de credencial que daria
    /// o token. Imprime `token-do-jira`/`token-do-gitlab` se alguém o rodar,
    /// texto que não aparece no próprio comando.
    const JIRA_CMD: &str = "printf 'token-do-%s' jira";
    const GITLAB_CMD: &str = "printf 'token-do-%s' gitlab";

    fn assign_jira_gitlab(env: &TestEnv, attribution: bool) {
        env.assign_config(1, |p| {
            p.owner = "grupo/projeto".into();
            p.repo = "app".into();
            p.name = "app".into();
            p.tracker = ct_core::TrackerConfig::Jira(ct_core::JiraConfig {
                url: "https://jira.example.com".into(),
                project_key: "PROJ".into(),
                credential_cmd: JIRA_CMD.into(),
                config_file: "/home/u/.config/.jira/jira.yml".into(),
                ..Default::default()
            });
            p.forge = ct_core::ForgeConfig::Gitlab(ct_core::GitlabConfig {
                host: "gitlab.example.com".into(),
                path: "grupo/projeto/app".into(),
                clone_url: "https://gitlab.example.com/grupo/projeto/app.git".into(),
                credential_cmd: GITLAB_CMD.into(),
            });
            p.branch_pattern = Some("{key}".into());
            p.attribution = attribution;
        });
    }

    fn jira_gitlab_project(env: &TestEnv) -> Project {
        let l = env.sessions.ledger();
        l.list_projects()
            .unwrap()
            .into_iter()
            .find(|p| p.gh_repo == "app")
            .expect("projeto do exemplo no cache")
    }

    /// O token não aparece em nada que a sessão grava: saída do agente falso
    /// (argv e ambiente), `launch_cmd`, arquivos da sessão e eventos.
    fn sem_token(env: &TestEnv, s: &Session, out: &str) {
        let dir = env.cfg.dirs.session_dir(&s.id);
        let mut textos = vec![out.to_string(), s.launch_cmd.join("\n")];
        for f in ["mcp.json", "settings.json"] {
            textos.push(std::fs::read_to_string(dir.join(f)).unwrap());
        }
        let p = jira_gitlab_project(env);
        let events = env.sessions.ledger().list_events(p.id, 1000).unwrap();
        textos.push(serde_json::to_string(&events).unwrap());
        for t in &textos {
            assert!(!t.contains("token-do-jira"), "{t}");
            assert!(!t.contains("token-do-gitlab"), "{t}");
        }
        // As variáveis do token nunca vêm do daemon (só do ambiente de quem
        // roda o teste, se houver); no prompt, só o comando de credencial.
        for v in ["JIRA_API_TOKEN", "GITLAB_TOKEN"] {
            if std::env::var_os(v).is_none() {
                let prefix = format!("{v}=");
                assert!(!out.lines().any(|l| l.starts_with(&prefix)), "{v}: {out}");
            }
        }
    }

    /// Critério de aceite da #351 com o agente falso: o prompt renderizado
    /// do template Jira chega inteiro no argv (com o comando de credencial,
    /// sem a senha), a branch sai do padrão do projeto, o spawn tem
    /// `CTOWER_TRACKER`, `CTOWER_ISSUE_KEY`, `GITLAB_HOST` e
    /// `JIRA_CONFIG_FILE`, sem `CTOWER_ISSUE`, e a atribuição desligada vai
    /// ao settings.json.
    #[test]
    fn sessao_de_issue_jira_gitlab_com_prompt_variaveis_e_sem_token() {
        let env = TestEnv::new(FakeClaude::exit(0));
        assign_jira_gitlab(&env, false);
        let p = jira_gitlab_project(&env);
        let jira = ct_core::jira_cli_prefix(p.tracker.jira().unwrap());
        let glab = ct_core::glab_cli_prefix(p.forge.gitlab().unwrap());
        // O que a UI manda depois de renderizar o template `jira_issue`.
        let prompt = format!(
            "Trabalhe na issue PROJ-5450 do Jira.\n\
             Leia a issue com `{jira} issue view PROJ-5450 --comments 20 --plain`.\n\
             Abra o MR com `{glab} mr create -R grupo/projeto/app -s PROJ-5450 -b develop -y`."
        );
        let s = env
            .sessions
            .start(StartRequest {
                project_id: p.id,
                prompt: prompt.clone(),
                ..env.request(5450, "Portar o plugin")
            })
            .unwrap();
        let out = env.wait_fake_output(&s.id);

        let wi = env.sessions.ledger().get_work_item(s.work_item_id).unwrap();
        assert_eq!(wi.branch.as_deref(), Some("PROJ-5450"));
        let wt = wi.worktree_path.clone().unwrap();
        assert!(wt.ends_with("app/PROJ-5450"), "{wt}");

        assert_eq!(s.launch_cmd.last(), Some(&prompt));
        assert!(out.contains(&prompt), "{out}");
        assert!(prompt.contains(&format!("JIRA_API_TOKEN=\"$({JIRA_CMD})\" jira issue view")));
        assert!(prompt.contains(&format!("GITLAB_TOKEN=\"$({GITLAB_CMD})\" glab mr create")));

        assert!(out.contains("CTOWER_TRACKER=jira\n"), "{out}");
        assert!(out.contains("CTOWER_ISSUE_KEY=PROJ-5450\n"), "{out}");
        assert!(out.contains("GITLAB_HOST=gitlab.example.com\n"), "{out}");
        assert!(
            out.contains("JIRA_CONFIG_FILE=/home/u/.config/.jira/jira.yml\n"),
            "{out}"
        );
        assert!(!out.contains("CTOWER_ISSUE="), "{out}");
        sem_token(&env, &s, &out);

        let dir = env.cfg.dirs.session_dir(&s.id);
        let settings: Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("settings.json")).unwrap())
                .unwrap();
        assert_eq!(settings["attribution"], json!({ "commit": "", "pr": "" }));
        assert_eq!(settings["includeCoAuthoredBy"], false);
        assert_eq!(settings["hooks"], hook_settings("ctower")["hooks"]);
    }

    /// Épico e sessão do projeto num projeto Jira: o épico também tem a
    /// chave; a do projeto só o perfil Jira. Com a atribuição ligada, o
    /// settings.json fica só com os hooks.
    #[test]
    fn epico_e_planejamento_jira_tem_o_perfil_jira() {
        let env = TestEnv::new(FakeClaude::exit(0));
        assign_jira_gitlab(&env, true);
        let p = jira_gitlab_project(&env);
        let s = env
            .sessions
            .start(StartRequest {
                project_id: p.id,
                kind: WorkItemKind::Epic,
                ..env.request(5014, "Plugin")
            })
            .unwrap();
        let out = env.wait_fake_output(&s.id);
        assert!(out.contains("CTOWER_TRACKER=jira\n"), "{out}");
        assert!(out.contains("CTOWER_ISSUE_KEY=PROJ-5014\n"), "{out}");
        assert!(!out.contains("CTOWER_ISSUE="), "{out}");
        let wi = env.sessions.ledger().get_work_item(s.work_item_id).unwrap();
        assert_eq!(wi.branch.as_deref(), Some("PROJ-5014"));
        let settings: Value = serde_json::from_str(
            &std::fs::read_to_string(env.cfg.dirs.session_dir(&s.id).join("settings.json"))
                .unwrap(),
        )
        .unwrap();
        assert_eq!(settings, hook_settings("ctower"));
        sem_token(&env, &s, &out);

        let s = env
            .sessions
            .start_project(p.id, "planejar", None, &AgentOptions::default())
            .unwrap();
        let out = env.wait_fake_output(&s.id);
        assert!(out.contains("CTOWER_TRACKER=jira\n"), "{out}");
        assert!(out.contains("GITLAB_HOST=gitlab.example.com\n"), "{out}");
        assert!(!out.contains("CTOWER_ISSUE"), "{out}");
        sem_token(&env, &s, &out);
    }

    #[test]
    fn inicia_sessao_em_worktree_com_argv_env_e_arquivos() {
        let env = TestEnv::new(FakeClaude::exit(0));
        let s = env.sessions.start(env.request(12, "Autenticação")).unwrap();
        let out = env.wait_fake_output(&s.id);

        let wi = env.sessions.ledger().get_work_item(s.work_item_id).unwrap();
        assert_eq!(wi.branch.as_deref(), Some("feat/12-autenticacao"));
        let wt = wi.worktree_path.clone().unwrap();
        assert!(wt.ends_with("demo/feat__12-autenticacao"), "{wt}");
        assert_eq!(s.cwd, wt);
        // `pwd` devolve o caminho real (no macOS, /var → /private/var).
        let real = std::fs::canonicalize(&wt).unwrap();
        assert!(out.contains(&format!("PWD={}", real.display())), "{out}");

        let tool_id = s.tool_session_id.clone().unwrap();
        let dir = env.cfg.dirs.session_dir(&s.id);
        let expected_args = [
            "--session-id".to_string(),
            tool_id,
            "--mcp-config".into(),
            dir.join("mcp.json").to_string_lossy().into(),
            "--settings".into(),
            dir.join("settings.json").to_string_lossy().into(),
            "prompt inicial".into(),
        ];
        assert!(out.contains(&expected_args.join("\n")), "{out}");
        assert_eq!(s.launch_cmd[1..], expected_args);
        assert!(out.contains(&format!("CTOWER_SESSION={}", s.id)));
        assert!(out.contains("CTOWER_URL=http://127.0.0.1:7878"));
        assert!(out.contains("CTOWER_TOKEN=hook-secreto"));
        assert!(out.contains("CTOWER_PROTECTED=develop,main"));
        assert!(out.contains("CTOWER_ISSUE=12\n"), "{out}");
        // Projeto GitHub: nada das variáveis do Jira e do GitLab (#351), e o
        // settings.json só com os hooks.
        for v in ["CTOWER_TRACKER", "CTOWER_ISSUE_KEY", "GITLAB_HOST="] {
            assert!(!out.contains(v), "{v}: {out}");
        }
        let settings: Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("settings.json")).unwrap())
                .unwrap();
        assert_eq!(settings, hook_settings("ctower"));

        for f in ["mcp.json", "settings.json"] {
            let meta = std::fs::metadata(dir.join(f)).unwrap();
            assert_eq!(meta.permissions().mode() & 0o777, 0o600, "{f}");
        }
        let mcp: Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("mcp.json")).unwrap()).unwrap();
        assert_eq!(
            mcp["mcpServers"]["ctower"]["headers"]["X-Ctower-Session"],
            s.id.as_str()
        );
        assert!(s.pid.is_some());
        assert_eq!(wi.status, WorkItemStatus::EmExecucao);
    }

    #[test]
    fn sessao_do_projeto_roda_no_clone_sem_worktree() {
        let env = TestEnv::new(FakeClaude::exit(0));
        let s = env
            .sessions
            .start_project(env.project_id, "planejar", None, &AgentOptions::default())
            .unwrap();
        let out = env.wait_fake_output(&s.id);
        let repo = env.repo.to_string_lossy().to_string();
        assert_eq!(s.cwd, repo);
        let real = std::fs::canonicalize(&env.repo).unwrap();
        assert!(out.contains(&format!("PWD={}", real.display())), "{out}");
        assert!(out.contains("planejar"));
        assert!(out.contains("CTOWER_PROTECTED=develop,main"));
        assert!(!out.contains("CTOWER_ISSUE"), "{out}");
        let wi = env.sessions.ledger().get_work_item(s.work_item_id).unwrap();
        assert_eq!(wi.gh_number, PROJECT_WORK_ITEM);
        assert_eq!((wi.branch, wi.worktree_path), (None, None));
        assert!(
            !env.cfg.worktrees_root.join("demo").exists(),
            "nenhum worktree"
        );
        // Uma segunda sessão reaproveita o mesmo work item #0.
        let s2 = env
            .sessions
            .start_project(env.project_id, "de novo", None, &AgentOptions::default())
            .unwrap();
        assert_eq!(s2.work_item_id, s.work_item_id);
        // O caminho normal recusa o #0.
        assert!(matches!(
            env.sessions.start(env.request(0, "x")),
            Err(SessionsError::Invalid(_))
        ));
    }

    #[test]
    fn pty_nasce_em_120x40_sem_tamanho() {
        let env = TestEnv::new(FakeClaude::exit(0));
        let s = env.sessions.start(env.request(14, "Padrão")).unwrap();
        assert!(env.wait_fake_output(&s.id).contains("SIZE=40 120\n"));
        let p = env
            .sessions
            .start_project(env.project_id, "planejar", None, &AgentOptions::default())
            .unwrap();
        assert!(env.wait_fake_output(&p.id).contains("SIZE=40 120\n"));
    }

    #[test]
    fn pty_nasce_no_tamanho_pedido() {
        let env = TestEnv::new(FakeClaude::exit(0));
        let mut req = env.request(15, "Tamanho");
        req.size = Some(PtySize { cols: 97, rows: 31 });
        let s = env.sessions.start(req).unwrap();
        let out = env.wait_fake_output(&s.id);
        assert!(out.contains("SIZE=31 97\n"), "{out}");
        let p = env
            .sessions
            .start_project(
                env.project_id,
                "planejar",
                Some(PtySize {
                    cols: 200,
                    rows: 50,
                }),
                &AgentOptions::default(),
            )
            .unwrap();
        let out = env.wait_fake_output(&p.id);
        assert!(out.contains("SIZE=50 200\n"), "{out}");
    }

    #[test]
    fn modelo_e_modo_vao_no_argv_e_ficam_na_sessao() {
        let env = TestEnv::new(FakeClaude::exit(0));
        let mut req = env.request(16, "Modelo");
        req.agent = AgentOptions {
            model: Some("opus".into()),
            mode: SessionMode::Subagents,
            ..AgentOptions::default()
        };
        let s = env.sessions.start(req).unwrap();
        let out = env.wait_fake_output(&s.id);
        let tail = [
            "--model".to_string(),
            "opus".into(),
            "--append-system-prompt".into(),
            SUBAGENTS_PROMPT.into(),
            "prompt inicial".into(),
        ];
        assert!(out.contains(&tail.join("\n")), "{out}");
        assert!(s.launch_cmd.ends_with(&tail));
        assert_eq!(
            (s.model.as_deref(), s.mode),
            (Some("opus"), SessionMode::Subagents)
        );

        // Padrão: nenhum argumento a mais, modo native.
        let p = env
            .sessions
            .start_project(env.project_id, "planejar", None, &AgentOptions::default())
            .unwrap();
        assert!(!p
            .launch_cmd
            .iter()
            .any(|a| a == "--model" || a == "--append-system-prompt"));
        assert_eq!((p.model, p.mode), (None, SessionMode::Native));
    }

    fn rules(project: Option<(i64, &str)>, epics: &[(i64, i64, &str)]) -> ct_ledger::ConfigRules {
        ct_ledger::ConfigRules {
            project: project.map(|(version, c)| ct_ledger::RuleText {
                version,
                content: c.into(),
            }),
            epics: epics
                .iter()
                .map(|(number, version, c)| ct_ledger::EpicRuleText {
                    number: *number,
                    version: *version,
                    content: (*c).into(),
                })
                .collect(),
        }
    }

    /// Texto do único `--append-system-prompt` do argv, se houver.
    fn appended(s: &Session) -> Option<String> {
        let i = s
            .launch_cmd
            .iter()
            .position(|a| a == "--append-system-prompt")?;
        assert_eq!(
            s.launch_cmd
                .iter()
                .filter(|a| *a == "--append-system-prompt")
                .count(),
            1
        );
        Some(s.launch_cmd[i + 1].clone())
    }

    /// Regras do agente (#357, spec do Jira e GitLab §10.1): sub-issue
    /// recebe as do projeto e depois as do épico pai; o épico, as dele;
    /// issue sem épico e sessão do projeto, só as do projeto. O agente falso
    /// recebe o argumento como veio.
    #[test]
    fn regras_do_projeto_e_do_epico_chegam_na_ordem_ao_system_prompt() {
        let env = TestEnv::new(FakeClaude::exit(0));
        env.assign_with_rules(
            ct_core::TrackerConfig::Github,
            ct_core::ForgeConfig::Github,
            false,
            1,
            rules(
                Some((7, "Branch exatamente PROJ-NNNN.\n")),
                &[(40, 3, "Só toque o openapi.yaml."), (50, 1, "outro épico")],
            ),
        );
        env.mirror_issues(&[(40, None, true), (41, Some(40), false), (42, None, false)]);
        let projeto =
            "Regras do projeto demo (versão 7, Control Tower):\nBranch exatamente PROJ-NNNN.";
        let epico =
            "Regras do épico deep-ion-ai/demo#40 (versão 3, Control Tower):\nSó toque o openapi.yaml.";

        let sub = env.sessions.start(env.request(41, "Sub")).unwrap();
        let out = env.wait_fake_output(&sub.id);
        let want = format!("{projeto}\n\n{epico}");
        assert_eq!(appended(&sub).as_deref(), Some(want.as_str()));
        assert!(
            out.contains(&format!("--append-system-prompt\n{want}\n")),
            "{out}"
        );

        let mut req = env.request(40, "Épico");
        req.kind = WorkItemKind::Epic;
        let ep = env.sessions.start(req).unwrap();
        assert_eq!(appended(&ep).as_deref(), Some(want.as_str()));

        let solta = env.sessions.start(env.request(42, "Solta")).unwrap();
        assert_eq!(appended(&solta).as_deref(), Some(projeto));
        let p = env
            .sessions
            .start_project(env.project_id, "planejar", None, &AgentOptions::default())
            .unwrap();
        assert_eq!(appended(&p).as_deref(), Some(projeto));

        // Com o modo subagents, as regras vêm antes da instrução do modo.
        let mut req = env.request(43, "Sem espelho");
        req.agent.mode = SessionMode::Subagents;
        let s = env.sessions.start(req).unwrap();
        assert_eq!(
            appended(&s).unwrap(),
            format!("{projeto}\n\n{SUBAGENTS_PROMPT}")
        );
    }

    /// Docs fora do repositório (#440): com `docs_root` inacessível nada é
    /// criado e a sessão sobe; com ele no ar, a sessão da sub-issue cria
    /// `_projeto/` e a pasta do épico pai, com o `README.md` e as pastas do
    /// modelo; a do épico reaproveita a mesma pasta.
    #[test]
    fn docs_da_sub_issue_criam_a_pasta_do_epico_pai() {
        let env = TestEnv::new(FakeClaude::exit(0));
        env.assign_config(1, |p| {
            p.docs = ct_core::DocsConfig {
                enabled: true,
                project_dirs: vec!["analises".into()],
                epic_dirs: vec!["prompts".into()],
            };
        });
        env.mirror_issues(&[(40, None, true), (41, Some(40), false), (42, None, false)]);
        let root = env.tmp.path().join("docs");

        // Montagem caída: a raiz não existe, nada é criado.
        env.sessions.start(env.request(41, "Sub")).unwrap();
        assert!(!root.exists());

        // Montagem caída com o ponto de montagem como pasta local vazia (sem
        // o marcador do cthubd, #441): nada é criado nela.
        std::fs::create_dir_all(&root).unwrap();
        env.sessions.start(env.request(41, "Sub")).unwrap();
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0);

        env.docs_root_up();
        env.sessions.start(env.request(42, "Solta")).unwrap();
        let proj = root.join("deep-ion-ai__demo");
        assert!(proj.join("_projeto/analises").is_dir());
        assert!(
            !proj.join("epicos").exists(),
            "issue sem épico não cria épico"
        );

        env.sessions.start(env.request(41, "Sub")).unwrap();
        let epico = proj.join("epicos/40-issue-40");
        assert!(epico.join("prompts").is_dir());
        let readme = std::fs::read_to_string(epico.join("README.md")).unwrap();
        assert!(readme.starts_with("# Issue 40\n"), "{readme}");
        assert!(readme.contains("deep-ion-ai/demo#40"), "{readme}");
        assert!(readme.contains("[#41]"), "{readme}");

        let mut req = env.request(40, "Épico");
        req.kind = WorkItemKind::Epic;
        env.sessions.start(req).unwrap();
        let epicos: Vec<_> = std::fs::read_dir(proj.join("epicos"))
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(epicos, vec!["40-issue-40"]);
    }

    /// Docs desligados no projeto: nada é criado, mesmo com a raiz no ar.
    #[test]
    fn docs_desligados_nao_criam_nada() {
        let env = TestEnv::new(FakeClaude::exit(0));
        env.assign_config(1, |p| {
            p.docs = ct_core::DocsConfig {
                enabled: false,
                project_dirs: vec!["analises".into()],
                epic_dirs: vec![],
            };
        });
        let root = env.docs_root_up();
        env.sessions.start(env.request(42, "Solta")).unwrap();
        env.sessions
            .start_project(env.project_id, "", None, &AgentOptions::default())
            .unwrap();
        assert_eq!(
            std::fs::read_dir(&root).unwrap().count(),
            1,
            "só o marcador"
        );
    }

    fn docs_ligados(env: &TestEnv) {
        env.assign_config(1, |p| {
            p.docs = ct_core::DocsConfig {
                enabled: true,
                project_dirs: vec!["analises".into()],
                epic_dirs: vec!["prompts".into()],
            };
        });
    }

    /// Valor de `--add-dir` na saída do `claude` falso.
    fn add_dir(out: &str) -> Option<&str> {
        let mut lines = out.lines();
        lines.find(|l| *l == "--add-dir")?;
        lines.next()
    }

    /// Sessões com docs (#441): a de issue recebe `CT_DOCS_DIR`, o
    /// `--add-dir` e a linha das instruções para `_projeto`; a de sub-issue
    /// também `CT_EPIC_DOCS_DIR` (a pasta do épico pai), e as instruções
    /// apontam para ela; a de planejamento, só `CT_DOCS_DIR`.
    #[test]
    fn docs_nas_sessoes_de_issue_sub_issue_e_planejamento() {
        let env = TestEnv::new(FakeClaude::exit(0));
        docs_ligados(&env);
        env.mirror_issues(&[(40, None, true), (41, Some(40), false), (42, None, false)]);
        let root = env.docs_root_up();
        let proj = root.join("deep-ion-ai__demo");
        let proj_s = proj.display().to_string();

        let s = env.sessions.start(env.request(42, "Solta")).unwrap();
        let out = env.wait_fake_output(&s.id);
        assert!(out.contains(&format!("CT_DOCS_DIR={proj_s}\n")), "{out}");
        assert!(!out.contains("CT_EPIC_DOCS_DIR"), "{out}");
        assert_eq!(add_dir(&out), Some(proj_s.as_str()), "{out}");
        assert!(
            out.contains(&format!(
                "$CT_DOCS_DIR/_projeto ({proj_s}/_projeto), nunca no repositório"
            )),
            "{out}"
        );
        // `--add-dir` (variádico) antes de outra opção; o prompt continua
        // o último argumento.
        assert!(
            out.starts_with(&format!(
                "--session-id\n{}\n--add-dir\n",
                s.tool_session_id.as_deref().unwrap()
            )),
            "{out}"
        );
        assert!(out.contains("prompt inicial\nPWD="), "{out}");

        let s = env.sessions.start(env.request(41, "Sub")).unwrap();
        let out = env.wait_fake_output(&s.id);
        let epico = proj.join("epicos/40-issue-40").display().to_string();
        assert!(out.contains(&format!("CT_DOCS_DIR={proj_s}\n")), "{out}");
        assert!(
            out.contains(&format!("CT_EPIC_DOCS_DIR={epico}\n")),
            "{out}"
        );
        assert_eq!(add_dir(&out), Some(proj_s.as_str()), "{out}");
        assert!(
            out.contains(&format!("$CT_EPIC_DOCS_DIR ({epico})")),
            "{out}"
        );
        assert!(!out.contains("$CT_DOCS_DIR/_projeto"), "{out}");

        let s = env
            .sessions
            .start_project(env.project_id, "planejar", None, &AgentOptions::default())
            .unwrap();
        let out = env.wait_fake_output(&s.id);
        assert!(out.contains(&format!("CT_DOCS_DIR={proj_s}\n")), "{out}");
        assert!(!out.contains("CT_EPIC_DOCS_DIR"), "{out}");
        assert_eq!(add_dir(&out), Some(proj_s.as_str()), "{out}");
        assert!(out.contains("$CT_DOCS_DIR/_projeto"), "{out}");
    }

    /// "Abrir pasta de docs" (#444): `docs_dir` recusa sem docs, com a raiz
    /// inacessível (sem criar nada) e com issue fora do espelho; com a raiz
    /// no ar, cria e devolve a pasta do projeto e a do épico, reaproveitando
    /// a `<n>-*` existente.
    #[test]
    fn docs_dir_da_pasta_do_projeto_e_do_epico() {
        let env = TestEnv::new(FakeClaude::exit(0));
        let id = env.project_id;
        let msg = |r: crate::Result<std::path::PathBuf>| r.unwrap_err().to_string();
        assert!(msg(env.sessions.docs_dir(id, None)).contains("docs desligados"));
        docs_ligados(&env);
        env.mirror_issues(&[(40, None, true), (41, Some(40), false)]);
        let root = env.tmp.path().join("docs");
        std::fs::create_dir_all(&root).unwrap();
        assert!(msg(env.sessions.docs_dir(id, None)).contains("docs_unreachable"));
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0);

        env.docs_root_up();
        let proj = root.join("deep-ion-ai__demo");
        assert_eq!(env.sessions.docs_dir(id, None).unwrap(), proj);
        assert!(proj.join("_projeto/analises").is_dir());
        std::fs::create_dir_all(proj.join("epicos/40-titulo-antigo")).unwrap();
        let e = env.sessions.docs_dir(id, Some(40)).unwrap();
        assert_eq!(e, proj.join("epicos/40-titulo-antigo"));
        assert!(e.join("prompts").is_dir() && e.join("README.md").is_file());
        assert!(msg(env.sessions.docs_dir(id, Some(999))).contains("espelho"));

        let mut env = TestEnv::new(FakeClaude::exit(0));
        env.cfg.docs_root = None;
        let bin = env.cfg.claude_bin.clone();
        env.set_claude_bin(&bin);
        docs_ligados(&env);
        assert!(
            msg(env.sessions.docs_dir(env.project_id, None)).contains("docs_root não configurado")
        );
    }

    /// Sem docs no projeto, ou sem `docs_root` no ambiente, nada muda: nem
    /// variáveis, nem `--add-dir`, nem a linha das instruções. O gestor
    /// nunca recebe.
    #[test]
    fn sem_docs_nada_muda_na_sessao() {
        let sem = |out: &str| {
            assert!(!out.contains("CT_DOCS_DIR"), "{out}");
            assert!(!out.contains("CT_EPIC_DOCS_DIR"), "{out}");
            assert!(!out.contains("--add-dir"), "{out}");
            assert!(!out.contains("Docs do projeto"), "{out}");
        };
        let env = TestEnv::new(FakeClaude::exit(0));
        env.docs_root_up();
        let s = env.sessions.start(env.request(42, "Solta")).unwrap();
        sem(&env.wait_fake_output(&s.id));
        let g = env
            .sessions
            .start_gestor(
                &GestorScope::Portfolio,
                "oi",
                None,
                &AgentOptions::default(),
            )
            .unwrap();
        sem(&env.wait_fake_output(&g.id));

        let mut env = TestEnv::new(FakeClaude::exit(0));
        env.cfg.docs_root = None;
        let bin = env.cfg.claude_bin.clone();
        env.set_claude_bin(&bin);
        docs_ligados(&env);
        let s = env.sessions.start(env.request(42, "Solta")).unwrap();
        sem(&env.wait_fake_output(&s.id));
        assert!(!env
            .notices()
            .iter()
            .any(|(n, d)| n == "alert" && d["kind"] == "docs_unreachable"));
    }

    /// `docs_root` inacessível (#441), inexistente ou ponto de montagem
    /// vazio (sem o marcador do cthubd): a sessão sobe sem as variáveis e
    /// sem o `--add-dir`, com o alerta `docs_unreachable` e o evento no log,
    /// e nada é criado no disco local.
    #[test]
    fn docs_root_inacessivel_sobe_sem_docs_e_com_aviso() {
        let env = TestEnv::new(FakeClaude::exit(0));
        docs_ligados(&env);
        let root = env.tmp.path().join("docs");
        let sem_docs = |n: i64| {
            let s = env.sessions.start(env.request(n, "Solta")).unwrap();
            let out = env.wait_fake_output(&s.id);
            assert!(!out.contains("CT_DOCS_DIR"), "{out}");
            assert!(!out.contains("--add-dir"), "{out}");
            assert!(!out.contains("Docs do projeto"), "{out}");
            assert!(
                env.notices().iter().any(|(n, d)| n == "alert"
                    && d["kind"] == "docs_unreachable"
                    && d["session_id"] == s.id.as_str()),
                "alerta docs_unreachable de {}",
                s.id
            );
            let evs = env
                .sessions
                .ledger()
                .list_events(s.work_item_id, 50)
                .unwrap();
            assert!(evs
                .iter()
                .any(|e| e.kind == "docs_unreachable" && e.session_id.as_deref() == Some(&s.id)));
        };
        sem_docs(42);
        assert!(!root.exists());

        std::fs::create_dir_all(&root).unwrap();
        sem_docs(43);
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0);
        let st = crate::docs::status(Some(&root));
        assert_eq!(st.warning.as_deref(), Some("docs_unreachable"), "{st:?}");
    }

    /// Projeto Jira: o cabeçalho do épico usa a chave (`PROJ-5014`), e
    /// sem regras de projeto a sub-issue recebe só as do épico.
    #[test]
    fn regras_do_epico_jira_usam_a_chave() {
        let env = TestEnv::new(FakeClaude::exit(0));
        env.assign_with_rules(
            ct_core::TrackerConfig::Jira(ct_core::JiraConfig {
                project_key: "PROJ".into(),
                ..Default::default()
            }),
            ct_core::ForgeConfig::Github,
            false,
            1,
            rules(None, &[(5014, 2, "Avise se a demanda for só de frontend.")]),
        );
        env.mirror_issues(&[(5014, None, true), (5450, Some(5014), false)]);
        let s = env.sessions.start(env.request(5450, "Tela")).unwrap();
        env.wait_fake_output(&s.id);
        assert_eq!(
            appended(&s).unwrap(),
            "Regras do épico PROJ-5014 (versão 2, Control Tower):\n\
             Avise se a demanda for só de frontend."
        );
        // Sem regras em nenhum escopo: nada de --append-system-prompt.
        env.assign_with_rules(
            ct_core::TrackerConfig::Github,
            ct_core::ForgeConfig::Github,
            false,
            2,
            Default::default(),
        );
        let s = env.sessions.start(env.request(5450, "Tela")).unwrap();
        assert_eq!(appended(&s), None);
    }

    #[test]
    fn sessao_do_gestor_inicia_sem_projeto_na_pasta_dedicada() {
        let env = TestEnv::new(FakeClaude::sleep(30));
        let dir = env.cfg.gestor_dir.clone();
        assert!(!dir.exists());
        let scope = GestorScope::Project {
            owner: "deep-ion-ai".into(),
            repo: "demo".into(),
        };
        let agent = AgentOptions {
            model: None,
            mode: SessionMode::Subagents,
            ..AgentOptions::default()
        };
        let s = env
            .sessions
            .start_gestor(&scope, "olá, gestor", None, &agent)
            .unwrap();
        assert_eq!(s.cwd, dir.to_string_lossy());
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
            0o700
        );
        let out = env.wait_fake_output(&s.id);
        // `pwd` devolve o caminho real (no macOS, /var → /private/var).
        let real = std::fs::canonicalize(&dir).unwrap();
        assert!(out.contains(&format!("PWD={}", real.display())), "{out}");
        assert!(out.contains("CTOWER_SESSION_KIND=gestor"), "{out}");
        assert!(out.contains("CTOWER_PROTECTED=\n"), "{out}");
        assert!(!out.contains("CTOWER_ISSUE"), "{out}");
        // Um só --append-system-prompt, com os limites e o modo subagents.
        let i = s
            .launch_cmd
            .iter()
            .position(|a| a == "--append-system-prompt")
            .unwrap();
        assert_eq!(
            s.launch_cmd[i + 1],
            format!("{GESTOR_LIMITS}\n\n{SUBAGENTS_PROMPT}")
        );
        assert_eq!(
            s.launch_cmd
                .iter()
                .filter(|a| *a == "--append-system-prompt")
                .count(),
            1
        );
        let (p, wi, evs) = {
            let l = env.sessions.ledger();
            let wi = l.get_work_item(s.work_item_id).unwrap();
            let p = l.get_project(wi.project_id).unwrap();
            let evs = l.list_events(wi.id, 10).unwrap();
            (p, wi, evs)
        };
        assert_eq!(p.kind, ProjectKind::Gestor);
        assert_eq!(
            (p.gh_owner.as_str(), p.gh_repo.as_str()),
            ("_ctower", "gestor")
        );
        assert_eq!(wi.gh_number, PROJECT_WORK_ITEM);
        assert!(evs.iter().any(|e| e.kind == "gestor_started"
            && e.payload["scope"]
                == json!({ "kind": "project", "owner": "deep-ion-ai", "repo": "demo" })));
        // O sentinela fica fora da lista de projetos; a segunda sessão usa
        // o mesmo projeto e o mesmo work item.
        assert!(env
            .sessions
            .ledger()
            .list_projects()
            .unwrap()
            .iter()
            .all(|p| p.kind == ProjectKind::Github));
        let s2 = env
            .sessions
            .start_gestor(&GestorScope::Health, "", None, &AgentOptions::default())
            .unwrap();
        assert_eq!(s2.work_item_id, s.work_item_id);
        // Sessão comum não tem o perfil do gestor.
        let c = env.sessions.start(env.request(31, "Comum")).unwrap();
        let out = env.wait_fake_output(&c.id);
        assert!(!out.contains("CTOWER_SESSION_KIND"), "{out}");
        for id in [&s.id, &s2.id, &c.id] {
            env.sessions.kill(id).ok();
        }
    }

    #[test]
    fn duas_sessoes_do_gestor_vivas_ao_mesmo_tempo() {
        let env = TestEnv::new(FakeClaude::sleep(30));
        let demo = GestorScope::Project {
            owner: "deep-ion-ai".into(),
            repo: "demo".into(),
        };
        let a = env
            .sessions
            .start_gestor(&GestorScope::Portfolio, "a", None, &AgentOptions::default())
            .unwrap();
        let b = env
            .sessions
            .start_gestor(&demo, "b", None, &AgentOptions::default())
            .unwrap();
        // Mesmo work item e mesma pasta; conversas e arquivos de sessão próprios.
        assert_eq!((a.work_item_id, &a.cwd), (b.work_item_id, &b.cwd));
        assert_ne!(a.tool_session_id, b.tool_session_id);
        assert_ne!(
            env.cfg.dirs.session_dir(&a.id),
            env.cfg.dirs.session_dir(&b.id)
        );
        for s in [&a, &b] {
            let out = env.wait_fake_output(&s.id);
            assert!(out.contains(&format!("CTOWER_SESSION={}", s.id)), "{out}");
        }
        let scopes = |env: &TestEnv| {
            let l = env.sessions.ledger();
            let live = l.live_sessions().unwrap();
            gestor_scopes(&l, a.work_item_id, &live).unwrap()
        };
        let m = scopes(&env);
        assert_eq!(m.len(), 2, "{m:?}");
        assert_eq!(m[&a.id], GestorScope::Portfolio);
        assert_eq!(m[&b.id], demo);
        // Encerrar uma não mexe na outra.
        env.sessions.kill(&a.id).unwrap();
        env.wait_state(&a.id, SessionState::Exited);
        assert!(env.sessions.pty(&b.id).is_some());
        let m = scopes(&env);
        assert_eq!(m.keys().collect::<Vec<_>>(), [&b.id]);
        env.sessions.kill(&b.id).ok();
    }

    #[test]
    fn modelo_fora_da_lista_e_recusado_sem_criar_nada() {
        let env = TestEnv::new(FakeClaude::exit(0));
        let bad = AgentOptions {
            model: Some("gpt-4".into()),
            mode: SessionMode::Native,
            ..AgentOptions::default()
        };
        let mut req = env.request(17, "Inválido");
        req.agent = bad.clone();
        let err = env.sessions.start(req).unwrap_err();
        assert!(
            matches!(&err, SessionsError::Invalid(m) if m.contains("gpt-4")),
            "{err}"
        );
        assert!(matches!(
            env.sessions
                .start_project(env.project_id, "planejar", None, &bad),
            Err(SessionsError::Invalid(_))
        ));
        assert!(
            !env.cfg.worktrees_root.join("demo").exists(),
            "nenhum worktree"
        );
        let l = env.sessions.ledger();
        assert!(l.find_work_item(env.project_id, 17).unwrap().is_none());
    }

    #[test]
    fn pre_aceita_confianca_no_repo_e_no_worktree() {
        let env = TestEnv::new(FakeClaude::exit(0));
        let s = env.sessions.start(env.request(2, "Confiança")).unwrap();
        let cj: Value = serde_json::from_str(
            &std::fs::read_to_string(env.cfg.claude_json.as_ref().unwrap()).unwrap(),
        )
        .unwrap();
        let repo = env.repo.to_string_lossy().to_string();
        assert_eq!(cj["projects"][&repo]["hasTrustDialogAccepted"], true);
        assert_eq!(cj["projects"][&s.cwd]["hasTrustDialogAccepted"], true);
        let evs = env
            .sessions
            .ledger()
            .list_events(s.work_item_id, 100)
            .unwrap();
        assert!(evs.iter().any(|e| e.kind == "trust_granted"));
    }

    #[test]
    fn claude_json_invalido_vira_alerta_e_sessao_segue() {
        let env = TestEnv::new(FakeClaude::exit(0));
        std::fs::write(env.cfg.claude_json.as_ref().unwrap(), "{ quebrado").unwrap();
        let s = env.sessions.start(env.request(3, "X")).unwrap();
        assert!(s.pid.is_some());
        assert!(env
            .notices()
            .iter()
            .any(|(n, d)| n == "alert" && d["kind"] == "trust_failed"));
    }

    #[test]
    fn saida_com_erro_vira_interrupted_com_codigo() {
        let env = TestEnv::new(FakeClaude::exit(3));
        let s = env.sessions.start(env.request(7, "Bug x")).unwrap();
        let got = env.wait_state(&s.id, SessionState::Interrupted);
        assert_eq!(got.exit_code, Some(3));
        assert!(env.sessions.pty(&s.id).is_none());
        let evs = env
            .sessions
            .ledger()
            .list_events(s.work_item_id, 100)
            .unwrap();
        assert!(evs.iter().any(|e| e.kind == "session_exit_error"));
        let notices = env.wait_notice(|n, _| n == "sessions_interrupted");
        assert!(notices.iter().any(|(n, d)| n == "session_exited"
            && d["exit_code"] == 3
            && d["state"] == "interrupted"));
        let rec = env.sessions.recovery_list().unwrap();
        assert_eq!(rec.len(), 1);
        assert_eq!(rec[0].session.id, s.id);
    }

    #[test]
    fn saida_limpa_vira_exited_e_fica_fora_da_recuperacao() {
        let env = TestEnv::new(FakeClaude::exit(0));
        let s = env.sessions.start(env.request(7, "Ok")).unwrap();
        let got = env.wait_state(&s.id, SessionState::Exited);
        assert_eq!(got.exit_code, Some(0));
        assert!(env.sessions.recovery_list().unwrap().is_empty());
        let evs = env
            .sessions
            .ledger()
            .list_events(s.work_item_id, 100)
            .unwrap();
        assert!(!evs.iter().any(|e| e.kind == "session_exit_error"));
        env.wait_notice(|n, d| n == "session_exited" && d["state"] == "exited");
    }

    #[test]
    fn processo_morto_por_sinal_vai_para_a_recuperacao() {
        let env = TestEnv::new(FakeClaude::sleep(30));
        let s = env.sessions.start(env.request(8, "Morta")).unwrap();
        let pid = libc::pid_t::try_from(s.pid.unwrap()).unwrap();
        // SAFETY: o pid é do processo falso desta sessão.
        assert_eq!(unsafe { libc::kill(pid, libc::SIGKILL) }, 0);
        let got = env.wait_state(&s.id, SessionState::Interrupted);
        assert_eq!(got.exit_code, None);
        let rec = env.sessions.recovery_list().unwrap();
        assert_eq!(rec.len(), 1);
        assert_eq!(rec[0].session.id, s.id);
    }

    #[test]
    fn session_end_antes_da_saida_mantem_exited() {
        let env = TestEnv::new(FakeClaude::sleep(30));
        let s = env.sessions.start(env.request(8, "Fim")).unwrap();
        env.sessions
            .ledger()
            .transition_session(&s.id, SessionState::Exited, None)
            .unwrap();
        let pid = libc::pid_t::try_from(s.pid.unwrap()).unwrap();
        // SAFETY: o pid é do processo falso desta sessão.
        assert_eq!(unsafe { libc::kill(pid, libc::SIGKILL) }, 0);
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while env.sessions.pty(&s.id).is_some() {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(20));
        }
        let l = env.sessions.ledger();
        assert_eq!(l.get_session(&s.id).unwrap().state, SessionState::Exited);
    }

    #[test]
    fn segunda_sessao_reutiliza_worktree() {
        let env = TestEnv::new(FakeClaude::exit(0));
        let a = env.sessions.start(env.request(5, "Tarefa")).unwrap();
        env.wait_state(&a.id, SessionState::Exited);
        let b = env.sessions.start(env.request(5, "Título mudou")).unwrap();
        assert_eq!(a.cwd, b.cwd);
        assert_ne!(a.tool_session_id, b.tool_session_id);
    }

    #[test]
    fn worktree_ocupado_por_outra_branch_nao_grava_sessao() {
        let env = TestEnv::new(FakeClaude::exit(0));
        let occupied = env.cfg.worktrees_root.join("demo").join("feat__9-ocupado");
        env.git(&[
            "worktree",
            "add",
            "-b",
            "outra",
            occupied.to_str().unwrap(),
            "develop",
        ]);
        let err = env.sessions.start(env.request(9, "Ocupado")).unwrap_err();
        assert!(matches!(err, SessionsError::Git(_)), "{err}");
        let l = env.sessions.ledger();
        let wi = l.find_work_item(env.project_id, 9).unwrap().unwrap();
        assert!(l.list_sessions(wi.id).unwrap().is_empty());
        assert!(wi.branch.is_none());
    }

    #[test]
    fn falha_no_spawn_marca_exited_e_registra_evento() {
        let mut env = TestEnv::new(FakeClaude::exit(0));
        env.set_claude_bin("/nao/existe/claude");
        let err = env.sessions.start(env.request(3, "X"));
        assert!(err.is_err());
        let l = env.sessions.ledger();
        let wi = l.find_work_item(env.project_id, 3).unwrap().unwrap();
        let ss = l.list_sessions(wi.id).unwrap();
        // portable-pty pode falhar no spawn (erro) ou o filho sair com 127.
        assert_eq!(ss.len(), 1);
        drop(l);
        // Erro no spawn → `exited`; filho que sai com 127 → `interrupted`.
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let s = loop {
            let s = env.sessions.ledger().get_session(&ss[0].id).unwrap();
            if !s.state.is_live() {
                break s;
            }
            assert!(std::time::Instant::now() < deadline, "{:?}", s.state);
            std::thread::sleep(Duration::from_millis(20));
        };
        assert_ne!(s.exit_code, Some(0));
    }

    /// Log do `tracing` capturado em memória (o `launch` é síncrono: os
    /// avisos saem na thread do teste).
    #[derive(Clone, Default)]
    struct Captured(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

    impl std::io::Write for Captured {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn with_log<T>(f: impl FnOnce() -> T) -> (T, String) {
        let log = Captured::default();
        let w = log.clone();
        let sub = tracing_subscriber::fmt()
            .with_writer(move || w.clone())
            .with_ansi(false)
            .finish();
        let out = tracing::subscriber::with_default(sub, f);
        let text = String::from_utf8(log.0.lock().unwrap().clone()).unwrap();
        (out, text)
    }

    /// Valores de teste: nenhum pode aparecer em ledger, arquivos do
    /// ambiente, `launch_cmd` ou log.
    const VALORES: &[&str] = &[
        "valor-global-1",
        "valor-global-sobreposto",
        "valor-do-projeto-2",
        "valor-de-outro-projeto",
        "token-falso-do-cache",
        "/caminho/falso",
        "/home/falsa",
        "/tmp/falso.so",
    ];

    fn cache_de_teste(env: &TestEnv) {
        env.write_secrets_cache(&[
            (None, "T_GLOBAL", "valor-global-1"),
            (None, "T_SOBRE", "valor-global-sobreposto"),
            (Some("deep-ion-ai/demo"), "T_SOBRE", "valor-do-projeto-2"),
            (
                Some("deep-ion-ai/outro"),
                "T_OUTRO",
                "valor-de-outro-projeto",
            ),
            (None, "CTOWER_TOKEN", "token-falso-do-cache"),
            (None, "PATH", "/caminho/falso"),
            (Some("deep-ion-ai/demo"), "HOME", "/home/falsa"),
            (None, "LD_PRELOAD", "/tmp/falso.so"),
        ]);
    }

    /// Bytes de todos os arquivos sob `dir` (ledger, WAL, arquivos das
    /// sessões, spool), menos o cache cifrado.
    fn varre(dir: &Path, skip: &Path, found: &mut Vec<(PathBuf, String)>) {
        for e in std::fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                varre(&p, skip, found);
            } else if p != skip {
                let bytes = std::fs::read(&p).unwrap();
                let text = String::from_utf8_lossy(&bytes);
                for v in VALORES {
                    if text.contains(v) {
                        found.push((p.clone(), (*v).to_string()));
                    }
                }
            }
        }
    }

    #[test]
    fn secrets_do_cache_vao_no_ambiente_com_precedencia_e_reservados_protegidos() {
        let env = TestEnv::with_file_ledger(FakeClaude::exit(0));
        cache_de_teste(&env);
        let (s, log) = with_log(|| env.sessions.start(env.request(21, "Secrets")).unwrap());
        let out = env.wait_fake_output(&s.id);

        // Global entra; projeto sobrepõe global; outro projeto não entra.
        assert!(out.contains("T_GLOBAL=valor-global-1\n"), "{out}");
        assert!(out.contains("T_SOBRE=valor-do-projeto-2\n"), "{out}");
        assert!(!out.contains("valor-global-sobreposto"), "{out}");
        assert!(!out.contains("T_OUTRO"), "{out}");
        // Reservados do cache não sobrescrevem: as `CTOWER_*` do daemon e o
        // ambiente do processo continuam.
        assert!(out.contains("CTOWER_TOKEN=hook-secreto\n"), "{out}");
        assert!(!out.contains("token-falso-do-cache"), "{out}");
        assert!(!out.contains("ENV_PATH=/caminho/falso"), "{out}");
        assert!(!out.contains("ENV_HOME=/home/falsa"), "{out}");
        // O aviso cita o nome, nunca o valor.
        for n in ["CTOWER_TOKEN", "PATH", "HOME", "LD_PRELOAD"] {
            assert!(log.contains(&format!("{n} é um nome reservado")), "{log}");
        }

        env.wait_state(&s.id, SessionState::Exited);
        let s = env.sessions.ledger().get_session(&s.id).unwrap();
        for v in VALORES {
            assert!(!s.launch_cmd.iter().any(|a| a.contains(v)), "{v}");
            assert!(!log.contains(v), "{v} no log: {log}");
        }
        let mut found = Vec::new();
        varre(
            &env.cfg.dirs.data,
            &env.cfg.dirs.secrets_cache_file(),
            &mut found,
        );
        varre(&env.cfg.dirs.config, Path::new(""), &mut found);
        assert!(env.cfg.dirs.ledger_db().exists());
        assert!(found.is_empty(), "valores persistidos: {found:?}");
    }

    #[test]
    fn gestor_recebe_so_os_secrets_globais() {
        let env = TestEnv::new(FakeClaude::exit(0));
        // Até um projeto com a chave do sentinela fica de fora: o gestor não
        // está preso a um projeto.
        env.write_secrets_cache(&[
            (None, "T_GLOBAL", "valor-global-1"),
            (None, "T_SOBRE", "valor-global-sobreposto"),
            (Some("deep-ion-ai/demo"), "T_SOBRE", "valor-do-projeto-2"),
            (Some("_ctower/gestor"), "T_SENTINELA", "valor-do-sentinela"),
        ]);
        let s = env
            .sessions
            .start_gestor(&GestorScope::Portfolio, "", None, &AgentOptions::default())
            .unwrap();
        let out = env.wait_fake_output(&s.id);
        assert!(out.contains("T_GLOBAL=valor-global-1\n"), "{out}");
        assert!(out.contains("T_SOBRE=valor-global-sobreposto\n"), "{out}");
        assert!(!out.contains("valor-do-projeto-2"), "{out}");
        assert!(!out.contains("T_SENTINELA"), "{out}");
        assert!(out.contains("CTOWER_SESSION_KIND=gestor"), "{out}");
    }

    #[test]
    fn sem_cache_de_secrets_spawn_normal() {
        let env = TestEnv::new(FakeClaude::exit(0));
        let s = env.sessions.start(env.request(22, "Sem cache")).unwrap();
        let out = env.wait_fake_output(&s.id);
        assert!(out.contains("CTOWER_TOKEN=hook-secreto"), "{out}");
        assert!(!out.contains("T_"), "{out}");
        assert!(!env
            .notices()
            .iter()
            .any(|(n, d)| n == "alert" && d["kind"].as_str().unwrap_or("").starts_with("secrets")));
    }

    #[test]
    fn cache_ilegivel_sobe_sem_secrets_com_alerta() {
        let env = TestEnv::new(FakeClaude::exit(0));
        cache_de_teste(&env);
        // Chave trocada por fora: o cache não abre mais.
        std::fs::remove_file(env.cfg.dirs.env_key_file()).unwrap();
        env.sessions.env_cache().ensure_key().unwrap();
        let (s, log) = with_log(|| env.sessions.start(env.request(23, "Ilegível")).unwrap());
        let out = env.wait_fake_output(&s.id);
        assert!(out.contains("CTOWER_TOKEN=hook-secreto"), "{out}");
        assert!(!out.contains("T_GLOBAL"), "{out}");
        assert!(log.contains("sobe sem secrets"), "{log}");
        assert!(env
            .notices()
            .iter()
            .any(|(n, d)| n == "alert" && d["kind"] == "secrets_unavailable"));
    }

    #[test]
    fn cache_velho_vale_com_aviso() {
        let env = TestEnv::new(FakeClaude::exit(0));
        cache_de_teste(&env);
        // Sem pull registrado, a idade é a do cache: gravado há 2 dias
        // (limite de 24h no TestEnv).
        let dois_dias = std::time::SystemTime::now() - std::time::Duration::from_secs(48 * 3600);
        std::fs::File::options()
            .write(true)
            .open(env.cfg.dirs.secrets_cache_file())
            .unwrap()
            .set_modified(dois_dias)
            .unwrap();
        let age = env.sessions.config_age_ms().unwrap();
        assert!(age >= 48 * 3_600_000, "{age}");
        let (s, log) = with_log(|| env.sessions.start(env.request(24, "Velho")).unwrap());
        let out = env.wait_fake_output(&s.id);
        assert!(out.contains("T_GLOBAL=valor-global-1"), "{out}");
        assert!(log.contains("sem sincronizar há 48h (limite 24h)"), "{log}");
        let stale = |env: &TestEnv| {
            env.notices()
                .iter()
                .filter(|(n, d)| n == "alert" && d["kind"] == "secrets_stale")
                .count()
        };
        assert_eq!(stale(&env), 1);

        // Um pull bem-sucedido, mesmo sem mudança, prova que o cache está em
        // dia: vale o `config_pulled_at`, e o aviso some.
        let cfg = ct_ledger::ConfigResponse {
            config_rev: 1,
            config_epoch: 1,
            changed: false,
            projects: None,
            removed: None,
        };
        env.sessions
            .ledger()
            .apply_hub_config(&cfg, |_| String::new())
            .unwrap();
        assert!(env.sessions.config_age_ms().unwrap() < 3_600_000);
        let s = env.sessions.start(env.request(25, "Em dia")).unwrap();
        assert!(env
            .wait_fake_output(&s.id)
            .contains("T_GLOBAL=valor-global-1"));
        assert_eq!(stale(&env), 1);
    }

    /// Saída do agente com secrets (#167): o scrollback e os assinantes
    /// veem `***NOME***`; secret curto e variável saem como estão.
    #[test]
    fn secrets_impressos_saem_mascarados_no_scrollback_e_no_broadcast() {
        use ct_secrets::Kind;
        let env = TestEnv::new(FakeClaude::script(
            "echo \"antes=$T_TOKEN\"; read x; printf 'depois=%s/%s/%s\\n' \"$T_TOKEN\" \"$T_CURTO\" \"$T_VAR\"; printf '%s' \"$T_TOKEN\" | head -c 5; sleep 0.01; printf '%s' \"$T_TOKEN\" | tail -c +6; echo; read y",
        ));
        env.write_secrets_cache_kinds(&[
            (None, "T_TOKEN", Kind::Secret, "token-muito-secreto"),
            (None, "T_CURTO", Kind::Secret, "curto7x"),
            (None, "T_VAR", Kind::Variable, "variavel-visivel"),
        ]);
        let s = env.sessions.start(env.request(26, "Mascarar")).unwrap();
        let pty = env.sessions.pty(&s.id).unwrap();
        let deadline = std::time::Instant::now() + T;
        while !String::from_utf8_lossy(&pty.snapshot()).contains("antes=") {
            assert!(std::time::Instant::now() < deadline, "sem saída");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let (snap, rx) = pty.attach();
        pty.write(b"\n").unwrap();
        let mut live = Vec::new();
        while !String::from_utf8_lossy(&live).contains("***T_TOKEN***\r\n") {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            live.extend(rx.recv_timeout(left).expect("sem broadcast"));
        }
        let live = String::from_utf8_lossy(&live).to_string();
        assert!(
            live.contains("depois=***T_TOKEN***/curto7x/variavel-visivel"),
            "{live}"
        );
        assert!(!live.contains("token-muito-secreto"), "{live}");
        pty.write(b"\n").unwrap();
        env.wait_state(&s.id, SessionState::Exited);
        let all = String::from_utf8_lossy(&pty.snapshot()).to_string();
        assert!(String::from_utf8_lossy(&snap).contains("antes=***T_TOKEN***"));
        assert!(all.contains("antes=***T_TOKEN***"), "{all}");
        assert!(all.contains("depois=***T_TOKEN***/curto7x/"), "{all}");
        assert!(!all.contains("token-muito-secreto"), "{all}");
        // O processo recebeu o valor de verdade.
        assert!(env
            .wait_fake_output(&s.id)
            .contains("T_TOKEN=token-muito-secreto\n"));
    }

    #[test]
    fn kill_encerra_sessao_viva() {
        let env = TestEnv::new(FakeClaude::sleep(30));
        let s = env.sessions.start(env.request(4, "Longa")).unwrap();
        assert!(env.sessions.pty(&s.id).is_some());
        env.sessions.kill(&s.id).unwrap();
        env.wait_state(&s.id, SessionState::Exited);
        assert!(env.sessions.kill(&s.id).is_err());
    }
}
