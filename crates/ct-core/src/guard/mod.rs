//! Guarda-corpo de branch protegida (spec §7.3). Puro: o chamador fornece o
//! HEAD atual, como descobrir o HEAD de outro diretório e a base de um PR.
//! É uma barreira contra erro, não contra adversário. No GitLab, o
//! `glab mr merge` é sempre bloqueado ([`glab::check_glab`]). Nos projetos
//! Jira, o perfil [`jira::check_jira`] bloqueia transição e edição.

pub mod closes;
pub mod gestor;
pub mod glab;
pub mod jira;
pub mod tokenize;

use tokenize::split_segments;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Allow,
    /// Pede a confirmação do usuário no Claude Code (`permissionDecision:
    /// "ask"`); os perfis `gestor` e Jira usam.
    Ask(String),
    Block(String),
}

impl Verdict {
    /// O mais restritivo dos dois (`Block` > `Ask` > `Allow`).
    pub fn max(self, other: Verdict) -> Verdict {
        match (&self, &other) {
            (Verdict::Block(_), _) => self,
            (_, Verdict::Block(_)) => other,
            (Verdict::Ask(_), _) => self,
            (_, Verdict::Ask(_)) => other,
            _ => Verdict::Allow,
        }
    }
}

pub struct GuardContext<'a> {
    /// Branch atual do worktree (cwd); `None` se desconhecida ou HEAD destacado.
    pub head_branch: Option<&'a str>,
    pub protected: &'a [String],
    /// Base do PR indicado, ou do PR da branch atual (do cwd) quando `None`.
    /// O alvo é passado como `gh pr view` o aceita: número (`"5"`), URL ou
    /// branch. Quando o comando tem `-R/--repo`, o alvo numérico vira URL
    /// (`"https://github.com/o/r/pull/5"`), preservando o repositório.
    /// Retorna `None` se não conseguir determinar.
    pub pr_base: &'a dyn Fn(Option<&str>) -> Option<String>,
    /// Branch do HEAD no diretório indicado (caminho literal do comando, como
    /// em `git -C <dir>`, `--git-dir`, `--work-tree` ou `cd <dir>`; relativo ao
    /// cwd quando não absoluto). `None` se desconhecida ou HEAD destacado.
    pub head_of: &'a dyn Fn(&str) -> Option<String>,
}

impl GuardContext<'_> {
    fn is_protected(&self, branch: &str) -> bool {
        self.protected.iter().any(|p| p == branch)
    }
}

/// Estado que se propaga entre segmentos: diretório após `cd` e HEAD efetivo.
#[derive(Clone)]
struct Loc {
    /// Diretório corrente relativo ao cwd do hook (`None` = o próprio cwd).
    dir: Option<String>,
    head: Option<String>,
}

const MAX_DEPTH: u8 = 3;

pub fn check_command(cmd: &str, ctx: &GuardContext) -> Verdict {
    let mut loc = Loc {
        dir: None,
        head: ctx.head_branch.map(str::to_string),
    };
    check_segments(cmd, ctx, 0, &mut loc)
}

fn check_segments(cmd: &str, ctx: &GuardContext, depth: u8, loc: &mut Loc) -> Verdict {
    for seg in split_segments(cmd) {
        let v = check_segment(&seg, ctx, depth, loc);
        if v != Verdict::Allow {
            return v;
        }
    }
    Verdict::Allow
}

/// Palavras de controle do shell que podem preceder o programa.
const KEYWORDS: &[&str] = &[
    "{", "}", "!", "then", "do", "else", "elif", "if", "while", "until",
];

