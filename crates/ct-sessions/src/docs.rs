//! Pasta de docs do projeto e do épico fora do repositório (épico #445,
//! issue #440). Uma cópia só, na máquina do `cthubd`; os outros ambientes a
//! acessam por um compartilhamento montado em `docs_root`. O CT não monta
//! nada: `docs_root` vazio = sem docs neste ambiente, e `docs_root`
//! inacessível (montagem caída) = não cria nada e registra aviso.
//!
//! A estrutura nunca sobrescreve o que existe: só cria pastas que faltam e
//! o `README.md` do épico com `create_new`. A pasta de um épico cujo título
//! mudou é achada por `<n>-*` e reaproveitada, sem renomear. A limpeza
//! (`ctower clean`, épico #284) não conhece `docs_root`.
//!
//! Sessões (#441): a de issue, épico ou planejamento de um projeto com docs
//! recebe `CT_DOCS_DIR`, `CT_EPIC_DOCS_DIR` (se há épico), `--add-dir` e
//! uma linha nas instruções; com `docs_root` inacessível, nada disso e o
//! aviso `docs_unreachable`. O teste de acesso tem prazo
//! ([`REACHABLE_TIMEOUT`]): a montagem Samba/CIFS caída pode travar o
//! `stat` por minutos.

use crate::{Sessions, SessionsError};
use ct_core::docs::{
    epic_docs_dir_name, epic_readme, is_epic_docs_dir, project_docs_dir_name, EpicDocs,
    EpicSubIssue, DOCS_ROOT_MARKER, EPICS_DOCS_DIR, EPIC_README, PROJECT_DOCS_DIR,
};
use ct_core::ProjectKind;
use ct_ledger::{Ledger, Project, WorkItem};
use serde::Serialize;
use std::collections::BTreeSet;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Mutex};
use std::time::Duration;

/// Aviso de `docs_root` inacessível: `env.info.docs.warning`, alerta e
/// evento da sessão que subiu sem a pasta de docs.
pub const DOCS_UNREACHABLE: &str = "docs_unreachable";

/// Prazo do teste de acesso a `docs_root`; passou = inacessível.
pub const REACHABLE_TIMEOUT: Duration = Duration::from_secs(2);

/// Pasta do projeto na sessão (`<docs_root>/<owner>__<repo>`).
pub const ENV_DOCS_DIR: &str = "CT_DOCS_DIR";
/// Pasta do épico da sessão (o próprio ou o pai da sub-issue).
pub const ENV_EPIC_DOCS_DIR: &str = "CT_EPIC_DOCS_DIR";

/// `docs` do `env.info`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct DocsStatus {
    /// `docs_root` do `config.toml` (com `~` expandido); `None` = sem docs
    /// neste ambiente.
    pub root: Option<String>,
    /// A pasta existe e é legível agora.
    pub reachable: bool,
    /// `docs_unreachable` com `docs_root` configurado e inacessível (#441):
    /// as sessões novas sobem sem a pasta de docs. Ausente = sem aviso.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub warning: Option<String>,
}

/// Resposta de `docs.dir` (#444): a pasta que a UI abre no gerenciador de
/// arquivos.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct DocsDir {
    /// Caminho absoluto no ambiente (`<docs_root>/<owner>__<repo>` ou a
    /// pasta do épico).
    pub path: String,
}

/// Estado de `docs_root`.
pub fn status(root: Option<&Path>) -> DocsStatus {
    let reachable = root.is_some_and(reachable);
    DocsStatus {
        root: root.map(|r| r.display().to_string()),
        reachable,
        warning: (root.is_some() && !reachable).then(|| DOCS_UNREACHABLE.to_string()),
    }
}

/// `docs_root` é pasta e tem o marcador [`DOCS_ROOT_MARKER`] (criado pelo
/// `cthubd` na pasta real), em até [`REACHABLE_TIMEOUT`]. O CT nunca cria a
/// raiz nem o marcador: com a montagem caída, o ponto de montagem continua
/// ali como pasta local vazia, e sem o marcador não é aceito.
pub fn reachable(root: &Path) -> bool {
    reachable_within(root, REACHABLE_TIMEOUT, probe)
}

fn probe(root: &Path) -> bool {
    root.is_dir() && root.join(DOCS_ROOT_MARKER).is_file()
}

