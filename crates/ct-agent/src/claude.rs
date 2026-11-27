//! Runtime do Claude Code: argv (`--session-id`/`--resume`, `--add-dir`,
//! `--mcp-config`, `--settings`, ...), `mcp.json` e `settings.json` (hooks e
//! atribuição) da sessão.

use crate::{AgentRuntime, Conversation, LaunchContext, LaunchPlan, PlanFile};
use ct_core::{SessionMode, Tool, CLAUDE_EFFORTS};
use serde_json::{json, Value};

/// O Claude Code como [`AgentRuntime`].
#[derive(Debug, Clone, Copy, Default)]
pub struct ClaudeRuntime;

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

/// Instrução do modo `subagents`, somada ao system prompt do Claude Code.
pub const SUBAGENTS_PROMPT: &str =
    "Modo subagents (Control Tower): delegue cada tarefa do trabalho \
a um subagente (ferramenta Agent), com contexto suficiente para executá-la sozinho, em paralelo \
quando as tarefas forem independentes. Nesta sessão, planeje, coordene, revise o que os \
subagentes entregarem e integre o resultado.";

/// Nomes dos arquivos da sessão, na pasta dela.
pub const MCP_FILE: &str = "mcp.json";
pub const SETTINGS_FILE: &str = "settings.json";

impl AgentRuntime for ClaudeRuntime {
    fn tool(&self) -> Tool {
        Tool::Claude
    }

    fn launch_plan(&self, ctx: &LaunchContext<'_>) -> LaunchPlan {
        let mcp_path = ctx.session_dir.join(MCP_FILE);
        let settings_path = ctx.session_dir.join(SETTINGS_FILE);
        let files = vec![
            PlanFile {
                path: mcp_path.clone(),
                contents: mcp_config(ctx.port, ctx.hook_token, ctx.session_id)
                    .to_string()
                    .into_bytes(),
            },
            PlanFile {
                path: settings_path.clone(),
                contents: session_settings(ctx.ctower_bin, ctx.attribution)
                    .to_string()
                    .into_bytes(),
            },
        ];

        let mut args = match ctx.conversation {
            Conversation::New(id) => vec!["--session-id".to_string(), id.to_string()],
            Conversation::Resume(id) => vec!["--resume".to_string(), id.to_string()],
        };
        // Antes de outra opção: `--add-dir` aceita vários valores e
        // engoliria o prompt posicional.
        for dir in ctx.extra_dirs {
            args.extend(["--add-dir".to_string(), dir.to_string_lossy().to_string()]);
        }
        args.extend([
            "--mcp-config".to_string(),
            mcp_path.to_string_lossy().to_string(),
            "--settings".to_string(),
            settings_path.to_string_lossy().to_string(),
        ]);
        if let Some(name) = ctx.remote_control {
            args.extend(["--remote-control".to_string(), name.to_string()]);
        }
        if let Some(m) = ctx.model {
            args.extend(["--model".to_string(), m.to_string()]);
        }
        if let Some(e) = ctx.effort {
            args.extend(["--effort".to_string(), e.to_string()]);
        }
        // Um só `--append-system-prompt`, com o do modo `subagents` por último.
        let mut append = ctx.system_prompt.to_vec();
        if ctx.mode == SessionMode::Subagents {
            append.push(SUBAGENTS_PROMPT);
        }
        if !append.is_empty() {
            args.extend(["--append-system-prompt".to_string(), append.join("\n\n")]);
        }
        if let Some(p) = ctx.prompt {
            args.push(p.to_string());
        }

        LaunchPlan {
            program: ctx.program.to_string(),
            args,
            env: Vec::new(),
            files,
        }
    }
}

/// Esforço (`--effort`) do argv de uma sessão já lançada, para a retomada
/// seguir com ele (#182): o ledger guarda o modelo, não o esforço.
pub fn effort_from_argv(argv: &[String]) -> Option<String> {
    argv.windows(2)
        .find(|w| w[0] == "--effort")
        .map(|w| w[1].clone())
        .filter(|e| CLAUDE_EFFORTS.contains(&e.as_str()))
}

