//! Projetos com origem no hub (spec do hub como plano de controle §3.1,
//! §3.2, §3.4 e §5; issue #175): cadastro, edição, exclusão com lápide,
//! `config_rev` e a quarentena de push.
//!
//! Toda mudança em projeto (inclusive a lápide) faz, na mesma transação,
//! `config_rev = config_rev + 1` e grava o valor em `project.rev`. As
//! atribuições (#176, `assignments.rs`) entram no mesmo contador.

use super::{row_to_project, Hub, HubAssignment, HubProject, HubSession, PROJECT_COLS};
use crate::sync::project_key;
use crate::{LedgerError, Result, SyncChange};
use ct_core::{now_ms, ForgeConfig, TrackerConfig};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Parâmetros de `hub.projects.add` depois do parse da URL e da validação
/// no GitHub.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NewHubProject {
    pub owner: String,
    pub repo: String,
    pub name: String,
    pub base_branch: String,
    pub protected_branches: Vec<String>,
}

/// Rastreador, forja, padrão de branch e atribuição do cadastro (spec do
/// Jira e GitLab §3, §4 e §9.3; issue #348). O padrão é o projeto GitHub
/// de sempre.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectProvider {
    pub tracker: TrackerConfig,
    pub forge: ForgeConfig,
    /// `None` = o padrão do rastreador.
    pub branch_pattern: Option<String>,
    pub attribution: bool,
    /// Pasta de docs fora do repositório (#440).
    pub docs: ct_core::DocsConfig,
}

impl Default for ProjectProvider {
    fn default() -> Self {
        ProjectProvider {
            tracker: TrackerConfig::Github,
            forge: ForgeConfig::Github,
            branch_pattern: None,
            attribution: true,
            docs: ct_core::DocsConfig::default(),
        }
    }
}

impl ProjectProvider {
    /// Confere o cadastro sem rede: combinação suportada (GitHub + GitHub
    /// ou Jira + GitLab), campos obrigatórios do Jira e do GitLab e o padrão
    /// de branch. O Jira e o GitLab em si só o ambiente de sync confere, no
    /// primeiro ciclo (`tracker_status`).
    pub fn validate(&self) -> std::result::Result<(), String> {
        self.validate_base()?;
        match self.tracker.jira() {
            Some(j) => validate_jira_filter(j),
            None => Ok(()),
        }
    }

    /// [`ProjectProvider::validate`] sem o filtro do sync do Jira: a edição
    /// que não mexe no rastreador (nome, ambiente de sync...) de um cadastro
    /// anterior ao modo sprint ativa, ainda sem JQL, continua possível.
    pub(crate) fn validate_base(&self) -> std::result::Result<(), String> {
        let (tk, fk) = (self.tracker.kind(), self.forge.kind());
        if !ct_core::supported_combination(tk, fk) {
            return Err(format!(
                "combinação de rastreador e forja não suportada: {tk} + {fk} (use github + \
                 github ou jira + gitlab)"
            ));
        }
        if let Some(j) = self.tracker.jira() {
            let url = j.url.to_ascii_lowercase();
            if !(url.starts_with("https://") || url.starts_with("http://")) || url.len() < 10 {
                return Err(format!("URL do Jira inválida: {:?}", j.url));
            }
            let key_ok = j.project_key.starts_with(|c: char| c.is_ascii_uppercase())
                && j.project_key
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
            if !key_ok {
                return Err(format!(
                    "chave do projeto Jira inválida: {:?} (maiúsculas, como PROJ)",
                    j.project_key
                ));
            }
            if j.credential_cmd.trim().is_empty() {
                return Err("informe o comando de credencial do Jira (ex.: pass show \
                            jira | head -1)"
                    .into());
            }
        }
        if let Some(g) = self.forge.gitlab() {
            if g.host.is_empty() || !g.path.contains('/') {
                return Err(format!(
                    "repositório do GitLab inválido: host {:?}, caminho {:?}",
                    g.host, g.path
                ));
            }
            if g.credential_cmd.trim().is_empty() {
                return Err("informe o comando de credencial do GitLab (ex.: pass show \
                            gitlab | head -1)"
                    .into());
            }
        }
        if let Some(pat) = &self.branch_pattern {
            ct_core::branch::validate_branch_pattern(pat).map_err(|e| e.to_string())?;
        }
        self.docs.validate()
    }
}