/// Raízes com teste de acesso em andamento: enquanto a montagem não
/// responde, os pedidos seguintes respondem "inacessível" na hora, sem
/// empilhar threads presas no mesmo `stat`.
static PROBING: Mutex<BTreeSet<PathBuf>> = Mutex::new(BTreeSet::new());

fn probing() -> std::sync::MutexGuard<'static, BTreeSet<PathBuf>> {
    PROBING.lock().unwrap_or_else(|e| e.into_inner())
}

/// `probe(root)` numa thread à parte, esperando no máximo `timeout`; prazo
/// estourado ou teste anterior ainda preso = inacessível.
fn reachable_within(
    root: &Path,
    timeout: Duration,
    probe: impl FnOnce(&Path) -> bool + Send + 'static,
) -> bool {
    let path = root.to_path_buf();
    if !probing().insert(path.clone()) {
        return false;
    }
    let (tx, rx) = mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("ct-docs-probe".into())
        .spawn({
            let path = path.clone();
            move || {
                let ok = probe(&path);
                probing().remove(&path);
                let _ = tx.send(ok);
            }
        });
    if spawned.is_err() {
        probing().remove(&path);
        return false;
    }
    rx.recv_timeout(timeout).unwrap_or(false)
}

/// Pasta do projeto: `<docs_root>/<owner>__<repo>`.
pub fn project_dir(root: &Path, owner: &str, repo: &str) -> PathBuf {
    root.join(project_docs_dir_name(owner, repo))
}

/// Cria `<projeto>/_projeto/<project_dirs>` (o que faltar). Devolve a pasta
/// do projeto.
pub fn ensure_project(root: &Path, p: &Project) -> io::Result<PathBuf> {
    let dir = project_dir(root, &p.gh_owner, &p.gh_repo);
    let base = dir.join(PROJECT_DOCS_DIR);
    std::fs::create_dir_all(&base)?;
    for d in &p.docs.project_dirs {
        std::fs::create_dir_all(base.join(d))?;
    }
    Ok(dir)
}

/// Pasta já existente do épico `n` em `epicos/` (`<n>` ou `<n>-*`, a
/// primeira em ordem alfabética).
pub fn find_epic_dir(epics: &Path, number: i64) -> io::Result<Option<PathBuf>> {
    let rd = match std::fs::read_dir(epics) {
        Ok(rd) => rd,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let mut found: Vec<PathBuf> = Vec::new();
    for e in rd {
        let e = e?;
        let name = e.file_name();
        if e.file_type()?.is_dir() && is_epic_docs_dir(&name.to_string_lossy(), number) {
            found.push(e.path());
        }
    }
    found.sort();
    Ok(found.into_iter().next())
}

/// Cria (ou reaproveita) `epicos/<n>-<slug>/` com o `README.md`, gerado só
/// se não existe, e as `epic_dirs`. Devolve a pasta do épico.
pub fn ensure_epic(root: &Path, p: &Project, epic: &EpicDocs) -> io::Result<PathBuf> {
    let epics = project_dir(root, &p.gh_owner, &p.gh_repo).join(EPICS_DOCS_DIR);
    let dir = match find_epic_dir(&epics, epic.number)? {
        Some(d) => d,
        None => epics.join(epic_docs_dir_name(epic.number, &epic.title)),
    };
    std::fs::create_dir_all(&dir)?;
    let readme = dir.join(EPIC_README);
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&readme)
    {
        Ok(mut f) => {
            use std::io::Write;
            f.write_all(epic_readme(&p.tracker_ref(), epic).as_bytes())?;
        }
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e),
    }
    for d in &p.docs.epic_dirs {
        std::fs::create_dir_all(dir.join(d))?;
    }
    Ok(dir)
}

/// Título, link e sub-issues do épico `n`, do espelho; `fallback` = o
/// título quando o épico ainda não está no espelho.
fn epic_docs(l: &Ledger, project_id: i64, n: i64, fallback: &str) -> crate::Result<EpicDocs> {
    let issue = l.get_issue(project_id, n)?;
    let mut sub_issues: Vec<EpicSubIssue> = l
        .list_issues(project_id)?
        .into_iter()
        .filter(|i| i.parent_number == Some(n))
        .map(|i| EpicSubIssue {
            number: i.gh_number,
            title: i.title,
        })
        .collect();
    sub_issues.sort_by_key(|s| s.number);
    let (title, url) = match issue {
        Some(i) => (i.title, i.url),
        None => (fallback.to_string(), String::new()),
    };
    Ok(EpicDocs {
        number: n,
        title,
        url,
        sub_issues,
    })
}

