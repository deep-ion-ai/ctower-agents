use ct_git::{
    current_branch, ensure_worktree, remove_worktree, GitError, WorktreeOutcome, WorktreeSpec,
};
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn repo() -> TempDir {
    let d = TempDir::new().unwrap();
    git(d.path(), &["init", "-q", "-b", "develop"]);
    git(d.path(), &["config", "user.email", "t@t"]);
    git(d.path(), &["config", "user.name", "t"]);
    git(d.path(), &["commit", "-q", "--allow-empty", "-m", "init"]);
    d
}

#[test]
fn cria_worktree_com_branch_nova_a_partir_da_base() {
    let r = repo();
    let wts = TempDir::new().unwrap();
    let wt = wts.path().join("demo/wip__12-auth");
    let spec = WorktreeSpec {
        repo_path: r.path(),
        branch: "wip/12-auth",
        base: "develop",
        worktree_path: &wt,
    };
    assert_eq!(ensure_worktree(&spec).unwrap(), WorktreeOutcome::Created);
    assert_eq!(current_branch(&wt).unwrap().as_deref(), Some("wip/12-auth"));
    git(
        r.path(),
        &["merge-base", "--is-ancestor", "develop", "wip/12-auth"],
    );
}

#[test]
fn segunda_chamada_reutiliza() {
    let r = repo();
    let wts = TempDir::new().unwrap();
    let wt = wts.path().join("wt");
    let spec = WorktreeSpec {
        repo_path: r.path(),
        branch: "wip/12-auth",
        base: "develop",
        worktree_path: &wt,
    };
    ensure_worktree(&spec).unwrap();
    assert_eq!(ensure_worktree(&spec).unwrap(), WorktreeOutcome::Reused);
}

#[test]
fn usa_branch_local_existente() {
    let r = repo();
    git(r.path(), &["branch", "feat/1-x"]);
    let wts = TempDir::new().unwrap();
    let wt = wts.path().join("wt");
    let spec = WorktreeSpec {
        repo_path: r.path(),
        branch: "feat/1-x",
        base: "develop",
        worktree_path: &wt,
    };
    assert_eq!(ensure_worktree(&spec).unwrap(), WorktreeOutcome::Created);
    assert_eq!(current_branch(&wt).unwrap().as_deref(), Some("feat/1-x"));
}

#[test]
fn usa_branch_remota_com_tracking() {
    let origin = repo();
    git(origin.path(), &["branch", "wip/9-fisica"]);
    let clones = TempDir::new().unwrap();
    let clone = clones.path().join("clone");
    let out = Command::new("git")
        .args(["clone", "-q"])
        .arg(origin.path())
        .arg(&clone)
        .output()
        .unwrap();
    assert!(out.status.success());
    let wts = TempDir::new().unwrap();
    let wt = wts.path().join("wt");
    let spec = WorktreeSpec {
        repo_path: &clone,
        branch: "wip/9-fisica",
        base: "develop",
        worktree_path: &wt,
    };
    ensure_worktree(&spec).unwrap();
    assert_eq!(
        current_branch(&wt).unwrap().as_deref(),
        Some("wip/9-fisica")
    );
    assert_eq!(
        git(&wt, &["rev-parse", "--abbrev-ref", "@{upstream}"]),
        "origin/wip/9-fisica"
    );
}

#[test]
fn caminho_ocupado_por_outra_branch_falha_sem_alterar() {
    let r = repo();
    let wts = TempDir::new().unwrap();
    let wt = wts.path().join("wt");
    let a = WorktreeSpec {
        repo_path: r.path(),
        branch: "wip/1-a",
        base: "develop",
        worktree_path: &wt,
    };
    ensure_worktree(&a).unwrap();
    let b = WorktreeSpec {
        repo_path: r.path(),
        branch: "wip/2-b",
        base: "develop",
        worktree_path: &wt,
    };
    let err = ensure_worktree(&b).unwrap_err();
    assert!(matches!(err, GitError::BranchMismatch { ref actual, .. } if actual == "wip/1-a"));
    assert_eq!(current_branch(&wt).unwrap().as_deref(), Some("wip/1-a"));
    assert!(git(r.path(), &["branch", "--list", "wip/2-b"]).is_empty());
}

