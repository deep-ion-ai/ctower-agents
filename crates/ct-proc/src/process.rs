//! Processo externo com prazo (spec §6.1, §8.1): `stdin` nulo, sessão nova
//! (sem terminal de controle, então o pinentry não consegue abrir o TTY) e
//! grupo próprio, morto inteiro no estouro. Serve ao jira-cli, ao glab e ao
//! comando de credencial.

use std::ffi::{OsStr, OsString};
use std::io::{self, Read, Write};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use crate::Secret;

/// Tamanho máximo da saída de um processo citada numa mensagem de erro.
const MAX_ERROR_CHARS: usize = 300;

#[derive(Debug)]
pub struct Output {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

#[derive(Debug, thiserror::Error)]
pub enum RunError {
    /// O binário não existe (ou não está no `PATH`).
    #[error("binário não encontrado")]
    NotFound,
    /// Estourou o prazo: o grupo do processo foi morto.
    #[error("prazo de {} s estourado", .0.as_secs())]
    Timeout(Duration),
    /// A saída passou do limite de [`Job::max_stdout`]: o grupo foi morto.
    #[error("saída maior que {0} bytes")]
    OutputTooLarge(usize),
    #[error("{0}")]
    Io(#[from] io::Error),
}

/// O que muda em volta do processo: diretório, variáveis, entrada e limite
/// da saída. O resto herda do ambiente do daemon.
#[derive(Default)]
pub struct Job<'a> {
    pub cwd: Option<&'a Path>,
    /// Variáveis a mais (ou sobrepostas).
    pub envs: Vec<(OsString, OsString)>,
    /// Bytes escritos no `stdin` (fechado ao fim). Sem eles, `stdin` é nulo.
    pub stdin: Option<&'a [u8]>,
    /// Limite do `stdout`: estourado, o grupo morre e sai
    /// [`RunError::OutputTooLarge`].
    pub max_stdout: Option<usize>,
}

/// Roda `program args...` com [`Job`] e prazo. É a forma de rodar um
/// programa sem montar o `Command` à mão.
pub fn exec(
    program: &OsStr,
    args: &[OsString],
    job: Job<'_>,
    deadline: Duration,
) -> Result<Output, RunError> {
    let mut cmd = Command::new(program);
    cmd.args(args);
    if let Some(dir) = job.cwd {
        cmd.current_dir(dir);
    }
    cmd.envs(job.envs.iter().map(|(k, v)| (k, v)));
    run_job(cmd, job.stdin, job.max_stdout, deadline)
}

/// Roda `cmd` até o fim ou até `deadline`, com `stdout` e `stderr`
/// capturados. Os filhos do comando que segurarem a saída aberta depois que
/// ele terminou contam no mesmo prazo.
pub fn run(cmd: Command, deadline: Duration) -> Result<Output, RunError> {
    run_job(cmd, None, None, deadline)
}

fn kill_group(group: libc::pid_t) {
    // SAFETY: só sinaliza o grupo criado por `setsid` em `run_job`.
    unsafe {
        libc::kill(-group, libc::SIGKILL);
    }
}

fn run_job(
    mut cmd: Command,
    stdin: Option<&[u8]>,
    max_stdout: Option<usize>,
    deadline: Duration,
) -> Result<Output, RunError> {
    cmd.stdin(if stdin.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    })
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());
    // SAFETY: `setsid` é async-signal-safe e não aloca; roda no filho entre
    // o `fork` e o `exec`.
    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = cmd.spawn().map_err(|e| match e.kind() {
        io::ErrorKind::NotFound | io::ErrorKind::PermissionDenied => RunError::NotFound,
        _ => RunError::Io(e),
    })?;
    // O pid do líder é o id do grupo (`setsid`).
    let group = child.id() as libc::pid_t;
    if let (Some(bytes), Some(mut input)) = (stdin, child.stdin.take()) {
        // Escrita numa thread à parte: se o filho não ler, o prazo mata o
        // grupo e a escrita falha sozinha (quebra de pipe).
        let bytes = bytes.to_vec();
        std::thread::spawn(move || {
            let _ = input.write_all(&bytes);
        });
    }

    let overflow = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::channel::<(bool, Vec<u8>)>();
    for (is_out, pipe) in [
        (
            true,
            child
                .stdout
                .take()
                .map(|p| Box::new(p) as Box<dyn Read + Send>),
        ),
        (
            false,
            child
                .stderr
                .take()
                .map(|p| Box::new(p) as Box<dyn Read + Send>),
        ),
    ] {
        let tx = tx.clone();
        let Some(mut pipe) = pipe else { continue };
        // Só o stdout tem limite (o stderr é pequeno por natureza).
        let limit = if is_out { max_stdout } else { None };
        let overflow = Arc::clone(&overflow);
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            match limit {
                Some(max) => {
                    let _ = (&mut pipe).take(max as u64 + 1).read_to_end(&mut buf);
                    if buf.len() > max {
                        overflow.store(true, Ordering::SeqCst);
                        kill_group(group);
                    }
                }
                None => {
                    let _ = pipe.read_to_end(&mut buf);
                }
            }
            let _ = tx.send((is_out, buf));
        });
    }
    drop(tx);

    let end = Instant::now() + deadline;
    let status = loop {
        if let Some(s) = child.try_wait()? {
            break s;
        }
        if Instant::now() >= end {
            kill_group(group);
            let _ = child.kill();
            let _ = child.wait();
            return Err(RunError::Timeout(deadline));
        }
        std::thread::sleep(Duration::from_millis(10));
    };

    let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
    for _ in 0..2 {
        let left = end.saturating_duration_since(Instant::now());
        match rx.recv_timeout(left) {
            Ok((true, b)) => stdout = b,
            Ok((false, b)) => stderr = b,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                // Um filho em segundo plano segura a saída: o grupo ainda
                // existe (é ele que segura), então o sinal não acerta outro.
                kill_group(group);
                return Err(RunError::Timeout(deadline));
            }
        }
    }
    if let Some(max) = max_stdout {
        if overflow.load(Ordering::SeqCst) {
            return Err(RunError::OutputTooLarge(max));
        }
    }
    Ok(Output {
        status,
        stdout,
        stderr,
    })
}

