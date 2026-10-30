use crate::{forge_col, json_col, tracker_col, Ledger, LedgerError, LocationState, Result};
use ct_core::branch::default_branch_pattern;
use ct_core::{
    now_ms, ForgeConfig, ProjectKind, TrackerConfig, TrackerRef, GESTOR_OWNER, GESTOR_REPO,
};
use rusqlite::{params, OptionalExtension, Row};
use serde::Serialize;

pub struct NewProject {
    pub name: String,
    pub gh_owner: String,
    pub gh_repo: String,
    pub repo_path: String,
    pub base_branch: String,
    pub protected_branches: Vec<String>,
}

/// Campos a alterar num projeto local legado; `None` mantém o valor atual.
/// O ambiente não edita projetos (vêm do hub, #178): só os testes simulam
/// ambientes antigos que ainda editavam.
#[cfg(test)]
#[derive(Debug, Clone, Default)]
pub struct ProjectPatch {
    pub name: Option<String>,
    pub repo_path: Option<String>,
    pub base_branch: Option<String>,
    pub protected_branches: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Project {
    pub id: i64,
    pub name: String,
    /// `owner` do repositório na forja (no GitLab, o caminho sem o último
    /// segmento: `grupo/projeto`). O nome `gh_*` fica por compatibilidade.
    pub gh_owner: String,
    /// Último segmento do caminho do repositório na forja.
    pub gh_repo: String,
    pub repo_path: String,
    pub base_branch: String,
    pub protected_branches: Vec<String>,
    pub created_at: i64,
    /// `rev` do hub que gerou esta linha do cache; `None` = projeto local
    /// legado (anterior ao pull da configuração, issue #177).
    pub config_rev: Option<i64>,
    /// Atribuição removida no hub: o projeto saiu da lista, mas o clone e
    /// os worktrees ficaram no disco para limpar à mão.
    pub unassigned_at: Option<i64>,
    /// Último resultado da aplicação da atribuição (clone ou validação).
    pub location_state: Option<LocationState>,
    pub location_error: Option<String>,
    /// `config_rev` da atribuição que o ambiente aplicou (ou tentou).
    pub applied_rev: Option<i64>,
    /// `gestor` = o sentinela das sessões do gestor (não é repositório).
    #[serde(default)]
    pub kind: ProjectKind,
    /// Onde estão as issues (spec do Jira e GitLab §3.1). As linhas
    /// anteriores à #347 são GitHub.
    pub tracker: TrackerConfig,
    /// Onde está o repositório.
    pub forge: ForgeConfig,
    /// Padrão do nome da branch (§4); `None` = o padrão do rastreador.
    pub branch_pattern: Option<String>,
    /// Linhas de atribuição nos commits e PRs das sessões (§9.3).
    pub attribution: bool,
    /// Este ambiente é o de sync do projeto e envia o espelho do
    /// rastreador ao hub (§5.1).
    pub sync_role: bool,
    /// Pasta de docs do projeto fora do repositório, como veio do hub
    /// (#440); desligada no projeto local legado.
    pub docs: ct_core::DocsConfig,
}

impl Default for Project {
    fn default() -> Self {
        Project {
            id: 0,
            name: String::new(),
            gh_owner: String::new(),
            gh_repo: String::new(),
            repo_path: String::new(),
            base_branch: String::new(),
            protected_branches: Vec::new(),
            created_at: 0,
            config_rev: None,
            unassigned_at: None,
            location_state: None,
            location_error: None,
            applied_rev: None,
            kind: ProjectKind::default(),
            tracker: TrackerConfig::default(),
            forge: ForgeConfig::default(),
            branch_pattern: None,
            attribution: true,
            sync_role: false,
            docs: ct_core::DocsConfig::default(),
        }
    }
}

impl Project {
    /// A linha veio do hub (cache somente leitura).
    pub fn from_hub(&self) -> bool {
        self.config_rev.is_some()
    }

    /// O par de que as referências de work item precisam.
    pub fn tracker_ref(&self) -> TrackerRef<'_> {
        TrackerRef::new(&self.gh_owner, &self.gh_repo, &self.tracker)
    }

