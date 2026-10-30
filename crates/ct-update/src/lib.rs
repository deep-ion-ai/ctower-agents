//! Atualização por build local (issues #90 e #181): um clone dedicado em
//! `<data>/update/src` segue o ref configurado (`update_ref`). Aqui ficam a
//! verificação, o estado (`state.json`) e o atualizador genérico
//! ([`runner::Runner`]), usados pelo `ctower update` (ctowerd + ctower) e
//! pelo `cthubd update` (cthubd).

pub mod launch;
pub mod runner;

/// Erro do atualizador (mensagem pronta para o usuário).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error(pub String);

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, Error>;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Quantos commits novos listar na verificação.
const MAX_COMMITS: usize = 20;

/// Versão e commit de um build.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Build {
    pub version: String,
    /// Commit completo; vazio se o build foi feito fora do git.
    pub commit: String,
}

impl Build {
    /// O build deste binário.
    pub fn current() -> Self {
        Build {
            version: ct_core::VERSION.into(),
            commit: ct_core::BUILD_COMMIT.into(),
        }
    }
}

/// Posição do commit instalado em relação ao do ref remoto.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum Relation {
    /// Mesmo commit.
    Same,
    /// O remoto contém o instalado: há atualização.
    Behind,
    /// O instalado contém o remoto (build de desenvolvimento à frente).
    Ahead,
    /// Históricos separados (ex.: instalado de `develop`, ref `main`).
    Diverged,
    /// Commit instalado desconhecido (fora do git ou não publicado).
    Unknown,
}

/// Resultado de uma verificação.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Available {
    #[serde(rename = "ref")]
    #[cfg_attr(feature = "ts", ts(rename = "ref"))]
    pub git_ref: String,
    pub build: Build,
    pub relation: Relation,
    pub update_available: bool,
    /// Commits novos (`<hash curto> <assunto>`), do mais novo ao mais
    /// antigo, no máximo 20; só com `relation = behind`.
    pub commits: Vec<String>,
    /// Quantos commits o instalado está atrás; só com `relation = behind`.
    pub behind: Option<u32>,
    pub checked_at: i64,
}

/// De onde vêm as atualizações.
#[derive(Debug, Clone)]
pub struct Source {
    pub repo: String,
    pub git_ref: String,
    /// `<data>/update`.
    pub dir: PathBuf,
}

impl Source {
    /// Clone dedicado (nunca o de desenvolvimento do usuário).
    pub fn src(&self) -> PathBuf {
        self.dir.join("src")
    }

    /// Clona, se preciso, e busca o ref. Devolve o commit dele.
    pub fn fetch(&self) -> Result<String> {
        if self.repo.is_empty() || self.git_ref.is_empty() {
            return Err(Error(
                "update_repo e update_ref não podem ficar vazios".into(),
            ));
        }
        let src = self.src();
        if src.join(".git").exists() {
            // Respeita uma troca de `update_repo` no config.
            run(git(&src).args(["remote", "set-url", "origin", &self.repo]))?;
        } else {
            ct_core::files::private_dir(&self.dir)?;
            let _ = std::fs::remove_dir_all(&src);
            run(git(&self.dir)
                .args(["clone", "--quiet", "--no-checkout", &self.repo])
                .arg(&src))?;
        }
        run(git(&src).args(["fetch", "--quiet", "--force", "origin", &self.git_ref]))?;
        run(git(&src).args(["rev-parse", "FETCH_HEAD^{commit}"]))
    }

    /// Busca o ref e compara com `installed`.
    pub fn check(&self, installed: &Build, now_ms: i64) -> Result<Available> {
        let commit = self.fetch()?;
        let src = self.src();
        let manifest = run(git(&src).args(["show", &format!("{commit}:Cargo.toml")]))?;
        let version = workspace_version(&manifest).unwrap_or_default();
        let relation = relation(&src, &installed.commit, &commit)?;
        let (commits, behind) = if relation == Relation::Behind {
            let range = format!("{}..{commit}", installed.commit);
            let log = run(git(&src).args([
                "log",
                "--format=%h %s",
                &format!("-n{MAX_COMMITS}"),
                &range,
            ]))?;
            let count = run(git(&src).args(["rev-list", "--count", &range]))?;
            (log.lines().map(String::from).collect(), count.parse().ok())
        } else {
            (Vec::new(), None)
        };
        Ok(Available {
            git_ref: self.git_ref.clone(),
            build: Build { version, commit },
            update_available: matches!(
                relation,
                Relation::Behind | Relation::Diverged | Relation::Unknown
            ),
            relation,
            commits,
            behind,
            checked_at: now_ms,
        })
    }
}

