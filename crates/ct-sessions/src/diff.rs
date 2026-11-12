//! Diff de código do work item para o visualizador da UI (#168):
//! `git.diff`, `git.diff_file` e `git.commits`.
//!
//! O daemon só lê a worktree de um work item de projeto (não o gestor, nem
//! o clone principal do #0), e só se ela for de fato uma worktree
//! secundária registrada no clone do projeto. O git em si está no
//! [`ct_git::diff`].

use crate::{Sessions, SessionsError};
use ct_core::ProjectKind;
use ct_git::diff::{
    branch_commits, diff_file, diff_summary, BranchCommits, DiffMode, DiffPatch, DiffSummary,
};
use std::path::{Path, PathBuf};

/// Modo pedido pela UI (`mode` e, no `commit`, o hash).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffRequest {
    Branch,
    Uncommitted,
    Commit(String),
}

impl DiffRequest {
    /// `mode`: `branch`, `uncommitted` ou `commit` (com `commit`).
    pub fn parse(mode: &str, commit: Option<&str>) -> crate::Result<Self> {
        match (mode, commit) {
            ("branch", None) => Ok(Self::Branch),
            ("uncommitted", None) => Ok(Self::Uncommitted),
            ("commit", Some(c)) => Ok(Self::Commit(c.to_string())),
            ("commit", None) => Err(invalid("o modo commit pede o commit")),
            ("branch" | "uncommitted", Some(_)) => Err(invalid("commit só vale no modo commit")),
            (m, _) => Err(invalid(format!(
                "modo de diff desconhecido: {m} (branch, uncommitted ou commit)"
            ))),
        }
    }
}

fn invalid(msg: impl Into<String>) -> SessionsError {
    SessionsError::Invalid(msg.into())
}

impl Sessions {
    /// Worktree do work item e a branch base do projeto, validadas.
    pub fn diff_worktree(&self, work_item_id: i64) -> crate::Result<(PathBuf, String)> {
        let (wi, project) = {
            let l = self.ledger();
            let wi = l.get_work_item(work_item_id)?;
            let p = l.get_project(wi.project_id)?;
            (wi, p)
        };
        if project.kind == ProjectKind::Gestor {
            return Err(invalid("o gestor de projetos não tem diff de código"));
        }
        let Some(wt) = wi.worktree_path.filter(|w| !w.is_empty()) else {
            return Err(invalid(format!(
                "o work item #{} não tem worktree neste ambiente",
                wi.gh_number
            )));
        };
        let wt = PathBuf::from(wt);
        validate_worktree(Path::new(&project.repo_path), &wt)?;
        Ok((wt, project.base_branch))
    }

    fn diff_mode(
        &self,
        work_item_id: i64,
        req: &DiffRequest,
    ) -> crate::Result<(PathBuf, DiffMode)> {
        let (wt, base) = self.diff_worktree(work_item_id)?;
        let mode = match req {
            DiffRequest::Branch => DiffMode::Branch { base },
            DiffRequest::Uncommitted => DiffMode::Uncommitted,
            DiffRequest::Commit(c) => DiffMode::Commit(c.clone()),
        };
        Ok((wt, mode))
    }

    /// `git.diff`: a lista de arquivos do diff.
    pub fn git_diff(&self, work_item_id: i64, req: &DiffRequest) -> crate::Result<DiffSummary> {
        let (wt, mode) = self.diff_mode(work_item_id, req)?;
        Ok(diff_summary(&wt, &mode)?)
    }

    /// `git.diff_file`: o patch de um arquivo.
    pub fn git_diff_file(
        &self,
        work_item_id: i64,
        req: &DiffRequest,
        path: &str,
        old_path: Option<&str>,
    ) -> crate::Result<DiffPatch> {
        let (wt, mode) = self.diff_mode(work_item_id, req)?;
        Ok(diff_file(&wt, &mode, path, old_path)?)
    }

    /// `git.commits`: os commits da branch desde o merge-base.
    pub fn git_commits(&self, work_item_id: i64) -> crate::Result<BranchCommits> {
        let (wt, base) = self.diff_worktree(work_item_id)?;
        Ok(branch_commits(&wt, &base)?)
    }
}

