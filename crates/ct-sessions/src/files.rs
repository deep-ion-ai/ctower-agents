//! Leitura dos arquivos `.md` da sessão para o visualizador da UI (#167):
//! `file.read`, `files.shared`, `docs.list` e o `share_file` do MCP.
//!
//! Só lê dentro das pastas permitidas: a pasta da sessão (`cwd`: a worktree
//! da issue ou o clone do planejamento), a worktree do work item e a pasta
//! de docs do projeto (`CT_DOCS_DIR`, que contém as dos épicos). No acesso
//! pelo projeto (lista da pasta de docs), só a pasta de docs. Recusa, com a
//! mensagem do motivo:
//!
//! - caminho com `..` (mesmo que voltasse para dentro);
//! - caminho que, com os symlinks resolvidos, sai das pastas permitidas;
//! - extensão diferente de `.md` (no nome final, depois dos symlinks);
//! - o que não é arquivo regular, ou acima de [`MD_MAX_BYTES`].
//!
//! O conteúdo volta como texto (bytes inválidos em UTF-8 viram `�`).

use crate::{Sessions, SessionsError};
use ct_core::ProjectKind;
use serde::Serialize;
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::time::UNIX_EPOCH;

/// Maior `.md` que o visualizador lê (1 MiB).
pub const MD_MAX_BYTES: u64 = 1024 * 1024;
/// No máximo isto de arquivos na lista da pasta de docs.
pub const DOCS_LIST_MAX: usize = 500;
/// Profundidade máxima das subpastas na lista da pasta de docs.
pub const DOCS_LIST_DEPTH: usize = 8;

/// Resposta de `file.read`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct MdFile {
    /// Caminho absoluto e canônico no ambiente.
    pub path: String,
    /// A pasta permitida onde o arquivo está.
    pub root: String,
    pub content: String,
    /// Tamanho em bytes.
    pub size: u64,
    /// Última modificação (ms); `None` se o sistema não informa.
    pub modified_at: Option<i64>,
}

/// Um `.md` da lista da pasta de docs (`docs.list`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct DocsFile {
    /// Caminho absoluto no ambiente (o que o `file.read` recebe).
    pub path: String,
    /// Relativo à pasta listada, com `/`.
    pub rel: String,
    pub size: u64,
    pub modified_at: Option<i64>,
}

/// Resposta de `docs.list`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct DocsFiles {
    /// A pasta listada (a do projeto ou a do épico).
    pub root: String,
    /// Em ordem de caminho.
    pub files: Vec<DocsFile>,
    /// Havia mais que [`DOCS_LIST_MAX`]: a lista foi cortada.
    pub truncated: bool,
}

/// De onde vêm as pastas permitidas de `file.read`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileScope {
    /// Sessão: `cwd`, worktree do work item e pasta de docs do projeto.
    Session(String),
    /// Projeto: só a pasta de docs.
    Project(i64),
}

fn invalid(msg: impl Into<String>) -> SessionsError {
    SessionsError::Invalid(msg.into())
}

/// Extensão `.md` (sem diferenciar maiúsculas).
pub fn is_md(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("md"))
}

fn modified_ms(meta: &fs::Metadata) -> Option<i64> {
    let d = meta.modified().ok()?.duration_since(UNIX_EPOCH).ok()?;
    i64::try_from(d.as_millis()).ok()
}

