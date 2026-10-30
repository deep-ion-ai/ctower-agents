//! Ambiente WSL (spec §13): sobe a distro antes de conectar, lê o token de
//! UI de dentro dela e abre o VS Code remoto no worktree.

use std::process::Command;

/// Onde o daemon grava o token de UI (relativo ao `$HOME` da distro).
const TOKEN_PATH: &str = ".config/ctower/ui.token";

fn wsl(distro: &str) -> Command {
    let mut c = Command::new("wsl.exe");
    hide_console(&mut c);
    c.args(["-d", distro]);
    c
}

#[cfg(windows)]
fn hide_console(c: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    c.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn hide_console(_: &mut Command) {}

fn run(mut c: Command) -> Result<Vec<u8>, String> {
    let out = c
        .output()
        .map_err(|e| format!("não foi possível executar: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(format!("{} ({})", err, out.status));
    }
    Ok(out.stdout)
}

/// `wsl -d <distro> --exec true`: garante a distro (e o serviço) de pé.
pub fn bootstrap(distro: &str) -> Result<(), String> {
    if !cfg!(windows) {
        return Ok(());
    }
    let mut c = wsl(distro);
    c.args(["--exec", "true"]);
    run(c).map(|_| ())
}

/// Token de UI lido da distro (no Windows) ou do `$HOME` local (app rodando
/// no próprio Linux, para desenvolvimento). Nunca é gravado pelo app.
pub fn read_token(distro: &str) -> Result<String, String> {
    let raw = if cfg!(windows) {
        let mut c = wsl(distro);
        c.args(["--exec", "sh", "-c", &format!("cat \"$HOME/{TOKEN_PATH}\"")]);
        run(c)?
    } else {
        let home = std::env::var("HOME").map_err(|_| "HOME não definido".to_string())?;
        std::fs::read(format!("{home}/{TOKEN_PATH}")).map_err(|e| e.to_string())?
    };
    parse_token(&raw)
}

/// Token do GitHub da distro, para o updater do app (releases de repositório
/// privado): o mesmo do daemon (`github.token`) ou o login do `gh`. Nunca vai
/// para o JavaScript. `None` = sem token (a busca segue sem autenticação).
pub fn read_github_token(distro: &str) -> Option<String> {
    const SCRIPT: &str =
        "cat \"$HOME/.config/ctower/github.token\" 2>/dev/null || gh auth token 2>/dev/null";
    let raw = if cfg!(windows) {
        let mut c = wsl(distro);
        c.args(["--exec", "sh", "-c", SCRIPT]);
        run(c).ok()?
    } else {
        let mut c = Command::new("sh");
        c.args(["-c", SCRIPT]);
        run(c).ok()?
    };
    parse_token(&raw).ok()
}

fn parse_token(raw: &[u8]) -> Result<String, String> {
    let t = String::from_utf8_lossy(raw).trim().to_string();
    if t.is_empty() || t.contains(char::is_whitespace) {
        return Err(format!("token de UI inválido em ~/{TOKEN_PATH}"));
    }
    Ok(t)
}

/// Programa e argumentos para abrir o VS Code remoto no worktree
/// (`code --remote wsl+<distro> <caminho>`; no Windows via `cmd /C`, porque
/// `code` é um `.cmd`).
pub fn vscode_command(distro: &str, path: &str) -> Result<(String, Vec<String>), String> {
    let ok = |s: &str| {
        !s.is_empty()
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || "/_.-~".contains(c))
    };
    if !ok(distro) || !ok(path) {
        return Err("distro ou caminho com caracteres não suportados".into());
    }
    let mut args = vec![
        "--remote".to_string(),
        format!("wsl+{distro}"),
        path.to_string(),
    ];
    if cfg!(windows) {
        args.splice(0..0, ["/C".to_string(), "code".to_string()]);
        Ok(("cmd".into(), args))
    } else {
        Ok(("code".into(), args))
    }
}

/// Abre o VS Code sem esperar ele terminar.
pub fn open_vscode(distro: &str, path: &str) -> Result<(), String> {
    let (prog, args) = vscode_command(distro, path)?;
    let mut c = Command::new(prog);
    hide_console(&mut c);
    c.args(args)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("não foi possível abrir o VS Code: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_e_aparado_e_validado() {
        assert_eq!(parse_token(b"abc123\n").unwrap(), "abc123");
        assert!(parse_token(b"  \n").is_err());
        assert!(parse_token(b"a b").is_err());
    }

    #[test]
    fn comando_do_vscode() {
        let (prog, args) =
            vscode_command("control-tower", "/home/a/worktrees/ct/feat__31-x").unwrap();
        let tail = &args[args.len() - 3..];
        assert_eq!(
            tail,
            [
                "--remote",
                "wsl+control-tower",
                "/home/a/worktrees/ct/feat__31-x"
            ]
        );
        if cfg!(windows) {
            assert_eq!(prog, "cmd");
            assert_eq!(&args[..2], ["/C", "code"]);
        } else {
            assert_eq!(prog, "code");
        }
        assert!(vscode_command("ct", "/x & calc").is_err());
        assert!(vscode_command("ct\" x", "/x").is_err());
    }
}