fn check_segment(words: &[String], ctx: &GuardContext, depth: u8, loc: &mut Loc) -> Verdict {
    let mut words = words;
    loop {
        let n = words.len();
        words = skip_env_assignments(words);
        let k = words
            .iter()
            .take_while(|w| KEYWORDS.contains(&w.as_str()))
            .count();
        words = &words[k..];
        if words.len() == n {
            break;
        }
    }
    let Some(first) = words.first() else {
        return Verdict::Allow;
    };
    let prog = first.rsplit('/').next().unwrap_or(first);
    match prog {
        "git" => check_git(&words[1..], ctx, loc),
        "gh" => check_gh(&words[1..], ctx),
        "glab" => glab::check_glab(&words[1..]),
        "cd" | "pushd" => {
            let target = words[1..]
                .iter()
                .find(|w| !w.starts_with('-') || w.as_str() == "-")
                .map(String::as_str)
                .unwrap_or("~");
            let dir = join(loc.dir.as_deref(), target);
            loc.head = (ctx.head_of)(&dir);
            loc.dir = Some(dir);
            Verdict::Allow
        }
        "bash" | "sh" | "zsh" | "dash" if depth < MAX_DEPTH => {
            match words
                .iter()
                .position(|w| w.starts_with('-') && w.ends_with('c'))
            {
                Some(i) => match words.get(i + 1) {
                    // Subshell: `cd` lá dentro não vaza para fora.
                    Some(inner) => check_segments(inner, ctx, depth + 1, &mut loc.clone()),
                    None => Verdict::Allow,
                },
                None => Verdict::Allow,
            }
        }
        "sudo" | "env" | "command" | "exec" | "nohup" | "time" | "nice" if depth < MAX_DEPTH => {
            let rest: Vec<String> = words[1..]
                .iter()
                .skip_while(|w| w.starts_with('-'))
                .cloned()
                .collect();
            check_segment(&rest, ctx, depth + 1, loc)
        }
        _ => Verdict::Allow,
    }
}

/// Compõe caminhos como o git faz com `-C`: absoluto substitui, relativo
/// é anexado ao anterior.
fn join(base: Option<&str>, path: &str) -> String {
    match base {
        Some(b) if !path.starts_with('/') => format!("{}/{path}", b.trim_end_matches('/')),
        _ => path.to_string(),
    }
}

pub(crate) fn skip_env_assignments(words: &[String]) -> &[String] {
    let n = words.iter().take_while(|w| is_assignment(w)).count();
    &words[n..]
}

fn is_assignment(word: &str) -> bool {
    match word.split_once('=') {
        Some((name, _)) => {
            !name.is_empty()
                && !name.starts_with(|c: char| c.is_ascii_digit())
                && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        }
        None => false,
    }
}

fn check_git(args: &[String], ctx: &GuardContext, loc: &Loc) -> Verdict {
    let mut base_dir: Option<String> = loc.dir.clone();
    let mut has_c = false;
    let mut git_dir: Option<&str> = None;
    let mut work_tree: Option<&str> = None;
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        let next = args.get(i + 1).map(String::as_str);
        match a {
            "-C" => {
                if let Some(d) = next {
                    base_dir = Some(join(base_dir.as_deref(), d));
                    has_c = true;
                }
                i += 2;
            }
            "--git-dir" => {
                git_dir = next;
                i += 2;
            }
            "--work-tree" => {
                work_tree = next;
                i += 2;
            }
            "-c" | "--namespace" => i += 2,
            _ if a.starts_with("--git-dir=") => {
                git_dir = a.split_once('=').map(|(_, v)| v);
                i += 1;
            }
            _ if a.starts_with("--work-tree=") => {
                work_tree = a.split_once('=').map(|(_, v)| v);
                i += 1;
            }
            _ if a.starts_with('-') => i += 1,
            _ => break,
        }
    }
    let Some(sub) = args.get(i) else {
        return Verdict::Allow;
    };
    let rest = &args[i + 1..];

    // HEAD efetivo: o do repositório indicado por --git-dir/--work-tree/-C,
    // senão o do diretório corrente (cwd ou último `cd`).
    let head: Option<String> = match git_dir.or(work_tree) {
        Some(p) => (ctx.head_of)(&join(base_dir.as_deref(), p)),
        None if has_c => base_dir.as_deref().and_then(|d| (ctx.head_of)(d)),
        None => loc.head.clone(),
    };
    let head = head.as_deref();
    let protected_head = head.filter(|h| ctx.is_protected(h));

    match sub.as_str() {
        "push" => check_push(rest, ctx, head),
        "merge" | "commit" | "rebase" | "cherry-pick" | "reset" | "revert" | "pull" | "am" => {
            match protected_head {
                Some(h) => {
                    Verdict::Block(format!("`git {sub}` com HEAD na branch protegida `{h}`"))
                }
                None => Verdict::Allow,
            }
        }
        "checkout" | "switch" => check_switch(sub, rest, ctx),
        _ => Verdict::Allow,
    }
}

