//! Atribuições de projetos a ambientes (spec do hub como plano de controle
//! §3.3 e §3.4; issue #176).
//!
//! `assignment` é a intenção do hub: qual ambiente executa qual projeto, com
//! o caminho do clone (`''` = padrão do ambiente, `<repos_root>/<repo>`).
//! `project_location` é o relato do ambiente. O estado mostrado na UI vem do
//! cruzamento dos dois ([`AssignmentState`]).
//!
//! Toda mudança de atribuição faz, na mesma transação, `config_rev + 1` e
//! grava o valor em `assignment.rev`. Remover não apaga a linha: grava
//! `removed_at` (o pull da #177 propaga a remoção por `rev`).

use super::{row_to_session, Hub, HubSession, SESSION_COLS};
use crate::sync::project_key;
use crate::{
    ConfigDone, ConfigProject, ConfigRemoved, ConfigResponse, DoneResponse, LedgerError, Result,
};
use ct_core::now_ms;
use rusqlite::{params, OptionalExtension, Row};
use serde::{Deserialize, Serialize};

/// Estado da atribuição para a UI, derivado de `assignment` ×
/// `project_location`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum AssignmentState {
    /// Sem relato, ou relato com `applied_rev` menor que `rev`.
    Pending,
    /// Relato `ok` com `applied_rev >= rev`.
    Applied,
    /// Relato `error` com `applied_rev >= rev` (a mensagem vem em `error`).
    Error,
}

impl AssignmentState {
    /// A comparação de revisão vem antes: um erro relatado para uma revisão
    /// anterior não vale para a atribuição atual, que fica pendente.
    pub fn derive(rev: i64, reported: Option<(Option<&str>, Option<i64>)>) -> Self {
        match reported {
            Some((state, Some(applied))) if applied >= rev => match state {
                Some("ok") => AssignmentState::Applied,
                Some("error") => AssignmentState::Error,
                _ => AssignmentState::Pending,
            },
            _ => AssignmentState::Pending,
        }
    }
}

/// Atribuição ativa com o relato do ambiente (`hub.assignments.list` e
/// `hub.projects.list`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct HubAssignment {
    pub project_id: i64,
    pub gh_owner: String,
    pub gh_repo: String,
    pub env_id: String,
    pub env_name: String,
    /// Caminho pedido pelo hub; vazio = padrão do ambiente
    /// (`<repos_root>/<repo>`).
    pub repo_path: String,
    pub created_at: i64,
    /// `config_rev` da última mudança desta atribuição.
    pub rev: i64,
    pub state: AssignmentState,
    /// Mensagem do último relato de erro do ambiente.
    pub error: Option<String>,
    /// `config_rev` que o ambiente relatou ter aplicado (ou tentado).
    pub applied_rev: Option<i64>,
    /// Caminho do clone relatado pelo ambiente.
    pub reported_path: Option<String>,
    pub reported_at: Option<i64>,
}

/// Filtros de `hub.assignments.list`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AssignmentFilter {
    /// `env_id` ou nome do ambiente.
    pub env: Option<String>,
    pub owner: Option<String>,
    pub repo: Option<String>,
}

/// Resposta de `hub.assignments.add`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct AssignmentAdded {
    pub assignment: HubAssignment,
    /// `false` quando a atribuição já existia igual (sem novo `config_rev`).
    pub changed: bool,
    /// A atribuição tinha sido removida e voltou.
    pub reassigned: bool,
    /// O ambiente virou o ambiente de sync do projeto Jira (primeira
    /// atribuição sem ambiente de sync).
    pub sync_env: bool,
}

const ASSIGNMENT_SELECT: &str = "SELECT a.project_id, p.gh_owner, p.gh_repo, a.env_id,
        coalesce(e.name, t.name, a.env_id), a.repo_path, a.created_at, a.rev,
        l.state, l.error, l.applied_rev, l.repo_path, l.reported_at
     FROM assignment a
     JOIN project p ON p.id = a.project_id AND p.deleted_at IS NULL
     LEFT JOIN env e ON e.env_id = a.env_id
     LEFT JOIN env_token t ON t.env_id = a.env_id
     LEFT JOIN project_location l
        ON l.project_id = a.project_id AND l.env_id = a.env_id AND l.removed = 0
     WHERE a.removed_at IS NULL";

