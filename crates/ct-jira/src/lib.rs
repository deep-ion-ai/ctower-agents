//! Leitura do Jira pelo [jira-cli](https://github.com/ankitpokhrel/jira-cli)
//! (spec do Jira e GitLab §6, issue #349), sem rede própria: o `jira` roda
//! como processo externo, com prazo, sem TTY e com `stdin` nulo.
//!
//! - [`JiraCli`]: as chamadas (`version`, `me`, `serverinfo`, `issue list`,
//!   `issue view`), com a senha só no ambiente do processo filho
//!   (`JIRA_API_TOKEN`);
//! - [`run_credential`]: o comando de credencial do projeto (`pass show ... |
//!   head -1`), cuja primeira linha vira um [`Secret`];
//! - [`issue_row`]: o JSON da API v2 (`issue view --raw`) → linha do espelho,
//!   com as sprints ([`parse_sprints`]);
//! - [`description_markdown`]: a descrição da issue (wiki markup ou ADF) →
//!   Markdown, em memória, para o "Ver issue" (#434).
//!
//! O `--raw` do `issue list` não é o JSON da API: o jira-cli reserializa a
//! própria estrutura e perde a categoria do status e os campos do épico
//! (conferido com o jira-cli 1.7.0). Por isso a listagem só descobre as
//! chaves e o `updated`, e cada issue que mudou é lida com `issue view --raw`.

mod cli;
pub mod issue;
pub mod markdown;
pub mod sprint;

/// O processo com prazo, o comando de credencial e o [`Secret`] ficam no
/// `ct-proc`, comum ao glab (`ct-gitlab`); os caminhos de antes continuam.
pub use ct_proc::process;
pub use ct_proc::{run_credential, CredentialError, Secret, CREDENTIAL_DEADLINE};

#[cfg(any(test, feature = "test-support"))]
pub mod fake;

pub use cli::{
    parse_cli_version, server_version, JiraCli, JiraError, ListEvent, ListQuery, DEADLINE,
    DEFAULT_CONFIG, LIST_DEADLINE, LIST_RETRY_WAITS, PAGE_SIZE,
};
pub use issue::{issue_row, list_entry, parse_jira_time, ListEntry};
pub use markdown::description_markdown;
pub use sprint::{detect_sprint_field, issue_sprints, parse_sprints};