/// O filtro do sync do Jira: no modo sprint ativa, nada a conferir; fora
/// dele, JQL obrigatório e limite de issues entre 1 e
/// [`ct_core::MAX_ISSUES_CEILING`].
fn validate_jira_filter(j: &ct_core::JiraConfig) -> std::result::Result<(), String> {
    if j.active_sprint_only {
        return Ok(());
    }
    if j.jql.trim().is_empty() {
        return Err(
            "informe o JQL do sync (ex.: labels = minha-equipe AND updated >= -90d) \
                    ou marque \"sincronizar somente a sprint ativa\""
                .into(),
        );
    }
    if j.max_issues == 0 || j.max_issues > ct_core::MAX_ISSUES_CEILING {
        return Err(format!(
            "limite de issues sincronizadas inválido: {} (de 1 a {})",
            j.max_issues,
            ct_core::MAX_ISSUES_CEILING
        ));
    }
    Ok(())
}

/// Campos de `hub.projects.update`; `None` = não muda.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct HubProjectPatch {
    pub name: Option<String>,
    pub base_branch: Option<String>,
    pub protected_branches: Option<Vec<String>>,
    /// Configuração nova do rastreador, do mesmo tipo (o tipo não muda
    /// depois do cadastro).
    pub tracker: Option<TrackerConfig>,
    /// Configuração nova da forja, do mesmo tipo e do mesmo repositório
    /// (host e caminho, que dão o `owner/repo`).
    pub forge: Option<ForgeConfig>,
    /// `Some("")` = volta ao padrão do rastreador.
    pub branch_pattern: Option<String>,
    pub attribution: Option<bool>,
    /// Ambiente de sync do projeto Jira (`env_id` ou nome), entre os
    /// atribuídos (spec §5.1).
    pub sync_env: Option<String>,
    /// Docs do projeto (#440): o objeto inteiro substitui o anterior.
    pub docs: Option<ct_core::DocsConfig>,
}

impl HubProjectPatch {
    pub fn is_empty(&self) -> bool {
        self.name.is_none()
            && self.base_branch.is_none()
            && self.protected_branches.is_none()
            && self.tracker.is_none()
            && self.forge.is_none()
            && self.branch_pattern.is_none()
            && self.attribution.is_none()
            && self.sync_env.is_none()
            && self.docs.is_none()
    }
}

/// Resposta de `hub.projects.add`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct ProjectAdded {
    pub project: HubProject,
    /// `config_rev` do cadastro.
    pub rev: i64,
    /// Linhas da quarentena reaplicadas pela ingestão normal.
    pub reapplied: usize,
    /// O cadastro reaproveitou a linha de uma lápide.
    pub restored: bool,
}

/// O que a exclusão do projeto atinge (`hub.projects.delete` sem `confirm`).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct DeleteImpact {
    pub project: HubProject,
    /// Atribuições ativas (`assignment` sem `removed_at`).
    pub assignments: Vec<HubAssignment>,
    /// Sessões vivas relatadas pelos ambientes.
    pub live_sessions: Vec<HubSession>,
}

/// Linhas em quarentena de um projeto vindas de um ambiente.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct QuarantineGroup {
    /// `owner/repo` em minúsculas.
    pub project_key: String,
    pub env_id: String,
    pub env_name: String,
    pub rows: i64,
    pub first_received_at: i64,
    pub last_received_at: i64,
}

impl Hub {
    /// `config_rev + 1`, na transação de quem chama. Nunca volta.
    pub(super) fn bump_config_rev(&self) -> Result<i64> {
        Ok(self.conn.query_row(
            "INSERT INTO hub_meta (key, value) VALUES ('config_rev', 1)
             ON CONFLICT (key) DO UPDATE SET value = value + 1 RETURNING value",
            [],
            |r| r.get(0),
        )?)
    }

    /// `config_rev + 1` para uma mudança no store de secrets e variáveis
    /// (`ct-secrets`, issue #164), que vive fora do `hub.db`: o valor vira o
    /// `rev` da entrada. Se a gravação do store falhar depois, sobra só um
    /// `config_rev` sem mudança, o que é inofensivo.
    pub fn bump_config_rev_for_secrets(&self) -> Result<i64> {
        self.bump_config_rev()
    }

