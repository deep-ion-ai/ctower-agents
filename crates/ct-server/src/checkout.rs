//! Clone do repositório de um projeto na aplicação das atribuições vindas
//! do hub (#177), o fluxo que era o do antigo cadastro por URL (#40). No
//! GitHub, pelo `gh repo clone`; no GitLab (spec do Jira e GitLab §7.1), pelo
//! `git clone` da URL do cadastro, HTTPS ou SSH, com a credencial do git do
//! ambiente. Nos dois, o `origin` é conferido pelo host e pelo caminho
//! ([`ct_core::repo_from_url`]), e HTTPS e SSH do mesmo repositório valem.

use ct_core::{repo_from_url, ForgeConfig, RepoRef};
use std::path::Path;

/// Por que o clone não ficou pronto.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckoutError {
    /// O `gh repo clone` ou o `git clone` falhou (rede, credencial,
    /// repositório inexistente).
    Clone(String),
    /// A pasta existe, mas não é um clone do repositório (erro do usuário).
    Invalid(String),
}

impl std::fmt::Display for CheckoutError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CheckoutError::Clone(m) | CheckoutError::Invalid(m) => f.write_str(m),
        }
    }
}

/// `~` e `~/...` com o `HOME`.
pub fn expand_home(s: &str) -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    match s.strip_prefix('~') {
        Some("") => home,
        Some(rest) if rest.starts_with('/') => format!("{home}{rest}"),
        _ => s.to_string(),
    }
}

/// O repositório que o `origin` precisa ser: `github.com/<owner>/<repo>` ou
/// o host e o caminho do cadastro do GitLab.
fn expected(owner: &str, repo: &str, forge: &ForgeConfig) -> RepoRef {
    match forge {
        ForgeConfig::Github => RepoRef {
            host: "github.com".into(),
            path: format!("{owner}/{repo}"),
        },
        ForgeConfig::Gitlab(g) => RepoRef {
            host: g.host.trim().to_ascii_lowercase(),
            path: match g.path.trim_matches('/') {
                "" => format!("{owner}/{repo}"),
                p => p.to_string(),
            },
        },
    }
}

