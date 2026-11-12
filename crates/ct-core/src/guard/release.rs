//! Perfil `release` do guarda-corpo (#204), para sessões com
//! `CTOWER_SESSION_KIND=release`. Soma-se ao de branch protegida.
//!
//! A sessão de release roda no clone principal só para leitura e só mescla
//! e aprova pelo `ctower release …`, que confere as pré-condições e usa
//! sempre merge commit preso ao SHA. Por isso o perfil bloqueia:
//!
//! - no git, o que escreve no clone ou no remoto: `push`, `merge`, `commit`,
//!   `rebase`, `cherry-pick`, `revert`, `am`, `pull`, `reset` e `tag`;
//! - no `gh`, `pr merge`, `pr review` e as escritas de `release` (a tag e a
//!   release são do workflow);
//! - toda escrita pela `gh api`, exceto reexecutar um run
//!   (`.../actions/runs/<id>/rerun` e `rerun-failed-jobs`).
//!
//! `gh run rerun` e `gh workflow run` continuam liberados: reexecutar jobs
//! faz parte da autonomia da sessão. Como o resto do guarda-corpo, é uma
//! barreira contra erro, não contra adversário.

use super::gestor::{api_call, git_subcommand, positional};
use super::tokenize::split_segments;
use super::{skip_env_assignments, Verdict};

const MAX_DEPTH: u8 = 3;

const GIT_WRITES: &[&str] = &[
    "push",
    "merge",
    "commit",
    "rebase",
    "cherry-pick",
    "revert",
    "am",
    "pull",
    "reset",
    "tag",
];

const RELEASE_WRITES: &[&str] = &["create", "edit", "delete", "upload", "delete-asset"];

/// Veredito do perfil `release` para um comando do shell.
pub fn check_release(cmd: &str) -> Verdict {
    check_segments(cmd, 0)
}

fn check_segments(cmd: &str, depth: u8) -> Verdict {
    split_segments(cmd)
        .iter()
        .map(|seg| check_segment(seg, depth))
        .fold(Verdict::Allow, Verdict::max)
}

fn check_segment(words: &[String], depth: u8) -> Verdict {
    let words = skip_env_assignments(words);
    let words: Vec<&str> = words
        .iter()
        .map(String::as_str)
        .skip_while(|w| matches!(*w, "{" | "}" | "!" | "then" | "do" | "else" | "elif" | "if"))
        .collect();
    let Some(first) = words.first() else {
        return Verdict::Allow;
    };
    let prog = first.rsplit('/').next().unwrap_or(first);
    match prog {
        "git" => check_git(&words[1..]),
        "gh" => check_gh(&words[1..]),
        "bash" | "sh" | "zsh" | "dash" if depth < MAX_DEPTH => {
            match words
                .iter()
                .position(|w| w.starts_with('-') && w.ends_with('c'))
            {
                Some(i) => words
                    .get(i + 1)
                    .map_or(Verdict::Allow, |inner| check_segments(inner, depth + 1)),
                None => Verdict::Allow,
            }
        }
        "sudo" | "env" | "command" | "exec" | "nohup" | "time" | "nice" if depth < MAX_DEPTH => {
            let rest: Vec<String> = words[1..]
                .iter()
                .skip_while(|w| w.starts_with('-'))
                .map(|w| w.to_string())
                .collect();
            check_segment(&rest, depth + 1)
        }
        _ => Verdict::Allow,
    }
}

fn check_git(args: &[&str]) -> Verdict {
    match git_subcommand(args) {
        Some(sub) if GIT_WRITES.contains(&sub) => Verdict::Block(format!(
            "a sessão de release não faz `git {sub}`: o clone é só leitura, e merges e \
             aprovações passam pelo `ctower release …`"
        )),
        _ => Verdict::Allow,
    }
}