    /// `max(config_rev, min) + 1`: o `cthubd secrets restore` (#170) usa
    /// para o `config_rev` passar do maior `rev` do store restaurado (um
    /// `hub.db` novo começa em 0) e os ambientes receberem tudo de novo.
    pub fn bump_config_rev_past(&self, min: i64) -> Result<i64> {
        Ok(self.conn.query_row(
            "INSERT INTO hub_meta (key, value) VALUES ('config_rev', max(?1, 0) + 1)
             ON CONFLICT (key) DO UPDATE SET value = max(value, ?1) + 1 RETURNING value",
            [min],
            |r| r.get(0),
        )?)
    }

    fn project_by_key(&self, key: &str) -> Result<HubProject> {
        Ok(self.conn.query_row(
            &format!("SELECT {PROJECT_COLS} FROM project WHERE key = ?1"),
            [key],
            row_to_project,
        )?)
    }

    /// `config_rev` gravado no projeto pela última mudança.
    pub fn project_rev(&self, owner: &str, repo: &str) -> Result<Option<i64>> {
        Ok(self
            .conn
            .query_row(
                "SELECT rev FROM project WHERE key = ?1",
                [project_key(owner, repo)],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// `deleted_at` da lápide de `owner/repo`, se houver.
    pub fn project_tombstone(&self, owner: &str, repo: &str) -> Result<Option<i64>> {
        Ok(self
            .conn
            .query_row(
                "SELECT deleted_at FROM project WHERE key = ?1",
                [project_key(owner, repo)],
                |r| r.get::<_, Option<i64>>(0),
            )
            .optional()?
            .flatten())
    }

    /// Cadastra o projeto (`hub.projects.add`). Recadastrar um `owner/repo`
    /// com lápide reaproveita a linha e limpa `deleted_at`. Na mesma
    /// transação, a quarentena do projeto é reaplicada pela ingestão normal,
    /// na ordem (`env_id`, `seq`), e as linhas reaplicadas saem dela.
    pub fn add_project(&self, p: &NewHubProject) -> Result<ProjectAdded> {
        self.add_project_with(p, &ProjectProvider::default())
    }

    /// [`Hub::add_project`] com rastreador, forja, padrão de branch e
    /// atribuição (#348). Recusa combinação não suportada e padrão de branch
    /// inválido ([`ProjectProvider::validate`]).
    pub fn add_project_with(
        &self,
        p: &NewHubProject,
        provider: &ProjectProvider,
    ) -> Result<ProjectAdded> {
        let pv = &ProjectProvider {
            docs: provider.docs.normalized(),
            ..provider.clone()
        };
        pv.validate().map_err(LedgerError::Conflict)?;
        let key = project_key(&p.owner, &p.repo);
        let protected =
            serde_json::to_string(&p.protected_branches).expect("Vec<String> serializa");
        let tx = self.conn.unchecked_transaction()?;
        let existing: Option<(String, Option<i64>)> = self
            .conn
            .query_row(
                "SELECT name, deleted_at FROM project WHERE key = ?1",
                [&key],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let rev = self.bump_config_rev()?;
        let now = now_ms();
        let restored = match existing {
            Some((name, None)) => {
                return Err(LedgerError::Conflict(format!(
                    "{}/{} já está cadastrado no hub como {name}",
                    p.owner, p.repo
                )))
            }
            Some((_, Some(_))) => {
                // O recadastro leva o rastreador e a forja do cadastro novo,
                // nunca os da lápide.
                self.conn.execute(
                    "UPDATE project SET gh_owner = ?2, gh_repo = ?3, name = ?4, base_branch = ?5,
                            protected_branches = ?6, created_at = ?7, rev = ?8, deleted_at = NULL,
                            tracker_kind = ?9, tracker_config = ?10,
                            forge_kind = ?11, forge_config = ?12, branch_pattern = ?13,
                            attribution = ?14, sync_env_id = NULL, docs_config = ?15
                     WHERE key = ?1",
                    params![
                        key,
                        p.owner,
                        p.repo,
                        p.name,
                        p.base_branch,
                        protected,
                        now,
                        rev,
                        pv.tracker.kind().as_str(),
                        pv.tracker.config_json(),
                        pv.forge.kind().as_str(),
                        pv.forge.config_json(),
                        pv.branch_pattern,
                        pv.attribution,
                        pv.docs.config_json()
                    ],
                )?;
                // A purga já apagou; garante o espelho zerado no recadastro
                // mesmo assim.
                for t in ["sync_state", "tracker_status"] {
                    self.conn.execute(
                        &format!(
                            "DELETE FROM {t} WHERE project_id = (SELECT id FROM project WHERE key = ?1)"
                        ),
                        [&key],
                    )?;
                }
                true
            }
            None => {
                self.conn.execute(
                    "INSERT INTO project (key, gh_owner, gh_repo, name, base_branch,
                                          protected_branches, created_at, rev, tracker_kind,
                                          tracker_config, forge_kind, forge_config,
                                          branch_pattern, attribution, docs_config)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
                    params![
                        key,
                        p.owner,
                        p.repo,
                        p.name,
                        p.base_branch,
                        protected,
                        now,
                        rev,
                        pv.tracker.kind().as_str(),
                        pv.tracker.config_json(),
                        pv.forge.kind().as_str(),
                        pv.forge.config_json(),
                        pv.branch_pattern,
                        pv.attribution,
                        pv.docs.config_json()
                    ],
                )?;
                false
            }
        };
        let reapplied = self.reapply_quarantine(&key)?;
        let project = self.project_by_key(&key)?;
        tx.commit()?;
        Ok(ProjectAdded {
            project,
            rev,
            reapplied,
            restored,
        })
    }

    /// Reaplica a quarentena de `key` pela ingestão normal, na ordem
    /// (`env_id`, `seq`), e apaga as linhas reaplicadas.
    fn reapply_quarantine(&self, key: &str) -> Result<usize> {
        let rows: Vec<(i64, String, String)> = {
            let mut stmt = self.conn.prepare(
                "SELECT id, env_id, payload FROM push_quarantine
                 WHERE project_key = ?1 ORDER BY env_id, seq",
            )?;
            let rows = stmt.query_map([key], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        for (id, env, payload) in &rows {
            let change: SyncChange = serde_json::from_str(payload).map_err(|e| {
                LedgerError::Corrupt(format!("linha {id} da quarentena ilegível: {e}"))
            })?;
            self.apply(env, &change)?;
        }
        self.conn
            .execute("DELETE FROM push_quarantine WHERE project_key = ?1", [key])?;
        Ok(rows.len())
    }

    /// Edita nome, `base_branch`, protegidas, a configuração do rastreador e
    /// da forja (sem mudar o tipo nem o repositório), padrão de branch,
    /// atribuição e o ambiente de sync (`hub.projects.update`). Tudo numa
    /// transação, com um `config_rev` só.
    pub fn update_project(
        &self,
        owner: &str,
        repo: &str,
        patch: &HubProjectPatch,
    ) -> Result<HubProject> {
        let key = project_key(owner, repo);
        let Some(cur) = self.find_project(owner, repo)? else {
            return Err(LedgerError::NotFound(
                "projeto",
                format!("{owner}/{repo} não está no hub"),
            ));
        };
        let provider = self.patched_provider(&cur, patch)?;
        let sync_env = match patch.sync_env.as_deref() {
            Some(e) => Some((self.resolve_env(e)?, e.trim().to_string())),
            None => None,
        };
        let protected = patch
            .protected_branches
            .as_ref()
            .map(|v| serde_json::to_string(v).expect("Vec<String> serializa"));
        let tx = self.conn.unchecked_transaction()?;
        let rev = self.bump_config_rev()?;
        let changed = self.conn.execute(
            "UPDATE project SET name = coalesce(?2, name),
                    base_branch = coalesce(?3, base_branch),
                    protected_branches = coalesce(?4, protected_branches), rev = ?5,
                    tracker_config = ?6, forge_config = ?7, branch_pattern = ?8,
                    attribution = ?9, docs_config = ?10
             WHERE key = ?1 AND deleted_at IS NULL",
            params![
                key,
                patch.name,
                patch.base_branch,
                protected,
                rev,
                provider.tracker.config_json(),
                provider.forge.config_json(),
                provider.branch_pattern,
                provider.attribution,
                provider.docs.config_json()
            ],
        )?;
        if changed == 0 {
            return Err(LedgerError::NotFound(
                "projeto",
                format!("{owner}/{repo} não está no hub"),
            ));
        }
        if let Some((env_id, label)) = &sync_env {
            self.set_sync_env_in_tx(&cur, env_id, label, rev)?;
        }
        let p = self.project_by_key(&key)?;
        tx.commit()?;
        Ok(p)
    }

    /// Rastreador, forja, padrão e atribuição do projeto com o `patch`
    /// aplicado e conferido. O tipo do rastreador e da forja e o
    /// repositório da forja (que dá o `owner/repo`) não mudam.
    fn patched_provider(
        &self,
        cur: &HubProject,
        patch: &HubProjectPatch,
    ) -> Result<ProjectProvider> {
        let refuse = |m: String| Err(LedgerError::Conflict(m));
        let tracker = match &patch.tracker {
            Some(t) if t.kind() != cur.tracker.kind() => {
                return refuse(format!(
                    "o rastreador não muda depois do cadastro ({} → {}): cadastre outro projeto",
                    cur.tracker.kind(),
                    t.kind()
                ))
            }
            Some(t) => t.clone(),
            None => cur.tracker.clone(),
        };
        let forge = match &patch.forge {
            Some(f) if f.kind() != cur.forge.kind() => {
                return refuse(format!(
                    "a forja não muda depois do cadastro ({} → {}): cadastre outro projeto",
                    cur.forge.kind(),
                    f.kind()
                ))
            }
            Some(ForgeConfig::Gitlab(g)) => {
                let old = cur.forge.gitlab().expect("mesmo tipo");
                if !g.host.eq_ignore_ascii_case(&old.host)
                    || !g.path.eq_ignore_ascii_case(&old.path)
                {
                    return refuse(format!(
                        "o repositório do projeto não muda ({}/{} → {}/{}): cadastre outro projeto",
                        old.host, old.path, g.host, g.path
                    ));
                }
                ForgeConfig::Gitlab(g.clone())
            }
            Some(f) => f.clone(),
            None => cur.forge.clone(),
        };
        let branch_pattern = match patch.branch_pattern.as_deref().map(str::trim) {
            Some("") => None,
            Some(p) => Some(p.to_string()),
            None => cur.branch_pattern.clone(),
        };
        let provider = ProjectProvider {
            tracker,
            forge,
            branch_pattern,
            attribution: patch.attribution.unwrap_or(cur.attribution),
            docs: match &patch.docs {
                Some(d) => d.normalized(),
                None => cur.docs.clone(),
            },
        };
        // O filtro do Jira só é conferido quando a edição manda o rastreador.
        let checked = if patch.tracker.is_some() {
            provider.validate()
        } else {
            provider.validate_base()
        };
        checked.map_err(LedgerError::Conflict)?;
        Ok(provider)
    }

    /// O que a exclusão de `owner/repo` atinge: atribuições ativas e sessões
    /// vivas relatadas (de qualquer ambiente, atribuído ou não). `None` se o
    /// projeto não está cadastrado.
    pub fn delete_impact(&self, owner: &str, repo: &str) -> Result<Option<DeleteImpact>> {
        let Some(project) = self.find_project(owner, repo)? else {
            return Ok(None);
        };
        let assignments = self.project_assignments(project.id)?;
        let live_sessions = self.live_sessions(project.id, None)?;
        Ok(Some(DeleteImpact {
            project,
            assignments,
            live_sessions,
        }))
    }

    /// Snapshot fora da rotação antes de uma exclusão:
    /// `<dir>/hub-<AAAAMMDD-HHMM>-pre-delete.db` (UTC), com `-2`, `-3`... se
    /// já existe um daquele minuto. Nunca sobrescreve.
    pub fn snapshot_pre_delete(&self, dir: &Path, now_ms: i64) -> Result<PathBuf> {
        std::fs::create_dir_all(dir).map_err(|e| LedgerError::Conflict(e.to_string()))?;
        let stamp = ct_core::utc_stamp(now_ms);
        let path = std::iter::once(format!("hub-{stamp}-pre-delete.db"))
            .chain((2u32..).map(|n| format!("hub-{stamp}-pre-delete-{n}.db")))
            .map(|n| dir.join(n))
            .find(|p| !p.exists())
            .expect("algum sufixo livre");
        self.conn
            .execute("VACUUM INTO ?1", [path.to_string_lossy().as_ref()])?;
        Ok(path)
    }

    /// Exclui o projeto (`hub.projects.delete`) numa transação: as
    /// atribuições recebem `removed_at` e `rev`, o relato dos ambientes sai, o histórico do projeto (work items, sessões,
    /// decisões, eventos, espelho do GitHub) é apagado com o projeto em
    /// `purging`, e a linha de `project` fica como lápide (`deleted_at`,
    /// `rev`). Devolve o `config_rev` da lápide. A confirmação e o snapshot
    /// prévio são de quem chama (`cthubd`).
    pub fn delete_project(&self, owner: &str, repo: &str) -> Result<i64> {
        let Some(p) = self.find_project(owner, repo)? else {
            return Err(LedgerError::NotFound(
                "projeto",
                format!("{owner}/{repo} não está no hub"),
            ));
        };
        let tx = self.conn.unchecked_transaction()?;
        let rev = self.bump_config_rev()?;
        // As atribuições ficam como removidas, com o `rev` da lápide (o pull
        // da #177 propaga a remoção). O relato dos ambientes é apagado com o
        // histórico: no recadastro, o projeto começa sem localização, e o
        // ambiente volta a relatar quando receber uma atribuição nova.
        self.conn.execute(
            "UPDATE assignment SET removed_at = ?2, rev = ?3
             WHERE project_id = ?1 AND removed_at IS NULL",
            params![p.id, now_ms(), rev],
        )?;
        self.conn
            .execute("DELETE FROM project_location WHERE project_id = ?1", [p.id])?;
        let wi = "SELECT id FROM work_item WHERE project_id = ?1";
        self.conn
            .execute("INSERT INTO purging (project_id) VALUES (?1)", [p.id])?;
        for sql in [
            "DELETE FROM event WHERE project_id = ?1".to_string(),
            format!("DELETE FROM decision WHERE work_item_id IN ({wi})"),
            "DELETE FROM pr_cache WHERE project_id = ?1".to_string(),
            format!("DELETE FROM session WHERE work_item_id IN ({wi})"),
            format!("DELETE FROM work_item_location WHERE work_item_id IN ({wi})"),
            "DELETE FROM work_item WHERE project_id = ?1".to_string(),
            "DELETE FROM issue_cache WHERE project_id = ?1".to_string(),
            "DELETE FROM sync_state WHERE project_id = ?1".to_string(),
            "DELETE FROM tracker_status WHERE project_id = ?1".to_string(),
            "DELETE FROM proposal WHERE project_id = ?1".to_string(),
            "DELETE FROM agent_rules WHERE project_id = ?1".to_string(),
            "DELETE FROM agent_rules_proposal WHERE project_id = ?1".to_string(),
            "DELETE FROM purging WHERE project_id = ?1".to_string(),
        ] {
            self.conn.execute(&sql, [p.id])?;
        }
        self.conn.execute(
            "DELETE FROM gh_etag WHERE key LIKE ?1",
            [format!("/repos/{}/{}/%", p.gh_owner, p.gh_repo)],
        )?;
        self.conn.execute(
            "DELETE FROM push_quarantine WHERE project_key = ?1",
            [project_key(owner, repo)],
        )?;
        self.conn.execute(
            "UPDATE project SET deleted_at = ?2, rev = ?3, sync_env_id = NULL WHERE id = ?1",
            params![p.id, now_ms(), rev],
        )?;
        tx.commit()?;
        Ok(rev)
    }

    // --- quarentena -------------------------------------------------------

    /// Quarentena agrupada por projeto e ambiente (`hub.quarantine.list` e
    /// os avisos de `hub.status`).
    pub fn quarantine_summary(&self) -> Result<Vec<QuarantineGroup>> {
        let mut stmt = self.conn.prepare(
            "SELECT q.project_key, q.env_id, coalesce(e.name, q.env_id), count(*),
                    min(q.received_at), max(q.received_at)
             FROM push_quarantine q LEFT JOIN env e ON e.env_id = q.env_id
             GROUP BY q.project_key, q.env_id ORDER BY q.project_key, 3",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(QuarantineGroup {
                project_key: r.get(0)?,
                env_id: r.get(1)?,
                env_name: r.get(2)?,
                rows: r.get(3)?,
                first_received_at: r.get(4)?,
                last_received_at: r.get(5)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Linhas em quarentena de `owner/repo`.
    pub fn quarantine_count(&self, owner: &str, repo: &str) -> Result<i64> {
        Ok(self.conn.query_row(
            "SELECT count(*) FROM push_quarantine WHERE project_key = ?1",
            [project_key(owner, repo)],
            |r| r.get(0),
        )?)
    }

    /// Descarta a quarentena de `owner/repo`. Devolve quantas linhas saíram.
    pub fn discard_quarantine(&self, owner: &str, repo: &str) -> Result<usize> {
        Ok(self.conn.execute(
            "DELETE FROM push_quarantine WHERE project_key = ?1",
            [project_key(owner, repo)],
        )?)
    }
}
