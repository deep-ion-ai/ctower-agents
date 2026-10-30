//! Store cifrado de secrets e variáveis do Control Tower (issue #164, épico
//! #171). O `cthubd` guarda tudo em `<config>/secrets.age` (0600), cifrado
//! com [age] (X25519) para a chave do hub (`<config>/secrets.key`, 0600) e,
//! quando configurada (#169), também para a chave pública de recuperação.
//! O `ctowerd` reaproveita o crate no cache cifrado do ambiente (#184):
//! veja [`EnvCache`] (chave `env.key`, cache `secrets-cache.age`) e
//! [`seal_delivery`] (o que o hub manda em `GET /sync/config`).
//!
//! - Entradas `{escopo, nome, tipo, valor, rev}`: escopo global ou de um
//!   projeto (`owner/repo`); tipo secret ou variável; nome válido como
//!   variável de ambiente (`[A-Z_][A-Z0-9_]*`) e fora dos reservados
//!   ([`is_reserved_name`]: o hub recusa, o `ctowerd` ignora no spawn).
//! - O `rev` vem de quem chama (no hub, o `config_rev` do `hub.db`): o crate
//!   não conhece o ledger. O store guarda o maior `rev` aplicado
//!   (`store_rev`), que também sobe nas remoções.
//! - Escrita atômica (temporário, `fsync`, `rename`): uma falha no meio
//!   deixa o arquivo anterior intacto.
//! - Valores ficam em [`SecretValue`], que não tem `Display` e cujo `Debug`
//!   imprime `***`; os erros ([`SecretsError`]) nunca carregam valores, só
//!   nomes e caminhos.
//!
//! [age]: https://age-encryption.org

mod env;
mod keys;
mod store;
mod value;

pub use age::x25519::{Identity, Recipient};
pub use env::{decode_delivery, seal_delivery, EnvCache, EnvSecrets, Rotated, DELIVERY_FORMAT};
pub use keys::{
    decrypt, encrypt, ensure_identity, identity_text, parse_identity, read_identity,
    read_recipient, write_identity,
};
pub use store::{
    is_hub_only, is_reserved_name, resolve_entries, validate_name, Entry, Kind, Removal, Scope,
    SecretsStore, FORMAT_VERSION, HUB_ONLY_PREFIX, RESERVED_NAMES, RESERVED_PREFIXES,
};
pub use value::SecretValue;
pub use zeroize::Zeroizing;

use std::path::PathBuf;

pub type Result<T> = std::result::Result<T, SecretsError>;

/// Tamanho mínimo (em bytes) de um secret para ser mascarado na saída das
/// sessões e no backup de transcript (#167): valores mais curtos
/// mascarariam texto comum. A UI do hub avisa ao cadastrar um secret menor
/// (cópia em `ui/src/hub/HubSecrets.tsx`). Variáveis não são mascaradas.
pub const MIN_MASK_LEN: usize = 8;

/// Erros do store. Nenhuma variante carrega um valor (nem trecho do
/// plaintext): só nomes, caminhos e posições.
#[derive(Debug, thiserror::Error)]
pub enum SecretsError {
    #[error("nome inválido: \"{0}\" (use [A-Z_][A-Z0-9_]*, como variável de ambiente)")]
    InvalidName(String),
    #[error("projeto inválido: \"{0}\" (use owner/repo)")]
    InvalidProject(String),
    #[error("{}: {source}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    /// Arquivo de chave ilegível. O conteúdo não entra na mensagem.
    #[error("{}: chave inválida ({reason})", path.display())]
    InvalidKey { path: PathBuf, reason: &'static str },
    #[error("falha ao cifrar: {0}")]
    Encrypt(String),
    #[error("falha ao decifrar: {0}")]
    Decrypt(String),
    /// Plaintext decifrado, mas fora do formato. Só a posição: a mensagem
    /// do serde pode citar o trecho (e o valor) que não bateu.
    #[error("conteúdo do store inválido (linha {line}, coluna {column})")]
    Format { line: usize, column: usize },
    #[error("versão de formato do store desconhecida: {0} (esperada {FORMAT_VERSION})")]
    UnknownFormat(u32),
}

impl SecretsError {
    pub(crate) fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        SecretsError::Io {
            path: path.into(),
            source,
        }
    }
}