#[test]
fn base_inexistente_gera_erro_do_git() {
    let r = repo();
    let wts = TempDir::new().unwrap();
    let wt = wts.path().join("proj/wt");
    let spec = WorktreeSpec {
        repo_path: r.path(),
        branch: "wip/3-c",
        base: "nao-existe",
        worktree_path: &wt,
    };
    assert!(matches!(
        ensure_worktree(&spec),
        Err(GitError::Failed { .. })
    ));
    assert!(!wts.path().join("proj").exists());
    assert!(git(r.path(), &["branch", "--list", "wip/3-c"]).is_empty());
}

#[test]
fn remove_worktree_limpo() {
    let r = repo();
    let wts = TempDir::new().unwrap();
    let wt = wts.path().join("wt");
    let spec = WorktreeSpec {
        repo_path: r.path(),
        branch: "wip/4-d",
        base: "develop",
        worktree_path: &wt,
    };
    ensure_worktree(&spec).unwrap();
    remove_worktree(r.path(), &wt).unwrap();
    assert!(!wt.exists());
}

fn spec<'a>(repo: &'a Path, branch: &'a str, base: &'a str, wt: &'a Path) -> WorktreeSpec<'a> {
    WorktreeSpec {
        repo_path: repo,
        branch,
        base,
        worktree_path: wt,
    }
}

#[test]
fn diretorio_comum_dentro_de_outro_repositorio_nao_e_reutilizado() {
    let r = repo();
    let outro = repo();
    git(outro.path(), &["checkout", "-q", "-b", "wip/1-a"]);
    let wt = outro.path().join("sub/dir");
    std::fs::create_dir_all(&wt).unwrap();
    let err = ensure_worktree(&spec(r.path(), "wip/1-a", "develop", &wt)).unwrap_err();
    assert!(
        matches!(err, GitError::NotAWorktreeOf { ref path } if path == &wt),
        "{err:?}"
    );
    assert!(git(r.path(), &["branch", "--list", "wip/1-a"]).is_empty());
    assert_eq!(
        current_branch(outro.path()).unwrap().as_deref(),
        Some("wip/1-a")
    );
}

#[test]
fn worktree_de_outro_repositorio_com_mesma_branch_nao_e_reutilizado() {
    let r = repo();
    let outro = repo();
    let wts = TempDir::new().unwrap();
    let wt = wts.path().join("wt");
    ensure_worktree(&spec(outro.path(), "wip/1-a", "develop", &wt)).unwrap();
    let err = ensure_worktree(&spec(r.path(), "wip/1-a", "develop", &wt)).unwrap_err();
    assert!(matches!(err, GitError::NotAWorktreeOf { .. }), "{err:?}");
    assert!(git(r.path(), &["branch", "--list", "wip/1-a"]).is_empty());
    assert_eq!(current_branch(&wt).unwrap().as_deref(), Some("wip/1-a"));
}

#[test]
fn diretorio_vazio_fora_de_repositorio_nao_e_reutilizado() {
    let r = repo();
    let wts = TempDir::new().unwrap();
    let wt = wts.path().join("wt");
    std::fs::create_dir_all(&wt).unwrap();
    let err = ensure_worktree(&spec(r.path(), "wip/1-a", "develop", &wt)).unwrap_err();
    assert!(matches!(err, GitError::NotAWorktreeOf { .. }), "{err:?}");
    assert!(git(r.path(), &["branch", "--list", "wip/1-a"]).is_empty());
    assert_eq!(std::fs::read_dir(&wt).unwrap().count(), 0);
}

#[test]
fn base_iniciando_com_hifen_e_rejeitada_sem_efeito() {
    let r = repo();
    let wts = TempDir::new().unwrap();
    for base in ["--force", "--detach", "-f", "--orphan"] {
        let wt = wts.path().join("proj/wt");
        let res = ensure_worktree(&spec(r.path(), "wip/9-z", base, &wt));
        assert!(res.is_err(), "base {base:?}: {res:?}");
        assert!(!wt.exists(), "base {base:?}");
        assert!(!wts.path().join("proj").exists(), "base {base:?}");
        assert!(git(r.path(), &["branch", "--list", "wip/9-z"]).is_empty());
    }
}

#[test]
fn branch_invalida_e_rejeitada_sem_efeito() {
    let r = repo();
    let wts = TempDir::new().unwrap();
    for branch in ["-f", "--detach", "-x", "a..b", "com espaco"] {
        let wt = wts.path().join("proj/wt");
        let res = ensure_worktree(&spec(r.path(), branch, "develop", &wt));
        assert!(res.is_err(), "branch {branch:?}: {res:?}");
        assert!(!wt.exists(), "branch {branch:?}");
        assert!(!wts.path().join("proj").exists(), "branch {branch:?}");
    }
}