    /// Referência do work item `n` (`owner/repo#n` ou `PROJ-n`).
    pub fn item_ref(&self, n: i64) -> String {
        ct_core::item_ref(&self.tracker_ref(), n)
    }

    /// Padrão de branch em vigor: o do projeto ou o do rastreador.
    pub fn effective_branch_pattern(&self) -> &str {
        self.branch_pattern
            .as_deref()
            .unwrap_or_else(|| default_branch_pattern(self.tracker.kind()))
    }
}

pub(crate) const COLS: &str =
    "id, name, gh_owner, gh_repo, repo_path, base_branch, protected_branches, created_at, \
     config_rev, unassigned_at, location_state, location_error, applied_rev, kind, \
     tracker_kind, tracker_config, forge_kind, forge_config, branch_pattern, attribution, \
     sync_role, docs_config";

/// Quantas colunas `COLS` tem (a seguinte é a primeira extra de uma consulta).
pub(crate) const COLS_LEN: usize = 22;

pub(crate) fn location_state_col(r: &Row, idx: usize) -> rusqlite::Result<Option<LocationState>> {
    let s: Option<String> = r.get(idx)?;
    Ok(match s.as_deref() {
        Some("ok") => Some(LocationState::Ok),
        Some("error") => Some(LocationState::Error),
        _ => None,
    })
}

pub(crate) fn row_to_project(r: &Row) -> rusqlite::Result<Project> {
    Ok(Project {
        id: r.get(0)?,
        name: r.get(1)?,
        gh_owner: r.get(2)?,
        gh_repo: r.get(3)?,
        repo_path: r.get(4)?,
        base_branch: r.get(5)?,
        protected_branches: json_col(r, 6)?,
        created_at: r.get(7)?,
        config_rev: r.get(8)?,
        unassigned_at: r.get(9)?,
        location_state: location_state_col(r, 10)?,
        location_error: r.get(11)?,
        applied_rev: r.get(12)?,
        kind: crate::parse_col(r, 13)?,
        tracker: tracker_col(r, 14)?,
        forge: forge_col(r, 16)?,
        branch_pattern: r.get(18)?,
        attribution: r.get(19)?,
        sync_role: r.get(20)?,
        docs: json_col(r, 21)?,
    })
}

/// O cache vindo do hub é somente leitura: atributos de projeto vêm do hub.
#[cfg(test)]
fn refuse_hub_row(p: &Project, action: &str) -> Result<()> {
    if p.from_hub() {
        return Err(LedgerError::Conflict(format!(
            "o projeto {}/{} vem do hub: {action} no hub (hub.projects.* ou hub.assignments.*)",
            p.gh_owner, p.gh_repo
        )));
    }
    Ok(())
}

impl Ledger {
    pub fn add_project(&self, p: &NewProject) -> Result<Project> {
        let protected =
            serde_json::to_string(&p.protected_branches).expect("Vec<String> serializa");
        let id = self.atomic(|| {
            self.conn.execute(
                "INSERT INTO project (name, gh_owner, gh_repo, repo_path, base_branch, protected_branches, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![p.name, p.gh_owner, p.gh_repo, p.repo_path, p.base_branch, protected, now_ms()],
            )?;
            let id = self.conn.last_insert_rowid();
            self.enqueue_project(id)?;
            Ok(id)
        })?;
        self.get_project(id)
    }

    pub fn get_project(&self, id: i64) -> Result<Project> {
        self.conn
            .query_row(
                &format!("SELECT {COLS} FROM project WHERE id = ?1"),
                [id],
                row_to_project,
            )
            .optional()?
            .ok_or_else(|| LedgerError::NotFound("project", id.to_string()))
    }