fn check_push(args: &[String], ctx: &GuardContext, head: Option<&str>) -> Verdict {
    let mut positionals: Vec<&str> = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--all" | "--mirror" | "--branches" => {
                return Verdict::Block(format!("`git push {a}` pode atingir branches protegidas"));
            }
            "-o" | "--push-option" | "--repo" | "--receive-pack" | "--exec" => {
                it.next();
            }
            s if s.starts_with('-') => {}
            s => positionals.push(s),
        }
    }
    let refspecs = positionals.get(1..).unwrap_or(&[]);
    if refspecs.is_empty() {
        return match head.filter(|h| ctx.is_protected(h)) {
            Some(h) => Verdict::Block(format!("`git push` com HEAD na branch protegida `{h}`")),
            None => Verdict::Allow,
        };
    }
    for r in refspecs {
        let r = r.trim_start_matches('+');
        let dst = r.rsplit_once(':').map(|(_, d)| d).unwrap_or(r);
        let dst = if dst == "HEAD" {
            head.unwrap_or("")
        } else {
            dst
        };
        let dst = dst.strip_prefix("refs/heads/").unwrap_or(dst);
        if ctx.is_protected(dst) {
            return Verdict::Block(format!("push para a branch protegida `{dst}`"));
        }
    }
    Verdict::Allow
}

fn check_switch(sub: &str, args: &[String], ctx: &GuardContext) -> Verdict {
    let dd = args.iter().position(|a| a == "--");
    let candidates: Vec<&String> = match (sub, dd) {
        // switch não aceita pathspec: `--` é só separador.
        ("switch", _) => args.iter().filter(|a| a.as_str() != "--").collect(),
        // checkout com pathspec após `--` restaura arquivos, não troca de branch.
        (_, Some(i)) if i + 1 < args.len() => return Verdict::Allow,
        (_, Some(i)) => args[..i].iter().collect(),
        (_, None) => args.iter().collect(),
    };
    let creates = candidates.iter().any(|a| {
        matches!(
            a.as_str(),
            "-b" | "-B" | "-c" | "-C" | "--create" | "--force-create" | "--orphan"
        )
    });
    if creates {
        return Verdict::Allow;
    }
    let tracks = candidates
        .iter()
        .any(|a| a.as_str() == "-t" || a.as_str() == "--track" || a.starts_with("--track="));
    for a in candidates.iter().filter(|a| !a.starts_with('-')) {
        // Com --track sem -b/-c, o nome local é o que vem após o primeiro `/`.
        let local = match a.split_once('/') {
            Some((_, l)) if tracks => l,
            _ => a.as_str(),
        };
        if ctx.is_protected(local) {
            return Verdict::Block(format!("`git {sub}` para a branch protegida `{local}`"));
        }
    }
    Verdict::Allow
}

fn check_gh(args: &[String], ctx: &GuardContext) -> Verdict {
    let mut repo: Option<&str> = None;
    let mut pos: Vec<&str> = Vec::new();
    let mut it = args.iter().map(String::as_str);
    while let Some(a) = it.next() {
        match a {
            "-R" | "--repo" => repo = it.next(),
            _ if a.starts_with("--repo=") => repo = a.split_once('=').map(|(_, v)| v),
            _ if a.starts_with("-R") && a.len() > 2 => repo = Some(&a[2..]),
            // Opções de `gh pr merge` que recebem valor.
            "-b"
            | "--body"
            | "-F"
            | "--body-file"
            | "-t"
            | "--subject"
            | "-A"
            | "--author-email"
            | "--match-head-commit"
                if pos.first() != Some(&"api") =>
            {
                it.next();
            }
            _ if a.starts_with('-') => {}
            _ => pos.push(a),
        }
    }
    if let Some(i) = pos.windows(2).position(|w| w == ["pr", "merge"]) {
        let target = pos.get(i + 2).copied();
        let target: Option<String> = match (repo, target) {
            (None, t) => t.map(str::to_string),
            (Some(_), Some(t)) if t.starts_with("https://") || t.starts_with("http://") => {
                Some(t.to_string())
            }
            (Some(r), Some(t))
                if !t.is_empty()
                    && t.trim_start_matches('#')
                        .bytes()
                        .all(|b| b.is_ascii_digit()) =>
            {
                Some(pr_url(r, t.trim_start_matches('#')))
            }
            (Some(r), _) => {
                return Verdict::Block(format!(
                    "não foi possível determinar a base do PR em `{r}` para `gh pr merge`"
                ));
            }
        };
        return match (ctx.pr_base)(target.as_deref()) {
            Some(base) if !ctx.is_protected(&base) => Verdict::Allow,
            Some(base) => {
                Verdict::Block(format!("`gh pr merge` em PR com base protegida `{base}`"))
            }
            None => {
                Verdict::Block("não foi possível determinar a base do PR para `gh pr merge`".into())
            }
        };
    }
    if pos.first() == Some(&"api") {
        return check_gh_api(args);
    }
    Verdict::Allow
}

