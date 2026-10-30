//! `gh pr create` numa sessão de issue sem `Closes #n` (#375). O GitHub só
//! fecha a issue ao mesclar o PR com as palavras-chave em inglês; "Fecha #n"
//! deixa a issue aberta. Como o resto do guarda-corpo, é uma barreira contra
//! erro, não contra adversário: corpo que o hook não consegue ler passa.

use super::tokenize::split_segments;
use super::{skip_env_assignments, Verdict};

/// Palavras-chave de fechamento do GitHub (sem diferenciar maiúsculas).
const KEYWORDS: &[&str] = &[
    "close", "closes", "closed", "fix", "fixes", "fixed", "resolve", "resolves", "resolved",
];

const MAX_DEPTH: u8 = 3;

/// `issue` é o número da issue da sessão; `read_file` lê o arquivo de
/// `--body-file` (caminho literal do comando), `None` se não conseguir.
pub fn check_pr_closes(
    cmd: &str,
    issue: i64,
    read_file: &dyn Fn(&str) -> Option<String>,
) -> Verdict {
    check_segments(cmd, cmd, issue, read_file, 0)
}

fn check_segments(
    cmd: &str,
    raw: &str,
    issue: i64,
    read_file: &dyn Fn(&str) -> Option<String>,
    depth: u8,
) -> Verdict {
    for seg in split_segments(cmd) {
        let v = check_segment(&seg, raw, issue, read_file, depth);
        if v != Verdict::Allow {
            return v;
        }
    }
    Verdict::Allow
}

fn check_segment(
    words: &[String],
    raw: &str,
    issue: i64,
    read_file: &dyn Fn(&str) -> Option<String>,
    depth: u8,
) -> Verdict {
    let words = skip_env_assignments(words);
    let Some(first) = words.first() else {
        return Verdict::Allow;
    };
    match first.rsplit('/').next().unwrap_or(first) {
        "gh" => check_gh(&words[1..], raw, issue, read_file),
        "bash" | "sh" | "zsh" | "dash" if depth < MAX_DEPTH => {
            match words
                .iter()
                .position(|w| w.starts_with('-') && w.ends_with('c'))
                .and_then(|i| words.get(i + 1))
            {
                Some(inner) => check_segments(inner, raw, issue, read_file, depth + 1),
                None => Verdict::Allow,
            }
        }
        "sudo" | "env" | "command" | "exec" | "nohup" | "time" | "nice" if depth < MAX_DEPTH => {
            let rest: Vec<String> = words[1..]
                .iter()
                .skip_while(|w| w.starts_with('-'))
                .cloned()
                .collect();
            check_segment(&rest, raw, issue, read_file, depth + 1)
        }
        _ => Verdict::Allow,
    }
}

fn check_gh(
    args: &[String],
    raw: &str,
    issue: i64,
    read_file: &dyn Fn(&str) -> Option<String>,
) -> Verdict {
    let mut pos: Vec<&str> = Vec::new();
    let mut body_file: Option<&str> = None;
    let mut it = args.iter().map(String::as_str);
    while let Some(a) = it.next() {
        match a {
            "-F" | "--body-file" => body_file = it.next(),
            _ if a.starts_with("--body-file=") => body_file = a.split_once('=').map(|(_, v)| v),
            // Opções de `gh` e de `gh pr create` que recebem valor.
            "-R" | "--repo" | "-b" | "--body" | "-t" | "--title" | "-B" | "--base" | "-H"
            | "--head" | "-a" | "--assignee" | "-l" | "--label" | "-m" | "--milestone" | "-p"
            | "--project" | "-r" | "--reviewer" | "-T" | "--template" => {
                it.next();
            }
            _ if a.starts_with('-') => {}
            _ => pos.push(a),
        }
    }
    if pos.get(..2) != Some(&["pr", "create"][..]) {
        return Verdict::Allow;
    }
    // O comando inteiro conta: o corpo pode vir de `$(cat <<EOF ...)` ou de
    // um heredoc em `--body-file -`, que a tokenização descarta.
    if closes(raw, issue) {
        return Verdict::Allow;
    }
    match body_file.filter(|f| *f != "-").map(read_file) {
        Some(Some(body)) if closes(&body, issue) => Verdict::Allow,
        // Arquivo ilegível: não há como conferir.
        Some(None) => Verdict::Allow,
        _ => Verdict::Block(format!(
            "`gh pr create` sem `Closes #{issue}` no corpo: o GitHub só fecha a issue com \
             Closes, Fixes ou Resolves (\"Fecha #{issue}\" não fecha). Ponha `Closes #{issue}` \
             no corpo do PR (`--body` ou `--body-file`) e rode de novo"
        )),
    }
}