fn row_to_assignment(r: &Row) -> rusqlite::Result<HubAssignment> {
    let rev: i64 = r.get(7)?;
    let state: Option<String> = r.get(8)?;
    let applied_rev: Option<i64> = r.get(10)?;
    let reported_at: Option<i64> = r.get(12)?;
    let reported = reported_at.map(|_| (state.as_deref(), applied_rev));
    Ok(HubAssignment {
        project_id: r.get(0)?,
        gh_owner: r.get(1)?,
        gh_repo: r.get(2)?,
        env_id: r.get(3)?,
        env_name: r.get(4)?,
        repo_path: r.get(5)?,
        created_at: r.get(6)?,
        rev,
        state: AssignmentState::derive(rev, reported),
        error: r.get(9)?,
        applied_rev,
        reported_path: r.get(11)?,
        reported_at,
    })
}

/// `repo_path` vazio = padrão do ambiente; senão, absoluto (`/`, `~` ou
/// letra de unidade do Windows).
pub(super) fn normalize_repo_path(path: Option<&str>) -> Result<String> {
    let p = path.map(str::trim).unwrap_or_default();
    let b = p.as_bytes();
    let windows = b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':';
    if p.is_empty() || p.starts_with('/') || p.starts_with('~') || windows {
        Ok(p.to_string())
    } else {
        Err(LedgerError::Conflict(format!(
            "repo_path deve ser absoluto (ou vazio para o padrão do ambiente): {p}"
        )))
    }
}

