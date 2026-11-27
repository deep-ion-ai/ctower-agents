//! `origin_url` e `gh_clone` (com um `gh` falso que clona de um repositório local).

use ct_git::{gh_clone, origin_url, GitError};
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

fn git(dir: &Path, args: &[&str]) {
    let ok = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .status()
        .unwrap()
        .success();
    assert!(ok, "git {args:?}");
}

/// `gh` falso: `gh repo clone o/r <dest>` clona `src` e registra o GH_TOKEN.
fn fake_gh(dir: &Path, src: &Path) -> String {
    let bin = dir.join("gh");
    write_exec(
        &bin,
        &format!(
            "#!/bin/sh\necho \"$GH_TOKEN\" > {log}\ngit clone -q {src} \"$4\" && git -C \"$4\" remote set-url origin https://github.com/$3.git\n",
            log = dir.join("token.log").display(),
            src = src.display()
        ),
    );
    bin.to_string_lossy().into_owned()
}

/// Grava um script executável por um `sh` filho, e não por este processo.
///
/// Os testes rodam em threads do mesmo processo. Se este processo abrisse o
/// script para escrita, um `fork` concorrente de outro teste (git, gh) herdaria
/// o descritor até o seu `exec`, e executar o script nesse intervalo dá
/// ETXTBSY ("Text file busy") no Linux. Gravando no filho, o descritor de
/// escrita nunca existe aqui e não há o que herdar.
fn write_exec(path: &Path, body: &str) {
    let ok = Command::new("sh")
        .args([
            "-c",
            "printf '%s' \"$2\" > \"$1\" && chmod 755 \"$1\"",
            "sh",
        ])
        .arg(path)
        .arg(body)
        .status()
        .unwrap()
        .success();
    assert!(ok, "gravar {}", path.display());
}

#[test]
fn clona_com_token_e_le_o_origin() {
    let src = TempDir::new().unwrap();
    git(src.path(), &["init", "-q", "-b", "develop"]);
    git(
        src.path(),
        &[
            "-c",
            "user.email=t@t",
            "-c",
            "user.name=t",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "init",
        ],
    );
    let d = TempDir::new().unwrap();
    let gh = fake_gh(d.path(), src.path());
    let dest = d.path().join("projetos/demo");
    gh_clone(&gh, "deep-ion-ai", "demo", &dest, Some("tok-x")).unwrap();
    assert_eq!(
        origin_url(&dest).unwrap().as_deref(),
        Some("https://github.com/deep-ion-ai/demo.git")
    );
    let log = std::fs::read_to_string(d.path().join("token.log")).unwrap();
    assert_eq!(log.trim(), "tok-x");
    // Sem token, o gh usa o próprio login (GH_TOKEN vazio).
    gh_clone(&gh, "deep-ion-ai", "demo", &d.path().join("outro"), None).unwrap();
    let log = std::fs::read_to_string(d.path().join("token.log")).unwrap();
    assert_eq!(log.trim(), "");
}

#[test]
fn falhas() {
    let d = TempDir::new().unwrap();
    // Pasta que não é repositório git.
    assert!(origin_url(d.path()).is_err());
    // Repositório sem origin.
    git(d.path(), &["init", "-q"]);
    assert_eq!(origin_url(d.path()).unwrap(), None);
    // gh que falha.
    let err = gh_clone("false", "o", "r", &d.path().join("x"), None).unwrap_err();
    assert!(matches!(err, GitError::Failed { .. }), "{err}");
}

/// `clone_url` (#350): clone direto pela URL do cadastro, de qualquer host.
/// Aqui a "URL" é um caminho local; o `origin` fica sendo ela.
#[test]
fn clona_pela_url_do_cadastro() {
    let src = TempDir::new().unwrap();
    git(src.path(), &["init", "-q", "-b", "develop"]);
    git(
        src.path(),
        &[
            "-c",
            "user.email=t@t",
            "-c",
            "user.name=t",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "init",
        ],
    );
    let d = TempDir::new().unwrap();
    let dest = d.path().join("projetos/app");
    let url = src.path().to_str().unwrap();
    ct_git::clone_url(url, &dest, None).unwrap();
    assert_eq!(origin_url(&dest).unwrap().as_deref(), Some(url));

    // URL que o git não alcança: erro com o motivo, sem pedir senha.
    let err =
        ct_git::clone_url("https://127.0.0.1:9/g/r.git", &d.path().join("x"), None).unwrap_err();
    assert!(
        matches!(&err, GitError::Failed { args, .. } if args == "clone https://127.0.0.1:9/g/r.git"),
        "{err}"
    );
    // Opção no lugar da URL é recusada antes do git.
    assert!(matches!(
        ct_git::clone_url("--upload-pack=x", &d.path().join("y"), None),
        Err(GitError::InvalidRef { .. })
    ));
}