/// Fase do atualizador (`<data>/update/state.json`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Starting,
    Fetch,
    Build,
    Backup,
    Install,
    Restart,
    Verify,
    /// Terminou com a versão nova (ou já estava em dia).
    Done,
    /// Falhou antes de tocar nos binários: a versão anterior segue no ar.
    Failed,
    /// O daemon novo não subiu; a versão anterior foi restaurada.
    RolledBack,
}

impl Phase {
    pub fn is_terminal(self) -> bool {
        matches!(self, Phase::Done | Phase::Failed | Phase::RolledBack)
    }
}

/// Andamento e resultado da última atualização (ou rollback manual).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct UpdateState {
    /// `update` ou `rollback`.
    pub kind: String,
    pub phase: Phase,
    #[serde(rename = "ref")]
    #[cfg_attr(feature = "ts", ts(rename = "ref"))]
    pub git_ref: String,
    pub from: Build,
    /// Versão alvo, conhecida depois do fetch (no rollback, a anterior).
    pub to: Option<Build>,
    pub started_at: i64,
    pub updated_at: i64,
    pub finished_at: Option<i64>,
    /// Observação do resultado (ex.: "já está em dia").
    pub message: Option<String>,
    pub error: Option<String>,
    /// Fim do `update.log` quando falha.
    pub log_tail: Vec<String>,
}

pub fn state_file(update_dir: &Path) -> PathBuf {
    update_dir.join("state.json")
}

pub fn log_file(update_dir: &Path) -> PathBuf {
    update_dir.join("update.log")
}

/// Lê o `state.json`; ausente ou ilegível = `None`.
pub fn read_state(update_dir: &Path) -> Option<UpdateState> {
    let raw = std::fs::read_to_string(state_file(update_dir)).ok()?;
    serde_json::from_str(&raw).ok()
}

/// Grava o `state.json` de forma atômica (arquivo temporário + rename).
pub fn write_state(update_dir: &Path, st: &UpdateState) -> Result<()> {
    ct_core::files::private_dir(update_dir)?;
    let tmp = update_dir.join(".state.json.tmp");
    std::fs::write(
        &tmp,
        serde_json::to_vec_pretty(st).map_err(std::io::Error::other)?,
    )?;
    std::fs::rename(&tmp, state_file(update_dir))?;
    Ok(())
}

/// Últimas `n` linhas de um arquivo (vazio se não existir).
pub fn tail(path: &Path, n: usize) -> Vec<String> {
    let s = std::fs::read_to_string(path).unwrap_or_default();
    let lines: Vec<&str> = s.lines().collect();
    lines[lines.len().saturating_sub(n)..]
        .iter()
        .map(|l| l.to_string())
        .collect()
}

/// Uma atualização em andamento: estado não terminal atualizado há menos de
/// 30 min (um atualizador morto não trava as próximas para sempre).
pub fn running(update_dir: &Path, now_ms: i64) -> Option<UpdateState> {
    read_state(update_dir)
        .filter(|s| !s.phase.is_terminal() && now_ms - s.updated_at < 30 * 60 * 1000)
}

/// Caminho absoluto de um binário: como veio, se já for absoluto; senão ao
/// lado deste executável; senão no `PATH`.
pub fn resolve_bin(name: &str) -> PathBuf {
    let p = Path::new(name);
    if p.is_absolute() {
        return p.to_path_buf();
    }
    let beside = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|d| d.join(name)))
        .filter(|p| p.is_file());
    if let Some(b) = beside {
        return b;
    }
    std::env::var_os("PATH")
        .and_then(|path| {
            std::env::split_paths(&path)
                .map(|d| d.join(name))
                .find(|p| p.is_file())
        })
        .unwrap_or_else(|| p.to_path_buf())
}