    /// Altera só os campos presentes em `patch`; dono e repositório no GitHub
    /// não mudam. Só nos testes (ambiente anterior à #178).
    #[cfg(test)]
    pub(crate) fn update_project(&self, id: i64, patch: &ProjectPatch) -> Result<Project> {
        if let Ok(p) = self.get_project(id) {
            refuse_hub_row(&p, "edite")?;
        }
        let protected = patch
            .protected_branches
            .as_ref()
            .map(|v| serde_json::to_string(v).expect("Vec<String> serializa"));
        self.atomic(|| {
            let changed = self.conn.execute(
                "UPDATE project SET name = COALESCE(?1, name), repo_path = COALESCE(?2, repo_path),
                     base_branch = COALESCE(?3, base_branch),
                     protected_branches = COALESCE(?4, protected_branches)
                 WHERE id = ?5",
                params![
                    patch.name,
                    patch.repo_path,
                    patch.base_branch,
                    protected,
                    id
                ],
            )?;
            if changed == 0 {
                return Err(LedgerError::NotFound("project", id.to_string()));
            }
            self.enqueue_project(id)
        })?;
        self.get_project(id)
    }

    /// Projetos deste ambiente: o cache dos atribuídos pelo hub (e os locais
    /// legados, até o primeiro pull). Sem os desatribuídos nem o sentinela
    /// do gestor.
    pub fn list_projects(&self) -> Result<Vec<Project>> {
        self.query_projects("unassigned_at IS NULL AND kind = 'github'")
    }

