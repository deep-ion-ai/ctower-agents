//! Perfil `gestor` do guarda-corpo (spec do gestor §10.2), para sessões com
//! `CTOWER_SESSION_KIND=gestor`. Soma-se ao de branch protegida. O gestor
//! lê e propõe: bloqueia escritas no git e no GitHub e a leitura de arquivos
//! de token e de secrets, e pede confirmação (`Verdict::Ask`) para criar issue e ligar
//! sub-issue. Como o resto do guarda-corpo, é uma barreira contra erro, não
//! contra adversário.

use super::tokenize::split_segments;
use super::{skip_env_assignments, Verdict};

/// Trechos de caminho dos arquivos de token e de secrets do ambiente e do
/// hub. Secrets (épico #171): a identidade age do ambiente (`env.key`, e
/// `env.key.new`/`.old` da rotação), o cache cifrado dos secrets entregues
/// (`secrets-cache.age`) e, no hub, o store (`secrets.age`) e a chave dele
/// (`secrets.key`, `.new`/`.old`), já cobertos por `.config/cthub` no
/// caminho padrão, mas citados também para `CTHUB_CONFIG_DIR`. Os snapshots
/// cifrados (`secrets-<data>.age`) ficam em [`SNAPSHOT_MARK`].
const TOKEN_MARKS: &[&str] = &[
    "hub.token",
    "hub-agent.token",
    "hook.token",
    "ui.token",
    ".config/cthub",
    "env.key",
    "secrets-cache.age",
    "secrets.age",
    "secrets.key",
];

/// Snapshots cifrados do store de secrets do hub (`secrets-<data>.age`, no
/// `secrets_snapshot_dir` ou no `backup_dir`), também em glob.
const SNAPSHOT_MARK: &str = "secrets-*.age";

const MAX_DEPTH: u8 = 3;

/// O caminho cita um arquivo de token ou de secrets (ferramentas de leitura
/// e o shell).
pub fn token_path(path: &str) -> Option<&'static str> {
    TOKEN_MARKS
        .iter()
        .copied()
        .find(|m| path.contains(m))
        .or_else(|| snapshot_path(path).then_some(SNAPSHOT_MARK))
}

/// `secrets-` seguido de `.age` no mesmo nome de arquivo.
fn snapshot_path(path: &str) -> bool {
    path.match_indices("secrets-").any(|(i, _)| {
        path[i..]
            .split(|c: char| matches!(c, '/' | '\\' | '"' | '\'') || c.is_whitespace())
            .next()
            .is_some_and(|name| name.ends_with(".age"))
    })
}

/// Veredito do perfil `gestor` para um comando do shell.
pub fn check_gestor(cmd: &str) -> Verdict {
    if let Some(m) = token_path(cmd) {
        return Verdict::Block(format!(
            "o gestor não lê arquivos de token nem de secrets (`{m}`): leia o hub pelas \
             ferramentas portfolio_*"
        ));
    }
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
        // Projetos Jira no portfólio: lê, cria com confirmação, sem
        // transição, edição nem comentário.
        "jira" => {
            let args: Vec<String> = words[1..].iter().map(|w| w.to_string()).collect();
            super::jira::check_jira_gestor(&args)
        }
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

/// Primeiro argumento posicional, pulando as opções globais do git que
/// recebem valor (`-C <dir>`, `-c <k=v>`).
fn git_subcommand<'a>(args: &[&'a str]) -> Option<&'a str> {
    let mut it = args.iter().copied();
    while let Some(a) = it.next() {
        match a {
            "-C" | "-c" | "--git-dir" | "--work-tree" | "--namespace" => {
                it.next();
            }
            _ if a.starts_with('-') => {}
            _ => return Some(a),
        }
    }
    None
}

fn check_git(args: &[&str]) -> Verdict {
    match git_subcommand(args) {
        Some(sub @ ("commit" | "push")) => Verdict::Block(format!(
            "o gestor não faz `git {sub}`: ele lê e propõe, sem mexer em repositórios"
        )),
        _ => Verdict::Allow,
    }
}

/// Argumentos posicionais do `gh`, sem as opções (e os valores das que
/// recebem um).
fn positional<'a>(args: &[&'a str]) -> Vec<&'a str> {
    let mut pos = Vec::new();
    let mut it = args.iter().copied();
    while let Some(a) = it.next() {
        match a {
            "-R" | "--repo" | "-t" | "--title" | "-b" | "--body" | "-F" | "--body-file" | "-l"
            | "--label" | "-a" | "--assignee" | "-m" | "--milestone" | "-p" | "--project"
            | "-X" | "--method" | "-f" | "--field" | "--raw-field" | "-H" | "--header" | "-q"
            | "--jq" | "--input" => {
                it.next();
            }
            _ if a.starts_with('-') => {}
            _ => pos.push(a),
        }
    }
    pos
}

