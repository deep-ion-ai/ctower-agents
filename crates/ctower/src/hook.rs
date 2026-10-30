//! `ctower hook <evento>`: repassa eventos do Claude Code ao daemon (e, no
//! `PermissionRequest`, devolve a resposta dada na UI) e aplica o
//! guarda-corpo de branch protegida e, nas sessões do gestor
//! (`CTOWER_SESSION_KIND=gestor`), o perfil `gestor`. Nas sessões de issue
//! (`CTOWER_ISSUE`), exige `Closes #n` no `gh pr create` (#375). O
//! `glab mr merge` é sempre bloqueado (#350), sem variável da sessão: o glab
//! só fala com o GitLab. Nos projetos Jira (`CTOWER_TRACKER=jira`, #351),
//! o perfil Jira bloqueia transição e edição e pede confirmação para criar
//! issue. Nunca bloqueia o agente por falha de comunicação (spec §7.2,
//! §7.3).

use clap::ValueEnum;
use ct_core::guard::closes::check_pr_closes;
use ct_core::guard::gestor::{check_gestor, token_path};
use ct_core::guard::jira::check_jira;
use ct_core::guard::{check_command, GuardContext, Verdict};
use ct_core::now_ms;
use serde_json::{json, Value};
use std::fs::{DirBuilder, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum HookEvent {
    SessionStart,
    UserPromptSubmit,
    Notification,
    Stop,
    SessionEnd,
    PreToolUse,
    PermissionRequest,
}

impl HookEvent {
    fn wire_name(self) -> &'static str {
        match self {
            HookEvent::SessionStart => "SessionStart",
            HookEvent::UserPromptSubmit => "UserPromptSubmit",
            HookEvent::Notification => "Notification",
            HookEvent::Stop => "Stop",
            HookEvent::SessionEnd => "SessionEnd",
            HookEvent::PreToolUse => "PreToolUse",
            HookEvent::PermissionRequest => "PermissionRequest",
        }
    }
}

const DEFAULT_PROTECTED: &str = "develop,main,master";

/// Prazo máximo para cada processo externo (`git`, `gh`) consultado pelo hook.
const SUBPROCESS_DEADLINE: Duration = Duration::from_secs(5);

