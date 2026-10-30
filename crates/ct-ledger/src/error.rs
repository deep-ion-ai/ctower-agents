use ct_core::{ParseEnumError, SessionState};

#[derive(Debug, thiserror::Error)]
pub enum LedgerError {
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    #[error("{0} não encontrado: {1}")]
    NotFound(&'static str, String),
    #[error("transição inválida de sessão: {from} -> {to}")]
    InvalidTransition {
        from: SessionState,
        to: SessionState,
    },
    #[error("dado inválido no banco: {0}")]
    Corrupt(String),
    /// Operação recusada pelo estado atual (ex.: excluir projeto com sessão viva).
    #[error("{0}")]
    Conflict(String),
}

impl From<ParseEnumError> for LedgerError {
    fn from(e: ParseEnumError) -> Self {
        LedgerError::Corrupt(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, LedgerError>;
