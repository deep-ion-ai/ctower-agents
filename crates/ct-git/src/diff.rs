//! Diff de uma worktree para o visualizador da UI (#168): a lista de
//! arquivos (status e +/−), o patch de um arquivo e os commits da branch.
//!
//! Três modos, sempre um `git diff <de> [<até>]`:
//!
//! - **Branch:** do merge-base com a branch base até a working tree (os
//!   commits da branch mais o que não foi commitado);
//! - **Não commitado:** de `HEAD` até a working tree (index e working tree);
//! - **Commit:** do primeiro pai do commit (ou da árvore vazia, no commit
//!   inicial) até o commit.
//!
//! Nos dois modos que chegam à working tree, os arquivos novos ainda não
//! rastreados entram também (como o agente os criou), marcados com
//! `untracked`. Para isso o diff roda com uma **cópia** do index em que eles
//! entram com `git add -N`: o index da worktree não é tocado.
//!
//! Só lê: nenhum comando escreve na worktree, no index ou nas referências
//! (`GIT_OPTIONAL_LOCKS=0`; os caminhos são literais, sem glob).

use crate::{run_env, GitError};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

/// No máximo isto de arquivos na lista (o resto é cortado, com aviso).
pub const DIFF_MAX_FILES: usize = 3000;
/// Maior patch de um arquivo que o visualizador recebe (1 MiB).
pub const PATCH_MAX_BYTES: usize = 1024 * 1024;
/// No máximo isto de commits na lista da branch.
pub const COMMITS_MAX: usize = 500;

/// O que comparar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffMode {
    /// Do merge-base com a branch base (`base`, sem `origin/`) até a
    /// working tree.
    Branch { base: String },
    /// De `HEAD` até a working tree.
    Uncommitted,
    /// O commit em relação ao primeiro pai.
    Commit(String),
}

/// Status de um arquivo no diff (`--name-status`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub enum DiffStatus {
    Added,
    Modified,
    Deleted,
    Renamed,
    Copied,
    /// Mudou de tipo (arquivo ↔ symlink, por exemplo).
    Typechange,
    /// Em conflito de merge.
    Unmerged,
}

/// Um arquivo da lista do diff.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct DiffFile {
    /// Caminho relativo à raiz da worktree (o novo, numa renomeação).
    pub path: String,
    /// Caminho antigo, só em renomeação ou cópia.
    pub old_path: Option<String>,
    pub status: DiffStatus,
    /// Linhas adicionadas; `None` em binário.
    pub additions: Option<u64>,
    /// Linhas removidas; `None` em binário.
    pub deletions: Option<u64>,
    pub binary: bool,
    /// Arquivo novo que ainda não foi adicionado ao git.
    pub untracked: bool,
}

/// Resposta de `git.diff`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct DiffSummary {
    /// Commit de onde o diff parte (merge-base, `HEAD`, pai do commit ou a
    /// árvore vazia).
    pub from: String,
    /// Commit até onde vai; `None` = a working tree.
    pub to: Option<String>,
    /// Referência usada como base no modo branch (`origin/develop`).
    pub base_ref: Option<String>,
    /// Em ordem de caminho.
    pub files: Vec<DiffFile>,
    /// Havia mais que [`DIFF_MAX_FILES`]: a lista foi cortada.
    pub truncated: bool,
}

/// Resposta de `git.diff_file`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct DiffPatch {
    pub path: String,
    /// O patch unificado (com cabeçalho); `None` se grande demais.
    pub patch: Option<String>,
    /// Passou de [`PATCH_MAX_BYTES`].
    pub too_large: bool,
}

/// Um commit da branch (`git.commits`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct BranchCommit {
    pub sha: String,
    pub short: String,
    pub subject: String,
    pub author: String,
    /// Data do autor (ms).
    pub date: i64,
    /// Commit de merge (o diff é contra o primeiro pai).
    pub merge: bool,
}

/// Resposta de `git.commits`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct BranchCommits {
    /// Referência usada como base (`origin/develop` ou `develop`).
    pub base_ref: String,
    pub merge_base: String,
    /// Do mais novo para o mais antigo.
    pub commits: Vec<BranchCommit>,
    /// Havia mais que [`COMMITS_MAX`].
    pub truncated: bool,
}

fn invalid(reference: impl Into<String>) -> GitError {
    GitError::InvalidRef {
        reference: reference.into(),
    }
}

