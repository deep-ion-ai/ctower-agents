//! Branch de integração do modo autônomo (issue #181): criar no remoto a
//! partir da base e trazer a base para ela (merge ou rebase) antes de cada
//! issue autônoma.
//!
//! Nada aqui mexe na árvore de trabalho do clone principal (onde pode haver
//! uma sessão do projeto): o alinhamento roda num worktree temporário,
//! destacado, removido no fim mesmo em caso de erro.

use crate::{ref_exists, run, run_env, valid_branch, GitError};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Como trazer a base para a branch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Strategy {
    Merge,
    Rebase,
}

/// Resultado do alinhamento.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AlignOutcome {
    /// A base já estava contida na branch.
    UpToDate,
    /// A base entrou e a branch foi enviada ao remoto.
    Updated,
    /// Conflito: nada foi enviado; os arquivos em conflito.
    Conflict { files: Vec<String> },
}

const REMOTE: &str = "origin";

/// Busca `base` e `branch` do `origin` para `refs/remotes/origin/*`. A
/// branch pode não existir no remoto (não é erro).
fn fetch(repo: &Path, branch: &str, base: &str) -> Result<(), GitError> {
    run(
        repo,
        [
            "fetch",
            "--quiet",
            REMOTE,
            &format!("+refs/heads/{base}:refs/remotes/{REMOTE}/{base}"),
        ],
    )?;
    // Sem a branch no remoto o fetch falha: só some a referência local.
    if run(
        repo,
        [
            "fetch",
            "--quiet",
            REMOTE,
            &format!("+refs/heads/{branch}:refs/remotes/{REMOTE}/{branch}"),
        ],
    )
    .is_err()
    {
        let _ = run(
            repo,
            [
                "update-ref",
                "-d",
                &format!("refs/remotes/{REMOTE}/{branch}"),
            ],
        );
    }
    Ok(())
}

/// Cria `branch` no `origin` a partir de `origin/<base>`, se ela ainda não
/// existe lá. `true` = criou. A base precisa existir no remoto.
pub fn ensure_remote_branch(repo: &Path, branch: &str, base: &str) -> Result<bool, GitError> {
    valid_branch(branch)?;
    valid_branch(base)?;
    fetch(repo, branch, base)?;
    if ref_exists(repo, &format!("refs/remotes/{REMOTE}/{branch}"))? {
        return Ok(false);
    }
    run(
        repo,
        [
            "push",
            "--quiet",
            REMOTE,
            &format!("refs/remotes/{REMOTE}/{base}:refs/heads/{branch}"),
        ],
    )?;
    fetch(repo, branch, base)?;
    Ok(true)
}

/// Worktree temporário destacado, removido no `drop`.
struct TempWorktree {
    repo: PathBuf,
    path: PathBuf,
}

impl TempWorktree {
    fn add(repo: &Path, start: &str) -> Result<Self, GitError> {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "ct-align-{}-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        ));
        let p: OsString = path.clone().into_os_string();
        run(
            repo,
            [
                OsString::from("worktree"),
                "add".into(),
                "--detach".into(),
                "--quiet".into(),
                p,
                start.into(),
            ],
        )?;
        Ok(TempWorktree {
            repo: repo.to_path_buf(),
            path,
        })
    }
}

impl Drop for TempWorktree {
    fn drop(&mut self) {
        let _ = run(
            &self.repo,
            [
                OsString::from("worktree"),
                "remove".into(),
                "--force".into(),
                self.path.clone().into_os_string(),
            ],
        );
        let _ = std::fs::remove_dir_all(&self.path);
        let _ = run(&self.repo, ["worktree", "prune"]);
    }
}

/// Identidade do commit de merge: a do git do ambiente; sem ela, a do
/// Control Tower (para o merge não falhar por falta de `user.email`).
fn identity(repo: &Path) -> Vec<(&'static str, OsString)> {
    let has = |k: &str| {
        run(repo, ["config", "--get", k])
            .map(|v| !v.is_empty())
            .unwrap_or(false)
    };
    let mut env = Vec::new();
    if !has("user.name") {
        env.push(("GIT_AUTHOR_NAME", OsString::from("Control Tower")));
        env.push(("GIT_COMMITTER_NAME", OsString::from("Control Tower")));
    }
    if !has("user.email") {
        env.push(("GIT_AUTHOR_EMAIL", OsString::from("ctower@localhost")));
        env.push(("GIT_COMMITTER_EMAIL", OsString::from("ctower@localhost")));
    }
    env
}