/// Onde ficaram as pastas de docs de uma sessão.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionDocs {
    /// `<docs_root>/<owner>__<repo>`.
    pub project: Option<PathBuf>,
    /// `epicos/<n>-<slug>`, se o work item é épico ou sub-issue de um.
    pub epic: Option<PathBuf>,
    /// Projeto com docs, mas `docs_root` inacessível: a sessão sobe sem as
    /// pastas e com o aviso `docs_unreachable`.
    pub unreachable: bool,
}

impl SessionDocs {
    /// `CT_DOCS_DIR` e, com épico, `CT_EPIC_DOCS_DIR`.
    pub fn env(&self) -> Vec<(String, String)> {
        let Some(project) = &self.project else {
            return Vec::new();
        };
        let mut env = vec![(ENV_DOCS_DIR.to_string(), project.display().to_string())];
        if let Some(epic) = &self.epic {
            env.push((ENV_EPIC_DOCS_DIR.to_string(), epic.display().to_string()));
        }
        env
    }

    /// Linha das instruções da sessão: onde gravar os artefatos.
    pub fn instruction(&self) -> Option<String> {
        let project = self.project.as_ref()?;
        let (var, dir) = match &self.epic {
            Some(e) => (format!("${ENV_EPIC_DOCS_DIR}"), e.clone()),
            None => (
                format!("${ENV_DOCS_DIR}/{PROJECT_DOCS_DIR}"),
                project.join(PROJECT_DOCS_DIR),
            ),
        };
        Some(format!(
            "Docs do projeto (Control Tower): grave artefatos de análise, especificação, prompts \
             e evidências em {var} ({}), nunca no repositório.",
            dir.display()
        ))
    }
}

impl Sessions {
    /// Prepara as pastas de docs do projeto e, se o work item é um épico ou
    /// sub-issue de um, as do épico (o pai). Docs desligados no projeto ou
    /// sem `docs_root` = nada; `docs_root` inacessível ou erro de disco =
    /// aviso no log, e a sessão sobe assim mesmo. `title` = o título do
    /// pedido, para o épico ainda fora do espelho.
    pub fn ensure_docs(
        &self,
        project: &Project,
        wi: Option<&WorkItem>,
        title: &str,
    ) -> SessionDocs {
        match self.try_ensure_docs(project, wi, title) {
            Ok(d) => d,
            Err(e) => {
                tracing::warn!(
                    "docs: pastas de {}/{} não criadas: {e}",
                    project.gh_owner,
                    project.gh_repo
                );
                SessionDocs::default()
            }
        }
    }

    fn try_ensure_docs(
        &self,
        project: &Project,
        wi: Option<&WorkItem>,
        title: &str,
    ) -> crate::Result<SessionDocs> {
        let Some(root) = self.config().docs_root.as_deref() else {
            return Ok(SessionDocs::default());
        };
        if project.kind == ProjectKind::Gestor || !project.docs.enabled {
            return Ok(SessionDocs::default());
        }
        if !reachable(root) {
            tracing::warn!(
                "docs: {DOCS_UNREACHABLE}: docs_root {} inacessível (montagem caída?): nada \
                 criado para {}/{}",
                root.display(),
                project.gh_owner,
                project.gh_repo
            );
            return Ok(SessionDocs {
                unreachable: true,
                ..Default::default()
            });
        }
        let mut out = SessionDocs {
            project: Some(ensure_project(root, project)?),
            epic: None,
            unreachable: false,
        };
        let Some(wi) = wi else { return Ok(out) };
        let epic = {
            let l = self.ledger();
            match l.rules_epic(wi)? {
                Some(n) => {
                    let fallback = if n == wi.gh_number { title } else { "" };
                    Some(epic_docs(&l, project.id, n, fallback)?)
                }
                None => None,
            }
        };
        if let Some(epic) = epic {
            out.epic = Some(ensure_epic(root, project, &epic)?);
        }
        Ok(out)
    }