/// `wt` existe e é uma worktree secundária registrada em `repo` (o clone
/// principal fica de fora: nele não há branch do work item).
fn validate_worktree(repo: &Path, wt: &Path) -> crate::Result<()> {
    if !wt.is_dir() {
        return Err(invalid(format!(
            "a worktree {} não existe mais neste ambiente",
            wt.display()
        )));
    }
    let list = ct_git::list_worktrees(repo)?;
    let canon = wt.canonicalize()?;
    let same = |p: &Path| p.canonicalize().is_ok_and(|c| c == canon);
    match list.iter().position(|e| same(&e.path)) {
        Some(i) if i > 0 => Ok(()),
        Some(_) => Err(invalid(
            "o clone principal do projeto não tem diff de work item",
        )),
        None => Err(invalid(format!(
            "{} não é uma worktree do projeto",
            wt.display()
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{FakeClaude, TestEnv};
    use std::fs;

    #[test]
    fn modo_pedido() {
        assert_eq!(
            DiffRequest::parse("branch", None).unwrap(),
            DiffRequest::Branch
        );
        assert_eq!(
            DiffRequest::parse("commit", Some("abcd")).unwrap(),
            DiffRequest::Commit("abcd".into())
        );
        assert!(DiffRequest::parse("commit", None).is_err());
        assert!(DiffRequest::parse("branch", Some("abcd")).is_err());
        assert!(DiffRequest::parse("tudo", None).is_err());
    }

    /// Lê a worktree do work item; recusa o work item sem worktree, a
    /// worktree apagada, o caminho que não é worktree do projeto e o
    /// clone principal.
    #[test]
    fn so_le_worktrees_de_work_items_do_projeto() {
        let env = TestEnv::new(FakeClaude::exit(0));
        let s = env.sessions.start(env.request(42, "Diff")).unwrap();
        let wt = PathBuf::from(&s.cwd);
        fs::write(wt.join("novo.txt"), "oi\n").unwrap();
        let d = env
            .sessions
            .git_diff(s.work_item_id, &DiffRequest::Branch)
            .unwrap();
        assert_eq!(d.files.len(), 1, "{d:?}");
        assert!(d.files[0].untracked);
        let p = env
            .sessions
            .git_diff_file(s.work_item_id, &DiffRequest::Uncommitted, "novo.txt", None)
            .unwrap();
        assert!(p.patch.unwrap().contains("+oi"));
        assert!(env
            .sessions
            .git_commits(s.work_item_id)
            .unwrap()
            .commits
            .is_empty());

        // Commit pedido precisa existir na worktree.
        let e = env
            .sessions
            .git_diff(s.work_item_id, &DiffRequest::Commit("deadbeef".into()))
            .unwrap_err();
        assert!(e.to_string().contains("não encontrado"), "{e}");

        // Worktree trocada para uma pasta qualquer, para o clone principal
        // e para uma que não existe.
        let set = |path: &Path| {
            env.sessions
                .ledger()
                .set_work_item_branch(s.work_item_id, "feat/42-diff", &path.display().to_string())
                .unwrap();
        };
        let outra = env.tmp.path().join("outra");
        fs::create_dir_all(&outra).unwrap();
        crate::test_support::git_in(&outra, &["init", "-q"]);
        set(&outra);
        let e = env
            .sessions
            .git_diff(s.work_item_id, &DiffRequest::Branch)
            .unwrap_err();
        assert!(
            e.to_string().contains("não é uma worktree do projeto"),
            "{e}"
        );
        set(&env.repo);
        let e = env
            .sessions
            .git_diff(s.work_item_id, &DiffRequest::Branch)
            .unwrap_err();
        assert!(e.to_string().contains("clone principal"), "{e}");
        set(&env.tmp.path().join("sumiu"));
        let e = env.sessions.git_commits(s.work_item_id).unwrap_err();
        assert!(e.to_string().contains("não existe mais"), "{e}");
    }
}
