//! Processos externos dos provedores sem rede própria (spec do Jira e
//! GitLab §6.1, §7.2, §8.1): o jira-cli (`ct-jira`), o glab (`ct-gitlab`) e o
//! comando de credencial do projeto. Fica num crate à parte para os dois
//! clientes não dependerem um do outro.
//!
//! - [`process::run`]: processo com prazo, `stdin` nulo, sessão nova (sem
//!   TTY) e grupo próprio, morto inteiro no estouro;
//! - [`run_credential`]: o comando de credencial (`pass show ... | head -1`),
//!   cuja primeira linha vira um [`Secret`];
//! - [`Secret`]: senha ou token que nunca aparece em log nem em erro.

mod credential;
pub mod process;
mod secret;

pub use credential::{run_credential, CredentialError, CREDENTIAL_DEADLINE};
pub use secret::Secret;