fn env_nonempty(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

pub fn run(event: HookEvent) -> i32 {
    let mut input = String::new();
    let _ = std::io::stdin().read_to_string(&mut input);
    let Some(session) = env_nonempty("CTOWER_SESSION") else {
        return 0;
    };
    let payload: Value = serde_json::from_str(&input).unwrap_or(Value::Null);
    match event {
        HookEvent::PreToolUse => pre_tool_use(&session, &payload),
        HookEvent::PermissionRequest => permission_request(&session, &payload),
        other => {
            deliver(&session, other.wire_name(), &payload);
            0
        }
    }
}

/// Ferramentas de arquivo e o campo com o caminho, para o perfil `gestor`
/// não ler arquivos de token.
const FILE_TOOLS: &[(&str, &str)] = &[
    ("Read", "file_path"),
    ("Edit", "file_path"),
    ("Write", "file_path"),
    ("NotebookEdit", "notebook_path"),
    ("Grep", "path"),
    ("Glob", "path"),
];

fn pre_tool_use(session: &str, payload: &Value) -> i32 {
    let gestor = env_nonempty("CTOWER_SESSION_KIND").as_deref() == Some("gestor");
    let tool = payload.get("tool_name").and_then(Value::as_str);
    if gestor {
        if let Some((_, field)) = FILE_TOOLS.iter().find(|(t, _)| Some(*t) == tool) {
            let path = payload
                .pointer(&format!("/tool_input/{field}"))
                .and_then(Value::as_str)
                .unwrap_or_default();
            if let Some(m) = token_path(path) {
                let reason = format!(
                    "o gestor não lê arquivos de token nem de secrets (`{m}`): leia o hub \
                     pelas ferramentas portfolio_*"
                );
                return verdict(session, path, Verdict::Block(reason), None);
            }
        }
    }
    if tool != Some("Bash") {
        return 0;
    }
    let Some(cmd) = payload
        .pointer("/tool_input/command")
        .and_then(Value::as_str)
    else {
        return 0;
    };
    let cwd: Option<PathBuf> = payload
        .get("cwd")
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .or_else(|| std::env::current_dir().ok());
    let protected = protected_branches(env_nonempty("CTOWER_PROTECTED").as_deref());
    let head = cwd.as_deref().and_then(git_head);
    let gh = env_nonempty("CTOWER_GH").unwrap_or_else(|| "gh".to_string());
    let pr_base = |target: Option<&str>| gh_pr_base(&gh, cwd.as_deref(), target);
    let head_of = |path: &str| resolve_dir(cwd.as_deref(), path).and_then(|d| git_head(&d));
    let ctx = GuardContext {
        head_branch: head.as_deref(),
        protected: &protected,
        pr_base: &pr_base,
        head_of: &head_of,
    };

    let branch = check_command(cmd, &ctx);
    // O complemento de branch só vale para o bloqueio de branch protegida.
    let hint = (!gestor && matches!(branch, Verdict::Block(_))).then_some(BRANCH_HINT);
    let mut v = branch;
    if gestor {
        v = v.max(check_gestor(cmd));
    }
    if env_nonempty("CTOWER_TRACKER").as_deref() == Some("jira") {
        v = v.max(check_jira(cmd));
    }
    if v != Verdict::Allow {
        return verdict(session, cmd, v, hint);
    }
    let issue = env_nonempty("CTOWER_ISSUE").and_then(|n| n.trim().parse::<i64>().ok());
    if let Some(n) = issue {
        let read = |path: &str| {
            resolve_dir(cwd.as_deref(), path).and_then(|p| std::fs::read_to_string(p).ok())
        };
        return verdict(session, cmd, check_pr_closes(cmd, n, &read), None);
    }
    0
}

/// Complemento da mensagem dos bloqueios de branch protegida.
const BRANCH_HINT: &str =
    "Trabalhe apenas na branch da sessão; o merge na base é feito pelo humano.";

/// Aplica o veredito: `Block` sai com 2 e avisa o daemon; `Ask` devolve
/// `permissionDecision: "ask"`, e o Claude Code pede a confirmação do
/// usuário antes de rodar. `hint` complementa a mensagem do bloqueio.
fn verdict(session: &str, cmd: &str, v: Verdict, hint: Option<&str>) -> i32 {
    match v {
        Verdict::Allow => 0,
        Verdict::Ask(reason) => {
            println!(
                "{}",
                json!({
                    "hookSpecificOutput": {
                        "hookEventName": "PreToolUse",
                        "permissionDecision": "ask",
                        "permissionDecisionReason": format!("[Control Tower] {reason}"),
                    }
                })
            );
            0
        }
        Verdict::Block(reason) => {
            match hint {
                Some(h) => eprintln!("[Control Tower] Bloqueado: {reason}. {h}"),
                None => eprintln!("[Control Tower] Bloqueado: {reason}."),
            }
            deliver(
                session,
                "GuardBlocked",
                &json!({ "command": cmd, "reason": reason }),
            );
            2
        }
    }
}

/// Lista de branches protegidas a partir de `CTOWER_PROTECTED` (separada por
/// vírgulas). Ausente ou vazia após filtrar (ex.: `" , "`) usa o padrão.
fn protected_branches(raw: Option<&str>) -> Vec<String> {
    let parse = |s: &str| -> Vec<String> {
        s.split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    };
    let list = raw.map(parse).unwrap_or_default();
    if list.is_empty() {
        parse(DEFAULT_PROTECTED)
    } else {
        list
    }
}

/// Resolve um caminho literal do comando (`git -C`, `cd`, `--git-dir`,
/// `--work-tree`): expande `~` e `~$USER` com `HOME` e torna relativo ao cwd
/// do hook.
///
/// `None` significa HEAD desconhecido, e o guard então permite: é uma barreira
/// contra erro do agente, não contra adversário. Resolvem para `None`:
/// `cd -` (diretório anterior), `~outro_usuario/...` (não consultamos o
/// passwd) e `~` sem `HOME`.
fn resolve_dir(cwd: Option<&Path>, path: &str) -> Option<PathBuf> {
    if path == "-" {
        return None;
    }
    let home = || std::env::var_os("HOME").map(PathBuf::from);
    let p = if let Some(tilde) = path.strip_prefix('~') {
        let (user, rest) = match tilde.split_once('/') {
            Some((u, r)) => (u, Some(r)),
            None => (tilde, None),
        };
        if !user.is_empty() && env_nonempty("USER").as_deref() != Some(user) {
            return None;
        }
        match rest {
            Some(r) => home()?.join(r),
            None => home()?,
        }
    } else {
        PathBuf::from(path)
    };
    if p.is_absolute() {
        Some(p)
    } else {
        Some(cwd?.join(p))
    }
}

/// Executa `cmd` com stdin nulo e devolve o stdout se ele terminar com sucesso
/// dentro de `deadline`. Ao estourar o prazo, mata o processo e devolve `None`:
/// o hook nunca pode travar o agente.
fn run_with_deadline(mut cmd: Command, deadline: Duration) -> Option<String> {
    let start = Instant::now();
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    // O stdout é lido numa thread para não travar com pipe cheio nem com
    // netos que herdem o descritor.
    let mut stdout = child.stdout.take()?;
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout.read_to_end(&mut buf);
        let _ = tx.send(buf);
    });
    let status = loop {
        match child.try_wait() {
            Ok(Some(st)) => break st,
            Ok(None) if start.elapsed() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    };
    if !status.success() {
        return None;
    }
    let left = deadline.saturating_sub(start.elapsed());
    let buf = rx.recv_timeout(left).ok()?;
    Some(String::from_utf8_lossy(&buf).trim().to_string())
}