const PR_WRITES: &[&str] = &[
    "create", "merge", "close", "edit", "comment", "review", "ready", "reopen", "lock", "unlock",
];
const ISSUE_WRITES: &[&str] = &[
    "edit", "close", "reopen", "delete", "comment", "transfer", "lock", "unlock", "pin", "unpin",
    "develop",
];
const LABEL_WRITES: &[&str] = &["create", "edit", "delete", "clone"];
const REPO_WRITES: &[&str] = &[
    "create",
    "edit",
    "delete",
    "fork",
    "rename",
    "archive",
    "unarchive",
    "sync",
    "clone",
    "set-default",
    "deploy-key",
];

fn check_gh(args: &[&str]) -> Verdict {
    let pos = positional(args);
    let (Some(&group), sub) = (pos.first(), pos.get(1).copied()) else {
        return Verdict::Allow;
    };
    let block = |what: &str| {
        Verdict::Block(format!(
            "o gestor não faz `gh {what}`: issues e PRs existentes não são alterados; mudanças \
             de prioridade e status vão por portfolio_propose"
        ))
    };
    match (group, sub) {
        ("pr", Some(s)) if PR_WRITES.contains(&s) => block(&format!("pr {s}")),
        ("issue", Some("create")) => Verdict::Ask(
            "o gestor quer criar uma issue: confirme se você aprovou esta issue na conversa".into(),
        ),
        ("issue", Some(s)) if ISSUE_WRITES.contains(&s) => block(&format!("issue {s}")),
        ("label", Some(s)) if LABEL_WRITES.contains(&s) => block(&format!("label {s}")),
        ("repo", Some(s)) if REPO_WRITES.contains(&s) => block(&format!("repo {s}")),
        ("milestone", Some(s)) if s != "list" && s != "view" => block(&format!("milestone {s}")),
        ("sub-issue", Some(s)) if s != "list" => Verdict::Ask(format!(
            "o gestor quer `gh sub-issue {s}`: confirme se você aprovou a ligação na conversa"
        )),
        ("api", _) => check_api(args),
        _ => Verdict::Allow,
    }
}