/// Pasta existente precisa ser um clone do repositório do projeto; ausente
/// é clonada: no GitHub com o `gh` (e o token do GitHub do ambiente, se
/// houver), no GitLab com o `git clone` da URL do cadastro.
pub fn ensure_checkout(
    owner: &str,
    repo: &str,
    forge: &ForgeConfig,
    repo_path: &str,
    gh_bin: &str,
    token: Option<&str>,
) -> Result<(), CheckoutError> {
    if !repo_path.starts_with('/') {
        return Err(CheckoutError::Invalid(format!(
            "caminho deve ser absoluto: {repo_path}"
        )));
    }
    let want = expected(owner, repo, forge);
    let path = Path::new(repo_path);
    if !path.exists() {
        match forge {
            ForgeConfig::Github => {
                ct_git::gh_clone(gh_bin, owner, repo, path, token).map_err(|e| {
                    CheckoutError::Clone(format!(
                        "clone de {owner}/{repo} em {repo_path} falhou: {e}"
                    ))
                })?
            }
            ForgeConfig::Gitlab(g) => {
                let url = g.clone_url.trim();
                if url.is_empty() {
                    return Err(CheckoutError::Clone(format!(
                        "URL de clone do GitLab ausente no cadastro de {}",
                        want.path
                    )));
                }
                // O credential helper (`osxkeychain` no macOS) pode não
                // responder ao daemon no launchd: a saída é o clone à mão.
                ct_git::clone_url(url, path).map_err(|e| {
                    CheckoutError::Clone(format!(
                        "clone de {url} em {repo_path} falhou: {e}. Se a credencial do git \
                         não responde a partir do ctowerd, clone à mão num terminal do \
                         ambiente: git clone {url} {repo_path}"
                    ))
                })?
            }
        }
    }
    let origin = ct_git::origin_url(path).map_err(|e| {
        CheckoutError::Invalid(format!(
            "{repo_path} existe mas não é um repositório git: {e}"
        ))
    })?;
    let same = origin
        .as_deref()
        .and_then(repo_from_url)
        .is_some_and(|r| r.host == want.host && r.path.eq_ignore_ascii_case(&want.path));
    if !same {
        return Err(CheckoutError::Invalid(format!(
            "{repo_path} é um clone de {}, não de {}",
            origin.unwrap_or_else(|| "(sem origin)".into()),
            match forge {
                ForgeConfig::Github => format!("{owner}/{repo}"),
                ForgeConfig::Gitlab(_) => format!("{}/{}", want.host, want.path),
            }
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ct_sessions::test_support::git_in;

    /// `gh` que falha: nenhum caso aqui pode clonar.
    const NO_GH: &str = "/bin/false";
    const GH: ForgeConfig = ForgeConfig::Github;

    fn gitlab(clone_url: &str) -> ForgeConfig {
        ForgeConfig::Gitlab(ct_core::GitlabConfig {
            host: "gitlab.example.com".into(),
            path: "grupo/projeto/app".into(),
            clone_url: clone_url.into(),
            credential_cmd: String::new(),
        })
    }

    /// Critério de aceite da #350: o `origin` SSH e o HTTPS do mesmo
    /// repositório do GitLab self-hosted valem; outro caminho ou outro host,
    /// não.
    #[test]
    fn gitlab_aceita_origin_ssh_e_https_do_mesmo_repositorio() {
        let tmp = tempfile::tempdir().unwrap();
        let forge = gitlab("https://gitlab.example.com/grupo/projeto/app.git");
        for (i, url) in [
            "git@gitlab.example.com:grupo/projeto/app.git",
            "ssh://git@gitlab.example.com:2222/grupo/projeto/app.git",
            "https://GitLab.Example.com/grupo/projeto/app",
            "https://usuario@gitlab.example.com/grupo/projeto/app.git",
        ]
        .iter()
        .enumerate()
        {
            let dir = tmp.path().join(format!("r{i}"));
            repo(&dir, Some(url));
            assert_eq!(
                ensure_checkout(
                    "grupo/projeto",
                    "app",
                    &forge,
                    dir.to_str().unwrap(),
                    NO_GH,
                    None
                ),
                Ok(()),
                "{url}"
            );
        }
        for (i, url) in [
            "git@gitlab.example.com:grupo/projeto/outro.git",
            "https://gitlab.com/grupo/projeto/app.git",
            "https://github.com/pje/app.git",
        ]
        .iter()
        .enumerate()
        {
            let dir = tmp.path().join(format!("x{i}"));
            repo(&dir, Some(url));
            let err = ensure_checkout(
                "grupo/projeto",
                "app",
                &forge,
                dir.to_str().unwrap(),
                NO_GH,
                None,
            )
            .unwrap_err();
            assert!(
                matches!(&err, CheckoutError::Invalid(m)
                    if m.ends_with("não de gitlab.example.com/grupo/projeto/app")),
                "{url}: {err:?}"
            );
        }
    }

    /// Critério de aceite da #350: pasta ausente é clonada pela URL do
    /// cadastro (aqui um repositório local no lugar do GitLab); a falha diz
    /// como clonar à mão.
    #[test]
    fn gitlab_clona_pela_url_do_cadastro() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("app.git");
        repo(
            &src,
            Some("https://gitlab.example.com/grupo/projeto/app.git"),
        );
        git_in(&src, &["commit", "-q", "--allow-empty", "-m", "init"]);
        // O clone local herda a URL do caminho; o `origin` passa a ser o do
        // GitLab, como num clone de verdade.
        let dest = tmp.path().join("projetos/app");
        let forge = gitlab(src.to_str().unwrap());
        let err = ensure_checkout(
            "grupo/projeto",
            "app",
            &forge,
            dest.to_str().unwrap(),
            NO_GH,
            None,
        )
        .unwrap_err();
        assert!(
            matches!(&err, CheckoutError::Invalid(m) if m.contains("é um clone de")),
            "o origin é o caminho local: {err:?}"
        );
        assert!(dest.join(".git").exists(), "clonou pela URL do cadastro");
        git_in(
            &dest,
            &[
                "remote",
                "set-url",
                "origin",
                "https://gitlab.example.com/grupo/projeto/app.git",
            ],
        );
        assert_eq!(
            ensure_checkout(
                "grupo/projeto",
                "app",
                &forge,
                dest.to_str().unwrap(),
                NO_GH,
                None
            ),
            Ok(())
        );

        let longe = tmp.path().join("longe");
        let err = ensure_checkout(
            "grupo/projeto",
            "app",
            &gitlab("https://127.0.0.1:9/grupo/projeto/app.git"),
            longe.to_str().unwrap(),
            NO_GH,
            None,
        )
        .unwrap_err();
        assert!(
            matches!(&err, CheckoutError::Clone(m)
            if m.contains("clone à mão") && m.contains(&format!(
                "git clone https://127.0.0.1:9/grupo/projeto/app.git {}",
                longe.display()
            ))),
            "{err:?}"
        );
        let err = ensure_checkout(
            "grupo/projeto",
            "app",
            &gitlab(""),
            longe.to_str().unwrap(),
            NO_GH,
            None,
        )
        .unwrap_err();
        assert!(
            matches!(&err, CheckoutError::Clone(m) if m.contains("URL de clone")),
            "{err:?}"
        );
    }

    fn repo(dir: &Path, origin: Option<&str>) {
        std::fs::create_dir_all(dir).unwrap();
        git_in(dir, &["init", "-q", "-b", "develop"]);
        if let Some(url) = origin {
            git_in(dir, &["remote", "add", "origin", url]);
        }
    }

    #[test]
    fn clone_existente_do_mesmo_repositorio_e_aceito() {
        let tmp = tempfile::tempdir().unwrap();
        for (i, url) in [
            "git@github.com:deep-ion-ai/demo.git",
            "https://github.com/Deep-Ion-AI/Demo",
        ]
        .iter()
        .enumerate()
        {
            let dir = tmp.path().join(format!("r{i}"));
            repo(&dir, Some(url));
            assert_eq!(
                ensure_checkout(
                    "deep-ion-ai",
                    "demo",
                    &GH,
                    dir.to_str().unwrap(),
                    NO_GH,
                    None
                ),
                Ok(()),
                "{url}"
            );
        }
    }

    #[test]
    fn pasta_que_nao_e_repositorio_git_e_recusada() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("solta");
        std::fs::create_dir_all(&dir).unwrap();
        let err = ensure_checkout(
            "deep-ion-ai",
            "demo",
            &GH,
            dir.to_str().unwrap(),
            NO_GH,
            None,
        )
        .unwrap_err();
        assert!(
            matches!(&err, CheckoutError::Invalid(m) if m.contains("existe mas não é um repositório git")),
            "{err:?}"
        );
    }

    #[test]
    fn clone_de_outro_repositorio_ou_sem_origin_e_recusado() {
        let tmp = tempfile::tempdir().unwrap();
        let outro = tmp.path().join("outro");
        repo(&outro, Some("https://github.com/o/r.git"));
        let err = ensure_checkout(
            "deep-ion-ai",
            "demo",
            &GH,
            outro.to_str().unwrap(),
            NO_GH,
            None,
        )
        .unwrap_err();
        assert!(
            matches!(&err, CheckoutError::Invalid(m)
                if m.contains("é um clone de https://github.com/o/r.git, não de deep-ion-ai/demo")),
            "{err:?}"
        );
        let sem = tmp.path().join("sem-origin");
        repo(&sem, None);
        let err = ensure_checkout(
            "deep-ion-ai",
            "demo",
            &GH,
            sem.to_str().unwrap(),
            NO_GH,
            None,
        )
        .unwrap_err();
        assert!(
            matches!(&err, CheckoutError::Invalid(m) if m.contains("(sem origin)")),
            "{err:?}"
        );
    }

    #[test]
    fn caminho_relativo_e_recusado() {
        let err = ensure_checkout("o", "r", &GH, "projetos/r", NO_GH, None).unwrap_err();
        assert!(matches!(err, CheckoutError::Invalid(_)), "{err:?}");
    }
}