fn check_gh(args: &[&str]) -> Verdict {
    let pos = positional(args);
    let (Some(&group), sub) = (pos.first(), pos.get(1).copied()) else {
        return Verdict::Allow;
    };
    match (group, sub) {
        ("pr", Some(s @ ("merge" | "review"))) => Verdict::Block(format!(
            "a sessão de release não faz `gh pr {s}`: use `ctower release merge-promotion`, \
             `merge-release` ou `merge-back`, que mesclam com merge commit preso ao SHA"
        )),
        ("release", Some(s)) if RELEASE_WRITES.contains(&s) => Verdict::Block(format!(
            "a sessão de release não faz `gh release {s}`: a tag e a release são do workflow \
             do release-please"
        )),
        ("api", _) => check_api(args),
        _ => Verdict::Allow,
    }
}

/// `gh api`: só leitura, exceto reexecutar um run.
fn check_api(args: &[&str]) -> Verdict {
    let (method, path) = api_call(args);
    if method == "GET" {
        return Verdict::Allow;
    }
    let path_only = path.split('?').next().unwrap_or(path).trim_end_matches('/');
    let rerun = method == "POST"
        && path_only.contains("/actions/runs/")
        && (path_only.ends_with("/rerun") || path_only.ends_with("/rerun-failed-jobs"));
    if rerun {
        return Verdict::Allow;
    }
    Verdict::Block(format!(
        "a sessão de release só lê pela `gh api` (`{method} {path}` recusado): merges e \
         aprovações passam pelo `ctower release …`"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blocked(cmd: &str) -> bool {
        matches!(check_release(cmd), Verdict::Block(_))
    }

    #[test]
    fn bloqueia_escritas_do_git() {
        for cmd in [
            "git push origin main",
            "git push",
            "git -C /x push origin HEAD:develop",
            "git merge develop",
            "git commit -m x",
            "git rebase main",
            "git reset --hard origin/main",
            "git tag v1.2.3",
            "git pull",
            "cd /x && git push origin develop",
            "bash -c 'git merge main'",
            "env FOO=1 git push",
        ] {
            assert!(blocked(cmd), "{cmd}");
        }
        for cmd in [
            "git fetch origin",
            "git log --oneline main..develop",
            "git diff main develop --stat",
            "git status",
        ] {
            assert!(!blocked(cmd), "{cmd}");
        }
    }

    #[test]
    fn bloqueia_merge_review_e_release_do_gh() {
        for cmd in [
            "gh pr merge 12 --merge",
            "gh pr merge 12 --squash --admin",
            "gh -R acme/acme-web pr merge 12",
            "gh pr review 12 --approve",
            "gh release create v1.2.3",
            "gh release edit v1.2.3 --draft=false",
        ] {
            assert!(blocked(cmd), "{cmd}");
        }
        for cmd in [
            "gh pr view 12 --json headRefOid",
            "gh pr list --base main",
            "gh pr checks 12",
            "gh run rerun 99 --failed",
            "gh run list --branch main",
            "gh run view 99 --log-failed",
            "gh release view v1.2.3",
            "gh workflow run ui -f tag=v1.2.3",
            "gh pr comment 12 --body-file /tmp/x",
        ] {
            assert!(!blocked(cmd), "{cmd}");
        }
    }

    #[test]
    fn gh_api_so_leitura_e_rerun() {
        for cmd in [
            "gh api -X PUT repos/acme/acme-web/pulls/12/merge -f merge_method=merge",
            "gh api repos/acme/acme-web/merges -f base=main -f head=develop",
            "gh api -X PATCH repos/acme/acme-web/git/refs/heads/main -f sha=abc",
            "gh api --method=POST repos/acme/acme-web/actions/runs/9/approve",
            "gh api -X DELETE repos/acme/acme-web/git/refs/heads/develop",
        ] {
            assert!(blocked(cmd), "{cmd}");
        }
        for cmd in [
            "gh api repos/acme/acme-web/compare/main...develop",
            "gh api -X POST repos/acme/acme-web/actions/runs/9/rerun-failed-jobs",
            "gh api -X POST repos/acme/acme-web/actions/runs/9/rerun",
        ] {
            assert!(!blocked(cmd), "{cmd}");
        }
    }

    #[test]
    fn ctower_release_passa() {
        for cmd in [
            "ctower release merge-promotion",
            "ctower release approve-runs 12",
            "ctower release promote --body-file /tmp/corpo.md",
        ] {
            assert!(!blocked(cmd), "{cmd}");
        }
    }
}
