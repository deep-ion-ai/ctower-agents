//! `[logs]` do `config.toml` do `ctowerd` e do `cthubd` (#65): o arquivo de
//! log em JSONL, com rotação por tamanho, que a tela Logs da UI lê.

use serde::{Deserialize, Serialize};

/// Arquivo de log do daemon. O stderr (journald/launchd) continua igual.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct LogsConfig {
    /// Pasta dos arquivos; vazio = padrão (`~/.local/state/ctower/logs`,
    /// `~/Library/Logs/ctower` no macOS ou `<data>/logs` com a pasta de
    /// dados trocada por variável de ambiente).
    pub dir: String,
    /// Tamanho de cada arquivo antes de rotacionar, em MB; `0` desliga o
    /// arquivo (fica só o stderr).
    pub max_file_mb: u64,
    /// Arquivos guardados, contando o atual (`<daemon>.jsonl`, `.1` … `.N-1`).
    pub max_files: u32,
}

impl Default for LogsConfig {
    fn default() -> Self {
        LogsConfig {
            dir: String::new(),
            max_file_mb: 10,
            max_files: 5,
        }
    }
}
