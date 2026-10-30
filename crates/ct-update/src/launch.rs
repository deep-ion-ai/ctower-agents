//! Disparo do atualizador fora do serviço que ele reinicia (o restart
//! encerraria uma atualização rodando dentro dele): no systemd, numa unidade
//! transitória de usuário (que recusa uma segunda enquanto a primeira roda);
//! no launchd, num processo próprio com sessão nova (`setsid`), que o launchd
//! não leva junto ao reiniciar o serviço (#255). Usado pelo `ctower update`
//! (ctowerd) e pelo `hub.update.*` do cthubd (#421).

use crate::{run, Error, Result};
use std::path::Path;
use std::process::Command;

/// Como disparar o atualizador.
pub enum Launch<'a> {
    /// `systemd-run --user --unit=<unit>` com `pass_env` repassadas (a
    /// unidade não herda o ambiente de quem a cria).
    Systemd {
        systemd_run: &'a str,
        unit: &'a str,
        pass_env: &'a [&'a str],
    },
    /// Processo destacado (`setsid`), com stdio descartado: o atualizador
    /// grava estado e log em `<data>/update`.
    Detached,
}

/// Dispara `program args...`. Uma unidade com o mesmo nome ainda rodando
/// vira "uma atualização já está em andamento".
pub fn spawn(how: Launch, program: &Path, args: &[&str]) -> Result<()> {
    match how {
        Launch::Systemd {
            systemd_run,
            unit,
            pass_env,
        } => {
            let mut c = Command::new(systemd_run);
            c.args(["--user", &format!("--unit={unit}"), "--collect", "--quiet"]);
            for k in pass_env {
                if let Ok(v) = std::env::var(k) {
                    c.arg(format!("--setenv={k}={v}"));
                }
            }
            c.arg(program).args(args);
            run(&mut c).map(|_| ()).map_err(|e| {
                if e.0.contains("already") || e.0.contains("exists") {
                    Error("uma atualização já está em andamento".into())
                } else {
                    e
                }
            })
        }
        Launch::Detached => {
            use std::os::unix::process::CommandExt;
            use std::process::Stdio;
            let mut c = Command::new(program);
            c.args(args)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            // SAFETY: só chama setsid, que é async-signal-safe, entre fork e exec.
            unsafe {
                c.pre_exec(|| {
                    libc::setsid();
                    Ok(())
                });
            }
            c.spawn().map(|_| ()).map_err(|e| {
                Error(format!(
                    "não foi possível iniciar o atualizador ({}): {e}",
                    program.display()
                ))
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    /// Grava o script executável por um `sh` filho, e não por este processo:
    /// um `fork` concorrente de outro teste herdaria o descritor de escrita
    /// até o seu `exec`, e executar o script nesse intervalo dá ETXTBSY
    /// ("Text file busy") no Linux.
    fn exe(dir: &Path, name: &str, body: &str) -> std::path::PathBuf {
        let p = dir.join(name);
        let ok = Command::new("sh")
            .args([
                "-c",
                "printf '%s' \"$2\" > \"$1\" && chmod 755 \"$1\"",
                "sh",
            ])
            .arg(&p)
            .arg(format!("#!/bin/sh\n{body}\n"))
            .status()
            .unwrap()
            .success();
        assert!(ok, "gravar {}", p.display());
        p
    }

    #[test]
    fn destacado_roda_em_sessao_propria() {
        let d = tempfile::tempdir().unwrap();
        let out = d.path().join("out");
        // `ps -o sess=` não existe em todo lugar: compara o pgid com o pid
        // (líder da sessão nova também lidera o grupo).
        let prog = exe(
            d.path(),
            "prog",
            &format!(
                "echo \"$1 $$ $(ps -o pgid= -p $$ | tr -d ' ')\" > {}",
                out.display()
            ),
        );
        spawn(Launch::Detached, &prog, &["arg"]).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        let got = loop {
            if let Ok(s) = std::fs::read_to_string(&out) {
                if !s.is_empty() {
                    break s;
                }
            }
            assert!(Instant::now() < deadline, "o processo não rodou");
            std::thread::sleep(Duration::from_millis(20));
        };
        let v: Vec<&str> = got.split_whitespace().collect();
        assert_eq!(v[0], "arg");
        assert_eq!(v[1], v[2], "não virou líder de sessão: {got}");
    }

    #[test]
    fn unidade_repassa_variaveis_e_traduz_a_ja_existente() {
        let d = tempfile::tempdir().unwrap();
        let args = d.path().join("args");
        let run = exe(
            d.path(),
            "systemd-run",
            &format!("echo \"$@\" > {}", args.display()),
        );
        let how = Launch::Systemd {
            systemd_run: run.to_str().unwrap(),
            unit: "x-update",
            pass_env: &["HOME", "CT_UPDATE_TESTE_AUSENTE"],
        };
        spawn(how, Path::new("/bin/prog"), &["update", "--run"]).unwrap();
        let home = std::env::var("HOME").unwrap();
        assert_eq!(
            std::fs::read_to_string(&args).unwrap().trim(),
            format!(
                "--user --unit=x-update --collect --quiet --setenv=HOME={home} /bin/prog update --run"
            )
        );
        let busy = exe(
            d.path(),
            "busy",
            "echo 'Failed to start transient service unit: Unit x-update.service already exists.' >&2; exit 1",
        );
        let how = Launch::Systemd {
            systemd_run: busy.to_str().unwrap(),
            unit: "x-update",
            pass_env: &[],
        };
        let e = spawn(how, Path::new("/bin/prog"), &[]).unwrap_err();
        assert_eq!(e.0, "uma atualização já está em andamento");
    }
}
