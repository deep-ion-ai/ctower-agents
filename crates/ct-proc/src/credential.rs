//! Comando de credencial do projeto (spec §8.1): `/bin/sh -c '<cmd>'`, com
//! `stdin` nulo, sem TTY e prazo de 10 s. Vale a primeira linha do
//! `stdout`, sem espaços nas pontas. O `stderr` (do `pass`, do `gpg`) não
//! entra em mensagem nenhuma.

use crate::process::{describe_status, run, RunError};
use crate::Secret;
use std::process::Command;
use std::time::Duration;
use zeroize::Zeroize;

/// Prazo do comando de credencial: corta o pinentry que ficaria esperando.
pub const CREDENTIAL_DEADLINE: Duration = Duration::from_secs(10);

/// Falha do comando de credencial. `what` é o provedor (`Jira`, `GitLab`);
/// `cmd` é o texto do cadastro (não é secret), citado para o usuário rodar
/// à mão.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CredentialError {
    #[error("comando de credencial do {what} não configurado no cadastro do projeto")]
    Missing { what: &'static str },
    #[error(
        "comando de credencial do {what} falhou ({status}): o cache do gpg-agent pode ter \
         expirado; rode \"{cmd}\" num terminal do ambiente"
    )]
    Failed {
        what: &'static str,
        status: String,
        cmd: String,
    },
    #[error(
        "comando de credencial do {what} não respondeu em {secs} s: o pinentry pode estar \
         pedindo a senha; rode \"{cmd}\" num terminal do ambiente"
    )]
    Timeout {
        what: &'static str,
        secs: u64,
        cmd: String,
    },
    #[error("comando de credencial do {what} não imprimiu nada")]
    Empty { what: &'static str },
    #[error("comando de credencial do {what}: {message}")]
    Io { what: &'static str, message: String },
}

/// Roda o comando e devolve a primeira linha da saída.
pub fn run_credential(
    what: &'static str,
    cmd: &str,
    deadline: Duration,
) -> Result<Secret, CredentialError> {
    if cmd.trim().is_empty() {
        return Err(CredentialError::Missing { what });
    }
    let mut c = Command::new("/bin/sh");
    c.arg("-c").arg(cmd);
    let mut out = match run(c, deadline) {
        Ok(o) => o,
        Err(RunError::Timeout(d)) => {
            return Err(CredentialError::Timeout {
                what,
                secs: d.as_secs().max(1),
                cmd: cmd.to_string(),
            })
        }
        Err(e) => {
            return Err(CredentialError::Io {
                what,
                message: e.to_string(),
            })
        }
    };
    out.stderr.zeroize();
    if !out.status.success() {
        out.stdout.zeroize();
        return Err(CredentialError::Failed {
            what,
            status: describe_status(&out.status),
            cmd: cmd.to_string(),
        });
    }
    let line = String::from_utf8_lossy(&out.stdout)
        .lines()
        .next()
        .map(|l| l.trim().to_string())
        .unwrap_or_default();
    out.stdout.zeroize();
    if line.is_empty() {
        return Err(CredentialError::Empty { what });
    }
    Ok(Secret::new(line))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn primeira_linha_sem_espacos() {
        let s = run_credential(
            "Jira",
            "printf '  senha-de-teste  \\nsegunda\\n'",
            CREDENTIAL_DEADLINE,
        )
        .unwrap();
        assert_eq!(s.expose(), "senha-de-teste");
    }

    #[test]
    fn pipe_do_usuario_funciona() {
        let s = run_credential("Jira", "printf 'a\\nb\\n' | head -1", CREDENTIAL_DEADLINE).unwrap();
        assert_eq!(s.expose(), "a");
    }

    #[test]
    fn falha_cita_o_comando_e_nao_a_saida() {
        let e = run_credential(
            "Jira",
            "printf 'senha-%s\\n' vazada; printf 'gpg: %s\\n' falhou >&2; exit 2",
            CREDENTIAL_DEADLINE,
        )
        .unwrap_err();
        let msg = e.to_string();
        assert!(msg.contains("falhou (saída 2)"), "{msg}");
        assert!(msg.contains("gpg-agent"), "{msg}");
        assert!(
            !msg.contains("senha-vazada") && !msg.contains("gpg: falhou"),
            "{msg}"
        );
    }

    #[test]
    fn prazo_estourado() {
        let start = Instant::now();
        let e = run_credential("Jira", "sleep 30", Duration::from_millis(300)).unwrap_err();
        assert!(matches!(e, CredentialError::Timeout { .. }), "{e:?}");
        assert!(e.to_string().contains("pinentry"), "{e}");
        assert!(start.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn vazio_e_ausente() {
        let e = run_credential("Jira", "true", CREDENTIAL_DEADLINE).unwrap_err();
        assert_eq!(e, CredentialError::Empty { what: "Jira" });
        let e = run_credential("Jira", "  ", CREDENTIAL_DEADLINE).unwrap_err();
        assert_eq!(e, CredentialError::Missing { what: "Jira" });
    }
}