    /// Pasta de docs do projeto `project_id` ou, com `epic`, a do épico
    /// (#444, "Abrir pasta de docs" da UI): cria o que faltar como a sessão
    /// faria (reaproveitando a `<n>-*` existente) e devolve o caminho.
    /// Recusa com docs desligados no projeto, sem `docs_root` ou com ele
    /// inacessível (sem criar nada no ponto de montagem vazio).
    pub fn docs_dir(&self, project_id: i64, epic: Option<i64>) -> crate::Result<PathBuf> {
        let Some(root) = self.config().docs_root.as_deref() else {
            return Err(SessionsError::Invalid(
                "docs_root não configurado neste ambiente".into(),
            ));
        };
        let project = self.ledger().get_project(project_id)?;
        if project.kind == ProjectKind::Gestor || !project.docs.enabled {
            return Err(SessionsError::Invalid(format!(
                "docs desligados no projeto {}/{}",
                project.gh_owner, project.gh_repo
            )));
        }
        if !reachable(root) {
            return Err(SessionsError::Invalid(format!(
                "{DOCS_UNREACHABLE}: docs_root {} inacessível (montagem caída?)",
                root.display()
            )));
        }
        let Some(n) = epic else {
            return Ok(ensure_project(root, &project)?);
        };
        let docs = {
            let l = self.ledger();
            if l.get_issue(project.id, n)?.is_none() {
                return Err(SessionsError::Invalid(format!(
                    "issue {n} não está no espelho do projeto"
                )));
            }
            epic_docs(&l, project.id, n, "")?
        };
        ensure_project(root, &project)?;
        Ok(ensure_epic(root, &project, &docs)?)
    }

    /// Só `_projeto/` (aplicação da atribuição pelo pull do hub).
    pub fn ensure_project_docs(&self, project: &Project) -> Option<PathBuf> {
        self.ensure_docs(project, None, "").project
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ct_core::DocsConfig;

    fn project(docs: DocsConfig) -> Project {
        Project {
            gh_owner: "deep-ion-ai".into(),
            gh_repo: "demo".into(),
            docs,
            ..Default::default()
        }
    }

    fn modelo() -> DocsConfig {
        DocsConfig {
            enabled: true,
            project_dirs: vec!["analises".into(), "specs/telas".into()],
            epic_dirs: vec!["prompts".into(), "evidencias".into()],
        }
    }

    fn epic(n: i64, title: &str) -> EpicDocs {
        EpicDocs {
            number: n,
            title: title.into(),
            url: String::new(),
            sub_issues: vec![EpicSubIssue {
                number: n + 1,
                title: "Sub".into(),
            }],
        }
    }

    #[test]
    fn cria_pelos_modelos_idempotente_sem_sobrescrever() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let p = project(modelo());
        let dir = ensure_project(root, &p).unwrap();
        assert_eq!(dir, root.join("deep-ion-ai__demo"));
        assert!(dir.join("_projeto/analises").is_dir());
        assert!(dir.join("_projeto/specs/telas").is_dir());
        let nota = dir.join("_projeto/analises/nota.md");
        std::fs::write(&nota, "minha").unwrap();

        let e = ensure_epic(root, &p, &epic(445, "Épico: docs fora do repositório")).unwrap();
        assert_eq!(e, dir.join("epicos/445-epico-docs-fora-do-repositorio"));
        assert!(e.join("prompts").is_dir() && e.join("evidencias").is_dir());
        let readme = std::fs::read_to_string(e.join("README.md")).unwrap();
        assert!(readme.contains("deep-ion-ai/demo#445"), "{readme}");
        assert!(readme.contains("#446"), "{readme}");
        std::fs::write(e.join("README.md"), "editado").unwrap();

        // De novo: nada muda, nada é sobrescrito.
        ensure_project(root, &p).unwrap();
        let e2 = ensure_epic(root, &p, &epic(445, "Épico: docs fora do repositório")).unwrap();
        assert_eq!(e2, e);
        assert_eq!(std::fs::read_to_string(&nota).unwrap(), "minha");
        assert_eq!(
            std::fs::read_to_string(e.join("README.md")).unwrap(),
            "editado"
        );
    }