    /// Work item `#0` do sentinela do gestor, se já existe.
    pub fn gestor_work_item_id(&self) -> Result<Option<i64>> {
        Ok(self
            .conn
            .query_row(
                "SELECT w.id FROM work_item w JOIN project p ON p.id = w.project_id
                 WHERE p.kind = 'gestor' AND w.gh_number = 0",
                [],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// O projeto sentinela do gestor (`_ctower/gestor`), criado sob demanda
    /// com `repo_path` = a pasta do gestor, e o work item `#0` dele (spec do
    /// gestor §2.1). A linha não vai para o outbox: o hub já tem o sentinela,
    /// e o ambiente não relata onde ele está.
    pub fn ensure_gestor_project(&self, gestor_dir: &str) -> Result<(Project, crate::WorkItem)> {
        let id = self.atomic(|| {
            let found: Option<(i64, String)> = self
                .conn
                .query_row(
                    "SELECT id, repo_path FROM project WHERE kind = 'gestor'",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            Ok(match found {
                Some((id, path)) => {
                    if path != gestor_dir {
                        self.conn.execute(
                            "UPDATE project SET repo_path = ?2 WHERE id = ?1",
                            params![id, gestor_dir],
                        )?;
                    }
                    id
                }
                None => {
                    self.conn.execute(
                        "INSERT INTO project (name, gh_owner, gh_repo, repo_path, base_branch,
                                              protected_branches, created_at, kind)
                         VALUES (?1, ?2, ?3, ?4, '', '[]', ?5, 'gestor')",
                        params![
                            format!("{GESTOR_OWNER}/{GESTOR_REPO}"),
                            GESTOR_OWNER,
                            GESTOR_REPO,
                            gestor_dir,
                            now_ms()
                        ],
                    )?;
                    self.conn.last_insert_rowid()
                }
            })
        })?;
        let wi = self.ensure_work_item(id, 0, ct_core::WorkItemKind::Issue)?;
        Ok((self.get_project(id)?, wi))
    }

    /// Projetos cuja atribuição o hub removeu (ou locais legados que o hub
    /// não atribuiu): o clone e os worktrees ficaram no disco.
    pub fn list_unassigned_projects(&self) -> Result<Vec<Project>> {
        self.query_projects("unassigned_at IS NOT NULL AND kind = 'github'")
    }

    /// Todas as linhas, inclusive as desatribuídas (sem o sentinela do gestor).
    pub fn list_all_projects(&self) -> Result<Vec<Project>> {
        self.query_projects("kind = 'github'")
    }

    fn query_projects(&self, filter: &str) -> Result<Vec<Project>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {COLS} FROM project WHERE {filter} ORDER BY name"
        ))?;
        let rows = stmt.query_map([], row_to_project)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// O projeto existe e não foi desatribuído (o espelho do GitHub só
    /// sincroniza esses).
    pub fn project_active(&self, id: i64) -> Result<bool> {
        Ok(self
            .conn
            .query_row(
                "SELECT unassigned_at IS NULL FROM project WHERE id = ?1",
                [id],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(false))
    }
}

#[cfg(test)]
mod tests {
    use crate::test_support::ledger_with_project;
    use crate::{LedgerError, ProjectPatch};

    #[test]
    fn adiciona_e_le_projeto() {
        let (l, p) = ledger_with_project();
        assert_eq!(p.protected_branches, vec!["develop", "main"]);
        assert_eq!(l.get_project(p.id).unwrap(), p);
        assert_eq!(l.list_projects().unwrap(), vec![p]);
    }

    #[test]
    fn atualiza_so_os_campos_informados() {
        let (l, p) = ledger_with_project();
        let patch = ProjectPatch {
            base_branch: Some("main".into()),
            protected_branches: Some(vec!["main".into()]),
            ..Default::default()
        };
        let novo = l.update_project(p.id, &patch).unwrap();
        assert_eq!(novo.base_branch, "main");
        assert_eq!(novo.protected_branches, vec!["main"]);
        assert_eq!(novo.name, p.name);
        assert_eq!(novo.repo_path, p.repo_path);
        let novo = l
            .update_project(
                p.id,
                &ProjectPatch {
                    name: Some("outro".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(novo.name, "outro");
        assert_eq!(novo.base_branch, "main");
        assert!(matches!(
            l.update_project(999, &ProjectPatch::default()),
            Err(LedgerError::NotFound("project", _))
        ));
    }

    #[test]
    fn projeto_inexistente() {
        let (l, _) = ledger_with_project();
        assert!(matches!(
            l.get_project(999),
            Err(LedgerError::NotFound("project", _))
        ));
    }
}

#[cfg(test)]
mod gestor_tests {
    use crate::{ConfigResponse, Ledger, NewProject};
    use ct_core::ProjectKind;

    #[test]
    fn sentinela_do_gestor_fica_fora_das_listas_do_outbox_e_do_pull() {
        let l = Ledger::open_in_memory().unwrap();
        l.add_project(&NewProject {
            name: "demo".into(),
            gh_owner: "deep-ion-ai".into(),
            gh_repo: "demo".into(),
            repo_path: "/x".into(),
            base_branch: "develop".into(),
            protected_branches: vec![],
        })
        .unwrap();
        l.outbox_ack(i64::MAX).unwrap();
        let (p, wi) = l.ensure_gestor_project("/g").unwrap();
        assert_eq!(p.kind, ProjectKind::Gestor);
        assert_eq!(
            (
                p.gh_owner.as_str(),
                p.gh_repo.as_str(),
                p.repo_path.as_str()
            ),
            ("_ctower", "gestor", "/g")
        );
        assert_eq!(wi.gh_number, 0);
        // De novo, com outra pasta: a mesma linha, com o caminho novo.
        let (p2, wi2) = l.ensure_gestor_project("/g2").unwrap();
        assert_eq!((p2.id, wi2.id, p2.repo_path.as_str()), (p.id, wi.id, "/g2"));
        for list in [
            l.list_projects().unwrap(),
            l.list_all_projects().unwrap(),
            l.list_unassigned_projects().unwrap(),
        ] {
            assert!(list.iter().all(|x| x.kind == ProjectKind::Github));
        }
        // A linha do projeto não vai para o outbox; o work item #0 vai.
        let out = l.outbox_pending(100).unwrap();
        assert!(!out
            .iter()
            .any(|e| matches!(e.change, crate::SyncChange::Project(_))));
        assert!(out
            .iter()
            .any(|e| matches!(e.change, crate::SyncChange::WorkItem(_))));
        // O resync também não põe o sentinela.
        l.enqueue_all().unwrap();
        assert!(!l
            .outbox_pending(1000)
            .unwrap()
            .iter()
            .any(|e| matches!(&e.change, crate::SyncChange::Project(p) if p.owner == "_ctower")));
        // O pull do hub (sem o sentinela) não o desatribui.
        let cfg = ConfigResponse {
            config_rev: 1,
            config_epoch: 1,
            changed: true,
            projects: Some(vec![]),
            removed: None,
        };
        l.apply_hub_config(&cfg, |_| String::new()).unwrap();
        assert_eq!(l.get_project(p.id).unwrap().unassigned_at, None);
    }
}