fn git_head(cwd: &Path) -> Option<String> {
    let mut c = Command::new("git");
    c.arg("-C")
        .arg(cwd)
        .args(["rev-parse", "--abbrev-ref", "HEAD"]);
    let b = run_with_deadline(c, SUBPROCESS_DEADLINE)?;
    (!b.is_empty() && b != "HEAD").then_some(b)
}

fn gh_pr_base(gh: &str, cwd: Option<&Path>, target: Option<&str>) -> Option<String> {
    let mut c = Command::new(gh);
    c.args(["pr", "view"]);
    if let Some(t) = target {
        c.arg(t);
    }
    c.args(["--json", "baseRefName", "--jq", ".baseRefName"]);
    c.env("GH_PROMPT_DISABLED", "1")
        .env("GH_NO_UPDATE_NOTIFIER", "1");
    if let Some(d) = cwd {
        c.current_dir(d);
    }
    let s = run_with_deadline(c, SUBPROCESS_DEADLINE)?;
    (!s.is_empty()).then_some(s)
}

/// Corpo do hook para o daemon. Com o Remote Control ligado, o Claude Code
/// põe `CLAUDE_CODE_BRIDGE_SESSION_ID` no ambiente do hook: vai fora do
/// `payload`, em `rc_session_id` (#197).
fn envelope(session: &str, event: &str, payload: &Value) -> String {
    let mut body =
        json!({ "session": session, "event": event, "payload": payload, "ts": now_ms() });
    if let Some(rc) = env_nonempty("CLAUDE_CODE_BRIDGE_SESSION_ID") {
        body["rc_session_id"] = json!(rc.trim());
    }
    body.to_string()
}

fn deliver(session: &str, event: &str, payload: &Value) {
    let body = envelope(session, event, payload);
    let sent = env_nonempty("CTOWER_URL").is_some_and(|url| post(&url, &body));
    if !sent {
        let _ = spool(&body);
    }
}

/// Espera máxima pela resposta da UI a um pedido de permissão: acima da do
/// daemon (2 min) e abaixo do `timeout` do hook no Claude Code (3 min).
const PERMISSION_TIMEOUT: Duration = Duration::from_secs(150);

/// Mensagem ao agente quando a UI nega sem texto.
const DENIED: &str = "[Control Tower] Negado pelo usuário.";

/// `PermissionRequest` (#328): pergunta ao daemon, que espera a resposta da
/// UI. Sem resposta (daemon fora, prazo, nenhuma UI, "responder no
/// terminal") não imprime nada, e o Claude Code mostra o menu de sempre.
/// Nunca bloqueia: sai sempre com 0.
fn permission_request(session: &str, payload: &Value) -> i32 {
    let body = envelope(session, "PermissionRequest", payload);
    let Some(url) = env_nonempty("CTOWER_URL") else {
        return 0;
    };
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(POST_TIMEOUT)
        .timeout(PERMISSION_TIMEOUT)
        .build();
    let mut req = agent
        .post(&format!("{}/hooks/permission", url.trim_end_matches('/')))
        .set("Content-Type", "application/json");
    if let Some(t) = env_nonempty("CTOWER_TOKEN") {
        req = req.set("Authorization", &format!("Bearer {t}"));
    }
    let answer: Value = match req.send_string(&body).ok().and_then(|r| r.into_json().ok()) {
        Some(v) => v,
        None => return 0,
    };
    if let Some(out) = permission_output(payload, &answer) {
        println!("{out}");
    }
    0
}

