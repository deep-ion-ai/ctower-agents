//! Runtimes de agente (épico #225): o que é específico de cada agente
//! (hoje só o Claude Code) fica atrás de [`AgentRuntime`].
//!
//! O runtime não escreve arquivos nem cria processos: devolve um
//! [`LaunchPlan`] declarativo (programa, argv, ambiente e arquivos). Quem
//! grava os arquivos, junta o ambiente com os secrets e as `CTOWER_*` e faz
//! o spawn no PTY continua sendo o `Sessions` (`ct-sessions`). O formato de
//! fora do processo também fica aqui (#241): a confiança nas pastas
//! ([`AgentRuntime::prepare_workspace`]), o layout das pastas de transcript
//! ([`AgentRuntime::transcript_dirs_for`]) e a leitura das linhas
//! ([`TranscriptReader`]).

pub mod claude;

pub use claude::ClaudeRuntime;

use ct_core::{SessionMode, Tool};
use serde::Serialize;
use std::io;
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

/// Onde o agente guarda o próprio estado neste ambiente. O `ctowerd` a
/// resolve no boot ([`AgentRuntime::home_from_env`]); os testes fixam pastas
/// temporárias.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentHome {
    /// Arquivo de configuração do agente em que
    /// [`AgentRuntime::prepare_workspace`] pré-aceita a confiança nas pastas
    /// (`None` = não mexe; o humano responde ao diálogo no terminal).
    pub config_file: Option<PathBuf>,
    /// Raiz dos transcripts do agente por pasta de trabalho; a limpeza apaga
    /// os do worktree removido (#285). `None` = nunca apaga.
    pub transcripts_root: Option<PathBuf>,
}

/// Uma fala da conversa: texto do usuário ou do agente, ou uma chamada de
/// ferramenta (`tool`, com o nome e o argumento principal).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct TranscriptEntry {
    /// `user` | `assistant` | `tool`.
    pub role: String,
    pub text: String,
    /// `timestamp` da linha (ISO 8601), se houver.
    pub ts: Option<String>,
}

/// Resposta de `session.context`: o uso da última resposta do agente
/// (fora de subagentes). `None` nos campos = ainda sem resposta no
/// transcript.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct ContextUsage {
    /// Tokens no contexto: entrada (com cache criado e lido) mais a saída
    /// da última resposta.
    pub tokens: Option<u64>,
    /// Modelo da última resposta (ex.: `claude-opus-5-5`).
    pub model: Option<String>,
    pub ts: Option<String>,
}

/// Leitura do formato do transcript do agente (#328). Recebe o fim do
/// arquivo, já mascarado e começando numa linha inteira.
pub trait TranscriptReader: Send + Sync {
    /// Falas da conversa principal, da mais antiga para a mais recente.
    fn entries(&self, data: &[u8]) -> Vec<TranscriptEntry>;

    /// Uso de contexto da última resposta do agente; `None` = nenhuma.
    fn last_usage(&self, data: &[u8]) -> Option<ContextUsage>;
}

/// Um agente que o Control Tower sabe lançar.
pub trait AgentRuntime: Send + Sync {
    /// Ferramenta do runtime (`sessions.tool` no ledger).
    fn tool(&self) -> Tool;

    /// Plano de lançamento de uma sessão. Puro: não toca disco nem processos.
    fn launch_plan(&self, ctx: &LaunchContext<'_>) -> LaunchPlan;

    /// [`AgentHome`] pelas variáveis do processo (`HOME` e as do agente).
    fn home_from_env(&self) -> AgentHome;

    /// Prepara as pastas da sessão antes do spawn: pré-aceita a confiança
    /// em `dirs` no `home.config_file`. Retorna as pastas que mudaram (vazio
    /// = nada a fazer, ou `config_file` ausente). Erro = nada foi alterado.
    fn prepare_workspace(&self, home: &AgentHome, dirs: &[&str]) -> io::Result<Vec<String>>;

    /// Pastas de transcript do agente da pasta de trabalho `cwd`, dentro de
    /// `transcripts_root`: a que o agente usa para o caminho e as que contêm
    /// `transcripts` (caminhos registrados pelos hooks das sessões nesse
    /// `cwd`). Não olha o disco: quem chama confere se existem.
    fn transcript_dirs_for(
        &self,
        transcripts_root: &Path,
        cwd: &str,
        transcripts: &[&Path],
    ) -> Vec<PathBuf>;

    /// Sessão (id na ferramenta) dona de uma entrada de uma pasta de
    /// [`Self::transcript_dirs_for`]; `None` = entrada desconhecida.
    fn transcript_entry_id<'a>(&self, name: &'a str, is_file: bool) -> Option<&'a str>;

    /// Leitor do formato do transcript.
    fn transcripts(&self) -> &dyn TranscriptReader;
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