/// #281: o token da conexão do GitLab vai ao git como cabeçalho do host
/// da URL, por variável de ambiente; SSH não leva token.
#[test]
fn token_do_gitlab_vira_cabecalho_do_host() {
    use std::ffi::OsString;
    let envs =
        ct_git::token_auth_envs("https://usuario@gitlab.example.com/g/r.git", "tok-ficticio")
            .unwrap();
    let get = |k: &str| {
        envs.iter()
            .find(|(n, _)| n == &OsString::from(k))
            .map(|(_, v)| v.to_string_lossy().into_owned())
    };
    assert_eq!(get("GIT_CONFIG_COUNT").as_deref(), Some("1"));
    assert_eq!(
        get("GIT_CONFIG_KEY_0").as_deref(),
        Some("http.https://gitlab.example.com/.extraHeader")
    );
    // base64("oauth2:tok-ficticio")
    assert_eq!(
        get("GIT_CONFIG_VALUE_0").as_deref(),
        Some("Authorization: Basic b2F1dGgyOnRvay1maWN0aWNpbw==")
    );
    assert!(ct_git::token_auth_envs("git@gitlab.example.com:g/r.git", "t").is_none());
    assert!(ct_git::token_auth_envs("http://gitlab.example.com/g/r.git", "t").is_none());

    // O erro do clone com token não leva o token.
    let d = TempDir::new().unwrap();
    let err = ct_git::clone_url(
        "https://127.0.0.1:9/g/r.git",
        &d.path().join("z"),
        Some("tok-ficticio"),
    )
    .unwrap_err()
    .to_string();
    assert!(
        !err.contains("tok-ficticio") && !err.contains("b2F1dGgy"),
        "{err}"
    );
}

/// #334: o credential helper entra por variável de ambiente, só para o host
/// da URL, depois do `GIT_CONFIG_COUNT` herdado; SSH e HTTP não levam.
#[test]
fn credential_helper_vale_so_para_o_host_da_url() {
    use std::ffi::OsString;
    let get = |envs: &[(OsString, OsString)], k: &str| {
        envs.iter()
            .find(|(n, _)| n == &OsString::from(k))
            .map(|(_, v)| v.to_string_lossy().into_owned())
    };
    let envs = ct_git::credential_helper_envs(
        "https://usuario@gitlab.example.com:8443/g/r.git",
        "'ctower' git-credential",
        0,
    )
    .unwrap();
    assert_eq!(get(&envs, "GIT_CONFIG_COUNT").as_deref(), Some("1"));
    assert_eq!(
        get(&envs, "GIT_CONFIG_KEY_0").as_deref(),
        Some("credential.https://gitlab.example.com:8443.helper")
    );
    assert_eq!(
        get(&envs, "GIT_CONFIG_VALUE_0").as_deref(),
        Some("!'ctower' git-credential")
    );
    let envs =
        ct_git::credential_helper_envs("https://gitlab.example.com/g/r.git", "h", 2).unwrap();
    assert_eq!(get(&envs, "GIT_CONFIG_COUNT").as_deref(), Some("3"));
    assert!(get(&envs, "GIT_CONFIG_KEY_2").is_some());
    assert!(ct_git::credential_helper_envs("git@gitlab.example.com:g/r.git", "h", 0).is_none());
    assert!(ct_git::credential_helper_envs("http://gitlab.example.com/g/r.git", "h", 0).is_none());
}

/// #334: o git de verdade chama o helper do ambiente só para o host dele, e
/// o token fica fora do `.git/config`.
#[test]
fn git_chama_o_helper_so_para_o_host_da_conexao() {
    use std::process::{Command, Stdio};
    let d = TempDir::new().unwrap();
    let helper = d.path().join("helper.sh");
    std::fs::write(
        &helper,
        "#!/bin/sh\ncat >/dev/null\nprintf 'username=oauth2\\npassword=tok-ficticio\\n'\n",
    )
    .unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let repo = d.path().join("r");
    std::fs::create_dir(&repo).unwrap();
    assert!(Command::new("git")
        .args(["init", "--quiet"])
        .arg(&repo)
        .status()
        .unwrap()
        .success());
    let envs = ct_git::credential_helper_envs(
        "https://gitlab.example.com/g/r.git",
        &format!("'{}'", helper.display()),
        0,
    )
    .unwrap();
    let fill = |host: &str| {
        let mut c = Command::new("git");
        c.current_dir(&repo)
            .args(["credential", "fill"])
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .envs(envs.iter().map(|(k, v)| (k, v)))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = c.spawn().unwrap();
        {
            use std::io::Write;
            child
                .stdin
                .take()
                .unwrap()
                .write_all(format!("protocol=https\nhost={host}\n\n").as_bytes())
                .unwrap();
        }
        child.wait_with_output().unwrap()
    };
    let ok = fill("gitlab.example.com");
    assert!(ok.status.success());
    let out = String::from_utf8_lossy(&ok.stdout);
    assert!(out.contains("username=oauth2"), "{out}");
    assert!(out.contains("password=tok-ficticio"), "{out}");
    // Outro host: o helper não é consultado e o git, sem prompt, falha.
    let outro = fill("outro.example.com");
    assert!(!outro.status.success());
    assert!(!String::from_utf8_lossy(&outro.stdout).contains("tok-ficticio"));
    // O `.git/config` não guarda o helper nem o token.
    let cfg = std::fs::read_to_string(repo.join(".git/config")).unwrap();
    assert!(
        !cfg.contains("helper") && !cfg.contains("tok-ficticio"),
        "{cfg}"
    );
}
