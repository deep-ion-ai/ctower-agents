//! Runtimes de agente (épico #225): o que é específico de cada agente
//! (hoje só o Claude Code) fica atrás de [`AgentRuntime`].
//!
//! O runtime não escreve arquivos nem cria processos: devolve um
//! [`LaunchPlan`] declarativo (programa, argv, ambiente e arquivos). Quem
//! grava os arquivos, junta o ambiente com os secrets e as `CTOWER_*` e faz
//! o spawn no PTY continua sendo o `Sessions` (`ct-sessions`).

pub mod claude;

pub use claude::ClaudeRuntime;

use ct_core::{SessionMode, Tool};
use std::path::{Path, PathBuf};

/// Conversa do agente, pelo id dela na ferramenta: nova com id fixo, ou
/// retomada.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Conversation<'a> {
    New(&'a str),
    Resume(&'a str),
}

impl<'a> Conversation<'a> {
    /// Id da conversa na ferramenta (`tool_session_id` no ledger).
    pub fn id(&self) -> &'a str {
        match self {
            Conversation::New(id) | Conversation::Resume(id) => id,
        }
    }
}

/// Tudo o que o runtime precisa para montar o lançamento de uma sessão. Os
/// valores já vêm resolvidos pelo `Sessions` (regras, docs, secrets,
/// credenciais no prompt); o runtime só os traduz para o agente.
#[derive(Debug, Clone)]
pub struct LaunchContext<'a> {
    /// Binário do agente (`claude_bin` da configuração).
    pub program: &'a str,
    /// Id da sessão no Control Tower (cabeçalho do MCP).
    pub session_id: &'a str,
    /// Pasta privada da sessão, onde ficam os arquivos do plano.
    pub session_dir: &'a Path,
    pub conversation: Conversation<'a>,
    /// Porta HTTP do daemon (servidor MCP do ledger).
    pub port: u16,
    /// Token dos hooks e do MCP.
    pub hook_token: &'a str,
    /// Binário chamado pelos hooks (`ctower`).
    pub ctower_bin: &'a str,
    /// Atribuição do agente nos commits e PRs/MRs (`attribution` do projeto).
    pub attribution: bool,
    /// Pastas fora do cwd a que o agente tem acesso (a de docs, #441).
    pub extra_dirs: &'a [PathBuf],
    /// Nome da sessão no Remote Control; `None` = desligado.
    pub remote_control: Option<&'a str>,
    pub model: Option<&'a str>,
    pub effort: Option<&'a str>,
    pub mode: SessionMode,
    /// Trechos somados ao system prompt, na ordem.
    pub system_prompt: &'a [&'a str],
    /// Prompt inicial (posicional, por último); `None` = nenhum.
    pub prompt: Option<&'a str>,
}

/// Arquivo que o `Sessions` grava (privado, `0600`) antes do spawn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanFile {
    pub path: PathBuf,
    pub contents: Vec<u8>,
}

/// Como lançar o agente: o `Sessions` grava `files`, junta `env` com os
/// secrets e as `CTOWER_*` e faz o spawn de `program` com `args`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchPlan {
    pub program: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub files: Vec<PlanFile>,
}

impl LaunchPlan {
    /// `program` seguido de `args`, como fica no `launch_cmd` da sessão.
    pub fn argv(&self) -> Vec<String> {
        let mut argv = Vec::with_capacity(self.args.len() + 1);
        argv.push(self.program.clone());
        argv.extend(self.args.iter().cloned());
        argv
    }
}

/// Um agente que o Control Tower sabe lançar.
pub trait AgentRuntime: Send + Sync {
    /// Ferramenta do runtime (`sessions.tool` no ledger).
    fn tool(&self) -> Tool;

    /// Plano de lançamento de uma sessão. Puro: não toca disco nem processos.
    fn launch_plan(&self, ctx: &LaunchContext<'_>) -> LaunchPlan;
}

static CLAUDE: ClaudeRuntime = ClaudeRuntime;

/// Runtime registrado para a ferramenta; `None` = ferramenta sem runtime.
pub fn runtime(tool: Tool) -> Option<&'static dyn AgentRuntime> {
    match tool {
        Tool::Claude => Some(&CLAUDE),
        Tool::Codex => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registro_por_tool() {
        assert_eq!(runtime(Tool::Claude).unwrap().tool(), Tool::Claude);
        assert!(runtime(Tool::Codex).is_none());
    }

    #[test]
    fn id_da_conversa_nova_e_retomada() {
        assert_eq!(Conversation::New("a").id(), "a");
        assert_eq!(Conversation::Resume("b").id(), "b");
    }
}