/// Valida `path` contra `roots` e devolve o caminho canônico e a pasta
/// (canônica) onde ele está. Relativo = relativo à primeira pasta.
pub fn resolve_md(roots: &[PathBuf], path: &str) -> crate::Result<(PathBuf, PathBuf)> {
    let path = path.trim();
    if path.is_empty() {
        return Err(invalid("caminho vazio"));
    }
    let p = Path::new(path);
    if p.components().any(|c| c == Component::ParentDir) {
        return Err(invalid(format!("caminho com '..' recusado: {path}")));
    }
    if !is_md(p) {
        return Err(invalid(format!(
            "só arquivos .md podem ser abertos no visualizador: {path}"
        )));
    }
    // Pastas que não existem (worktree apagada, docs fora do ar) ficam de fora.
    let canon_roots: Vec<PathBuf> = roots.iter().filter_map(|r| r.canonicalize().ok()).collect();
    if canon_roots.is_empty() {
        return Err(invalid(
            "nenhuma pasta permitida acessível neste ambiente (worktree ou pasta de docs)",
        ));
    }
    let full = if p.is_absolute() {
        p.to_path_buf()
    } else {
        roots
            .iter()
            .find(|r| r.is_dir())
            .map(|r| r.join(p))
            .ok_or_else(|| invalid("caminho relativo sem pasta da sessão acessível"))?
    };
    let canon = full.canonicalize().map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => invalid(format!("arquivo não encontrado: {path}")),
        _ => invalid(format!("não foi possível acessar {path}: {e}")),
    })?;
    let Some(root) = canon_roots.iter().find(|r| canon.starts_with(r)) else {
        return Err(invalid(format!(
            "{path} está fora das pastas da sessão (worktree e pasta de docs)"
        )));
    };
    // Um symlink `a.md` pode apontar para um arquivo de outra extensão.
    if !is_md(&canon) {
        return Err(invalid(format!(
            "{path} aponta para um arquivo que não é .md"
        )));
    }
    Ok((canon.clone(), root.clone()))
}

/// [`resolve_md`] e lê o arquivo, até [`MD_MAX_BYTES`].
pub fn read_md(roots: &[PathBuf], path: &str) -> crate::Result<MdFile> {
    read_md_limit(roots, path, MD_MAX_BYTES)
}

fn read_md_limit(roots: &[PathBuf], path: &str, max: u64) -> crate::Result<MdFile> {
    let (canon, root) = resolve_md(roots, path)?;
    let meta = fs::metadata(&canon)?;
    if !meta.is_file() {
        return Err(invalid(format!("{path} não é um arquivo")));
    }
    let too_big = || {
        invalid(format!(
            "{path} tem mais de {} KiB: grande demais para o visualizador",
            max / 1024
        ))
    };
    if meta.len() > max {
        return Err(too_big());
    }
    // O arquivo pode crescer entre o `metadata` e a leitura.
    let mut buf = Vec::with_capacity(meta.len() as usize);
    fs::File::open(&canon)?
        .take(max + 1)
        .read_to_end(&mut buf)?;
    if buf.len() as u64 > max {
        return Err(too_big());
    }
    Ok(MdFile {
        path: canon.display().to_string(),
        root: root.display().to_string(),
        size: buf.len() as u64,
        content: String::from_utf8_lossy(&buf).into_owned(),
        modified_at: modified_ms(&meta),
    })
}

/// Os `.md` sob `dir` (sem seguir symlinks nem entrar em pastas ocultas),
/// em ordem de caminho, até [`DOCS_LIST_MAX`].
pub fn list_md(dir: &Path) -> crate::Result<DocsFiles> {
    let root = dir.canonicalize()?;
    let mut files = Vec::new();
    let mut truncated = false;
    walk(&root, &root, 0, &mut files, &mut truncated)?;
    files.sort_by(|a: &DocsFile, b| a.rel.cmp(&b.rel));
    Ok(DocsFiles {
        root: root.display().to_string(),
        files,
        truncated,
    })
}

fn walk(
    root: &Path,
    dir: &Path,
    depth: usize,
    out: &mut Vec<DocsFile>,
    truncated: &mut bool,
) -> crate::Result<()> {
    let mut entries: Vec<_> = fs::read_dir(dir)?.filter_map(|e| e.ok()).collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        if e.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        let Ok(ft) = e.file_type() else { continue };
        let path = e.path();
        if ft.is_dir() {
            if depth + 1 < DOCS_LIST_DEPTH {
                walk(root, &path, depth + 1, out, truncated)?;
            }
        } else if ft.is_file() && is_md(&path) {
            if out.len() >= DOCS_LIST_MAX {
                *truncated = true;
                return Ok(());
            }
            let meta = e.metadata().ok();
            let rel = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .components()
                .map(|c| c.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/");
            out.push(DocsFile {
                path: path.display().to_string(),
                rel,
                size: meta.as_ref().map_or(0, |m| m.len()),
                modified_at: meta.as_ref().and_then(modified_ms),
            });
        }
    }
    Ok(())
}