#[test]
fn caminho_com_espacos_funciona_e_e_removido() {
    let r = repo();
    let wts = TempDir::new().unwrap();
    let wt = wts.path().join("meu projeto/wip__1 a");
    let s = spec(r.path(), "wip/1-a", "develop", &wt);
    assert_eq!(ensure_worktree(&s).unwrap(), WorktreeOutcome::Created);
    assert_eq!(ensure_worktree(&s).unwrap(), WorktreeOutcome::Reused);
    remove_worktree(r.path(), &wt).unwrap();
    assert!(!wt.exists());
}

#[test]
fn clone_ausente_da_erro_claro_e_nao_cria_nada() {
    let d = TempDir::new().unwrap();
    let repo = d.path().join("nao-existe");
    let wt = d.path().join("wt/demo/wip__32-x");
    let err = ensure_worktree(&WorktreeSpec {
        repo_path: &repo,
        branch: "wip/32-modo-multijogador-no-mesmo-aparelho-2-a",
        base: "develop",
        worktree_path: &wt,
    })
    .unwrap_err();
    assert!(matches!(err, GitError::RepoMissing { .. }), "{err}");
    assert!(err.to_string().contains("clone do projeto não encontrado"));
    assert!(!wt.exists() && !d.path().join("wt").exists());
}

/// Clone de um `origin` local (bare), com o worktree da branch `feat/5-x`
/// criado a partir de `develop` (que já está em `origin`).
struct Pushed {
    _tmp: TempDir,
    clone: std::path::PathBuf,
    wt: std::path::PathBuf,
}

fn clone_com_worktree() -> Pushed {
    let tmp = TempDir::new().unwrap();
    let seed = repo();
    let origin = tmp.path().join("origin.git");
    let out = Command::new("git")
        .args(["clone", "-q", "--bare"])
        .arg(seed.path())
        .arg(&origin)
        .output()
        .unwrap();
    assert!(out.status.success());
    let clone = tmp.path().join("clone");
    let out = Command::new("git")
        .args(["clone", "-q"])
        .arg(&origin)
        .arg(&clone)
        .output()
        .unwrap();
    assert!(out.status.success());
    git(&clone, &["config", "user.email", "t@t"]);
    git(&clone, &["config", "user.name", "t"]);
    let wt = tmp.path().join("wt/demo/feat__5-x");
    ensure_worktree(&spec(&clone, "feat/5-x", "develop", &wt)).unwrap();
    Pushed {
        _tmp: tmp,
        clone,
        wt,
    }
}

fn commit(dir: &Path, file: &str) {
    std::fs::write(dir.join(file), file).unwrap();
    git(dir, &["add", file]);
    git(dir, &["commit", "-q", "-m", file]);
}

/// Critério de aceite do #265: worktree limpo e enviado pode sair; o
/// `target/` (ignorado) vai junto, e o `prune` limpa o registro.
#[test]
fn worktree_limpo_e_enviado_sai() {
    let p = clone_com_worktree();
    std::fs::write(p.wt.join(".gitignore"), "/target\n").unwrap();
    git(&p.wt, &["add", ".gitignore"]);
    git(&p.wt, &["commit", "-q", "-m", "ignora target"]);
    git(&p.wt, &["push", "-q", "-u", "origin", "feat/5-x"]);
    std::fs::create_dir_all(p.wt.join("target/debug")).unwrap();
    std::fs::write(p.wt.join("target/debug/bin"), [0u8; 64]).unwrap();
    assert!(ct_git::is_ignored_untracked(&p.wt, "target").unwrap());
    assert_eq!(
        ct_git::worktree_blockers(&p.clone, &p.wt).unwrap(),
        Vec::<String>::new()
    );
    remove_worktree(&p.clone, &p.wt).unwrap();
    assert!(!p.wt.exists());
    ct_git::prune_worktrees(&p.clone).unwrap();
    assert_eq!(ct_git::find_worktree(&p.clone, &p.wt).unwrap(), None);
}

