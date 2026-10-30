//! Grava em `CTOWER_COMMIT` o commit do build (`git rev-parse HEAD`). Sem git
//! (ex.: tarball), fica vazio. `CTOWER_COMMIT` no ambiente do build sobrepõe.

use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
}

fn main() {
    println!("cargo:rerun-if-env-changed=CTOWER_COMMIT");
    let commit = match std::env::var("CTOWER_COMMIT") {
        Ok(c) => c,
        Err(_) => {
            // Recompila quando o HEAD ou a ref para onde ele aponta mudam
            // (com worktrees, `--git-path` resolve o diretório comum).
            for p in ["HEAD", "packed-refs"] {
                if let Some(path) = git(&["rev-parse", "--git-path", p]) {
                    println!("cargo:rerun-if-changed={path}");
                }
            }
            if let Some(r) = git(&["symbolic-ref", "-q", "HEAD"]) {
                if let Some(path) = git(&["rev-parse", "--git-path", &r]) {
                    println!("cargo:rerun-if-changed={path}");
                }
            }
            git(&["rev-parse", "HEAD"]).unwrap_or_default()
        }
    };
    println!("cargo:rustc-env=CTOWER_COMMIT={commit}");
}