impl Sessions {
    /// Pasta de docs do projeto, se ligada e acessível (sem criar nada).
    fn project_docs_root(&self, project: &ct_ledger::Project) -> Option<PathBuf> {
        let root = self.config().docs_root.as_deref()?;
        if project.kind == ProjectKind::Gestor || !project.docs.enabled {
            return None;
        }
        if !crate::docs::reachable(root) {
            return None;
        }
        Some(crate::docs::project_dir(
            root,
            &project.gh_owner,
            &project.gh_repo,
        ))
    }

    /// Pastas permitidas do `scope`, a primeira é a base dos relativos.
    pub fn file_roots(&self, scope: &FileScope) -> crate::Result<Vec<PathBuf>> {
        let mut roots = Vec::new();
        let project = match scope {
            FileScope::Session(id) => {
                let l = self.ledger();
                let s = l.get_session(id)?;
                let wi = l.get_work_item(s.work_item_id)?;
                roots.push(PathBuf::from(&s.cwd));
                if let Some(w) = wi.worktree_path.filter(|w| *w != s.cwd) {
                    roots.push(PathBuf::from(w));
                }
                l.get_project(wi.project_id)?
            }
            FileScope::Project(id) => self.ledger().get_project(*id)?,
        };
        if let Some(d) = self.project_docs_root(&project) {
            roots.push(d);
        }
        if roots.is_empty() {
            return Err(invalid(format!(
                "projeto {}/{} sem pasta de docs acessível neste ambiente",
                project.gh_owner, project.gh_repo
            )));
        }
        Ok(roots)
    }

    /// `file.read`: o `.md` em `path`, dentro das pastas do `scope`.
    pub fn read_file(&self, scope: &FileScope, path: &str) -> crate::Result<MdFile> {
        read_md(&self.file_roots(scope)?, path)
    }

    /// `share_file` do MCP: valida como o `file.read` (o arquivo tem que
    /// existir), registra na sessão e avisa a UI (`session_files`).
    pub fn share_file(
        &self,
        session_id: &str,
        path: &str,
        title: Option<&str>,
    ) -> crate::Result<ct_ledger::SharedFile> {
        let roots = self.file_roots(&FileScope::Session(session_id.to_string()))?;
        let (canon, _) = resolve_md(&roots, path)?;
        let meta = fs::metadata(&canon)?;
        if !meta.is_file() {
            return Err(invalid(format!("{path} não é um arquivo")));
        }
        if meta.len() > MD_MAX_BYTES {
            return Err(invalid(format!(
                "{path} tem mais de {} KiB: grande demais para o visualizador",
                MD_MAX_BYTES / 1024
            )));
        }
        let (shared, work_item_id) = {
            let l = self.ledger();
            let f = l.share_file(session_id, &canon.display().to_string(), title)?;
            (f, l.get_session(session_id)?.work_item_id)
        };
        self.notify(
            "session_files",
            serde_json::json!({
                "session_id": session_id,
                "work_item_id": work_item_id,
                "file": shared,
            }),
        );
        Ok(shared)
    }

    /// `files.shared`: os arquivos compartilhados na sessão.
    pub fn shared_files(&self, session_id: &str) -> crate::Result<Vec<ct_ledger::SharedFile>> {
        let l = self.ledger();
        l.get_session(session_id)?;
        Ok(l.list_shared_files(session_id)?)
    }