/// O texto tem `<palavra-chave> #n` (ou `owner/repo#n`, ou a URL da issue)
/// para a issue `n`.
pub fn closes(text: &str, issue: i64) -> bool {
    let lower = text.to_lowercase();
    let n = issue.to_string();
    let mut words = lower
        .split(|c: char| c.is_whitespace())
        .filter(|w| !w.is_empty())
        .peekable();
    while let Some(w) = words.next() {
        // Palavra-chave no fim da palavra (ex.: "- closes", "(closes"), com
        // ":" opcional ("Closes: #n").
        let w = w.trim_end_matches(':');
        let kw = w.trim_start_matches(|c: char| !c.is_ascii_alphanumeric());
        if !KEYWORDS.contains(&kw) {
            continue;
        }
        let Some(next) = words.peek() else {
            break;
        };
        // Pontuação e aspas no fim (o comando cru traz as aspas do shell).
        let target = next.trim_end_matches(|c: char| !c.is_ascii_alphanumeric());
        let num = if let Some((repo, num)) = target.split_once('#') {
            (repo.is_empty() || repo.split('/').count() == 2).then_some(num)
        } else if target.starts_with("https://") || target.starts_with("http://") {
            target.rsplit_once("/issues/").map(|(_, num)| num)
        } else {
            None
        };
        if num == Some(n.as_str()) {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(cmd: &str) -> Verdict {
        check_pr_closes(cmd, 375, &|_| None)
    }

    fn blocked(v: &Verdict) -> bool {
        matches!(v, Verdict::Block(_))
    }

    #[test]
    fn palavras_chave_do_github() {
        for t in [
            "Closes #375",
            "closes #375.",
            "Fixes: #375",
            "- Resolved #375",
            "Corrige o bug.\n\nCloses deep-ion-ai/ctower-agents#375",
            "Fix https://github.com/deep-ion-ai/ctower-agents/issues/375",
            "(closes #375)",
        ] {
            assert!(closes(t, 375), "{t}");
        }
        for t in [
            "Fecha #375",
            "Closes #3750",
            "Closes #37",
            "Refs #375",
            "Closes 375",
            "Closes a/b/c#375",
            "encloses #375",
            "Closes",
        ] {
            assert!(!closes(t, 375), "{t}");
        }
    }

    #[test]
    fn pr_create_sem_closes_bloqueia() {
        for cmd in [
            "gh pr create -t 'fix: x' -b 'Fecha #375'",
            "gh pr create --title x --body 'Closes #37'",
            "git push -u origin HEAD && gh pr create --fill",
            "gh -R deep-ion-ai/ctower-agents pr create -b 'nada'",
            "bash -c \"gh pr create -b 'Fecha #375'\"",
            "GH_PROMPT_DISABLED=1 gh pr create -b x",
        ] {
            assert!(blocked(&check(cmd)), "{cmd}");
        }
        let Verdict::Block(r) = check("gh pr create -b x") else {
            unreachable!()
        };
        assert!(r.contains("Closes #375"), "{r}");
    }

    #[test]
    fn pr_create_com_closes_passa() {
        for cmd in [
            "gh pr create -t 'fix: x' -b 'Closes #375'",
            "gh pr create --body=\"Corrige.\n\nFixes #375\"",
            "gh pr create -t x --body \"$(cat <<'EOF'\n## Resumo\n\nCloses #375\nEOF\n)\"",
            "gh pr create -t x --body-file - <<'EOF'\nResolves #375\nEOF",
        ] {
            assert_eq!(check(cmd), Verdict::Allow, "{cmd}");
        }
    }

    #[test]
    fn body_file_e_lido() {
        let read = |p: &str| match p {
            "/tmp/ok.md" => Some("Corrige.\n\nCloses #375\n".to_string()),
            "/tmp/ruim.md" => Some("Fecha #375".to_string()),
            _ => None,
        };
        let c = |cmd: &str| check_pr_closes(cmd, 375, &read);
        assert_eq!(c("gh pr create -t x -F /tmp/ok.md"), Verdict::Allow);
        assert_eq!(
            c("gh pr create -t x --body-file=/tmp/ok.md"),
            Verdict::Allow
        );
        assert!(blocked(&c("gh pr create -t x --body-file /tmp/ruim.md")));
        // Ilegível: não dá para conferir, passa.
        assert_eq!(c("gh pr create -t x -F /tmp/sumiu.md"), Verdict::Allow);
    }

    #[test]
    fn outros_comandos_passam() {
        for cmd in [
            "gh pr view 5",
            "gh pr edit 5 -b x",
            "gh issue create -t x -b y",
            "gh pr list",
            "git commit -m 'pr create'",
            "echo gh pr create",
        ] {
            assert_eq!(check(cmd), Verdict::Allow, "{cmd}");
        }
    }
}