/// Arquivos em conflito no worktree.
fn conflicted(dir: &Path) -> Vec<String> {
    run(dir, ["diff", "--name-only", "--diff-filter=U"])
        .map(|o| o.lines().map(String::from).collect())
        .unwrap_or_default()
}

/// Traz `origin/<base>` para `branch` no remoto. A branch precisa existir
/// no remoto ([`ensure_remote_branch`]). No conflito, desfaz tudo e não
/// envia nada.
pub fn align_branch(
    repo: &Path,
    branch: &str,
    base: &str,
    strategy: Strategy,
) -> Result<AlignOutcome, GitError> {
    valid_branch(branch)?;
    valid_branch(base)?;
    fetch(repo, branch, base)?;
    let remote_branch = format!("{REMOTE}/{branch}");
    let remote_base = format!("{REMOTE}/{base}");
    if !ref_exists(repo, &format!("refs/remotes/{remote_branch}"))? {
        return Err(GitError::Failed {
            args: format!("align {branch}"),
            stderr: format!("a branch {branch} não existe no remoto"),
        });
    }
    if crate::is_ancestor(repo, &remote_base, &remote_branch)? {
        return Ok(AlignOutcome::UpToDate);
    }
    let old = run(repo, ["rev-parse", "--verify", &remote_branch])?;
    let wt = TempWorktree::add(repo, &remote_branch)?;
    let env = identity(repo);
    let applied = match strategy {
        Strategy::Merge => run_env(
            &wt.path,
            [
                "merge",
                "--no-ff",
                "--no-edit",
                "-m",
                &format!("chore: alinha {branch} com {base}"),
                &remote_base,
            ],
            &env,
        ),
        Strategy::Rebase => run_env(&wt.path, ["rebase", &remote_base], &env),
    };
    if applied.is_err() {
        let files = conflicted(&wt.path);
        let abort = match strategy {
            Strategy::Merge => "merge",
            Strategy::Rebase => "rebase",
        };
        let _ = run(&wt.path, [abort, "--abort"]);
        return match (files.is_empty(), applied) {
            // Falhou sem conflito (identidade, hook...): é erro, não conflito.
            (true, Err(e)) => Err(e),
            _ => Ok(AlignOutcome::Conflict { files }),
        };
    }
    let target = format!("HEAD:refs/heads/{branch}");
    match strategy {
        Strategy::Merge => run(&wt.path, ["push", "--quiet", REMOTE, &target])?,
        Strategy::Rebase => run(
            &wt.path,
            [
                "push",
                "--quiet",
                &format!("--force-with-lease=refs/heads/{branch}:{old}"),
                REMOTE,
                &target,
            ],
        )?,
    };
    drop(wt);
    fetch(repo, branch, base)?;
    Ok(AlignOutcome::Updated)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn git(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_AUTHOR_NAME", "Teste")
            .env("GIT_AUTHOR_EMAIL", "teste@example.com")
            .env("GIT_COMMITTER_NAME", "Teste")
            .env("GIT_COMMITTER_EMAIL", "teste@example.com")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// Remoto `acme-web.git` (bare) com `develop`, e um clone dele.
    fn setup() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let t = tempfile::tempdir().unwrap();
        let remote = t.path().join("acme-web.git");
        let seed = t.path().join("seed");
        let clone = t.path().join("acme-web");
        std::fs::create_dir_all(&seed).unwrap();
        git(
            t.path(),
            &["init", "--quiet", "--bare", remote.to_str().unwrap()],
        );
        git(&seed, &["init", "--quiet", "-b", "develop"]);
        std::fs::write(seed.join("app.txt"), "linha 1\n").unwrap();
        git(&seed, &["add", "."]);
        git(&seed, &["commit", "--quiet", "-m", "inicial"]);
        git(
            &seed,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        );
        git(&seed, &["push", "--quiet", "origin", "develop"]);
        git(
            t.path(),
            &[
                "clone",
                "--quiet",
                "-b",
                "develop",
                remote.to_str().unwrap(),
                clone.to_str().unwrap(),
            ],
        );
        (t, seed, clone)
    }

    fn commit(dir: &Path, branch: &str, file: &str, text: &str) {
        git(dir, &["fetch", "--quiet", "origin"]);
        git(
            dir,
            &[
                "checkout",
                "--quiet",
                "-B",
                branch,
                &format!("origin/{branch}"),
            ],
        );
        std::fs::write(dir.join(file), text).unwrap();
        git(dir, &["add", "."]);
        git(dir, &["commit", "--quiet", "-m", &format!("muda {file}")]);
        git(dir, &["push", "--quiet", "origin", branch]);
    }

    fn remote_sha(dir: &Path, branch: &str) -> String {
        git(
            dir,
            &["ls-remote", "origin", &format!("refs/heads/{branch}")],
        )
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_string()
    }

    #[test]
    fn cria_a_branch_no_remoto_a_partir_da_base_uma_vez() {
        let (_t, seed, clone) = setup();
        assert!(ensure_remote_branch(&clone, "wip/autonoma", "develop").unwrap());
        assert_eq!(
            remote_sha(&seed, "wip/autonoma"),
            remote_sha(&seed, "develop")
        );
        assert!(!ensure_remote_branch(&clone, "wip/autonoma", "develop").unwrap());
        // A base precisa existir.
        assert!(ensure_remote_branch(&clone, "wip/outra", "inexistente").is_err());
        assert!(ensure_remote_branch(&clone, "-x", "develop").is_err());
    }

    #[test]
    fn alinhamento_por_merge_e_por_rebase() {
        for strategy in [Strategy::Merge, Strategy::Rebase] {
            let (_t, seed, clone) = setup();
            ensure_remote_branch(&clone, "wip/autonoma", "develop").unwrap();
            git(&seed, &["fetch", "--quiet", "origin"]);
            assert_eq!(
                align_branch(&clone, "wip/autonoma", "develop", strategy).unwrap(),
                AlignOutcome::UpToDate
            );
            // Trabalho na wip (ACME-123) e na base, em arquivos diferentes.
            commit(&seed, "wip/autonoma", "acme-123.txt", "feito\n");
            commit(&seed, "develop", "base.txt", "base\n");
            let head_clone = git(&clone, &["rev-parse", "HEAD"]);
            assert_eq!(
                align_branch(&clone, "wip/autonoma", "develop", strategy).unwrap(),
                AlignOutcome::Updated,
                "{strategy:?}"
            );
            git(&seed, &["fetch", "--quiet", "origin"]);
            git(
                &seed,
                &[
                    "merge-base",
                    "--is-ancestor",
                    "origin/develop",
                    "origin/wip/autonoma",
                ],
            );
            git(
                &seed,
                &["checkout", "--quiet", "-B", "x", "origin/wip/autonoma"],
            );
            assert!(seed.join("acme-123.txt").exists());
            assert!(seed.join("base.txt").exists());
            // A árvore de trabalho do clone não mudou, e o worktree
            // temporário sumiu.
            assert_eq!(git(&clone, &["rev-parse", "HEAD"]), head_clone);
            assert_eq!(crate::list_worktrees(&clone).unwrap().len(), 1);
        }
    }

    #[test]
    fn conflito_nao_envia_nada_e_lista_os_arquivos() {
        for strategy in [Strategy::Merge, Strategy::Rebase] {
            let (_t, seed, clone) = setup();
            ensure_remote_branch(&clone, "wip/autonoma", "develop").unwrap();
            git(&seed, &["fetch", "--quiet", "origin"]);
            commit(&seed, "wip/autonoma", "app.txt", "da wip\n");
            commit(&seed, "develop", "app.txt", "da base\n");
            let before = remote_sha(&seed, "wip/autonoma");
            assert_eq!(
                align_branch(&clone, "wip/autonoma", "develop", strategy).unwrap(),
                AlignOutcome::Conflict {
                    files: vec!["app.txt".into()]
                },
                "{strategy:?}"
            );
            assert_eq!(remote_sha(&seed, "wip/autonoma"), before);
            assert_eq!(crate::list_worktrees(&clone).unwrap().len(), 1);
        }
    }

    #[test]
    fn sem_a_branch_no_remoto_e_erro() {
        let (_t, _seed, clone) = setup();
        let e = align_branch(&clone, "wip/autonoma", "develop", Strategy::Merge).unwrap_err();
        assert!(e.to_string().contains("não existe no remoto"), "{e}");
    }
}