    /// `docs.list`: os `.md` da pasta de docs do projeto ou do épico.
    pub fn docs_files(&self, project_id: i64, epic: Option<i64>) -> crate::Result<DocsFiles> {
        list_md(&self.docs_dir(project_id, epic)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn setup() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let wt = tmp.path().join("wt");
        let docs = tmp.path().join("docs");
        fs::create_dir_all(wt.join("specs")).unwrap();
        fs::create_dir_all(&docs).unwrap();
        fs::write(wt.join("specs/a.md"), "# A\n").unwrap();
        fs::write(docs.join("b.MD"), "# B\n").unwrap();
        (tmp, wt, docs)
    }

    fn err(r: crate::Result<MdFile>) -> String {
        r.unwrap_err().to_string()
    }

    #[test]
    fn le_absoluto_e_relativo_dentro_das_pastas() {
        let (_tmp, wt, docs) = setup();
        let roots = vec![wt.clone(), docs.clone()];
        let a = read_md(&roots, "specs/a.md").unwrap();
        assert_eq!(a.content, "# A\n");
        assert_eq!(a.size, 4);
        assert_eq!(PathBuf::from(&a.root), wt.canonicalize().unwrap());
        let b = read_md(&roots, docs.join("b.MD").to_str().unwrap()).unwrap();
        assert_eq!(PathBuf::from(&b.root), docs.canonicalize().unwrap());
        assert!(b.modified_at.is_some());
    }

    #[test]
    fn recusa_ponto_ponto_mesmo_voltando_para_dentro() {
        let (_tmp, wt, docs) = setup();
        let roots = vec![wt.clone(), docs];
        assert!(err(read_md(&roots, "specs/../specs/a.md")).contains("'..'"));
        let fora = wt.join("../docs/b.MD");
        assert!(err(read_md(&roots, fora.to_str().unwrap())).contains("'..'"));
    }

    #[test]
    fn recusa_fora_das_pastas_e_symlink_que_escapa() {
        let (tmp, wt, docs) = setup();
        let roots = vec![wt.clone(), docs];
        let fora = tmp.path().join("segredo.md");
        fs::write(&fora, "x").unwrap();
        assert!(err(read_md(&roots, fora.to_str().unwrap())).contains("fora das pastas"));
        symlink(&fora, wt.join("link.md")).unwrap();
        assert!(err(read_md(&roots, "link.md")).contains("fora das pastas"));
        // Pasta inteira por symlink também escapa.
        let dir = tmp.path().join("outra");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("c.md"), "c").unwrap();
        symlink(&dir, wt.join("atalho")).unwrap();
        assert!(err(read_md(&roots, "atalho/c.md")).contains("fora das pastas"));
    }

    #[test]
    fn symlink_dentro_das_pastas_vale_mas_nao_troca_a_extensao() {
        let (_tmp, wt, docs) = setup();
        let roots = vec![wt.clone(), docs];
        symlink(wt.join("specs/a.md"), wt.join("alias.md")).unwrap();
        assert_eq!(read_md(&roots, "alias.md").unwrap().content, "# A\n");
        fs::write(wt.join(".env"), "TOKEN=x").unwrap();
        symlink(wt.join(".env"), wt.join("env.md")).unwrap();
        assert!(err(read_md(&roots, "env.md")).contains("não é .md"));
    }

    #[test]
    fn recusa_outra_extensao_pasta_inexistente_e_grande_demais() {
        let (_tmp, wt, docs) = setup();
        let roots = vec![wt.clone(), docs];
        fs::write(wt.join("x.txt"), "x").unwrap();
        assert!(err(read_md(&roots, "x.txt")).contains("só arquivos .md"));
        assert!(err(read_md(&roots, "nao-existe.md")).contains("não encontrado"));
        fs::create_dir_all(wt.join("pasta.md")).unwrap();
        assert!(err(read_md(&roots, "pasta.md")).contains("não é um arquivo"));
        fs::write(wt.join("grande.md"), vec![b'a'; 2048]).unwrap();
        let e = read_md_limit(&roots, "grande.md", 1024)
            .unwrap_err()
            .to_string();
        assert!(e.contains("grande demais"), "{e}");
        assert!(read_md_limit(&roots, "grande.md", 2048).is_ok());
        assert!(
            err(read_md(&[PathBuf::from("/nao/existe")], "/nao/existe/a.md"))
                .contains("nenhuma pasta permitida")
        );
    }