/// Critério de aceite do #265: mudança não commitada (inclusive arquivo
/// novo) nunca deixa o worktree sair.
#[test]
fn worktree_com_mudanca_nao_commitada_fica() {
    let p = clone_com_worktree();
    git(&p.wt, &["push", "-q", "-u", "origin", "feat/5-x"]);
    std::fs::write(p.wt.join("novo.txt"), "rascunho").unwrap();
    let b = ct_git::worktree_blockers(&p.clone, &p.wt).unwrap();
    assert_eq!(b.len(), 1, "{b:?}");
    assert!(
        b[0].contains("1 mudança(s) não commitada(s): novo.txt"),
        "{b:?}"
    );
    // O git também recusa sem --force: nada sai.
    assert!(remove_worktree(&p.clone, &p.wt).is_err());
    assert!(p.wt.join("novo.txt").is_file());
}

/// Critério de aceite do #265: commit não enviado (à frente do upstream ou
/// sem upstream) nunca deixa o worktree sair.
#[test]
fn worktree_com_commit_nao_enviado_fica() {
    let p = clone_com_worktree();
    // Sem upstream, com um commit só local.
    commit(&p.wt, "a.txt");
    let b = ct_git::worktree_blockers(&p.clone, &p.wt).unwrap();
    assert!(
        b.len() == 1 && b[0].contains("não estão em nenhuma branch remota"),
        "{b:?}"
    );
    // Com upstream, um commit à frente.
    git(&p.wt, &["push", "-q", "-u", "origin", "feat/5-x"]);
    assert!(ct_git::worktree_blockers(&p.clone, &p.wt)
        .unwrap()
        .is_empty());
    commit(&p.wt, "b.txt");
    let b = ct_git::worktree_blockers(&p.clone, &p.wt).unwrap();
    assert_eq!(
        b,
        vec!["1 commit(s) não enviado(s) para origin/feat/5-x".to_string()]
    );
    // HEAD destacado num commit só local.
    git(&p.wt, &["checkout", "-q", "--detach"]);
    let b = ct_git::worktree_blockers(&p.clone, &p.wt).unwrap();
    assert!(b[0].contains("HEAD destacado"), "{b:?}");
}

/// O clone principal, um diretório qualquer e um worktree travado nunca
/// saem.
#[test]
fn clone_principal_e_worktree_travado_ficam() {
    let p = clone_com_worktree();
    git(&p.wt, &["push", "-q", "-u", "origin", "feat/5-x"]);
    let b = ct_git::worktree_blockers(&p.clone, &p.clone).unwrap();
    assert!(b[0].contains("é o clone principal"), "{b:?}");
    let outro = TempDir::new().unwrap();
    let b = ct_git::worktree_blockers(&p.clone, outro.path()).unwrap();
    assert!(b[0].contains("não é um worktree deste clone"), "{b:?}");
    git(&p.clone, &["worktree", "lock", p.wt.to_str().unwrap()]);
    let b = ct_git::worktree_blockers(&p.clone, &p.wt).unwrap();
    assert_eq!(b, vec!["worktree travado (git worktree lock)".to_string()]);
    let e = ct_git::find_worktree(&p.clone, &p.wt).unwrap().unwrap();
    assert!(e.locked && e.branch.as_deref() == Some("feat/5-x"));
}

/// Diretório apagado à mão: o registro fica `prunable` até o `prune`.
#[test]
fn registro_orfao_sai_no_prune() {
    let p = clone_com_worktree();
    std::fs::remove_dir_all(&p.wt).unwrap();
    let e = ct_git::find_worktree(&p.clone, &p.wt).unwrap().unwrap();
    assert!(e.prunable);
    ct_git::prune_worktrees(&p.clone).unwrap();
    assert_eq!(ct_git::find_worktree(&p.clone, &p.wt).unwrap(), None);
}

/// `target/` versionado ou fora do `.gitignore` não conta como descartável.
#[test]
fn target_so_e_descartavel_se_ignorado_e_nao_versionado() {
    let p = clone_com_worktree();
    std::fs::create_dir_all(p.wt.join("target")).unwrap();
    std::fs::write(p.wt.join("target/x"), "x").unwrap();
    assert!(!ct_git::is_ignored_untracked(&p.wt, "target").unwrap());
    git(&p.wt, &["add", "-f", "target/x"]);
    git(&p.wt, &["commit", "-q", "-m", "x"]);
    std::fs::write(p.wt.join(".gitignore"), "target/\n").unwrap();
    assert!(!ct_git::is_ignored_untracked(&p.wt, "target").unwrap());
}