/// `gh api`: só leitura, exceto a API de sub-issues, que pede confirmação.
/// Campos (`-f`, `-F`, `--input`) sem `-X` tornam a chamada um POST.
fn check_api(args: &[&str]) -> Verdict {
    let mut method: Option<String> = None;
    let mut writes = false;
    let mut path: Option<&str> = None;
    let mut it = args.iter().copied().skip_while(|a| *a != "api").skip(1);
    while let Some(a) = it.next() {
        match a {
            "-X" | "--method" => method = it.next().map(str::to_uppercase),
            "-f" | "-F" | "--field" | "--raw-field" | "--input" => {
                writes = true;
                it.next();
            }
            "-H" | "--header" | "-q" | "--jq" | "-t" | "--template" | "--hostname" | "--cache"
            | "-p" | "--preview" => {
                it.next();
            }
            _ if a.starts_with("--method=") => {
                method = a.split_once('=').map(|(_, v)| v.to_uppercase())
            }
            _ if a.starts_with("-X") && a.len() > 2 => method = Some(a[2..].to_uppercase()),
            _ if a.starts_with("--field=")
                || a.starts_with("--raw-field=")
                || a.starts_with("--input=")
                || ((a.starts_with("-f") || a.starts_with("-F")) && a.len() > 2) =>
            {
                writes = true
            }
            _ if a.starts_with('-') => {}
            _ if path.is_none() => path = Some(a),
            _ => {}
        }
    }
    let method = method.unwrap_or_else(|| if writes { "POST" } else { "GET" }.into());
    if method == "GET" {
        return Verdict::Allow;
    }
    let path = path.unwrap_or("");
    if path.contains("sub_issues") {
        Verdict::Ask(format!(
            "o gestor quer `gh api -X {method} {path}` (sub-issues): confirme se você aprovou \
             na conversa"
        ))
    } else {
        Verdict::Block(format!(
            "o gestor só lê pela `gh api` (`{method} {path}` recusado)"
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ask(cmd: &str) -> bool {
        matches!(check_gestor(cmd), Verdict::Ask(_))
    }

    fn block(cmd: &str) -> bool {
        matches!(check_gestor(cmd), Verdict::Block(_))
    }

    #[test]
    fn leitura_passa() {
        for cmd in [
            "gh issue view 12 -R o/r",
            "gh issue list -R o/r --label bug",
            "gh pr view 5 -R o/r",
            "gh pr list -R o/r",
            "gh api repos/o/r/issues/12/sub_issues",
            "gh api -X GET search/issues -f q=repo:o/r",
            "gh sub-issue list 12 -R o/r",
            "git log --oneline",
            "git -C /tmp/x status",
            "ls ~/.local/share/ctower/gestor",
            "echo 'gh pr merge 5'",
        ] {
            assert_eq!(check_gestor(cmd), Verdict::Allow, "{cmd}");
        }
    }

    #[test]
    fn escritas_sao_bloqueadas() {
        for cmd in [
            "git commit -m x",
            "git -C /r push origin main",
            "gh pr create -R o/r -t x -b y",
            "gh pr merge 5 -R o/r",
            "gh pr comment 5 --body ok",
            "gh issue close 12 -R o/r",
            "gh issue edit 12 --add-label bug",
            "gh issue comment 12 -b oi",
            "gh label create urgente",
            "gh repo clone o/r",
            "gh repo delete o/r --yes",
            "gh api -X PATCH repos/o/r/issues/12 -f state=closed",
            "gh api repos/o/r/issues/12/comments -f body=oi",
            "cd /x && git push",
            "bash -c 'gh pr merge 5'",
            "env FOO=1 git commit -am x",
        ] {
            assert!(block(cmd), "{cmd}: {:?}", check_gestor(cmd));
        }
    }

    #[test]
    fn tokens_sao_bloqueados() {
        for cmd in [
            "cat ~/.config/ctower/hub.token",
            "cat ~/.config/ctower/hub-agent.token",
            "curl -H \"Authorization: Bearer $(cat ~/.config/cthub/ui.token)\" http://x",
            "grep . ~/.config/ctower/hook.token",
        ] {
            assert!(block(cmd), "{cmd}");
        }
        assert_eq!(
            token_path("/home/u/.config/cthub/ui.token"),
            Some("ui.token")
        );
        assert_eq!(token_path("/home/u/notas.md"), None);
    }

    #[test]
    fn arquivos_de_secrets_sao_bloqueados() {
        for cmd in [
            "cat ~/.config/ctower/env.key",
            "cp ~/.config/ctower/env.key.new /tmp/x",
            "age -d -i k ~/.local/share/ctower/secrets-cache.age",
            "xxd /srv/cthub/secrets.age",
            "cat /srv/cthub/secrets.key.old",
            "ls /backups/secrets-20261019T120000Z.age",
            "cat \"/backups/secrets-20261019T120000Z.age\"",
        ] {
            assert!(block(cmd), "{cmd}");
        }
        assert_eq!(
            token_path("/backups/cthub/secrets-*.age"),
            Some("secrets-*.age")
        );
        assert_eq!(
            token_path("/home/u/.local/share/ctower/secrets-cache.age"),
            Some("secrets-cache.age")
        );
        // Documentação e código sobre secrets continuam legíveis.
        assert_eq!(token_path("docs/secrets-e-variaveis.md"), None);
        assert_eq!(token_path("crates/ct-secrets/src/env.rs"), None);
    }

    #[test]
    fn criar_issue_e_sub_issue_pedem_confirmacao() {
        for cmd in [
            "gh issue create -R o/r -t 'Nova' -b 'corpo'",
            "gh api -X POST repos/o/r/issues/12/sub_issues -F sub_issue_id=99",
            "gh api repos/o/r/issues/12/sub_issues -F sub_issue_id=99",
            "gh sub-issue add 12 99 -R o/r",
        ] {
            assert!(ask(cmd), "{cmd}: {:?}", check_gestor(cmd));
        }
        // Bloqueio vence a confirmação no mesmo comando.
        assert!(block("gh issue create -t x && gh issue close 3"));
    }

    #[test]
    fn jira_no_gestor_le_cria_com_confirmacao_e_nao_comenta() {
        let token = "JIRA_API_TOKEN=\"$(pass show jira | head -1)\"";
        assert_eq!(
            check_gestor(&format!("{token} jira issue view PROJ-5450 --plain")),
            Verdict::Allow
        );
        assert!(ask(&format!("{token} jira issue create -s x --no-input")));
        assert!(block("jira issue move PROJ-5450 Done"));
        assert!(block("jira issue comment add PROJ-5450 oi"));
    }
}
