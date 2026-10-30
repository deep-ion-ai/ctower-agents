//! Dono e repositório a partir da URL de um repositório (cadastro de
//! projeto): só do GitHub ([`github_repo_from_url`]) ou de qualquer host
//! ([`repo_from_url`], GitLab self-hosted incluído).

/// Repositório de qualquer host: `host` em minúsculas e sem porta, `path`
/// com os segmentos do projeto (`grupo/projeto/app`), sem `.git`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoRef {
    pub host: String,
    pub path: String,
}

impl RepoRef {
    /// `(owner, repo)` da chave do projeto (spec do Jira e GitLab §3.2):
    /// `owner` = o caminho sem o último segmento (pode ter barras), `repo` =
    /// o último segmento.
    pub fn owner_repo(&self) -> (&str, &str) {
        self.path.rsplit_once('/').expect("path tem dois segmentos")
    }
}

fn valid_segment(s: &str) -> bool {
    !s.is_empty()
        && !s.starts_with('.')
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
}

/// Host e caminho de `https://host/grupo/sub/repo(.git)`, de
/// `ssh://git@host[:porta]/grupo/repo.git` e de `git@host:grupo/repo.git`,
/// de qualquer host. Caminho extra de página do GitLab (`/-/tree/develop`)
/// e, no `github.com`, o que vem depois de `owner/repo` (`/issues/5`) são
/// ignorados. `None` para URL sem host, com menos de dois segmentos ou com
/// segmento inválido.
pub fn repo_from_url(url: &str) -> Option<RepoRef> {
    let u = url.trim();
    let lower = u.to_ascii_lowercase();
    let (authority, rest) = if let Some(i) = ["https://", "http://", "ssh://", "git+ssh://"]
        .iter()
        .find_map(|p| lower.starts_with(p).then_some(p.len()))
    {
        u[i..].split_once('/')?
    } else if !u.contains("://") {
        // `git@host:caminho` (forma scp do git).
        let (authority, rest) = u.split_once(':')?;
        if authority.contains('/') {
            return None;
        }
        (authority, rest)
    } else {
        return None;
    };
    let host_port = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    let host = host_port
        .split_once(':')
        .map_or(host_port, |(h, _)| h)
        .to_ascii_lowercase();
    let host_ok = !host.is_empty()
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-.".contains(c));
    if !host_ok {
        return None;
    }
    let rest = rest.split(['?', '#']).next().unwrap_or_default();
    let rest = rest.split("/-/").next().unwrap_or_default();
    let mut segs: Vec<&str> = rest.split('/').filter(|s| !s.is_empty()).collect();
    if host == "github.com" || host == "www.github.com" {
        segs.truncate(2);
    }
    let last = segs.last_mut()?;
    *last = last.strip_suffix(".git").unwrap_or(last);
    (segs.len() >= 2 && segs.iter().all(|s| valid_segment(s))).then(|| RepoRef {
        host: if host == "www.github.com" {
            "github.com".into()
        } else {
            host
        },
        path: segs.join("/"),
    })
}