/// `mcp.json` da sessão: servidor HTTP do ledger com os cabeçalhos de autenticação.
pub fn mcp_config(port: u16, hook_token: &str, session_id: &str) -> Value {
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

/// `settings.json` da sessão: os hooks e, com a atribuição desligada no
/// projeto (spec do Jira e GitLab §9.3), as chaves do Claude Code que tiram o
/// `Co-Authored-By` dos commits e o rodapé dos PRs/MRs. `attribution` com
/// texto vazio esconde a atribuição (Claude Code 2.1); `includeCoAuthoredBy`
/// é a chave antiga, ainda lida, para versões anteriores.
pub fn session_settings(ctower_bin: &str, attribution: bool) -> Value {
    let mut s = hook_settings(ctower_bin);
    if !attribution {
        s["attribution"] = json!({ "commit": "", "pr": "" });
        s["includeCoAuthoredBy"] = json!(false);
    }
    s
}

/// `settings.json` da sessão com os hooks do Control Tower (passado em `--settings`).
pub fn hook_settings(ctower_bin: &str) -> Value {
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
    use std::path::{Path, PathBuf};

    fn ctx<'a>(conversation: Conversation<'a>) -> LaunchContext<'a> {
        LaunchContext {
            program: "/usr/bin/claude",
            session_id: "sess-1",
            session_dir: Path::new("/run/ct/sessions/sess-1"),
            conversation,
            port: 4317,
            hook_token: "tok",
            ctower_bin: "/usr/local/bin/ctower",
            attribution: true,
            extra_dirs: &[],
            remote_control: None,
            model: None,
            effort: None,
            mode: SessionMode::Native,
            system_prompt: &[],
            prompt: None,
        }
    }

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn conversa_nova_minima() {
        let plan = ClaudeRuntime.launch_plan(&ctx(Conversation::New("conv-1")));
        assert_eq!(plan.program, "/usr/bin/claude");
        assert_eq!(
            plan.args,
            s(&[
                "--session-id",
                "conv-1",
                "--mcp-config",
                "/run/ct/sessions/sess-1/mcp.json",
                "--settings",
                "/run/ct/sessions/sess-1/settings.json",
            ])
        );
        assert!(plan.env.is_empty());
        assert_eq!(plan.argv()[0], "/usr/bin/claude");
        assert_eq!(&plan.argv()[1..], plan.args.as_slice());
    }

    #[test]
    fn retomada_usa_resume() {
        let plan = ClaudeRuntime.launch_plan(&ctx(Conversation::Resume("conv-9")));
        assert_eq!(plan.args[..2].to_vec(), s(&["--resume", "conv-9"]));
    }

    #[test]
    fn todas_as_opcoes_na_ordem() {
        let dirs = [PathBuf::from("/docs/proj")];
        let prompt = ["Regras", "Docs"];
        let c = LaunchContext {
            extra_dirs: &dirs,
            remote_control: Some("demo #7"),
            model: Some("opus"),
            effort: Some("high"),
            mode: SessionMode::Subagents,
            system_prompt: &prompt,
            prompt: Some("faça a issue"),
            ..ctx(Conversation::New("conv-1"))
        };
        let plan = ClaudeRuntime.launch_plan(&c);
        let append = format!("Regras\n\nDocs\n\n{SUBAGENTS_PROMPT}");
        assert_eq!(
            plan.args,
            s(&[
                "--session-id",
                "conv-1",
                "--add-dir",
                "/docs/proj",
                "--mcp-config",
                "/run/ct/sessions/sess-1/mcp.json",
                "--settings",
                "/run/ct/sessions/sess-1/settings.json",
                "--remote-control",
                "demo #7",
                "--model",
                "opus",
                "--effort",
                "high",
                "--append-system-prompt",
                append.as_str(),
                "faça a issue",
            ])
        );
    }

    #[test]
    fn so_o_modo_subagents_tambem_vai_no_system_prompt() {
        let c = LaunchContext {
            mode: SessionMode::Subagents,
            ..ctx(Conversation::New("c"))
        };
        let plan = ClaudeRuntime.launch_plan(&c);
        let n = plan.args.len();
        assert_eq!(
            plan.args[n - 2..].to_vec(),
            s(&["--append-system-prompt", SUBAGENTS_PROMPT])
        );
    }

    #[test]
    fn arquivos_mcp_e_settings_na_pasta_da_sessao() {
        let c = LaunchContext {
            attribution: false,
            ..ctx(Conversation::New("c"))
        };
        let plan = ClaudeRuntime.launch_plan(&c);
        assert_eq!(plan.files.len(), 2);
        let mcp = &plan.files[0];
        assert_eq!(mcp.path, Path::new("/run/ct/sessions/sess-1/mcp.json"));
        let v: Value = serde_json::from_slice(&mcp.contents).unwrap();
        assert_eq!(v, mcp_config(4317, "tok", "sess-1"));
        let srv = &v["mcpServers"]["ctower"];
        assert_eq!(srv["url"], "http://127.0.0.1:4317/mcp");
        assert_eq!(srv["headers"]["Authorization"], "Bearer tok");
        assert_eq!(srv["headers"]["X-Ctower-Session"], "sess-1");

        let settings = &plan.files[1];
        assert_eq!(
            settings.path,
            Path::new("/run/ct/sessions/sess-1/settings.json")
        );
        let v: Value = serde_json::from_slice(&settings.contents).unwrap();
        assert_eq!(v, session_settings("/usr/local/bin/ctower", false));
        assert_eq!(v["includeCoAuthoredBy"], false);
    }

    #[test]
    fn esforco_lido_do_argv() {
        assert_eq!(
            effort_from_argv(&s(&["claude", "--effort", "high", "x"])),
            Some("high".into())
        );
        assert_eq!(effort_from_argv(&s(&["claude", "--effort", "turbo"])), None);
        assert_eq!(effort_from_argv(&s(&["claude"])), None);
    }
}
