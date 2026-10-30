//! Branches de issue concluída (#286): ponta local e remota, commits fora do
//! remoto, ancestralidade, remoção condicionada ao SHA e `fetch --prune`.
//! Tudo com um `origin` bare local; nenhum remoto de verdade.

use ct_git::{
    branch_sha, commits_not_on_remotes, delete_branch, fetch_prune, is_ancestor, remote_branch_sha,
    worktree_with_branch, GitError,
};
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::TempDir;

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

struct Clone {
    tmp: TempDir,
    repo: PathBuf,
    origin: PathBuf,
}

/// Clone em `develop` com `origin` (bare) e a branch `feat/1-x` enviada,
/// com um commit à frente de `develop`.
fn clone() -> Clone {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path().join("repo");
    let origin = tmp.path().join("origin.git");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "develop"]);
    git(&repo, &["config", "user.email", "t@t"]);
    git(&repo, &["config", "user.name", "t"]);
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "init"]);
    git(
        tmp.path(),
        &["init", "-q", "--bare", origin.to_str().unwrap()],
    );
    git(
        &repo,
        &["remote", "add", "origin", origin.to_str().unwrap()],
    );
    git(&repo, &["push", "-q", "-u", "origin", "develop"]);
    git(&repo, &["checkout", "-q", "-b", "feat/1-x"]);
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "trabalho"]);
    git(&repo, &["push", "-q", "-u", "origin", "feat/1-x"]);
    git(&repo, &["checkout", "-q", "develop"]);
    Clone { tmp, repo, origin }
}

#[test]
fn pontas_local_e_remota() {
    let c = clone();
    let local = branch_sha(&c.repo, "feat/1-x").unwrap().unwrap();
    assert_eq!(local, git(&c.repo, &["rev-parse", "feat/1-x"]));
    assert_eq!(
        remote_branch_sha(&c.repo, "origin", "feat/1-x").unwrap(),
        Some(local)
    );
    assert_eq!(branch_sha(&c.repo, "feat/2-y").unwrap(), None);
    assert_eq!(
        remote_branch_sha(&c.repo, "origin", "feat/2-y").unwrap(),
        None
    );
    // Nome que vira opção ou não é branch: erro, sem rodar nada.
    assert!(matches!(
        branch_sha(&c.repo, "--all"),
        Err(GitError::InvalidRef { .. })
    ));
    assert!(matches!(
        remote_branch_sha(&c.repo, "-o", "feat/1-x"),
        Err(GitError::InvalidRef { .. })
    ));
}

#[test]
fn commits_fora_do_remoto_e_ancestralidade() {
    let c = clone();
    assert_eq!(commits_not_on_remotes(&c.repo, "feat/1-x").unwrap(), 0);
    let enviado = branch_sha(&c.repo, "feat/1-x").unwrap().unwrap();
    git(&c.repo, &["checkout", "-q", "feat/1-x"]);
    git(&c.repo, &["commit", "-q", "--allow-empty", "-m", "local 1"]);
    git(&c.repo, &["commit", "-q", "--allow-empty", "-m", "local 2"]);
    git(&c.repo, &["checkout", "-q", "develop"]);
    assert_eq!(commits_not_on_remotes(&c.repo, "feat/1-x").unwrap(), 2);
    let tip = branch_sha(&c.repo, "feat/1-x").unwrap().unwrap();
    assert!(is_ancestor(&c.repo, &enviado, &tip).unwrap());
    assert!(is_ancestor(&c.repo, &tip, &tip).unwrap());
    assert!(!is_ancestor(&c.repo, &tip, &enviado).unwrap());
    // Commit desconhecido (head de PR nunca buscado): não é ancestral.
    let nada = "0123456789abcdef0123456789abcdef01234567";
    assert!(!is_ancestor(&c.repo, &tip, nada).unwrap());
}

#[test]
fn apaga_a_branch_so_no_sha_esperado() {
    let c = clone();
    let tip = branch_sha(&c.repo, "feat/1-x").unwrap().unwrap();
    let develop = git(&c.repo, &["rev-parse", "develop"]);
    // Alguém commitou depois da avaliação: o SHA não bate e nada sai.
    let err = delete_branch(&c.repo, "feat/1-x", &develop).unwrap_err();
    assert!(matches!(err, GitError::Failed { .. }), "{err}");
    assert_eq!(branch_sha(&c.repo, "feat/1-x").unwrap(), Some(tip.clone()));

    delete_branch(&c.repo, "feat/1-x", &tip).unwrap();
    assert_eq!(branch_sha(&c.repo, "feat/1-x").unwrap(), None);
    // O upstream sai junto (como no `git branch -D`); a remota fica.
    let cfg = Command::new("git")
        .arg("-C")
        .arg(&c.repo)
        .args(["config", "--get", "branch.feat/1-x.remote"])
        .output()
        .unwrap();
    assert!(!cfg.status.success());
    assert!(remote_branch_sha(&c.repo, "origin", "feat/1-x")
        .unwrap()
        .is_some());
}

#[test]
fn branch_em_uso_nao_sai() {
    let c = clone();
    // No clone principal.
    let develop = git(&c.repo, &["rev-parse", "develop"]);
    let e = worktree_with_branch(&c.repo, "develop").unwrap().unwrap();
    assert_eq!(e.branch.as_deref(), Some("develop"));
    let err = delete_branch(&c.repo, "develop", &develop).unwrap_err();
    assert!(err.to_string().contains("em uso"), "{err}");
    // Num worktree.
    let wt = c.tmp.path().join("wt");
    git(
        &c.repo,
        &["worktree", "add", "-q", wt.to_str().unwrap(), "feat/1-x"],
    );
    let tip = branch_sha(&c.repo, "feat/1-x").unwrap().unwrap();
    let err = delete_branch(&c.repo, "feat/1-x", &tip).unwrap_err();
    assert!(err.to_string().contains("em uso"), "{err}");
    assert!(branch_sha(&c.repo, "feat/1-x").unwrap().is_some());
    assert!(branch_sha(&c.repo, "develop").unwrap().is_some());
}

#[test]
fn fetch_prune_tira_a_remota_apagada() {
    let c = clone();
    git(&c.origin, &["branch", "-D", "feat/1-x"]);
    assert!(remote_branch_sha(&c.repo, "origin", "feat/1-x")
        .unwrap()
        .is_some());
    fetch_prune(&c.repo, "origin").unwrap();
    assert_eq!(
        remote_branch_sha(&c.repo, "origin", "feat/1-x").unwrap(),
        None
    );
    assert!(remote_branch_sha(&c.repo, "origin", "develop")
        .unwrap()
        .is_some());
    assert!(matches!(
        fetch_prune(&c.repo, "--all"),
        Err(GitError::InvalidRef { .. })
    ));
}