    #[test]
    fn lista_os_md_sem_ocultos_nem_symlinks() {
        let (tmp, _wt, docs) = setup();
        fs::create_dir_all(docs.join("_projeto/analises")).unwrap();
        fs::create_dir_all(docs.join(".git")).unwrap();
        fs::write(docs.join("_projeto/analises/x.md"), "x").unwrap();
        fs::write(docs.join("_projeto/notas.txt"), "x").unwrap();
        fs::write(docs.join(".git/y.md"), "y").unwrap();
        fs::write(tmp.path().join("fora.md"), "f").unwrap();
        symlink(tmp.path().join("fora.md"), docs.join("link.md")).unwrap();
        let l = list_md(&docs).unwrap();
        let rels: Vec<_> = l.files.iter().map(|f| f.rel.as_str()).collect();
        assert_eq!(rels, ["_projeto/analises/x.md", "b.MD"]);
        assert!(!l.truncated);
    }

    /// Pela sessão: worktree e pasta de docs do projeto valem, o resto
    /// não; `share_file` registra o caminho canônico e avisa a UI. Pelo
    /// projeto, só a pasta de docs.
    #[test]
    fn sessao_le_worktree_e_docs_e_compartilha() {
        use crate::test_support::{FakeClaude, TestEnv};
        let env = TestEnv::new(FakeClaude::exit(0));
        env.assign_config(1, |p| {
            p.docs = ct_core::DocsConfig {
                enabled: true,
                project_dirs: vec!["analises".into()],
                epic_dirs: vec![],
            };
        });
        env.docs_root_up();
        let s = env.sessions.start(env.request(42, "Visualizador")).unwrap();
        let cwd = PathBuf::from(&s.cwd);
        fs::write(cwd.join("plano.md"), "# Plano").unwrap();
        let docs = env
            .tmp
            .path()
            .join("docs/deep-ion-ai__demo/_projeto/analises");
        fs::write(docs.join("analise.md"), "# Análise").unwrap();

        let scope = FileScope::Session(s.id.clone());
        assert_eq!(
            env.sessions.read_file(&scope, "plano.md").unwrap().content,
            "# Plano"
        );
        let d = env
            .sessions
            .read_file(&scope, docs.join("analise.md").to_str().unwrap());
        assert_eq!(d.unwrap().content, "# Análise");
        let repo_md = env.repo.join("README.md");
        fs::write(&repo_md, "x").unwrap();
        let e = env.sessions.read_file(&scope, repo_md.to_str().unwrap());
        assert!(e.unwrap_err().to_string().contains("fora das pastas"));

        let f = env
            .sessions
            .share_file(&s.id, "plano.md", Some("Plano"))
            .unwrap();
        assert_eq!(
            PathBuf::from(&f.path),
            cwd.join("plano.md").canonicalize().unwrap()
        );
        assert!(env.sessions.share_file(&s.id, "nada.md", None).is_err());
        assert!(env.sessions.share_file(&s.id, "Cargo.toml", None).is_err());
        assert_eq!(env.sessions.shared_files(&s.id).unwrap(), vec![f.clone()]);
        env.wait_notice(|n, d| n == "session_files" && d["file"]["path"] == f.path.as_str());

        let proj = FileScope::Project(env.project_id);
        assert!(env
            .sessions
            .read_file(&proj, docs.join("analise.md").to_str().unwrap())
            .is_ok());
        assert!(env
            .sessions
            .read_file(&proj, cwd.join("plano.md").to_str().unwrap())
            .is_err());
        let list = env.sessions.docs_files(env.project_id, None).unwrap();
        assert_eq!(list.files.len(), 1);
        assert_eq!(list.files[0].rel, "_projeto/analises/analise.md");
    }
}