impl Hub {
    /// `env_id` do ambiente, por `env_id` ou nome (dos que já falaram com o
    /// hub ou cujo token já está preso a um `env_id`).
    pub fn resolve_env(&self, env: &str) -> Result<String> {
        let env = env.trim();
        let by_id: Option<String> = self
            .conn
            .query_row(
                "SELECT env_id FROM env WHERE env_id = ?1 AND last_seen_at IS NOT NULL
                 UNION SELECT env_id FROM env_token WHERE env_id = ?1",
                [env],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(id) = by_id {
            return Ok(id);
        }
        let ids: Vec<String> = {
            let mut stmt = self.conn.prepare(
                "SELECT env_id FROM env WHERE name = ?1 AND last_seen_at IS NOT NULL
                 UNION SELECT env_id FROM env_token WHERE name = ?1 AND env_id IS NOT NULL",
            )?;
            let rows = stmt.query_map([env], |r| r.get(0))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        match ids.as_slice() {
            [id] => Ok(id.clone()),
            [] => Err(LedgerError::NotFound(
                "ambiente",
                format!("{env} não é conhecido do hub (ele precisa ter se ligado ao hub uma vez)"),
            )),
            _ => Err(LedgerError::Conflict(format!(
                "há mais de um ambiente chamado {env}: use o env_id ({})",
                ids.join(", ")
            ))),
        }
    }

    fn live_project(&self, owner: &str, repo: &str) -> Result<i64> {
        self.live_project_id(owner, repo)?.ok_or_else(|| {
            LedgerError::NotFound("projeto", format!("{owner}/{repo} não está no hub"))
        })
    }

    /// Atribuições ativas, filtradas por ambiente e projeto.
    pub fn list_assignments(&self, f: &AssignmentFilter) -> Result<Vec<HubAssignment>> {
        let env = match f.env.as_deref() {
            Some(e) => Some(self.resolve_env(e)?),
            None => None,
        };
        let owner = f.owner.as_deref().map(str::to_lowercase);
        let repo = f.repo.as_deref().map(str::to_lowercase);
        let mut stmt = self.conn.prepare(&format!(
            "{ASSIGNMENT_SELECT}
               AND (?1 IS NULL OR a.env_id = ?1)
               AND (?2 IS NULL OR lower(p.gh_owner) = ?2)
               AND (?3 IS NULL OR lower(p.gh_repo) = ?3)
             ORDER BY p.key, 5, a.env_id"
        ))?;
        let rows = stmt.query_map(params![env, owner, repo], row_to_assignment)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Atribuições ativas do projeto.
    pub fn project_assignments(&self, project_id: i64) -> Result<Vec<HubAssignment>> {
        let mut stmt = self.conn.prepare(&format!(
            "{ASSIGNMENT_SELECT} AND a.project_id = ?1 ORDER BY 5, a.env_id"
        ))?;
        let rows = stmt.query_map([project_id], row_to_assignment)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    fn find_assignment(&self, project_id: i64, env_id: &str) -> Result<Option<HubAssignment>> {
        Ok(self
            .conn
            .query_row(
                &format!("{ASSIGNMENT_SELECT} AND a.project_id = ?1 AND a.env_id = ?2"),
                params![project_id, env_id],
                row_to_assignment,
            )
            .optional()?)
    }

    /// Sessões vivas relatadas do projeto, de um ambiente ou de todos.
    pub fn live_sessions(&self, project_id: i64, env_id: Option<&str>) -> Result<Vec<HubSession>> {
        let cols = SESSION_COLS
            .split(", ")
            .map(|c| format!("s.{}", c.trim()))
            .collect::<Vec<_>>()
            .join(", ");
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {cols} FROM session s JOIN work_item w ON w.id = s.work_item_id
             WHERE w.project_id = ?1 AND (?2 IS NULL OR s.env_id = ?2)
               AND s.state IN ('starting', 'running', 'idle')
             ORDER BY s.started_at"
        ))?;
        let rows = stmt.query_map(params![project_id, env_id], row_to_session)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Atribui `owner/repo` ao ambiente (`hub.assignments.add`). Reatribuir
    /// uma atribuição removida a reativa com `rev` novo; mudar o
    /// `repo_path` de uma ativa também. A mesma atribuição, igual, não muda
    /// nada (nem o `config_rev`).
    pub fn add_assignment(
        &self,
        owner: &str,
        repo: &str,
        env: &str,
        repo_path: Option<&str>,
    ) -> Result<AssignmentAdded> {
        let path = normalize_repo_path(repo_path)?;
        let pid = self.live_project(owner, repo)?;
        let env_id = self.resolve_env(env)?;
        let tx = self.conn.unchecked_transaction()?;
        let cur: Option<(String, Option<i64>)> = self
            .conn
            .query_row(
                "SELECT repo_path, removed_at FROM assignment
                 WHERE project_id = ?1 AND env_id = ?2",
                params![pid, env_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let (changed, reassigned) = match &cur {
            Some((p, None)) if *p == path => (false, false),
            Some((_, None)) => {
                let rev = self.bump_config_rev()?;
                self.conn.execute(
                    "UPDATE assignment SET repo_path = ?3, rev = ?4
                     WHERE project_id = ?1 AND env_id = ?2",
                    params![pid, env_id, path, rev],
                )?;
                (true, false)
            }
            Some((_, Some(_))) => {
                let rev = self.bump_config_rev()?;
                self.conn.execute(
                    "UPDATE assignment SET repo_path = ?3, rev = ?4, created_at = ?5,
                            removed_at = NULL
                     WHERE project_id = ?1 AND env_id = ?2",
                    params![pid, env_id, path, rev, now_ms()],
                )?;
                (true, true)
            }
            None => {
                let rev = self.bump_config_rev()?;
                self.conn.execute(
                    "INSERT INTO assignment (project_id, env_id, repo_path, created_at, rev)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![pid, env_id, path, now_ms(), rev],
                )?;
                (true, false)
            }
        };
        // Projeto Jira sem ambiente de sync: a atribuição nova (ou de volta)
        // vira o ambiente de sync (spec do Jira e GitLab §5.1), com o mesmo
        // `rev`, para o ambiente receber `sync: true` no pull.
        let jira_sem_sync: bool = self.conn.query_row(
            "SELECT tracker_kind = 'jira' AND sync_env_id IS NULL FROM project WHERE id = ?1",
            [pid],
            |r| r.get(0),
        )?;
        if jira_sem_sync && (cur.is_none() || reassigned) {
            self.ensure_env_row(&env_id)?;
        }
        let sync_env = jira_sem_sync
            && (cur.is_none() || reassigned)
            && self.conn.execute(
                "UPDATE project SET sync_env_id = ?2,
                        rev = (SELECT rev FROM assignment WHERE project_id = ?1 AND env_id = ?2)
                 WHERE id = ?1 AND tracker_kind = 'jira' AND sync_env_id IS NULL",
                params![pid, env_id],
            )? > 0;
        let assignment = self
            .find_assignment(pid, &env_id)?
            .ok_or_else(|| LedgerError::Corrupt("atribuição recém-gravada sumiu".into()))?;
        tx.commit()?;
        Ok(AssignmentAdded {
            assignment,
            changed,
            reassigned,
            sync_env,
        })
    }

    /// O que a remoção da atribuição atinge: a atribuição ativa e as sessões
    /// vivas relatadas pelo ambiente nesse projeto.
    pub fn assignment_impact(
        &self,
        owner: &str,
        repo: &str,
        env: &str,
    ) -> Result<(HubAssignment, Vec<HubSession>)> {
        let pid = self.live_project(owner, repo)?;
        let env_id = self.resolve_env(env)?;
        let a = self.find_assignment(pid, &env_id)?.ok_or_else(|| {
            LedgerError::NotFound(
                "atribuição",
                format!("{owner}/{repo} não está atribuído a {env}"),
            )
        })?;
        let live = self.live_sessions(pid, Some(&env_id))?;
        Ok((a, live))
    }

    /// Remove a atribuição (`hub.assignments.remove`): grava `removed_at` e
    /// `rev` novo. Devolve o `config_rev`. A confirmação (sessões vivas) é de
    /// quem chama; o ambiente nunca encerra sessão por isso.
    pub fn remove_assignment(&self, owner: &str, repo: &str, env: &str) -> Result<i64> {
        let (a, _) = self.assignment_impact(owner, repo, env)?;
        let tx = self.conn.unchecked_transaction()?;
        let rev = self.bump_config_rev()?;
        self.conn.execute(
            "UPDATE assignment SET removed_at = ?3, rev = ?4
             WHERE project_id = ?1 AND env_id = ?2 AND removed_at IS NULL",
            params![a.project_id, a.env_id, now_ms(), rev],
        )?;
        // Era o ambiente de sync: o projeto fica sem, e o hub não escolhe
        // outro sozinho (§5.1); a saúde avisa (`rastreador_sem_sync`).
        self.conn.execute(
            "UPDATE project SET sync_env_id = NULL, rev = ?3 WHERE id = ?1 AND sync_env_id = ?2",
            params![a.project_id, a.env_id, rev],
        )?;
        tx.commit()?;
        Ok(rev)
    }

    /// `rev` e `removed_at` da linha de atribuição (ativa ou removida).
    pub fn assignment_row(
        &self,
        owner: &str,
        repo: &str,
        env_id: &str,
    ) -> Result<Option<(i64, Option<i64>)>> {
        Ok(self
            .conn
            .query_row(
                "SELECT a.rev, a.removed_at FROM assignment a
                 JOIN project p ON p.id = a.project_id
                 WHERE p.key = ?1 AND a.env_id = ?2",
                params![project_key(owner, repo), env_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?)
    }

    /// Configuração do ambiente para `GET /sync/config?since=<rev>` (spec
    /// §4.1). `since` igual ao `config_rev` atual: `changed: false`. Senão,
    /// o conjunto **completo** dos projetos atribuídos a `env_id` (o `rev`
    /// de cada um é o maior entre o do projeto e o da atribuição) e as
    /// atribuições deste ambiente removidas depois de `since` (inclusive as
    /// de projetos excluídos). Projetos não atribuídos nunca aparecem.
    ///
    /// `epoch` é a época que o ambiente recebeu no último pull: diferente da
    /// atual (hub restaurado de snapshot) ou ausente = conjunto completo,
    /// mesmo com `since` igual ao `config_rev`.
    pub fn env_config(
        &self,
        env_id: &str,
        since: Option<i64>,
        epoch: Option<i64>,
    ) -> Result<ConfigResponse> {
        self.env_config_with(env_id, since, epoch, false)
    }

    /// Work items `concluido` dos projetos atribuídos ao ambiente (`GET
    /// /sync/done`, #298): o conjunto completo, sem delta.
    pub fn env_done(&self, env_id: &str) -> Result<DoneResponse> {
        let mut stmt = self.conn.prepare(
            "SELECT p.gh_owner, p.gh_repo, w.gh_number, w.closed_at
             FROM work_item w
             JOIN project p ON p.id = w.project_id
             JOIN assignment a ON a.project_id = p.id
             WHERE a.env_id = ?1 AND a.removed_at IS NULL AND p.deleted_at IS NULL
               AND w.status = 'concluido' AND w.gh_number > 0
             ORDER BY p.key, w.gh_number",
        )?;
        let items = stmt
            .query_map([env_id], |r| {
                Ok(ConfigDone {
                    owner: r.get(0)?,
                    repo: r.get(1)?,
                    number: r.get(2)?,
                    since: r.get(3)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(DoneResponse { items })
    }

    /// [`Hub::env_config`] com `force`: responde `changed: true` (conjunto
    /// completo) mesmo com `since` e `epoch` em dia. Usado quando a chave
    /// pública do ambiente mudou desde a última entrega de secrets (#184);
    /// `removed` continua a partir de `since`.
    pub fn env_config_with(
        &self,
        env_id: &str,
        since: Option<i64>,
        epoch: Option<i64>,
        force: bool,
    ) -> Result<ConfigResponse> {
        let config_rev = self.config_rev()?;
        let config_epoch = self.config_epoch()?;
        if !force && since == Some(config_rev) && epoch == Some(config_epoch) {
            return Ok(ConfigResponse {
                config_rev,
                config_epoch,
                changed: false,
                projects: None,
                removed: None,
            });
        }
        // Outra época: o `since` do ambiente não vale para este hub.
        let since = since.filter(|_| epoch == Some(config_epoch));
        let projects = {
            let mut stmt = self.conn.prepare(
                "SELECT p.gh_owner, p.gh_repo, p.name, p.base_branch, p.protected_branches,
                        a.repo_path, max(a.rev, p.rev), p.tracker_kind, p.tracker_config,
                        p.forge_kind, p.forge_config, p.branch_pattern, p.attribution,
                        p.sync_env_id IS ?1, p.id, p.docs_config
                 FROM assignment a JOIN project p ON p.id = a.project_id
                 WHERE a.env_id = ?1 AND a.removed_at IS NULL AND p.deleted_at IS NULL
                 ORDER BY p.key",
            )?;
            let rows = stmt.query_map([env_id], |r| {
                let id: i64 = r.get(14)?;
                let p = ConfigProject {
                    owner: r.get(0)?,
                    repo: r.get(1)?,
                    name: r.get(2)?,
                    base_branch: r.get(3)?,
                    protected_branches: crate::json_col(r, 4)?,
                    repo_path: r.get(5)?,
                    rev: r.get(6)?,
                    tracker: crate::tracker_col(r, 7)?,
                    forge: crate::forge_col(r, 9)?,
                    branch_pattern: r.get(11)?,
                    attribution: r.get(12)?,
                    sync: r.get(13)?,
                    rules: Default::default(),
                    docs: crate::json_col(r, 15)?,
                };
                Ok((id, p))
            })?;
            let rows = rows.collect::<rusqlite::Result<Vec<_>>>()?;
            let mut out = Vec::with_capacity(rows.len());
            for (id, mut p) in rows {
                // Regras do agente vigentes (#357): mudar incrementa o
                // `config_rev`, e o conjunto vai inteiro a cada mudança.
                p.rules = self.config_rules(id)?;
                out.push(p);
            }
            out
        };
        let removed = {
            let mut stmt = self.conn.prepare(
                "SELECT p.gh_owner, p.gh_repo, a.rev
                 FROM assignment a JOIN project p ON p.id = a.project_id
                 WHERE a.env_id = ?1 AND a.removed_at IS NOT NULL AND a.rev > ?2
                 ORDER BY a.rev",
            )?;
            let rows = stmt.query_map(params![env_id, since.unwrap_or(0)], |r| {
                Ok(ConfigRemoved {
                    owner: r.get(0)?,
                    repo: r.get(1)?,
                    rev: r.get(2)?,
                })
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        Ok(ConfigResponse {
            config_rev,
            config_epoch,
            changed: true,
            projects: Some(projects),
            removed: Some(removed),
        })
    }
}