fn failed(args: &str, stderr: impl Into<String>) -> GitError {
    GitError::Failed {
        args: args.to_string(),
        stderr: stderr.into(),
    }
}

/// Variáveis de todo comando do diff: sem locks opcionais, caminhos
/// literais e, com `index`, a cópia do index.
fn envs(index: Option<&Path>) -> Vec<(&'static str, std::ffi::OsString)> {
    let mut v = vec![
        ("GIT_OPTIONAL_LOCKS", "0".into()),
        ("GIT_LITERAL_PATHSPECS", "1".into()),
    ];
    if let Some(i) = index {
        v.push(("GIT_INDEX_FILE", i.as_os_str().to_owned()));
    }
    v
}

/// `git` com a saída em bytes, sem cortar espaços (patch, `-z`).
fn raw(dir: &Path, index: Option<&Path>, args: &[&str]) -> Result<Vec<u8>, GitError> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .envs(envs(index))
        .stdin(Stdio::null())
        .output()?;
    if !out.status.success() {
        return Err(failed(
            &args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim(),
        ));
    }
    Ok(out.stdout)
}

/// Como [`raw`], mas lê no máximo `max` bytes; `None` se passou disso.
fn raw_limited(
    dir: &Path,
    index: Option<&Path>,
    args: &[&str],
    max: usize,
) -> Result<Option<Vec<u8>>, GitError> {
    let mut child = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .envs(envs(index))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let mut buf = Vec::new();
    child
        .stdout
        .take()
        .expect("stdout")
        .take(max as u64 + 1)
        .read_to_end(&mut buf)?;
    if buf.len() > max {
        let _ = child.kill();
        let _ = child.wait();
        return Ok(None);
    }
    let out = child.wait_with_output()?;
    if !out.status.success() {
        return Err(failed(
            &args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim(),
        ));
    }
    Ok(Some(buf))
}

fn text(dir: &Path, args: &[&str]) -> Result<String, GitError> {
    run_env(dir, args, &envs(None))
}

/// SHA do commit `rev`, se existir.
fn commit_sha(dir: &Path, rev: &str) -> Result<Option<String>, GitError> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["rev-parse", "--verify", "--quiet", "--end-of-options"])
        .arg(format!("{rev}^{{commit}}"))
        .envs(envs(None))
        .output()?;
    Ok(out
        .status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string()))
}

/// Referência da base: `origin/<base>` (o que o PR compara) se existir, se
/// não a branch local. Devolve o nome mostrado e o merge-base com `HEAD`.
pub fn merge_base(dir: &Path, base: &str) -> Result<(String, String), GitError> {
    crate::valid_branch(base)?;
    let candidates = [
        (
            format!("origin/{base}"),
            format!("refs/remotes/origin/{base}"),
        ),
        (base.to_string(), format!("refs/heads/{base}")),
    ];
    let Some((name, full)) = candidates
        .into_iter()
        .find(|(_, full)| crate::ref_exists(dir, full).unwrap_or(false))
    else {
        return Err(failed(
            "merge-base",
            format!("branch base `{base}` não encontrada (nem `origin/{base}`) nesta worktree"),
        ));
    };
    let mb = text(dir, &["merge-base", &full, "HEAD"]).map_err(|_| {
        failed(
            "merge-base",
            format!("a branch não tem ancestral comum com `{name}`"),
        )
    })?;
    Ok((name, mb))
}

/// Valida `commit` (hexadecimal, 4 a 64 caracteres), confere que é um
/// commit desta worktree alcançável a partir de `HEAD` e devolve o SHA.
pub fn branch_commit(dir: &Path, commit: &str) -> Result<String, GitError> {
    let ok = (4..=64).contains(&commit.len()) && commit.bytes().all(|b| b.is_ascii_hexdigit());
    if !ok {
        return Err(invalid(commit));
    }
    let sha = commit_sha(dir, commit)?.ok_or_else(|| {
        failed(
            "rev-parse",
            format!("commit `{commit}` não encontrado nesta worktree"),
        )
    })?;
    if !crate::is_ancestor(dir, &sha, "HEAD")? {
        return Err(failed(
            "merge-base",
            format!("commit `{commit}` não faz parte da branch desta worktree"),
        ));
    }
    Ok(sha)
}

/// De onde e até onde, para o modo.
struct Range {
    from: String,
    to: Option<String>,
    base_ref: Option<String>,
}