/// `(dono, repositório)` de `https://github.com/o/r`, com `.git`, barra final
/// ou caminho extra (`/issues/5`, `/tree/develop`), de `git@github.com:o/r.git`
/// e de `ssh://git@github.com/o/r.git`. `None` para outro host ou nome inválido.
pub fn github_repo_from_url(url: &str) -> Option<(String, String)> {
    let u = url.trim();
    let rest = [
        "https://github.com/",
        "http://github.com/",
        "https://www.github.com/",
        "ssh://git@github.com/",
        "git@github.com:",
        "github.com/",
    ]
    .iter()
    .find_map(|p| {
        u.get(..p.len())
            .filter(|h| h.eq_ignore_ascii_case(p))
            .map(|_| &u[p.len()..])
    })?;
    let rest = rest.split(['?', '#']).next().unwrap_or_default();
    let mut parts = rest.split('/').filter(|s| !s.is_empty());
    let owner = parts.next()?;
    let repo = parts.next()?;
    let repo = repo.strip_suffix(".git").unwrap_or(repo);
    (valid_segment(owner) && valid_segment(repo)).then(|| (owner.to_string(), repo.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn par(o: &str, r: &str) -> Option<(String, String)> {
        Some((o.into(), r.into()))
    }

    #[test]
    fn formatos_aceitos() {
        for url in [
            "https://github.com/deep-ion-ai/ctower-agents",
            "https://github.com/deep-ion-ai/ctower-agents.git",
            "https://github.com/deep-ion-ai/ctower-agents/",
            "  https://github.com/deep-ion-ai/ctower-agents/issues/5  ",
            "https://github.com/deep-ion-ai/ctower-agents/tree/develop?x=1",
            "HTTPS://GitHub.com/deep-ion-ai/ctower-agents",
            "git@github.com:deep-ion-ai/ctower-agents.git",
            "ssh://git@github.com/deep-ion-ai/ctower-agents.git",
            "github.com/deep-ion-ai/ctower-agents",
        ] {
            assert_eq!(
                github_repo_from_url(url),
                par("deep-ion-ai", "ctower-agents"),
                "{url}"
            );
        }
        assert_eq!(
            github_repo_from_url("https://github.com/o/my.repo_2"),
            par("o", "my.repo_2")
        );
    }

    #[test]
    fn formatos_recusados() {
        for url in [
            "",
            "https://gitlab.com/o/r",
            "https://github.com/o",
            "https://github.com/",
            "https://github.com/o/../x",
            "https://github.com/o/r r",
            "https://evil.com/github.com/o/r",
            "o/r",
        ] {
            assert_eq!(github_repo_from_url(url), None, "{url}");
        }
    }

    fn rr(host: &str, path: &str) -> Option<RepoRef> {
        Some(RepoRef {
            host: host.into(),
            path: path.into(),
        })
    }

    #[test]
    fn qualquer_host_com_varios_segmentos() {
        for url in [
            "https://gitlab.example.com/grupo/projeto/app",
            "https://gitlab.example.com/grupo/projeto/app.git",
            "https://gitlab.example.com/grupo/projeto/app/",
            "https://usuario@gitlab.example.com/grupo/projeto/app.git",
            "HTTPS://GitLab.Example.com/grupo/projeto/app",
            "https://gitlab.example.com:8443/grupo/projeto/app.git",
            "  https://gitlab.example.com/grupo/projeto/app/-/tree/develop?x=1  ",
            "git@gitlab.example.com:grupo/projeto/app.git",
            "ssh://git@gitlab.example.com:2222/grupo/projeto/app.git",
        ] {
            assert_eq!(
                repo_from_url(url),
                rr("gitlab.example.com", "grupo/projeto/app"),
                "{url}"
            );
        }
        let r = repo_from_url("git@gitlab.example.com:grupo/projeto/app.git").unwrap();
        assert_eq!(r.owner_repo(), ("grupo/projeto", "app"));
    }

    #[test]
    fn github_por_repo_from_url_igual_ao_de_hoje() {
        for url in [
            "https://github.com/deep-ion-ai/ctower-agents/issues/5",
            "https://www.github.com/deep-ion-ai/ctower-agents",
            "git@github.com:deep-ion-ai/ctower-agents.git",
            "ssh://git@github.com/deep-ion-ai/ctower-agents.git",
        ] {
            let r = repo_from_url(url).unwrap();
            assert_eq!(
                r,
                rr("github.com", "deep-ion-ai/ctower-agents").unwrap(),
                "{url}"
            );
            assert_eq!(r.owner_repo(), ("deep-ion-ai", "ctower-agents"));
        }
    }

    #[test]
    fn qualquer_host_recusados() {
        for url in [
            "",
            "https://gitlab.example.com/",
            "https://gitlab.example.com/app",
            "https://gitlab.example.com/pje/../x",
            "https://gitlab.example.com/pje/a b",
            "ftp://gitlab.example.com/pje/x",
            "grupo/projeto",
            "git@:pje/x",
        ] {
            assert_eq!(repo_from_url(url), None, "{url}");
        }
    }
}
