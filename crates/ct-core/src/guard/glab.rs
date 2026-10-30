//! Guarda-corpo do glab (spec do Jira e GitLab §7.4, issue #350): o merge
//! de MR é sempre bloqueado. O hook não tem como saber a base do MR sem o
//! token, que o agente passa dentro do próprio comando (`GITLAB_TOKEN="$(pass
//! ...)" glab ...`) e o hook não deve avaliar; no exemplo, todo MR vai para a
//! `develop` protegida, e o merge é do humano de qualquer forma.
//!
//! Não depende do provedor da sessão: o glab só fala com o GitLab, e um
//! `glab mr merge` numa sessão de projeto GitHub também não é trabalho do
//! agente. Por isso o hook não precisa de variável nova (`CTOWER_FORGE`).
//! O push direto para a branch protegida continua no `check_push`, que vale
//! para qualquer forja.

use super::Verdict;

/// `glab mr merge` (e o apelido `accept`) e `glab api` de escrita num
/// caminho que termina em `/merge`.
pub fn check_glab(args: &[String]) -> Verdict {
    let mut pos: Vec<&str> = Vec::new();
    let mut it = args.iter().map(String::as_str);
    while let Some(a) = it.next() {
        match a {
            // Opções globais e de `mr` que recebem valor.
            "-R" | "--repo" | "-m" | "--message" | "--sha" | "-g" | "--group" => {
                it.next();
            }
            _ if a.starts_with('-') => {}
            _ => pos.push(a),
        }
        if pos.first() == Some(&"api") {
            break;
        }
    }
    if let Some(i) = pos.windows(2).position(|w| w[0] == "mr") {
        if matches!(pos[i + 1], "merge" | "accept") {
            return Verdict::Block(
                "`glab mr merge` é sempre bloqueado nos projetos GitLab: o merge do MR é feito \
                 pelo humano"
                    .into(),
            );
        }
    }
    if pos.first() == Some(&"api") {
        let i = args.iter().position(|a| a == "api").unwrap_or(0);
        return check_glab_api(&args[i + 1..]);
    }
    Verdict::Allow
}

/// `glab api .../merge` só bloqueia quando é escrita: método diferente de
/// GET ou envio de campos, formulário ou corpo (que tornam a requisição POST
/// por padrão).
fn check_glab_api(args: &[String]) -> Verdict {
    let mut method: Option<&str> = None;
    let mut writes = false;
    let mut merge_path = false;
    let mut it = args.iter().map(String::as_str);
    while let Some(a) = it.next() {
        match a {
            "-X" | "--method" => method = it.next(),
            "-f" | "-F" | "--field" | "--raw-field" | "--input" | "--form" => {
                writes = true;
                it.next();
            }
            "-H" | "--header" | "--hostname" | "--placeholder" | "--output" => {
                it.next();
            }
            _ if a.starts_with("--method=") => method = a.split_once('=').map(|(_, v)| v),
            _ if a.starts_with("-X") => method = Some(&a[2..]),
            _ if a.starts_with("--field=")
                || a.starts_with("--raw-field=")
                || a.starts_with("--input=")
                || a.starts_with("--form=")
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
        Verdict::Block("merge de MR via `glab api`".into())
    } else {
        Verdict::Allow
    }
}

#[cfg(test)]
mod tests {
    use crate::guard::{check_command, GuardContext, Verdict};

    fn check(cmd: &str) -> Verdict {
        let p = vec!["develop".to_string(), "main".to_string()];
        let base = |_: Option<&str>| Some("feat/1-x".to_string());
        check_command(
            cmd,
            &GuardContext {
                head_branch: Some("PROJ-5450"),
                protected: &p,
                pr_base: &base,
                head_of: &|_| None,
            },
        )
    }

    fn blocked(v: &Verdict) -> bool {
        matches!(v, Verdict::Block(_))
    }

    /// Critério de aceite da #350: `glab mr merge` é bloqueado sempre, com
    /// qualquer alvo, opção, prefixo de token ou wrapper.
    #[test]
    fn glab_mr_merge_sempre_bloqueia() {
        for cmd in [
            "glab mr merge",
            "glab mr merge 12",
            "glab mr merge PROJ-5450 --squash",
            "glab mr accept 12",
            "glab mr merge -R grupo/projeto/app 12 --auto-merge=false",
            "glab -R grupo/projeto/app mr merge 12",
            "GITLAB_TOKEN=\"$(pass show gitlab | head -1)\" glab mr merge 12 -y",
            "/opt/homebrew/bin/glab mr merge 12",
            "cargo test && glab mr merge 12",
            "bash -c 'glab mr merge 12'",
            "env GITLAB_HOST=gitlab.example.com glab mr merge 12",
        ] {
            let v = check(cmd);
            assert!(blocked(&v), "{cmd}: {v:?}");
        }
        let Verdict::Block(m) = check("glab mr merge 12") else {
            unreachable!()
        };
        assert!(m.contains("sempre bloqueado"), "{m}");
    }

    #[test]
    fn glab_api_de_merge_so_bloqueia_escrita() {
        for cmd in [
            "glab api -X PUT projects/grupo%2Fprojeto%2Fapp/merge_requests/12/merge",
            "glab api --method PUT projects/1/merge_requests/12/merge",
            "glab api --method=put projects/1/merge_requests/12/merge",
            "glab api -XPUT projects/1/merge_requests/12/merge",
            "glab api projects/1/merge_requests/12/merge -f squash=true",
            "glab api projects/1/merge_requests/12/merge --field=x=1",
            "glab api projects/1/merge_requests/12/merge --input corpo.json",
            "glab api --hostname gitlab.example.com -X PUT projects/1/merge_requests/12/merge",
        ] {
            assert!(blocked(&check(cmd)), "{cmd}");
        }
        for cmd in [
            "glab api projects/1/merge_requests/12/merge",
            "glab api -X GET projects/1/merge_requests/12/merge",
            "glab api projects/1/merge_requests/12",
            "glab api -X PUT projects/1/merge_requests/12 -f title=x",
        ] {
            assert_eq!(check(cmd), Verdict::Allow, "{cmd}");
        }
    }

    #[test]
    fn leitura_e_criacao_de_mr_passam() {
        for cmd in [
            "glab mr list -R grupo/projeto/app -s PROJ-5450",
            "glab mr view 12",
            "glab mr create -R grupo/projeto/app -s PROJ-5450 -b develop -t 'x' -d '' -y",
            "glab mr note 12 -m 'merge depois'",
            "glab mr update 12 --title merge",
            "glab repo view",
        ] {
            assert_eq!(check(cmd), Verdict::Allow, "{cmd}");
        }
    }

    /// Critério de aceite da #350: o push direto para a branch protegida
    /// continua bloqueado num repositório do GitLab, como no GitHub.
    #[test]
    fn push_para_protegida_no_gitlab_bloqueia() {
        for cmd in [
            "git push origin develop",
            "git push origin HEAD:develop",
            "git push https://gitlab.example.com/grupo/projeto/app.git PROJ-5450:develop",
            "git push git@gitlab.example.com:grupo/projeto/app.git main",
        ] {
            assert!(blocked(&check(cmd)), "{cmd}");
        }
        assert_eq!(
            check("git push -u origin PROJ-5450"),
            Verdict::Allow,
            "a branch da sessão passa"
        );
    }
}