    #[test]
    fn titulo_alterado_reaproveita_a_pasta_do_epico() {
        let tmp = tempfile::tempdir().unwrap();
        let p = project(modelo());
        let antiga = ensure_epic(tmp.path(), &p, &epic(12, "Título antigo")).unwrap();
        let nova = ensure_epic(tmp.path(), &p, &epic(12, "Título novo")).unwrap();
        assert_eq!(nova, antiga);
        assert!(antiga.ends_with("12-titulo-antigo"));
        let epicos = tmp.path().join("deep-ion-ai__demo/epicos");
        assert_eq!(std::fs::read_dir(&epicos).unwrap().count(), 1);
        // `120-*` não é do épico 12.
        std::fs::create_dir_all(epicos.join("120-outro")).unwrap();
        assert_eq!(find_epic_dir(&epicos, 12).unwrap(), Some(antiga));
        assert_eq!(find_epic_dir(&epicos, 1).unwrap(), None);
    }

    #[test]
    fn raiz_inexistente_nao_e_acessivel() {
        let tmp = tempfile::tempdir().unwrap();
        let fora = tmp.path().join("montagem");
        assert!(!reachable(&fora));
        assert_eq!(
            status(Some(&fora)),
            DocsStatus {
                root: Some(fora.display().to_string()),
                reachable: false,
                warning: Some(DOCS_UNREACHABLE.into()),
            }
        );
        assert_eq!(status(None), DocsStatus::default());

        // Ponto de montagem vazio (compartilhamento caído): pasta sem o
        // marcador não é a pasta de docs.
        let vazio = status(Some(tmp.path()));
        assert!(!vazio.reachable, "{vazio:?}");
        assert_eq!(vazio.warning.as_deref(), Some(DOCS_UNREACHABLE));

        std::fs::write(tmp.path().join(DOCS_ROOT_MARKER), "x").unwrap();
        let ok = status(Some(tmp.path()));
        assert!(ok.reachable && ok.warning.is_none(), "{ok:?}");
    }

    /// Montagem travada (#441): o teste desiste no prazo, e os seguintes
    /// respondem na hora, sem nova thread, até o preso voltar.
    #[test]
    fn montagem_travada_vira_inacessivel_no_prazo() {
        use std::time::Instant;
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        std::fs::write(root.join(DOCS_ROOT_MARKER), "x").unwrap();
        let (solta, preso) = mpsc::channel::<()>();
        let t0 = Instant::now();
        let travado = move |p: &Path| {
            let _ = preso.recv();
            probe(p)
        };
        assert!(!reachable_within(
            &root,
            Duration::from_millis(100),
            travado
        ));
        assert!(t0.elapsed() < Duration::from_secs(2), "{:?}", t0.elapsed());

        let t1 = Instant::now();
        assert!(!reachable_within(&root, Duration::from_secs(10), probe));
        assert!(t1.elapsed() < Duration::from_secs(1), "{:?}", t1.elapsed());

        solta.send(()).unwrap();
        let fim = Instant::now() + Duration::from_secs(10);
        while probing().contains(&root) {
            assert!(Instant::now() < fim, "teste preso não terminou");
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(reachable_within(&root, Duration::from_secs(10), probe));
    }

    #[test]
    fn env_e_instrucao_da_sessao() {
        assert!(SessionDocs::default().env().is_empty());
        assert_eq!(SessionDocs::default().instruction(), None);
        let proj = PathBuf::from("/docs/o__r");
        let so_projeto = SessionDocs {
            project: Some(proj.clone()),
            ..Default::default()
        };
        assert_eq!(
            so_projeto.env(),
            vec![("CT_DOCS_DIR".to_string(), "/docs/o__r".to_string())]
        );
        let i = so_projeto.instruction().unwrap();
        assert!(
            i.contains("$CT_DOCS_DIR/_projeto (/docs/o__r/_projeto)"),
            "{i}"
        );
        assert!(i.contains("nunca no repositório"), "{i}");
        let com_epico = SessionDocs {
            project: Some(proj.clone()),
            epic: Some(proj.join("epicos/7-x")),
            unreachable: false,
        };
        assert_eq!(com_epico.env()[1].0, "CT_EPIC_DOCS_DIR");
        let i = com_epico.instruction().unwrap();
        assert!(
            i.contains("$CT_EPIC_DOCS_DIR (/docs/o__r/epicos/7-x)"),
            "{i}"
        );
    }
}