/// `saída N` ou `sinal N`, para as mensagens de erro.
pub fn describe_status(s: &ExitStatus) -> String {
    use std::os::unix::process::ExitStatusExt;
    match (s.code(), s.signal()) {
        (Some(c), _) => format!("saída {c}"),
        (None, Some(sig)) => format!("sinal {sig}"),
        _ => "saída desconhecida".to_string(),
    }
}

/// Saída de erro do processo citável numa mensagem: sem cores ANSI, sem o
/// `✗` do jira-cli, sem a senha (trocada por `***`), numa linha e com no
/// máximo 300 caracteres.
pub fn cite(text: &str, token: Option<&Secret>) -> String {
    let mut plain = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            // CSI: ESC [ ... letra final.
            if chars.peek() == Some(&'[') {
                chars.next();
                for f in chars.by_ref() {
                    if f.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            continue;
        }
        plain.push(c);
    }
    let plain = match token {
        Some(t) => t.redact(&plain),
        None => plain,
    };
    let one_line = plain
        .replace('✗', "")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    match one_line.char_indices().nth(MAX_ERROR_CHARS) {
        Some((i, _)) => format!("{}…", &one_line[..i]),
        None => one_line,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sh(script: &str) -> Command {
        let mut c = Command::new("/bin/sh");
        c.arg("-c").arg(script);
        c
    }

    #[test]
    fn captura_saidas_e_status() {
        let o = run(sh("echo ok; echo erro >&2; exit 3"), Duration::from_secs(5)).unwrap();
        assert_eq!(o.stdout, b"ok\n");
        assert_eq!(o.stderr, b"erro\n");
        assert_eq!(describe_status(&o.status), "saída 3");
    }

    #[test]
    fn stdin_nulo_e_sem_terminal() {
        let o = run(
            sh("cat; tty >/dev/null 2>&1 && echo tty || echo sem-tty"),
            Duration::from_secs(5),
        )
        .unwrap();
        assert_eq!(String::from_utf8_lossy(&o.stdout), "sem-tty\n");
    }

    #[test]
    fn prazo_mata_o_grupo() {
        let start = Instant::now();
        let e = run(sh("sleep 30 & sleep 30"), Duration::from_millis(300)).unwrap_err();
        assert!(matches!(e, RunError::Timeout(_)), "{e:?}");
        assert!(start.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn filho_em_segundo_plano_segurando_a_saida_conta_no_prazo() {
        let start = Instant::now();
        let e = run(sh("sleep 30 & echo pronto"), Duration::from_millis(300)).unwrap_err();
        assert!(matches!(e, RunError::Timeout(_)), "{e:?}");
        assert!(start.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn saida_citada_sem_cores_nem_segredo() {
        let t = Secret::new("senha-x");
        assert_eq!(
            cite("\u{1b}[0;31m✗\u{1b}[0m falhou\ncom senha-x", Some(&t)),
            "falhou com ***"
        );
        assert_eq!(cite(&"x".repeat(400), None).chars().count(), 301);
    }

    #[test]
    fn binario_ausente() {
        let e = run(Command::new("/nao/existe/jira"), Duration::from_secs(1)).unwrap_err();
        assert!(matches!(e, RunError::NotFound), "{e:?}");
    }

    #[test]
    fn exec_entrega_stdin_e_aplica_cwd_e_variaveis() {
        let job = Job {
            cwd: Some(Path::new("/")),
            envs: vec![(OsString::from("CT_TESTE"), OsString::from("ok"))],
            stdin: Some(b"entrada"),
            max_stdout: None,
        };
        let args = [
            OsString::from("-c"),
            OsString::from("pwd; echo $CT_TESTE; cat"),
        ];
        let o = exec(OsStr::new("/bin/sh"), &args, job, Duration::from_secs(5)).unwrap();
        assert_eq!(o.stdout, b"/\nok\nentrada");
    }

    #[test]
    fn saida_acima_do_limite_mata_e_falha() {
        let job = Job {
            max_stdout: Some(10),
            ..Default::default()
        };
        let args = [OsString::from("-c"), OsString::from("yes")];
        let start = Instant::now();
        let e = exec(OsStr::new("/bin/sh"), &args, job, Duration::from_secs(30)).unwrap_err();
        assert!(matches!(e, RunError::OutputTooLarge(10)), "{e:?}");
        assert!(start.elapsed() < Duration::from_secs(10));
    }
}