/// Saída do hook para a resposta do daemon (`{decision, message}`); `None`
/// = sem decisão.
fn permission_output(payload: &Value, answer: &Value) -> Option<Value> {
    let decision = match answer.get("decision").and_then(Value::as_str)? {
        "allow" => json!({ "behavior": "allow" }),
        "allow_session" => json!({
            "behavior": "allow",
            "updatedPermissions": session_rules(payload),
        }),
        "deny" => json!({
            "behavior": "deny",
            "message": answer
                .get("message")
                .and_then(Value::as_str)
                .filter(|m| !m.trim().is_empty())
                .unwrap_or(DENIED),
        }),
        _ => return None,
    };
    Some(json!({
        "hookSpecificOutput": {
            "hookEventName": "PermissionRequest",
            "decision": decision,
        }
    }))
}

/// "Sempre nesta sessão": as sugestões do próprio Claude Code
/// (`permission_suggestions`), sempre com destino `session`; sem elas, uma
/// regra para a ferramenta (no Bash, só o comando exato).
fn session_rules(payload: &Value) -> Value {
    if let Some(s) = payload
        .get("permission_suggestions")
        .and_then(Value::as_array)
        .filter(|s| !s.is_empty())
    {
        let rules: Vec<Value> = s
            .iter()
            .cloned()
            .map(|mut r| {
                if r.get("destination").is_some() {
                    r["destination"] = json!("session");
                }
                r
            })
            .collect();
        return json!(rules);
    }
    let tool = payload
        .get("tool_name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let mut rule = json!({ "toolName": tool });
    if tool == "Bash" {
        if let Some(c) = payload
            .pointer("/tool_input/command")
            .and_then(Value::as_str)
        {
            rule["ruleContent"] = json!(c);
        }
    }
    json!([{
        "type": "addRules",
        "rules": [rule],
        "behavior": "allow",
        "destination": "session",
    }])
}

/// Prazo total de entrega de um evento ao daemon.
const POST_TIMEOUT: Duration = Duration::from_secs(1);

fn post(base: &str, body: &str) -> bool {
    // O `timeout_connect` do ureq 2 vale 30 s por padrão e tem precedência
    // sobre `timeout` na conexão: sem fixá-lo, um destino que descarta o SYN
    // (firewall, host fora do ar) trava o hook — e o agente — por 30 s.
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(POST_TIMEOUT)
        .timeout(POST_TIMEOUT)
        .build();
    let mut req = agent
        .post(&format!("{}/hooks", base.trim_end_matches('/')))
        .set("Content-Type", "application/json");
    if let Some(t) = env_nonempty("CTOWER_TOKEN") {
        req = req.set("Authorization", &format!("Bearer {t}"));
    }
    req.send_string(body).is_ok()
}

/// Diretório do spool: `CTOWER_SPOOL_DIR` ou `$HOME/.local/share/...`. Sem
/// nenhum dos dois, não há spool (nunca grava relativo ao cwd/worktree).
fn spool_dir() -> Option<PathBuf> {
    if let Some(d) = env_nonempty("CTOWER_SPOOL_DIR") {
        return Some(PathBuf::from(d));
    }
    let home = std::env::var_os("HOME").filter(|h| !h.is_empty())?;
    Some(PathBuf::from(home).join(".local/share/ctower/hooks-spool"))
}

/// Grava o evento no spool, privado ao usuário (diretório 0700, arquivo 0600).
fn spool(body: &str) -> std::io::Result<()> {
    let Some(dir) = spool_dir() else {
        return Ok(());
    };
    DirBuilder::new().recursive(true).mode(0o700).create(&dir)?;
    let name = format!("{}-{}.json", now_ms(), std::process::id());
    let tmp = dir.join(format!(".{name}.tmp"));
    let mut f = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&tmp)?;
    if let Err(e) = f.write_all(body.as_bytes()) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    drop(f);
    std::fs::rename(tmp, dir.join(name))
}