/// URL do PR `num` no repositório `[HOST/]OWNER/REPO` (forma aceita por `gh -R`).
fn pr_url(repo: &str, num: &str) -> String {
    let r = repo
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_end_matches('/')
        .trim_end_matches(".git");
    if r.split('/').count() >= 3 {
        format!("https://{r}/pull/{num}")
    } else {
        format!("https://github.com/{r}/pull/{num}")
    }
}

/// `gh api .../merge` só bloqueia quando é escrita: método diferente de GET
/// ou envio de campos/corpo (que tornam a requisição POST por padrão).
fn check_gh_api(args: &[String]) -> Verdict {
    let mut method: Option<&str> = None;
    let mut writes = false;
    let mut merge_path = false;
    let mut it = args.iter().map(String::as_str);
    while let Some(a) = it.next() {
        match a {
            "-X" | "--method" => method = it.next(),
            "-f" | "-F" | "--field" | "--raw-field" | "--input" => {
                writes = true;
                it.next();
            }
            "-H" | "--header" | "-q" | "--jq" | "-t" | "--template" | "--hostname" | "--cache"
            | "-p" | "--preview" => {
                it.next();
            }
            _ if a.starts_with("--method=") => method = a.split_once('=').map(|(_, v)| v),
            _ if a.starts_with("-X") => method = Some(&a[2..]),
            _ if a.starts_with("--field=")
                || a.starts_with("--raw-field=")
                || a.starts_with("--input=")
                || ((a.starts_with("-f") || a.starts_with("-F")) && a.len() > 2) =>
            {
                writes = true
            }
            _ if a.starts_with('-') => {}
            _ => merge_path |= a.trim_end_matches('/').ends_with("/merge"),
        }
    }
    let non_get = method.is_some_and(|m| !m.eq_ignore_ascii_case("GET"));
    if merge_path && (non_get || writes) {
        Verdict::Block("merge de PR via `gh api`".into())
    } else {
        Verdict::Allow
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn protected() -> Vec<String> {
        vec!["develop".into(), "main".into(), "master".into()]
    }

    fn check_on(head: Option<&str>, cmd: &str) -> Verdict {
        let p = protected();
        let base = |_: Option<&str>| Some("wip/12-auth".to_string());
        check_command(
            cmd,
            &GuardContext {
                head_branch: head,
                protected: &p,
                pr_base: &base,
                head_of: &|_| None,
            },
        )
    }

    fn check_with_pr_base(cmd: &str, pr_base: Option<&str>) -> Verdict {
        let p = protected();
        let b = pr_base.map(str::to_string);
        let base = move |_: Option<&str>| b.clone();
        check_command(
            cmd,
            &GuardContext {
                head_branch: Some("wip/12-auth"),
                protected: &p,
                pr_base: &base,
                head_of: &|_| None,
            },
        )
    }

    fn blocked(v: &Verdict) -> bool {
        matches!(v, Verdict::Block(_))
    }

    #[test]
    fn push_para_protegida_bloqueia() {
        for cmd in [
            "git push origin develop",
            "git push origin HEAD:develop",
            "git push origin wip/12:refs/heads/main",
            "git push -f origin +develop",
            "git push origin --delete develop",
            "git push origin :develop",
            "git push --all origin",
            "git push --mirror",
        ] {
            assert!(blocked(&check_on(Some("wip/12-auth"), cmd)), "{cmd}");
        }
    }

    #[test]
    fn push_da_branch_da_sessao_permite() {
        for cmd in [
            "git push",
            "git push -u origin wip/12-auth",
            "git push origin HEAD",
            "git push --set-upstream origin feat/9-x",
        ] {
            assert_eq!(check_on(Some("wip/12-auth"), cmd), Verdict::Allow, "{cmd}");
        }
    }

    #[test]
    fn push_implicito_com_head_protegido_bloqueia() {
        assert!(blocked(&check_on(Some("develop"), "git push")));
        assert!(blocked(&check_on(Some("develop"), "git push origin")));
        assert!(blocked(&check_on(Some("develop"), "git push origin HEAD")));
    }

    #[test]
    fn comandos_que_alteram_head_protegido_bloqueiam() {
        for cmd in [
            "git commit -m x",
            "git merge wip/12",
            "git rebase origin/x",
            "git cherry-pick abc",
            "git reset --hard",
            "git revert abc",
            "git pull",
        ] {
            assert!(blocked(&check_on(Some("develop"), cmd)), "{cmd}");
            assert_eq!(check_on(Some("wip/12-auth"), cmd), Verdict::Allow, "{cmd}");
        }
    }

    #[test]
    fn checkout_para_protegida_bloqueia() {
        assert!(blocked(&check_on(
            Some("wip/12-auth"),
            "git checkout develop"
        )));
        assert!(blocked(&check_on(Some("wip/12-auth"), "git switch main")));
    }

    #[test]
    fn checkout_que_cria_branch_ou_restaura_arquivo_permite() {
        for cmd in [
            "git switch -c feat/13-x develop",
            "git checkout -b feat/13-x develop",
            "git checkout develop -- Cargo.toml",
            "git checkout wip/12-auth",
        ] {
            assert_eq!(check_on(Some("wip/12-auth"), cmd), Verdict::Allow, "{cmd}");
        }
    }

    #[test]
    fn opcoes_globais_do_git_sao_ignoradas() {
        assert!(blocked(&check_on(
            Some("wip/12-auth"),
            "git -C /repo -c core.x=y --no-pager push origin develop"
        )));
    }

    #[test]
    fn segmentos_e_wrappers_sao_verificados() {
        assert!(blocked(&check_on(
            Some("wip/12-auth"),
            "cargo test && git push origin develop"
        )));
        assert!(blocked(&check_on(
            Some("wip/12-auth"),
            "GIT_TRACE=1 git push origin develop"
        )));
        assert!(blocked(&check_on(
            Some("wip/12-auth"),
            "sudo git push origin develop"
        )));
        assert!(blocked(&check_on(
            Some("wip/12-auth"),
            "bash -c 'git push origin develop'"
        )));
        assert!(blocked(&check_on(
            Some("wip/12-auth"),
            "/usr/bin/git push origin develop"
        )));
    }

    #[test]
    fn texto_entre_aspas_nao_e_comando() {
        assert_eq!(
            check_on(
                Some("wip/12-auth"),
                r#"git commit -m "fix && git push origin develop""#
            ),
            Verdict::Allow
        );
        assert_eq!(
            check_on(Some("wip/12-auth"), "echo 'git push origin develop'"),
            Verdict::Allow
        );
    }

    #[test]
    fn gh_pr_merge_depende_da_base() {
        assert_eq!(
            check_with_pr_base("gh pr merge 5 --squash", Some("wip/12-auth")),
            Verdict::Allow
        );
        assert!(blocked(&check_with_pr_base(
            "gh pr merge 5",
            Some("develop")
        )));
        assert!(blocked(&check_with_pr_base("gh pr merge", None)));
        assert!(blocked(&check_with_pr_base(
            "gh -R org/repo pr merge 5",
            Some("main")
        )));
    }

    #[test]
    fn gh_api_de_merge_bloqueia() {
        assert!(blocked(&check_on(
            Some("wip/12-auth"),
            "gh api -X PUT repos/o/r/pulls/5/merge"
        )));
        assert_eq!(
            check_on(Some("wip/12-auth"), "gh api repos/o/r/pulls/5"),
            Verdict::Allow
        );
    }

    #[test]
    fn head_desconhecido_nao_bloqueia_push_explicito_de_outra_branch() {
        assert_eq!(
            check_on(None, "git push origin wip/12-auth"),
            Verdict::Allow
        );
        assert_eq!(check_on(None, "git commit -m x"), Verdict::Allow);
    }

    fn check_head_of(head: Option<&str>, cmd: &str, dev_dir: &str) -> Verdict {
        let p = protected();
        let base = |_: Option<&str>| Some("wip/12-auth".to_string());
        let head_of = |d: &str| (d == dev_dir).then(|| "develop".to_string());
        check_command(
            cmd,
            &GuardContext {
                head_branch: head,
                protected: &p,
                pr_base: &base,
                head_of: &head_of,
            },
        )
    }

    fn pr_target(cmd: &str) -> (Verdict, Vec<Option<String>>) {
        let p = protected();
        let seen = std::cell::RefCell::new(Vec::new());
        let base = |t: Option<&str>| {
            seen.borrow_mut().push(t.map(str::to_string));
            Some("wip/12-auth".to_string())
        };
        let v = check_command(
            cmd,
            &GuardContext {
                head_branch: Some("wip/12-auth"),
                protected: &p,
                pr_base: &base,
                head_of: &|_| None,
            },
        );
        (v, seen.into_inner())
    }

    #[test]
    fn checkout_e_switch_com_duplo_traco() {
        for cmd in [
            "git checkout develop --",
            "git switch -- develop",
            "git switch main --",
        ] {
            assert!(blocked(&check_on(Some("wip/12-auth"), cmd)), "{cmd}");
        }
        for cmd in [
            "git checkout develop -- Cargo.toml",
            "git checkout -- develop",
            "git checkout -- Cargo.toml",
        ] {
            assert_eq!(check_on(Some("wip/12-auth"), cmd), Verdict::Allow, "{cmd}");
        }
    }

    #[test]
    fn head_efetivo_via_opcoes_de_local_do_git() {
        for cmd in [
            "git -C /repo commit -m x",
            "git --git-dir=/repo reset --hard",
            "git --git-dir /repo reset --hard",
            "git --work-tree /repo merge x",
            "git --work-tree=/repo rebase y",
            "git -C /repo push",
            "git -C /repo push origin HEAD",
            "git -C /repo am x.patch",
            "git -C / -C repo commit -m x",
            "git -C / --git-dir=repo commit -m x",
        ] {
            assert!(
                blocked(&check_head_of(Some("wip/12-auth"), cmd, "/repo")),
                "{cmd}"
            );
        }
        // -C relativo compõe com o anterior.
        assert!(blocked(&check_head_of(
            Some("wip/12-auth"),
            "git -C a -C b commit -m x",
            "a/b"
        )));
        // Outro diretório (HEAD não protegido) permite mesmo com HEAD do cwd protegido.
        assert_eq!(
            check_head_of(Some("develop"), "git -C /outro commit -m x", "/repo"),
            Verdict::Allow
        );
        // Comandos que não dependem do HEAD não mudam.
        assert_eq!(
            check_head_of(Some("wip/12-auth"), "git -C /repo status", "/repo"),
            Verdict::Allow
        );
    }

    #[test]
    fn cd_muda_o_head_dos_segmentos_seguintes() {
        for cmd in [
            "cd /repo && git merge x",
            "cd /repo; git commit -m x",
            "cd / && cd repo && git commit -m x",
            "cd / && git -C repo commit -m x",
            "cd /repo && bash -c 'git commit -m x'",
            "cd /repo && git push",
        ] {
            assert!(
                blocked(&check_head_of(Some("wip/12-auth"), cmd, "/repo")),
                "{cmd}"
            );
        }
        assert_eq!(
            check_head_of(Some("develop"), "cd /outro && git commit -m x", "/repo"),
            Verdict::Allow
        );
        assert_eq!(
            check_head_of(Some("wip/12-auth"), "git commit -m x && cd /repo", "/repo"),
            Verdict::Allow
        );
    }

    #[test]
    fn heredoc_nao_vira_comando() {
        for cmd in [
            "cat <<EOF > README.md\ngit checkout main\nEOF",
            "git commit -F - <<'EOF'\nfix: x\n\ngit push origin develop\nEOF",
            "cat <<-\"FIM\"\n\tgit push origin develop\n\tFIM\nls",
        ] {
            assert_eq!(
                check_on(Some("wip/12-auth"), cmd),
                Verdict::Allow,
                "{cmd:?}"
            );
        }
        assert!(blocked(&check_on(
            Some("wip/12-auth"),
            "cat <<EOF\nx\nEOF\ngit push origin develop"
        )));
    }

    #[test]
    fn agrupamentos_e_palavras_chave() {
        for cmd in [
            "(cd x && git push origin develop)",
            "{ git push origin develop; }",
            "if true; then git push origin develop; fi",
            "for b in a; do git push origin develop; done",
            "while true; do git push origin develop; done",
            "! git push origin develop",
            "if git push origin develop; then :; fi",
            "time git push origin develop",
        ] {
            assert!(blocked(&check_on(Some("wip/12-auth"), cmd)), "{cmd}");
        }
    }

    #[test]
    fn push_com_opcoes_que_recebem_valor() {
        for cmd in [
            "git push -o ci.skip origin",
            "git push --push-option ci.skip origin",
            "git push --receive-pack git-receive-pack origin",
            "git push --exec git-receive-pack origin",
            "git push --repo origin",
        ] {
            assert!(blocked(&check_on(Some("develop"), cmd)), "{cmd}");
        }
        assert_eq!(
            check_on(
                Some("wip/12-auth"),
                "git push -o ci.skip origin wip/12-auth"
            ),
            Verdict::Allow
        );
    }

    #[test]
    fn checkout_track_deriva_nome_local() {
        for cmd in [
            "git checkout -t origin/develop",
            "git checkout --track origin/develop",
            "git checkout --track=direct origin/main",
            "git switch --track origin/main",
        ] {
            assert!(blocked(&check_on(Some("wip/12-auth"), cmd)), "{cmd}");
        }
        for cmd in [
            "git checkout -t origin/feat/9-x",
            "git checkout -b feat/9-x -t origin/develop",
        ] {
            assert_eq!(check_on(Some("wip/12-auth"), cmd), Verdict::Allow, "{cmd}");
        }
    }

    #[test]
    fn comentario_encerra_segmento() {
        assert_eq!(
            check_on(
                Some("wip/12-auth"),
                "git push origin wip/12-auth # vai para develop depois"
            ),
            Verdict::Allow
        );
        assert!(blocked(&check_on(
            Some("wip/12-auth"),
            "ls # comentário\ngit push origin develop"
        )));
    }

    #[test]
    fn gh_api_merge_so_bloqueia_escrita() {
        for cmd in [
            "gh api repos/o/r/pulls/5/merge",
            "gh api -X GET repos/o/r/pulls/5/merge",
            "gh api --method=get repos/o/r/pulls/5/merge",
        ] {
            assert_eq!(check_on(Some("wip/12-auth"), cmd), Verdict::Allow, "{cmd}");
        }
        for cmd in [
            "gh api -XPUT repos/o/r/pulls/5/merge",
            "gh api --method PUT repos/o/r/pulls/5/merge",
            "gh api repos/o/r/pulls/5/merge -f merge_method=squash",
            "gh api repos/o/r/pulls/5/merge -F x=1",
            "gh api repos/o/r/pulls/5/merge --field x=1",
            "gh api repos/o/r/pulls/5/merge --raw-field=x=1",
            "gh api repos/o/r/pulls/5/merge --input body.json",
        ] {
            assert!(blocked(&check_on(Some("wip/12-auth"), cmd)), "{cmd}");
        }
    }

    #[test]
    fn gh_pr_merge_preserva_repositorio() {
        let (_, t) = pr_target("gh pr merge 5 --squash");
        assert_eq!(t, vec![Some("5".to_string())]);
        let (_, t) = pr_target("gh pr merge");
        assert_eq!(t, vec![None]);
        let (_, t) = pr_target("gh -R o/r pr merge 5");
        assert_eq!(t, vec![Some("https://github.com/o/r/pull/5".to_string())]);
        let (_, t) = pr_target("gh pr merge --repo=o/r 5 --body 'x y'");
        assert_eq!(t, vec![Some("https://github.com/o/r/pull/5".to_string())]);
        let (_, t) = pr_target("gh pr merge -R ghe.local/o/r 5");
        assert_eq!(t, vec![Some("https://ghe.local/o/r/pull/5".to_string())]);
        let (_, t) = pr_target("gh -R o/r pr merge https://github.com/o/r/pull/7");
        assert_eq!(t, vec![Some("https://github.com/o/r/pull/7".to_string())]);
        // Com -R e sem número/URL não dá para determinar a base no repositório certo.
        let (v, t) = pr_target("gh -R o/r pr merge minha-branch");
        assert!(blocked(&v));
        assert!(t.is_empty());
    }

    #[test]
    fn comandos_sem_relacao_permitem() {
        for cmd in [
            "cargo test",
            "ls -la",
            "git status",
            "git log --oneline develop",
            "git diff develop...HEAD",
            "",
        ] {
            assert_eq!(check_on(Some("wip/12-auth"), cmd), Verdict::Allow, "{cmd}");
        }
    }
}
