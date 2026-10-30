//! Operações git do Control Tower (via `git` CLI).

use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, thiserror::Error)]
pub enum GitError {
    #[error("git {args} falhou: {stderr}")]
    Failed { args: String, stderr: String },
    #[error("worktree em {path:?} está na branch `{actual}`, esperado `{expected}`")]
    BranchMismatch {
        path: PathBuf,
        expected: String,
        actual: String,
    },
    #[error("{path:?} não é um worktree deste repositório; nada foi alterado")]
    NotAWorktreeOf { path: PathBuf },
    #[error("referência git inválida: `{reference}`; nada foi alterado")]
    InvalidRef { reference: String },
    #[error("clone do projeto não encontrado em {path:?}; cadastre o projeto pela URL para o daemon clonar, ou corrija o caminho")]
    RepoMissing { path: PathBuf },
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub struct WorktreeSpec<'a> {
    pub repo_path: &'a Path,
    pub branch: &'a str,
    pub base: &'a str,
    pub worktree_path: &'a Path,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorktreeOutcome {
    Created,
    Reused,
}

fn run<I, S>(dir: &Path, args: I) -> Result<String, GitError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let args: Vec<OsString> = args.into_iter().map(|a| a.as_ref().to_owned()).collect();
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(&args)
        .output()?;
    if !out.status.success() {
        return Err(GitError::Failed {
            args: args
                .iter()
                .map(|a| a.to_string_lossy())
                .collect::<Vec<_>>()
                .join(" "),
            stderr: String::from_utf8_lossy(&out.stderr).trim().to_string(),
        });
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn ref_exists(dir: &Path, reference: &str) -> Result<bool, GitError> {
    let status = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["show-ref", "--verify", "--quiet", "--", reference])
        .status()?;
    Ok(status.success())
}

/// URL do remote `origin`; `None` se o repositório não tiver `origin`.
/// Erro se `path` não for um repositório git.
pub fn origin_url(path: &Path) -> Result<Option<String>, GitError> {
    run(path, ["rev-parse", "--git-dir"])?;
    Ok(run(path, ["remote", "get-url", "origin"]).ok())
}

/// Clona `owner/repo` em `dest` com o `gh` (`gh repo clone`). Com `token`, o
/// `gh` usa `GH_TOKEN` em vez do próprio login.
pub fn gh_clone(
    gh_bin: &str,
    owner: &str,
    repo: &str,
    dest: &Path,
    token: Option<&str>,
) -> Result<(), GitError> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut c = Command::new(gh_bin);
    c.args(["repo", "clone", &format!("{owner}/{repo}")])
        .arg(dest)
        .env("GH_PROMPT_DISABLED", "1")
        .env("GH_NO_UPDATE_NOTIFIER", "1");
    if let Some(t) = token {
        c.env("GH_TOKEN", t);
    }
    let out = c.output()?;
    if !out.status.success() {
        return Err(GitError::Failed {
            args: format!("{gh_bin} repo clone {owner}/{repo}"),
            stderr: String::from_utf8_lossy(&out.stderr).trim().to_string(),
        });
    }
    Ok(())
}

/// Clona `url` em `dest` com o `git` (HTTPS ou SSH, de qualquer host: o
/// GitLab self-hosted, spec do Jira e GitLab §7.1). A credencial é a do git
/// do ambiente (credential helper, como o `osxkeychain`, ou a chave SSH);
/// o git falha em vez de pedir senha (`GIT_TERMINAL_PROMPT=0`, `stdin`
/// nulo), para o daemon não travar. O `ssh` do ambiente fica como está
/// (`core.sshCommand`, chave do agente).
pub fn clone_url(url: &str, dest: &Path) -> Result<(), GitError> {
    if url.trim().is_empty() || url.trim_start().starts_with('-') {
        return Err(GitError::InvalidRef {
            reference: url.to_string(),
        });
    }
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut c = Command::new("git");
    c.args(["clone", "--quiet", "--", url.trim()])
        .arg(dest)
        .stdin(std::process::Stdio::null())
        .env("GIT_TERMINAL_PROMPT", "0");
    let out = c.output()?;
    if !out.status.success() {
        return Err(GitError::Failed {
            args: format!("clone {}", url.trim()),
            stderr: String::from_utf8_lossy(&out.stderr).trim().to_string(),
        });
    }
    Ok(())
}