fn relation(src: &Path, installed: &str, remote: &str) -> Result<Relation> {
    if installed.is_empty() {
        return Ok(Relation::Unknown);
    }
    if installed == remote {
        return Ok(Relation::Same);
    }
    let known = git(src)
        .args(["cat-file", "-e", &format!("{installed}^{{commit}}")])
        .output()?
        .status
        .success();
    if !known {
        return Ok(Relation::Unknown);
    }
    let ancestor = |a: &str, b: &str| -> Result<bool> {
        Ok(git(src)
            .args(["merge-base", "--is-ancestor", a, b])
            .output()?
            .status
            .success())
    };
    Ok(if ancestor(installed, remote)? {
        Relation::Behind
    } else if ancestor(remote, installed)? {
        Relation::Ahead
    } else {
        Relation::Diverged
    })
}

/// `[workspace.package] version` (ou `[package] version`) de um Cargo.toml.
pub fn workspace_version(manifest: &str) -> Option<String> {
    let v: toml::Value = toml::from_str(manifest).ok()?;
    v.get("workspace")
        .and_then(|w| w.get("package"))
        .or_else(|| v.get("package"))
        .and_then(|p| p.get("version"))
        .and_then(|v| v.as_str())
        .map(String::from)
}

/// `git` (ou `CTOWER_GIT`) em `dir`, sem pedir credencial no terminal.
pub fn git(dir: &Path) -> Command {
    let bin = std::env::var("CTOWER_GIT").unwrap_or_else(|_| "git".into());
    let mut c = Command::new(bin);
    c.current_dir(dir).env("GIT_TERMINAL_PROMPT", "0");
    c
}