fn range(dir: &Path, mode: &DiffMode) -> Result<Range, GitError> {
    Ok(match mode {
        DiffMode::Branch { base } => {
            let (name, mb) = merge_base(dir, base)?;
            Range {
                from: mb,
                to: None,
                base_ref: Some(name),
            }
        }
        DiffMode::Uncommitted => Range {
            from: commit_sha(dir, "HEAD")?
                .ok_or_else(|| failed("rev-parse HEAD", "a worktree ainda não tem commits"))?,
            to: None,
            base_ref: None,
        },
        DiffMode::Commit(c) => {
            let sha = branch_commit(dir, c)?;
            let from = match commit_sha(dir, &format!("{sha}^1"))? {
                Some(p) => p,
                // Commit inicial: contra a árvore vazia.
                None => text(dir, &["hash-object", "-t", "tree", "/dev/null"])?,
            };
            Range {
                from,
                to: Some(sha),
                base_ref: None,
            }
        }
    })
}

/// Cópia do index com os arquivos não rastreados como `add -N`; apagada no
/// `drop`.
struct TempIndex {
    path: PathBuf,
    untracked: HashSet<String>,
}

impl Drop for TempIndex {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// Arquivos não rastreados (fora do `.gitignore`), até `max`; `true` se
/// havia mais.
fn untracked(dir: &Path, max: usize) -> Result<(Vec<String>, bool), GitError> {
    let out = raw(
        dir,
        None,
        &["ls-files", "-z", "--others", "--exclude-standard"],
    )?;
    let mut list: Vec<String> = out
        .split(|b| *b == 0)
        .filter(|p| !p.is_empty() && !p.ends_with(b"/"))
        .map(|p| String::from_utf8_lossy(p).into_owned())
        .collect();
    let more = list.len() > max;
    list.truncate(max);
    Ok((list, more))
}

/// `None` se não há arquivo novo (o diff usa o index da worktree).
fn temp_index(dir: &Path, only: Option<&str>) -> Result<Option<TempIndex>, GitError> {
    let (mut files, _) = untracked(dir, DIFF_MAX_FILES)?;
    if let Some(p) = only {
        files.retain(|f| f == p);
    }
    if files.is_empty() {
        return Ok(None);
    }
    let real = text(
        dir,
        &["rev-parse", "--path-format=absolute", "--git-path", "index"],
    )?;
    let path = std::env::temp_dir().join(format!(
        "ct-diff-index-{}-{}",
        std::process::id(),
        TEMP_SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    let tmp = TempIndex {
        path,
        untracked: files.iter().cloned().collect(),
    };
    if Path::new(&real).exists() {
        fs::copy(&real, &tmp.path)?;
    }
    let mut child = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args([
            "add",
            "--intent-to-add",
            "--pathspec-from-file=-",
            "--pathspec-file-nul",
        ])
        .envs(envs(Some(&tmp.path)))
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()?;
    {
        let mut stdin = child.stdin.take().expect("stdin");
        for f in &files {
            stdin.write_all(f.as_bytes())?;
            stdin.write_all(&[0])?;
        }
    }
    let out = child.wait_with_output()?;
    if !out.status.success() {
        return Err(failed(
            "add --intent-to-add",
            String::from_utf8_lossy(&out.stderr).trim(),
        ));
    }
    Ok(Some(tmp))
}

/// Argumentos fixos do `git diff`: sem diff externo nem textconv, com
/// renomeações e prefixos `a/` e `b/` (independentes da configuração).
const DIFF_ARGS: &[&str] = &[
    "diff",
    "--no-ext-diff",
    "--no-textconv",
    "--no-color",
    "-M",
    "--src-prefix=a/",
    "--dst-prefix=b/",
];

fn diff_args<'a>(r: &'a Range, extra: &[&'a str]) -> Vec<&'a str> {
    let mut a: Vec<&str> = DIFF_ARGS.to_vec();
    a.extend_from_slice(extra);
    a.push(&r.from);
    if let Some(t) = &r.to {
        a.push(t);
    }
    a
}

fn parse_status(s: &str) -> Option<DiffStatus> {
    Some(match s.as_bytes().first()? {
        b'A' => DiffStatus::Added,
        b'M' => DiffStatus::Modified,
        b'D' => DiffStatus::Deleted,
        b'R' => DiffStatus::Renamed,
        b'C' => DiffStatus::Copied,
        b'T' => DiffStatus::Typechange,
        b'U' => DiffStatus::Unmerged,
        _ => return None,
    })
}

/// `--name-status -z`: `S\0caminho\0`, ou `R100\0antigo\0novo\0`.
fn parse_name_status(out: &[u8]) -> Vec<(DiffStatus, Option<String>, String)> {
    let mut it = out
        .split(|b| *b == 0)
        .map(|t| String::from_utf8_lossy(t).into_owned());
    let mut v = Vec::new();
    while let Some(st) = it.next() {
        if st.is_empty() {
            continue;
        }
        let Some(status) = parse_status(&st) else {
            // Status desconhecido: pula o caminho que vem junto.
            it.next();
            continue;
        };
        let two = matches!(status, DiffStatus::Renamed | DiffStatus::Copied);
        let first = it.next().unwrap_or_default();
        if two {
            let second = it.next().unwrap_or_default();
            v.push((status, Some(first), second));
        } else {
            v.push((status, None, first));
        }
    }
    v
}

/// `--numstat -z`: `a\td\tcaminho\0`, ou `a\td\t\0antigo\0novo\0`; `-` em
/// binário. Chave: o caminho novo.
fn parse_numstat(out: &[u8]) -> HashMap<String, (Option<u64>, Option<u64>)> {
    let mut map = HashMap::new();
    let mut it = out.split(|b| *b == 0);
    while let Some(tok) = it.next() {
        if tok.is_empty() {
            continue;
        }
        let t = String::from_utf8_lossy(tok);
        let mut parts = t.splitn(3, '\t');
        let add = parts.next().and_then(|n| n.parse().ok());
        let del = parts.next().and_then(|n| n.parse().ok());
        let path = parts.next().unwrap_or_default().to_string();
        let path = if path.is_empty() {
            it.next();
            it.next()
                .map(|p| String::from_utf8_lossy(p).into_owned())
                .unwrap_or_default()
        } else {
            path
        };
        map.insert(path, (add, del));
    }
    map
}

/// A lista de arquivos do diff no `mode`.
pub fn diff_summary(dir: &Path, mode: &DiffMode) -> Result<DiffSummary, GitError> {
    let r = range(dir, mode)?;
    let tmp = if r.to.is_none() {
        temp_index(dir, None)?
    } else {
        None
    };
    let index = tmp.as_ref().map(|t| t.path.as_path());
    let names = raw(dir, index, &diff_args(&r, &["-z", "--name-status"]))?;
    let stats = parse_numstat(&raw(dir, index, &diff_args(&r, &["-z", "--numstat"]))?);
    let mut files: Vec<DiffFile> = parse_name_status(&names)
        .into_iter()
        .map(|(status, old_path, path)| {
            let (add, del) = stats.get(&path).copied().unwrap_or((None, None));
            let binary = add.is_none() && del.is_none() && stats.contains_key(&path);
            let untracked = tmp.as_ref().is_some_and(|t| t.untracked.contains(&path));
            DiffFile {
                path,
                old_path,
                status,
                additions: add,
                deletions: del,
                binary,
                untracked,
            }
        })
        .collect();
    files.sort_by(|a, b| a.path.cmp(&b.path));
    let truncated = files.len() > DIFF_MAX_FILES;
    files.truncate(DIFF_MAX_FILES);
    Ok(DiffSummary {
        from: r.from,
        to: r.to,
        base_ref: r.base_ref,
        files,
        truncated,
    })
}

/// Caminho relativo à worktree, sem `..` nem absoluto.
fn valid_rel(path: &str) -> Result<(), GitError> {
    let p = Path::new(path);
    let ok = !path.is_empty()
        && !path.starts_with('-')
        && p.components().all(|c| matches!(c, Component::Normal(_)));
    if ok {
        Ok(())
    } else {
        Err(invalid(path))
    }
}

/// O patch de um arquivo (com o antigo, numa renomeação), até
/// [`PATCH_MAX_BYTES`].
pub fn diff_file(
    dir: &Path,
    mode: &DiffMode,
    path: &str,
    old_path: Option<&str>,
) -> Result<DiffPatch, GitError> {
    diff_file_limit(dir, mode, path, old_path, PATCH_MAX_BYTES)
}

fn diff_file_limit(
    dir: &Path,
    mode: &DiffMode,
    path: &str,
    old_path: Option<&str>,
    max: usize,
) -> Result<DiffPatch, GitError> {
    valid_rel(path)?;
    if let Some(o) = old_path {
        valid_rel(o)?;
    }
    let r = range(dir, mode)?;
    let tmp = if r.to.is_none() {
        temp_index(dir, Some(path))?
    } else {
        None
    };
    let mut args = diff_args(&r, &[]);
    args.push("--");
    if let Some(o) = old_path {
        args.push(o);
    }
    args.push(path);
    let out = raw_limited(dir, tmp.as_ref().map(|t| t.path.as_path()), &args, max)?;
    Ok(DiffPatch {
        path: path.to_string(),
        too_large: out.is_none(),
        patch: out.map(|b| String::from_utf8_lossy(&b).into_owned()),
    })
}

/// Os commits da branch desde o merge-base com `base`, do mais novo para
/// o mais antigo.
pub fn branch_commits(dir: &Path, base: &str) -> Result<BranchCommits, GitError> {
    let (base_ref, mb) = merge_base(dir, base)?;
    let range = format!("{mb}..HEAD");
    let max = format!("--max-count={}", COMMITS_MAX + 1);
    let out = raw(
        dir,
        None,
        &[
            "log",
            "-z",
            &max,
            "--format=%H%x1f%h%x1f%an%x1f%at%x1f%P%x1f%s",
            &range,
            "--",
        ],
    )?;
    let mut commits: Vec<BranchCommit> = out
        .split(|b| *b == 0)
        .filter(|r| !r.is_empty())
        .filter_map(|r| {
            let r = String::from_utf8_lossy(r);
            let f: Vec<&str> = r.trim_start_matches('\n').splitn(6, '\x1f').collect();
            let [sha, short, author, at, parents, subject] = f[..] else {
                return None;
            };
            Some(BranchCommit {
                sha: sha.to_string(),
                short: short.to_string(),
                subject: subject.to_string(),
                author: author.to_string(),
                date: at.parse::<i64>().unwrap_or(0) * 1000,
                merge: parents.split_whitespace().count() > 1,
            })
        })
        .collect();
    let truncated = commits.len() > COMMITS_MAX;
    commits.truncate(COMMITS_MAX);
    Ok(BranchCommits {
        base_ref,
        merge_base: mb,
        commits,
        truncated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_AUTHOR_NAME", "Fulano")
            .env("GIT_AUTHOR_EMAIL", "fulano@example.com")
            .env("GIT_COMMITTER_NAME", "Fulano")
            .env("GIT_COMMITTER_EMAIL", "fulano@example.com")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    /// Repositório com `develop` (2 commits) e a branch `feat` com 2
    /// commits: um muda `a.txt` e remove `velho.txt`, o outro renomeia
    /// `b.txt` e adiciona um binário.
    fn repo() -> (tempfile::TempDir, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path().join("repo");
        fs::create_dir_all(&d).unwrap();
        git(&d, &["init", "-q", "-b", "develop"]);
        git(&d, &["config", "commit.gpgsign", "false"]);
        fs::write(d.join("a.txt"), "um\ndois\n").unwrap();
        fs::write(d.join(".gitignore"), "ignorado.log\n").unwrap();
        git(&d, &["add", "."]);
        git(&d, &["commit", "-q", "-m", "inicial"]);
        let longo: String = (0..40).map(|i| format!("linha {i}\n")).collect();
        fs::write(d.join("b.txt"), &longo).unwrap();
        fs::write(d.join("velho.txt"), "x\n").unwrap();
        git(&d, &["add", "."]);
        git(&d, &["commit", "-q", "-m", "segundo"]);
        git(&d, &["checkout", "-q", "-b", "feat"]);
        fs::write(d.join("a.txt"), "um\nDOIS\ntrês\n").unwrap();
        fs::remove_file(d.join("velho.txt")).unwrap();
        git(&d, &["add", "-A"]);
        git(&d, &["commit", "-q", "-m", "muda a"]);
        git(&d, &["mv", "b.txt", "c.txt"]);
        fs::write(d.join("img.bin"), [0u8, 1, 2, 0, 255]).unwrap();
        git(&d, &["add", "-A"]);
        git(&d, &["commit", "-q", "-m", "renomeia b"]);
        (tmp, d)
    }

    fn by_path<'a>(s: &'a DiffSummary, p: &str) -> &'a DiffFile {
        s.files
            .iter()
            .find(|f| f.path == p)
            .unwrap_or_else(|| panic!("{p}: {s:?}"))
    }

    fn branch() -> DiffMode {
        DiffMode::Branch {
            base: "develop".into(),
        }
    }

    /// Nomes da lista, como o `git diff --name-only` faria.
    fn names(s: &DiffSummary) -> Vec<&str> {
        s.files.iter().map(|f| f.path.as_str()).collect()
    }

    #[test]
    fn branch_inclui_commits_nao_commitados_e_novos() {
        let (_tmp, d) = repo();
        fs::write(d.join("a.txt"), "um\nDOIS\ntrês\nquatro\n").unwrap();
        fs::write(d.join("novo.rs"), "fn main() {}\n").unwrap();
        fs::write(d.join("ignorado.log"), "x").unwrap();
        let s = diff_summary(&d, &branch()).unwrap();
        assert_eq!(s.base_ref.as_deref(), Some("develop"));
        assert_eq!(s.to, None);
        assert_eq!(
            names(&s),
            ["a.txt", "c.txt", "img.bin", "novo.rs", "velho.txt"]
        );
        let a = by_path(&s, "a.txt");
        assert_eq!(
            (a.status, a.additions, a.deletions),
            (DiffStatus::Modified, Some(3), Some(1))
        );
        let c = by_path(&s, "c.txt");
        assert_eq!(c.status, DiffStatus::Renamed);
        assert_eq!(c.old_path.as_deref(), Some("b.txt"));
        assert_eq!((c.additions, c.deletions), (Some(0), Some(0)));
        let img = by_path(&s, "img.bin");
        assert!(img.binary && img.additions.is_none());
        assert_eq!(by_path(&s, "velho.txt").status, DiffStatus::Deleted);
        let n = by_path(&s, "novo.rs");
        assert!(n.untracked && n.status == DiffStatus::Added, "{n:?}");
        // O mesmo que `git diff $(git merge-base develop HEAD)` + o novo.
        let mb = git(&d, &["merge-base", "develop", "HEAD"]);
        let plain = git(&d, &["diff", "--name-only", mb.trim()]);
        let mut esperado: Vec<&str> = plain.lines().collect();
        esperado.push("novo.rs");
        esperado.sort();
        assert_eq!(names(&s), esperado);
        // O index da worktree não mudou: o novo continua não rastreado.
        assert_eq!(
            git(&d, &["status", "--porcelain", "novo.rs"]),
            "?? novo.rs\n"
        );
    }

    #[test]
    fn nao_commitado_bate_com_diff_head_mais_novos() {
        let (_tmp, d) = repo();
        let vazio = diff_summary(&d, &DiffMode::Uncommitted).unwrap();
        assert!(vazio.files.is_empty(), "{vazio:?}");
        fs::write(d.join("a.txt"), "outro\n").unwrap();
        fs::write(d.join("stage.txt"), "s\n").unwrap();
        git(&d, &["add", "stage.txt"]);
        fs::write(d.join("solto.txt"), "n\n").unwrap();
        let s = diff_summary(&d, &DiffMode::Uncommitted).unwrap();
        assert_eq!(names(&s), ["a.txt", "solto.txt", "stage.txt"]);
        assert!(!by_path(&s, "stage.txt").untracked);
        assert!(by_path(&s, "solto.txt").untracked);
        let p = diff_file(&d, &DiffMode::Uncommitted, "a.txt", None).unwrap();
        let plain = git(&d, &["diff", "--no-color", "HEAD", "--", "a.txt"]);
        assert_eq!(p.patch.as_deref(), Some(plain.as_str()));
        let n = diff_file(&d, &DiffMode::Uncommitted, "solto.txt", None).unwrap();
        assert!(n.patch.unwrap().contains("+n\n"));
    }

    #[test]
    fn commit_bate_com_show_e_inicial_funciona() {
        let (_tmp, d) = repo();
        let log = branch_commits(&d, "develop").unwrap();
        let subjects: Vec<_> = log.commits.iter().map(|c| c.subject.as_str()).collect();
        assert_eq!(subjects, ["renomeia b", "muda a"]);
        assert_eq!(log.base_ref, "develop");
        let c = &log.commits[0];
        assert!(c.date > 0 && c.author == "Fulano" && !c.merge);
        let s = diff_summary(&d, &DiffMode::Commit(c.short.clone())).unwrap();
        assert_eq!(names(&s), ["c.txt", "img.bin"]);
        assert_eq!(s.to.as_deref(), Some(c.sha.as_str()));
        let p = diff_file(&d, &DiffMode::Commit(c.sha.clone()), "c.txt", Some("b.txt")).unwrap();
        let show = git(
            &d,
            &[
                "show",
                "--format=",
                "-M",
                "--no-color",
                &c.sha,
                "--",
                "b.txt",
                "c.txt",
            ],
        );
        assert_eq!(p.patch.as_deref(), Some(show.as_str()));
        assert!(show.contains("rename from b.txt"), "{show}");

        // Commit inicial: sem pai, contra a árvore vazia.
        let root = git(&d, &["rev-list", "--max-parents=0", "HEAD"]);
        let s = diff_summary(&d, &DiffMode::Commit(root.trim().into())).unwrap();
        assert_eq!(names(&s), [".gitignore", "a.txt"]);
        assert!(s.files.iter().all(|f| f.status == DiffStatus::Added));
        let p = diff_file(&d, &DiffMode::Commit(root.trim().into()), "a.txt", None).unwrap();
        let show = git(
            &d,
            &[
                "show",
                "--format=",
                "--no-color",
                root.trim(),
                "--",
                "a.txt",
            ],
        );
        assert_eq!(p.patch.as_deref(), Some(show.as_str()));
    }

    #[test]
    fn commit_precisa_ser_valido_nesta_worktree() {
        let (_tmp, d) = repo();
        for c in [
            "",
            "abc",
            "HEAD",
            "--all",
            "develop",
            "zzzz",
            "0123456789abcdef0123",
        ] {
            assert!(
                diff_summary(&d, &DiffMode::Commit(c.into())).is_err(),
                "{c}"
            );
        }
        // Commit que existe no repositório mas não está na branch.
        git(&d, &["checkout", "-q", "-b", "outra", "develop"]);
        fs::write(d.join("z.txt"), "z").unwrap();
        git(&d, &["add", "z.txt"]);
        git(&d, &["commit", "-q", "-m", "fora"]);
        let fora = git(&d, &["rev-parse", "HEAD"]);
        git(&d, &["checkout", "-q", "feat"]);
        let e = diff_summary(&d, &DiffMode::Commit(fora.trim().into())).unwrap_err();
        assert!(e.to_string().contains("não faz parte da branch"), "{e}");
    }

    #[test]
    fn base_origin_tem_preferencia_e_base_inexistente_falha() {
        let (_tmp, d) = repo();
        let e = diff_summary(
            &d,
            &DiffMode::Branch {
                base: "main".into(),
            },
        )
        .unwrap_err();
        assert!(e.to_string().contains("não encontrada"), "{e}");
        assert!(diff_summary(&d, &DiffMode::Branch { base: "-x".into() }).is_err());
        let first = git(&d, &["rev-list", "--max-parents=0", "HEAD"]);
        git(
            &d,
            &["update-ref", "refs/remotes/origin/develop", first.trim()],
        );
        let s = diff_summary(&d, &branch()).unwrap();
        assert_eq!(s.base_ref.as_deref(), Some("origin/develop"));
        assert_eq!(s.from, first.trim());
    }

    #[test]
    fn patch_grande_demais_e_caminho_invalido() {
        let (_tmp, d) = repo();
        let p = diff_file_limit(&d, &branch(), "a.txt", None, 10).unwrap();
        assert!(p.too_large && p.patch.is_none());
        for bad in ["", "/etc/passwd", "../x", "a/../../x", "-p"] {
            assert!(diff_file(&d, &branch(), bad, None).is_err(), "{bad}");
        }
        let bin = diff_file(&d, &branch(), "img.bin", None).unwrap();
        assert!(bin.patch.unwrap().contains("Binary files"));
    }

    #[test]
    fn caminho_com_glob_e_espaco_e_literal() {
        let (_tmp, d) = repo();
        fs::write(d.join("x [1] *.txt"), "a\n").unwrap();
        fs::write(d.join("x1.txt"), "b\n").unwrap();
        let s = diff_summary(&d, &DiffMode::Uncommitted).unwrap();
        assert_eq!(names(&s), ["x [1] *.txt", "x1.txt"]);
        let p = diff_file(&d, &DiffMode::Uncommitted, "x [1] *.txt", None).unwrap();
        let patch = p.patch.unwrap();
        assert!(patch.contains("+a\n") && !patch.contains("+b\n"), "{patch}");
    }
}
