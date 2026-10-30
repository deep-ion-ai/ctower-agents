//! `origin_url` e `gh_clone` (com um `gh` falso que clona de um repositório local).

use ct_git::{gh_clone, origin_url, GitError};
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

fn git(dir: &Path, args: &[&str]) {
    let ok = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .status()
        .unwrap()
        .success();
    assert!(ok, "git {args:?}");
}

/// `gh` falso: `gh repo clone o/r <dest>` clona `src` e registra o GH_TOKEN.
fn fake_gh(dir: &Path, src: &Path) -> String {
    let bin = dir.join("gh");
    write_exec(
        &bin,
        &format!(
            "#!/bin/sh\necho \"$GH_TOKEN\" > {log}\ngit clone -q {src} \"$4\" && git -C \"$4\" remote set-url origin https://github.com/$3.git\n",
            log = dir.join("token.log").display(),
            src = src.display()
        ),
    );
    bin.to_string_lossy().into_owned()
}

/// Grava um script executável por um `sh` filho, e não por este processo.
///
/// Os testes rodam em threads do mesmo processo. Se este processo abrisse o
/// script para escrita, um `fork` concorrente de outro teste (git, gh) herdaria
/// o descritor até o seu `exec`, e executar o script nesse intervalo dá
/// ETXTBSY ("Text file busy") no Linux. Gravando no filho, o descritor de
/// escrita nunca existe aqui e não há o que herdar.
fn write_exec(path: &Path, body: &str) {
    let ok = Command::new("sh")
        .args([
            "-c",
            "printf '%s' \"$2\" > \"$1\" && chmod 755 \"$1\"",
            "sh",
        ])
        .arg(path)
        .arg(body)
        .status()
        .unwrap()
        .success();
    assert!(ok, "gravar {}", path.display());
}

#[test]
fn clona_com_token_e_le_o_origin() {
    let src = TempDir::new().unwrap();
    git(src.path(), &["init", "-q", "-b", "develop"]);
    git(
        src.path(),
        &[
            "-c",
            "user.email=t@t",
            "-c",
            "user.name=t",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "init",
        ],
    );
    let d = TempDir::new().unwrap();
    let gh = fake_gh(d.path(), src.path());
    let dest = d.path().join("projetos/demo");
    gh_clone(&gh, "deep-ion-ai", "demo", &dest, Some("tok-x")).unwrap();
    assert_eq!(
        origin_url(&dest).unwrap().as_deref(),
        Some("https://github.com/deep-ion-ai/demo.git")
    );
    let log = std::fs::read_to_string(d.path().join("token.log")).unwrap();
    assert_eq!(log.trim(), "tok-x");
    // Sem token, o gh usa o próprio login (GH_TOKEN vazio).
    gh_clone(&gh, "deep-ion-ai", "demo", &d.path().join("outro"), None).unwrap();
    let log = std::fs::read_to_string(d.path().join("token.log")).unwrap();
    assert_eq!(log.trim(), "");
}

#[test]
fn falhas() {
    let d = TempDir::new().unwrap();
    // Pasta que não é repositório git.
    assert!(origin_url(d.path()).is_err());
    // Repositório sem origin.
    git(d.path(), &["init", "-q"]);
    assert_eq!(origin_url(d.path()).unwrap(), None);
    // gh que falha.
    let err = gh_clone("false", "o", "r", &d.path().join("x"), None).unwrap_err();
    assert!(matches!(err, GitError::Failed { .. }), "{err}");
}

/// `clone_url` (#350): clone direto pela URL do cadastro, de qualquer host.
/// Aqui a "URL" é um caminho local; o `origin` fica sendo ela.
#[test]
fn clona_pela_url_do_cadastro() {
    let src = TempDir::new().unwrap();
    git(src.path(), &["init", "-q", "-b", "develop"]);
    git(
        src.path(),
        &[
            "-c",
            "user.email=t@t",
            "-c",
            "user.name=t",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "init",
        ],
    );
    let d = TempDir::new().unwrap();
    let dest = d.path().join("projetos/app");
    let url = src.path().to_str().unwrap();
    ct_git::clone_url(url, &dest).unwrap();
    assert_eq!(origin_url(&dest).unwrap().as_deref(), Some(url));

    // URL que o git não alcança: erro com o motivo, sem pedir senha.
    let err = ct_git::clone_url("https://127.0.0.1:9/g/r.git", &d.path().join("x")).unwrap_err();
    assert!(
        matches!(&err, GitError::Failed { args, .. } if args == "clone https://127.0.0.1:9/g/r.git"),
        "{err}"
    );
    // Opção no lugar da URL é recusada antes do git.
    assert!(matches!(
        ct_git::clone_url("--upload-pack=x", &d.path().join("y")),
        Err(GitError::InvalidRef { .. })
    ));
}