/// Roda e devolve o stdout aparado; erro com o stderr.
pub fn run(c: &mut Command) -> Result<String> {
    let out = c.output()?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(Error(format!(
            "{} falhou ({}): {err}",
            c.get_program().to_string_lossy(),
            out.status
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sh(dir: &Path, args: &[&str]) -> String {
        run(git(dir).args(args)).unwrap()
    }

    fn commit(dir: &Path, version: &str, msg: &str) -> String {
        std::fs::write(
            dir.join("Cargo.toml"),
            format!("[workspace]\nmembers = []\n\n[workspace.package]\nversion = \"{version}\"\n"),
        )
        .unwrap();
        sh(dir, &["add", "."]);
        sh(dir, &["commit", "-q", "--allow-empty", "-m", msg]);
        sh(dir, &["rev-parse", "HEAD"])
    }

    /// Repositório "origem" com `main` e um `Source` apontando para ele.
    fn setup() -> (tempfile::TempDir, PathBuf, Source) {
        let tmp = tempfile::tempdir().unwrap();
        let origin = tmp.path().join("origin");
        std::fs::create_dir(&origin).unwrap();
        sh(&origin, &["init", "-q", "-b", "main"]);
        sh(&origin, &["config", "user.email", "t@t"]);
        sh(&origin, &["config", "user.name", "t"]);
        let src = Source {
            repo: origin.display().to_string(),
            git_ref: "main".into(),
            dir: tmp.path().join("data/update"),
        };
        (tmp, origin, src)
    }

    fn build(commit: &str) -> Build {
        Build {
            version: "0.1.0".into(),
            commit: commit.into(),
        }
    }

    #[test]
    fn estado_ida_e_volta_e_tail() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().join("update");
        assert_eq!(read_state(&dir), None);
        let st = UpdateState {
            kind: "update".into(),
            phase: Phase::Build,
            git_ref: "main".into(),
            from: build("abc"),
            to: None,
            started_at: 1,
            updated_at: 2,
            finished_at: None,
            message: None,
            error: None,
            log_tail: vec![],
        };
        write_state(&dir, &st).unwrap();
        assert_eq!(read_state(&dir), Some(st.clone()));
        let raw = std::fs::read_to_string(state_file(&dir)).unwrap();
        assert!(raw.contains("\"ref\": \"main\""), "{raw}");
        assert!(raw.contains("\"phase\": \"build\""), "{raw}");
        assert!(!st.phase.is_terminal() && Phase::RolledBack.is_terminal());

        std::fs::write(log_file(&dir), "a\nb\nc\n").unwrap();
        assert_eq!(tail(&log_file(&dir), 2), vec!["b", "c"]);
        assert_eq!(tail(&log_file(&dir), 9).len(), 3);
        assert!(tail(&dir.join("x"), 3).is_empty());
    }

    #[test]
    fn versao_do_manifesto() {
        assert_eq!(
            workspace_version("[workspace.package]\nversion = \"1.2.3\"\n").as_deref(),
            Some("1.2.3")
        );
        assert_eq!(
            workspace_version("[package]\nname = \"x\"\nversion = \"0.9.0\"\n").as_deref(),
            Some("0.9.0")
        );
        assert_eq!(workspace_version("[workspace]\n"), None);
    }

    #[test]
    fn atras_mesmo_e_a_frente() {
        let (_tmp, origin, src) = setup();
        let c1 = commit(&origin, "0.1.0", "primeiro");
        let c2 = commit(&origin, "0.2.0", "segundo");
        let c3 = commit(&origin, "0.2.0", "terceiro");

        let a = src.check(&build(&c1), 7).unwrap();
        assert_eq!(a.relation, Relation::Behind);
        assert!(a.update_available);
        assert_eq!(
            a.build,
            Build {
                version: "0.2.0".into(),
                commit: c3.clone()
            }
        );
        assert_eq!(a.behind, Some(2));
        assert_eq!(a.commits.len(), 2);
        assert!(a.commits[0].ends_with(" terceiro"));
        assert_eq!(a.checked_at, 7);

        let a = src.check(&build(&c3), 0).unwrap();
        assert_eq!((a.relation, a.update_available), (Relation::Same, false));

        // Ref volta para trás (ex.: ref trocado para uma tag antiga).
        sh(&origin, &["tag", "v0.2.0", &c2]);
        let tag = Source {
            git_ref: "v0.2.0".into(),
            ..src.clone()
        };
        let a = tag.check(&build(&c3), 0).unwrap();
        assert_eq!((a.relation, a.update_available), (Relation::Ahead, false));
        assert_eq!(a.build.commit, c2);
    }

    #[test]
    fn desconhecido_e_divergente() {
        let (_tmp, origin, src) = setup();
        let base = commit(&origin, "0.1.0", "base");
        sh(&origin, &["checkout", "-qb", "develop"]);
        let dev = commit(&origin, "0.2.0", "dev");
        sh(&origin, &["checkout", "-q", "main"]);
        commit(&origin, "0.1.1", "hotfix");

        let a = src.check(&build(""), 0).unwrap();
        assert_eq!((a.relation, a.update_available), (Relation::Unknown, true));
        let a = src.check(&build(&"f".repeat(40)), 0).unwrap();
        assert_eq!(a.relation, Relation::Unknown);

        // `develop` só chega ao clone quando é o ref seguido.
        let a = Source {
            git_ref: "develop".into(),
            ..src.clone()
        }
        .check(&build(&base), 0)
        .unwrap();
        assert_eq!(a.relation, Relation::Behind);
        let a = src.check(&build(&dev), 0).unwrap();
        assert_eq!((a.relation, a.update_available), (Relation::Diverged, true));
        assert!(a.commits.is_empty());
    }

    #[test]
    fn ref_inexistente_e_config_vazia() {
        let (_tmp, origin, src) = setup();
        commit(&origin, "0.1.0", "base");
        let e = Source {
            git_ref: "nao-existe".into(),
            ..src.clone()
        }
        .check(&build(""), 0)
        .unwrap_err();
        assert!(e.to_string().contains("falhou"), "{e}");
        assert!(Source {
            repo: " ".into(),
            ..src
        }
        .fetch()
        .is_err());
    }
}