/// Branch atual do diretório; `None` se HEAD estiver destacado.
pub fn current_branch(path: &Path) -> Result<Option<String>, GitError> {
    let b = run(path, ["rev-parse", "--abbrev-ref", "HEAD"])?;
    Ok((b != "HEAD").then_some(b))
}

/// Valida nome de branch e base antes de qualquer efeito colateral.
fn validate_refs(spec: &WorktreeSpec) -> Result<(), GitError> {
    // Sem o clone, o `git -C` falharia e pareceria um nome de branch inválido.
    if !spec.repo_path.join(".git").exists() {
        return Err(GitError::RepoMissing {
            path: spec.repo_path.to_path_buf(),
        });
    }
    let branch_ok = !spec.branch.starts_with('-')
        && Command::new("git")
            .args(["check-ref-format", "--branch", spec.branch])
            .output()?
            .status
            .success();
    if !branch_ok {
        return Err(GitError::InvalidRef {
            reference: spec.branch.to_string(),
        });
    }
    if spec.base.is_empty() || spec.base.starts_with('-') {
        return Err(GitError::InvalidRef {
            reference: spec.base.to_string(),
        });
    }
    Ok(())
}

/// Confirma que `worktree_path` é a raiz de um worktree do repositório `repo_path`.
fn is_worktree_of(repo_path: &Path, worktree_path: &Path) -> Result<bool, GitError> {
    let Ok(out) = run(
        worktree_path,
        [
            "rev-parse",
            "--path-format=absolute",
            "--show-toplevel",
            "--git-common-dir",
        ],
    ) else {
        return Ok(false);
    };
    let mut lines = out.lines();
    let (Some(toplevel), Some(common)) = (lines.next(), lines.next()) else {
        return Ok(false);
    };
    let repo_common = run(
        repo_path,
        ["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )?;
    let same = |a: &Path, b: &Path| -> bool {
        match (fs::canonicalize(a), fs::canonicalize(b)) {
            (Ok(a), Ok(b)) => a == b,
            _ => false,
        }
    };
    Ok(
        same(Path::new(toplevel), worktree_path)
            && same(Path::new(common), Path::new(&repo_common)),
    )
}

pub fn ensure_worktree(spec: &WorktreeSpec) -> Result<WorktreeOutcome, GitError> {
    validate_refs(spec)?;
    if spec.worktree_path.exists() {
        if !is_worktree_of(spec.repo_path, spec.worktree_path)? {
            return Err(GitError::NotAWorktreeOf {
                path: spec.worktree_path.to_path_buf(),
            });
        }
        let actual = current_branch(spec.worktree_path)?.unwrap_or_else(|| "HEAD".into());
        if actual == spec.branch {
            return Ok(WorktreeOutcome::Reused);
        }
        return Err(GitError::BranchMismatch {
            path: spec.worktree_path.to_path_buf(),
            expected: spec.branch.to_string(),
            actual,
        });
    }
    let repo = spec.repo_path;
    let wt = spec.worktree_path.as_os_str();
    let remote_branch = format!("origin/{}", spec.branch);
    let (mode, start): (Option<&str>, String) =
        if ref_exists(repo, &format!("refs/heads/{}", spec.branch))? {
            (None, spec.branch.to_string())
        } else if ref_exists(repo, &format!("refs/remotes/{remote_branch}"))? {
            (Some("--track"), remote_branch)
        } else {
            let remote_base = format!("origin/{}", spec.base);
            let base = if ref_exists(repo, &format!("refs/remotes/{remote_base}"))? {
                remote_base
            } else {
                spec.base.to_string()
            };
            // Falha cedo (sem criar diretórios) se a base não resolve para um commit.
            run(
                repo,
                [
                    "rev-parse",
                    "--verify",
                    "--quiet",
                    "--end-of-options",
                    &format!("{base}^{{commit}}"),
                ],
            )?;
            (Some("--no-track"), base)
        };

    let mut args: Vec<&OsStr> = vec![OsStr::new("worktree"), OsStr::new("add")];
    if let Some(mode) = mode {
        args.extend([OsStr::new(mode), OsStr::new("-b"), OsStr::new(spec.branch)]);
    }
    args.extend([OsStr::new("--"), wt, OsStr::new(&start)]);

    if let Some(parent) = spec.worktree_path.parent() {
        fs::create_dir_all(parent)?;
    }
    run(repo, args)?;
    Ok(WorktreeOutcome::Created)
}

/// Remove o worktree. Falha (sem `--force`) se houver mudanças não commitadas.
pub fn remove_worktree(repo_path: &Path, worktree_path: &Path) -> Result<(), GitError> {
    run(
        repo_path,
        [
            OsStr::new("worktree"),
            OsStr::new("remove"),
            OsStr::new("--"),
            worktree_path.as_os_str(),
        ],
    )?;
    Ok(())
}

/// Remove os registros de worktrees cujo diretório já não existe
/// (`git worktree prune`).
pub fn prune_worktrees(repo_path: &Path) -> Result<(), GitError> {
    run(repo_path, ["worktree", "prune"])?;
    Ok(())
}

/// Um worktree registrado no repositório (`git worktree list --porcelain`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeEntry {
    pub path: PathBuf,
    /// Branch em uso (sem o `refs/heads/`); `None` = HEAD destacado.
    pub branch: Option<String>,
    /// Travado (`git worktree lock`): o `remove` sem `--force` recusa.
    pub locked: bool,
    /// O diretório sumiu e o registro sai no `git worktree prune`.
    pub prunable: bool,
}

/// Worktrees registrados no repositório, o principal primeiro.
pub fn list_worktrees(repo_path: &Path) -> Result<Vec<WorktreeEntry>, GitError> {
    let out = run(repo_path, ["worktree", "list", "--porcelain"])?;
    let mut list: Vec<WorktreeEntry> = Vec::new();
    for line in out.lines() {
        let (key, value) = line.split_once(' ').unwrap_or((line, ""));
        if key == "worktree" {
            list.push(WorktreeEntry {
                path: PathBuf::from(value),
                branch: None,
                locked: false,
                prunable: false,
            });
            continue;
        }
        let Some(e) = list.last_mut() else {
            continue;
        };
        match key {
            "branch" => e.branch = Some(value.trim_start_matches("refs/heads/").to_string()),
            "locked" => e.locked = true,
            "prunable" => e.prunable = true,
            _ => {}
        }
    }
    Ok(list)
}

/// Mesmo caminho, mesmo que um dos lados já não exista: compara o pai
/// canônico e o nome (o `/var` → `/private/var` do macOS, por exemplo).
fn same_path(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    let canon = |p: &Path| -> Option<PathBuf> {
        fs::canonicalize(p).ok().or_else(|| {
            let parent = fs::canonicalize(p.parent()?).ok()?;
            Some(parent.join(p.file_name()?))
        })
    };
    matches!((canon(a), canon(b)), (Some(x), Some(y)) if x == y)
}

/// O registro do worktree em `worktree_path`, se houver.
pub fn find_worktree(
    repo_path: &Path,
    worktree_path: &Path,
) -> Result<Option<WorktreeEntry>, GitError> {
    Ok(list_worktrees(repo_path)?
        .into_iter()
        .find(|e| same_path(&e.path, worktree_path)))
}

/// O que impede remover o worktree sem perder trabalho, com o motivo para
/// pessoas (vazio = pode sair com [`remove_worktree`], sem `--force`):
/// - não é um worktree secundário de `repo_path` (inclusive o próprio
///   clone principal), ou está travado;
/// - mudanças não commitadas, inclusive arquivos novos fora do
///   `.gitignore` (que o `git worktree remove` também recusa);
/// - commits à frente do upstream;
/// - commits que não estão em nenhuma branch remota (sem upstream, upstream
///   apagado ou HEAD destacado). Um squash merge com a branch remota já
///   apagada também cai aqui: na dúvida, fica.
pub fn worktree_blockers(repo_path: &Path, worktree_path: &Path) -> Result<Vec<String>, GitError> {
    let list = list_worktrees(repo_path)?;
    let is_main = list
        .first()
        .is_some_and(|m| same_path(&m.path, worktree_path));
    let entry = list
        .into_iter()
        .skip(1)
        .find(|e| same_path(&e.path, worktree_path));
    let Some(entry) = entry.filter(|_| !is_main) else {
        let what = if is_main {
            "é o clone principal, não um worktree"
        } else {
            "não é um worktree deste clone"
        };
        return Ok(vec![format!(
            "{} {what} ({})",
            worktree_path.display(),
            repo_path.display()
        )]);
    };
    if !is_worktree_of(repo_path, worktree_path)? {
        return Ok(vec![format!(
            "{} não é um worktree de {}",
            worktree_path.display(),
            repo_path.display()
        )]);
    }
    let mut out = Vec::new();
    if entry.locked {
        out.push("worktree travado (git worktree lock)".to_string());
    }
    let status = run(
        worktree_path,
        ["status", "--porcelain=v1", "--untracked-files=all"],
    )?;
    let changes: Vec<&str> = status.lines().filter(|l| !l.is_empty()).collect();
    if !changes.is_empty() {
        const SHOW: usize = 5;
        let mut files: Vec<&str> = changes
            .iter()
            .take(SHOW)
            .map(|l| l.get(3..).unwrap_or(l))
            .collect();
        if changes.len() > SHOW {
            files.push("…");
        }
        out.push(format!(
            "{} mudança(s) não commitada(s): {}",
            changes.len(),
            files.join(", ")
        ));
    }
    let count = |args: &[&str]| -> Result<u64, GitError> {
        Ok(run(worktree_path, args)?.parse().unwrap_or(0))
    };
    let upstream = run(
        worktree_path,
        [
            "rev-parse",
            "--abbrev-ref",
            "--symbolic-full-name",
            "@{upstream}",
        ],
    )
    .ok();
    let ahead = match &upstream {
        Some(_) => count(&["rev-list", "--count", "@{upstream}..HEAD"])?,
        None => 0,
    };
    if ahead > 0 {
        out.push(format!(
            "{ahead} commit(s) não enviado(s) para {}",
            upstream.as_deref().unwrap_or_default()
        ));
    } else {
        let local = count(&["rev-list", "--count", "HEAD", "--not", "--remotes"])?;
        if local > 0 {
            out.push(format!(
                "{local} commit(s) que não estão em nenhuma branch remota \
                 (sem upstream, upstream apagado ou HEAD destacado)"
            ));
        }
    }
    Ok(out)
}

/// `rel` (ex.: `target`) dentro do worktree é ignorado pelo git e não tem
/// nenhum arquivo versionado: apagá-lo não perde código.
pub fn is_ignored_untracked(worktree_path: &Path, rel: &str) -> Result<bool, GitError> {
    let ignored = Command::new("git")
        .arg("-C")
        .arg(worktree_path)
        .args(["check-ignore", "-q", "--", rel])
        .status()?
        .success();
    if !ignored {
        return Ok(false);
    }
    Ok(run(worktree_path, ["ls-files", "--", rel])?.is_empty())
}

/// Nome de branch aceito pelo git e que não se confunde com opção.
fn valid_branch(branch: &str) -> Result<(), GitError> {
    let ok = !branch.starts_with('-')
        && Command::new("git")
            .args(["check-ref-format", "--branch", branch])
            .output()?
            .status
            .success();
    if ok {
        Ok(())
    } else {
        Err(GitError::InvalidRef {
            reference: branch.to_string(),
        })
    }
}

fn valid_remote(remote: &str) -> Result<(), GitError> {
    if remote.is_empty() || remote.starts_with('-') || remote.contains('/') {
        return Err(GitError::InvalidRef {
            reference: remote.to_string(),
        });
    }
    Ok(())
}

/// SHA para onde a referência completa aponta (`refs/heads/x`,
/// `refs/remotes/origin/x`); `None` se não existir.
fn ref_sha(repo_path: &Path, reference: &str) -> Result<Option<String>, GitError> {
    if !ref_exists(repo_path, reference)? {
        return Ok(None);
    }
    Ok(Some(run(
        repo_path,
        [
            "rev-parse",
            "--verify",
            "--quiet",
            "--end-of-options",
            reference,
        ],
    )?))
}

/// Ponta da branch local (`refs/heads/<branch>`); `None` se não existir.
pub fn branch_sha(repo_path: &Path, branch: &str) -> Result<Option<String>, GitError> {
    valid_branch(branch)?;
    ref_sha(repo_path, &format!("refs/heads/{branch}"))
}

/// Ponta da referência remota local (`refs/remotes/<remote>/<branch>`), do
/// último `fetch`; `None` se não existir.
pub fn remote_branch_sha(
    repo_path: &Path,
    remote: &str,
    branch: &str,
) -> Result<Option<String>, GitError> {
    valid_branch(branch)?;
    valid_remote(remote)?;
    ref_sha(repo_path, &format!("refs/remotes/{remote}/{branch}"))
}

/// Commits de `rev` que não estão em nenhuma referência remota
/// (`git rev-list --count <rev> --not --remotes`): o que se perderia se a
/// branch local saísse.
pub fn commits_not_on_remotes(repo_path: &Path, rev: &str) -> Result<u64, GitError> {
    if rev.starts_with('-') {
        return Err(GitError::InvalidRef {
            reference: rev.to_string(),
        });
    }
    Ok(run(
        repo_path,
        ["rev-list", "--count", rev, "--not", "--remotes", "--"],
    )?
    .parse()
    .unwrap_or(0))
}

/// `ancestor` é ancestral de `rev` (ou o mesmo commit). `false` também se
/// algum dos dois não existir no repositório (ex.: o head de um PR que
/// nunca foi buscado).
pub fn is_ancestor(repo_path: &Path, ancestor: &str, rev: &str) -> Result<bool, GitError> {
    if ancestor.starts_with('-') || rev.starts_with('-') {
        return Ok(false);
    }
    Ok(Command::new("git")
        .arg("-C")
        .arg(repo_path)
        .args(["merge-base", "--is-ancestor", ancestor, rev])
        .output()?
        .status
        .success())
}

/// Worktree (inclusive o clone principal) com a branch em uso, se houver.
pub fn worktree_with_branch(
    repo_path: &Path,
    branch: &str,
) -> Result<Option<WorktreeEntry>, GitError> {
    Ok(list_worktrees(repo_path)?
        .into_iter()
        .find(|e| e.branch.as_deref() == Some(branch)))
}

/// Apaga a branch local só se ela ainda apontar para `expected_sha` (`git
/// update-ref -d` com o valor antigo: atômico, sem corrida com um commit
/// feito entre a avaliação e a remoção) e não estiver em uso em nenhum
/// worktree, nem no clone principal. Leva junto a configuração
/// `branch.<nome>.*` (upstream), como o `git branch -D`.
pub fn delete_branch(repo_path: &Path, branch: &str, expected_sha: &str) -> Result<(), GitError> {
    valid_branch(branch)?;
    if expected_sha.is_empty() || expected_sha.starts_with('-') {
        return Err(GitError::InvalidRef {
            reference: expected_sha.to_string(),
        });
    }
    if let Some(wt) = worktree_with_branch(repo_path, branch)? {
        return Err(GitError::Failed {
            args: format!("branch -D {branch}"),
            stderr: format!("a branch está em uso no worktree {}", wt.path.display()),
        });
    }
    run(
        repo_path,
        [
            "update-ref",
            "-d",
            &format!("refs/heads/{branch}"),
            expected_sha,
        ],
    )?;
    // Sem a seção (branch sem upstream), o git falha: não é erro.
    let _ = run(
        repo_path,
        ["config", "--remove-section", &format!("branch.{branch}")],
    );
    Ok(())
}

/// `git fetch --prune <remote>`: tira as referências remotas locais das
/// branches que já não existem no remoto.
pub fn fetch_prune(repo_path: &Path, remote: &str) -> Result<(), GitError> {
    valid_remote(remote)?;
    run(repo_path, ["fetch", "--prune", "--quiet", remote])?;
    Ok(())
}
