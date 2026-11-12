//! Checks do head dos PRs abertos no espelho (#129, épico #136): por PR, a
//! situação geral e, por check (check run do GitHub ou commit status), o
//! nome, a conclusão, a URL e o `job_id` do Actions. A tabela `pr_checks`
//! existe no `ledger.db` e no `hub.db`; o sync do GitHub a preenche (só os
//! PRs abertos de work items e o da branch de integração do modo autônomo)
//! e a limpa quando o PR fecha.
//!
//! Leitura para quem consome: [`crate::Ledger::pr_checks`],
//! [`crate::Ledger::pr_checks_for_work_item`] e os mesmos no
//! [`crate::Hub`], além de [`crate::Hub::pr_checks_by_head`].

use crate::github::GhMirror;
use crate::{json_col, PrRow, Result};
use rusqlite::{params, OptionalExtension, Row};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Situação geral dos checks do head de um PR. Falha vence: um check falho
/// faz o PR ficar em `failure` mesmo com outros ainda em andamento. Sem
/// nenhum check, `pending` (o CI pode ainda não ter começado).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum ChecksState {
    Pending,
    Success,
    Failure,
}

impl ChecksState {
    pub fn as_str(self) -> &'static str {
        match self {
            ChecksState::Pending => "pending",
            ChecksState::Success => "success",
            ChecksState::Failure => "failure",
        }
    }
}

/// De onde veio o check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum CheckSource {
    /// `GET /commits/{sha}/check-runs` (Actions e apps de checks).
    CheckRun,
    /// `GET /commits/{sha}/status` (commit status, o modelo antigo).
    Status,
}

/// Um check do head do PR.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct PrCheck {
    /// `name` do check run ou `context` do status.
    pub name: String,
    pub source: CheckSource,
    /// Conclusão como vem do GitHub (`success`, `failure`, `neutral`,
    /// `skipped`, `cancelled`, `timed_out`, `action_required`; no status,
    /// `success`, `failure` ou `error`). `None` = ainda em andamento.
    pub conclusion: Option<String>,
    /// Página do check (`html_url` ou `details_url`; no status, `target_url`).
    pub url: Option<String>,
    /// Job do GitHub Actions, para buscar o log
    /// (`ct_gh::GhClient::job_log_tail`). `None` fora do Actions.
    pub job_id: Option<i64>,
}

impl PrCheck {
    /// Situação deste check: `success`, `neutral` e `skipped` passam; sem
    /// conclusão (ou `stale`) está em andamento; o resto falhou.
    pub fn state(&self) -> ChecksState {
        match self.conclusion.as_deref() {
            None | Some("stale") => ChecksState::Pending,
            Some("success" | "neutral" | "skipped") => ChecksState::Success,
            Some(_) => ChecksState::Failure,
        }
    }
}

/// Situação geral de uma lista de checks (falha vence; vazia = pendente).
pub fn checks_state(checks: &[PrCheck]) -> ChecksState {
    let states: Vec<ChecksState> = checks.iter().map(PrCheck::state).collect();
    if states.contains(&ChecksState::Failure) {
        ChecksState::Failure
    } else if states.is_empty() || states.contains(&ChecksState::Pending) {
        ChecksState::Pending
    } else {
        ChecksState::Success
    }
}

/// Os checks do head de um PR no espelho.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct PrChecks {
    pub pr_number: i64,
    pub head: String,
    pub base: String,
    /// SHA a que os checks se referem (o `head.sha` do PR na leitura).
    pub head_sha: String,
    pub state: ChecksState,
    /// Check runs e depois os statuses, cada grupo na ordem da API.
    pub checks: Vec<PrCheck>,
    /// Última leitura que mudou algo (ms).
    pub synced_at: i64,
    /// ETag da última leitura dos check runs deste SHA (só para o sync).
    #[serde(skip)]
    #[cfg_attr(feature = "ts", ts(skip))]
    pub runs_etag: Option<String>,
    /// ETag da última leitura do status combinado deste SHA (só para o sync).
    #[serde(skip)]
    #[cfg_attr(feature = "ts", ts(skip))]
    pub status_etag: Option<String>,
}

impl PrChecks {
    /// Todos os checks já concluíram (nenhum em andamento). Sem checks,
    /// `false`: o CI pode não ter começado.
    pub fn completed(&self) -> bool {
        !self.checks.is_empty()
            && self
                .checks
                .iter()
                .all(|c| c.state() != ChecksState::Pending)
    }

    /// Os checks que falharam, na ordem guardada.
    pub fn failed(&self) -> impl Iterator<Item = &PrCheck> {
        self.checks
            .iter()
            .filter(|c| c.state() == ChecksState::Failure)
    }

    pub fn summary(&self) -> PrChecksSummary {
        PrChecksSummary::of(&self.checks)
    }
}

/// Resumo dos checks de um PR para o hub e a UI ("3 checks, 1 falhou").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct PrChecksSummary {
    pub state: ChecksState,
    pub total: u32,
    pub failed: u32,
    pub pending: u32,
    /// Texto pronto: "sem checks", "3 checks, 1 falhou", "2 checks, 1 em
    /// andamento", "3 checks, todos passaram".
    pub text: String,
}

impl PrChecksSummary {
    pub fn of(checks: &[PrCheck]) -> Self {
        let count = |s| checks.iter().filter(|c| c.state() == s).count() as u32;
        let (total, failed, pending) = (
            checks.len() as u32,
            count(ChecksState::Failure),
            count(ChecksState::Pending),
        );
        let text = if total == 0 {
            "sem checks".to_string()
        } else {
            let mut parts = vec![format!(
                "{total} {}",
                if total == 1 { "check" } else { "checks" }
            )];
            if failed > 0 {
                parts.push(format!(
                    "{failed} {}",
                    if failed == 1 { "falhou" } else { "falharam" }
                ));
            }
            if pending > 0 {
                parts.push(format!("{pending} em andamento"));
            }
            if failed == 0 && pending == 0 {
                parts.push(
                    if total == 1 {
                        "passou"
                    } else {
                        "todos passaram"
                    }
                    .into(),
                );
            }
            parts.join(", ")
        };
        PrChecksSummary {
            state: checks_state(checks),
            total,
            failed,
            pending,
            text,
        }
    }
}

/// Uma leitura de checks que mudou (`200`): os checks de uma fonte e o ETag.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ChecksRead {
    pub checks: Vec<PrCheck>,
    pub etag: Option<String>,
}

/// O que o sync leu dos checks de um PR nesta rodada. `None` numa fonte =
/// `304` (fica o que estava gravado para o mesmo SHA).
#[derive(Debug, Clone, PartialEq)]
pub struct PrChecksFetch {
    pub pr_number: i64,
    pub head: String,
    pub base: String,
    pub head_sha: String,
    pub runs: Option<ChecksRead>,
    pub statuses: Option<ChecksRead>,
}

/// Checks da rodada do sync, no [`crate::GithubSnapshot`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PrChecksSnapshot {
    /// PRs abertos observados nesta rodada: os checks dos outros saem do
    /// espelho (PR fechado, mesclado ou de work item concluído).
    pub open: Vec<i64>,
    /// Leituras que mudaram algo.
    pub updates: Vec<PrChecksFetch>,
}

const CHECKS_COLS: &str =
    "pr_number, head, base, head_sha, state, checks, synced_at, runs_etag, status_etag";

fn row_to_checks(r: &Row) -> rusqlite::Result<PrChecks> {
    let checks: Vec<PrCheck> = json_col(r, 5)?;
    Ok(PrChecks {
        pr_number: r.get(0)?,
        head: r.get(1)?,
        base: r.get(2)?,
        head_sha: r.get(3)?,
        // Recalculada dos checks: a coluna `state` serve só a consultas SQL.
        state: checks_state(&checks),
        checks,
        synced_at: r.get(6)?,
        runs_etag: r.get(7)?,
        status_etag: r.get(8)?,
    })
}

impl GhMirror<'_> {
    pub(crate) fn pr_checks(&self, project_id: i64, number: i64) -> Result<Option<PrChecks>> {
        Ok(self
            .0
            .query_row(
                &format!(
                    "SELECT {CHECKS_COLS} FROM pr_checks WHERE project_id = ?1 AND pr_number = ?2"
                ),
                params![project_id, number],
                row_to_checks,
            )
            .optional()?)
    }

    pub(crate) fn list_pr_checks(&self, project_id: i64) -> Result<Vec<PrChecks>> {
        let mut stmt = self.0.prepare(&format!(
            "SELECT {CHECKS_COLS} FROM pr_checks WHERE project_id = ?1 ORDER BY pr_number"
        ))?;
        let rows = stmt.query_map([project_id], row_to_checks)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Checks dos PRs abertos do work item, do PR mais recente ao mais antigo.
    pub(crate) fn pr_checks_for_work_item(&self, work_item_id: i64) -> Result<Vec<PrChecks>> {
        let cols = CHECKS_COLS
            .split(", ")
            .map(|c| format!("c.{}", c.trim()))
            .collect::<Vec<_>>()
            .join(", ");
        let mut stmt = self.0.prepare(&format!(
            "SELECT {cols} FROM pr_checks c
             JOIN pr_cache p ON p.project_id = c.project_id AND p.number = c.pr_number
             WHERE p.work_item_id = ?1 AND p.state = 'open'
             ORDER BY c.pr_number DESC"
        ))?;
        let rows = stmt.query_map([work_item_id], row_to_checks)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Checks do PR aberto `head → base` (o da branch de integração do modo
    /// autônomo, que não fica no `pr_cache`); o de número maior, se houver
    /// mais de um.
    pub(crate) fn pr_checks_by_head(
        &self,
        project_id: i64,
        head: &str,
        base: &str,
    ) -> Result<Option<PrChecks>> {
        Ok(self
            .0
            .query_row(
                &format!(
                    "SELECT {CHECKS_COLS} FROM pr_checks
                     WHERE project_id = ?1 AND head = ?2 AND base = ?3
                     ORDER BY pr_number DESC LIMIT 1"
                ),
                params![project_id, head, base],
                row_to_checks,
            )
            .optional()?)
    }

    /// Resumo dos checks do PR, se ele está aberto e tem checks lidos.
    pub(crate) fn pr_checks_summary(
        &self,
        project_id: i64,
        pr: Option<&PrRow>,
    ) -> Result<Option<PrChecksSummary>> {
        let Some(pr) = pr.filter(|p| p.state == ct_core::PrState::Open) else {
            return Ok(None);
        };
        Ok(self.pr_checks(project_id, pr.number)?.map(|c| c.summary()))
    }

    /// Aplica os checks da rodada: tira os dos PRs que não estão mais em
    /// `open` e grava as leituras que mudaram. Uma fonte em `304` mantém os
    /// checks dela gravados para o mesmo SHA; um SHA novo começa do zero.
    pub(crate) fn apply_checks(
        &self,
        project_id: i64,
        s: &PrChecksSnapshot,
        synced_at: i64,
    ) -> Result<()> {
        let open: BTreeSet<i64> = s.open.iter().copied().collect();
        for old in self.list_pr_checks(project_id)? {
            if !open.contains(&old.pr_number) {
                self.0.execute(
                    "DELETE FROM pr_checks WHERE project_id = ?1 AND pr_number = ?2",
                    params![project_id, old.pr_number],
                )?;
            }
        }
        for u in s.updates.iter().filter(|u| open.contains(&u.pr_number)) {
            let prev = self
                .pr_checks(project_id, u.pr_number)?
                .filter(|p| p.head_sha == u.head_sha);
            let keep = |source: CheckSource| -> Vec<PrCheck> {
                prev.iter()
                    .flat_map(|p| p.checks.iter())
                    .filter(|c| c.source == source)
                    .cloned()
                    .collect()
            };
            let (runs, runs_etag) = match &u.runs {
                Some(r) => (r.checks.clone(), r.etag.clone()),
                None => (
                    keep(CheckSource::CheckRun),
                    prev.as_ref().and_then(|p| p.runs_etag.clone()),
                ),
            };
            let (statuses, status_etag) = match &u.statuses {
                Some(r) => (r.checks.clone(), r.etag.clone()),
                None => (
                    keep(CheckSource::Status),
                    prev.as_ref().and_then(|p| p.status_etag.clone()),
                ),
            };
            let checks: Vec<PrCheck> = runs.into_iter().chain(statuses).collect();
            let json = serde_json::to_string(&checks).expect("Vec<PrCheck> serializa");
            self.0.execute(
                "INSERT INTO pr_checks (project_id, pr_number, head, base, head_sha, state, checks,
                                        synced_at, runs_etag, status_etag)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                 ON CONFLICT (project_id, pr_number) DO UPDATE SET
                    head = excluded.head, base = excluded.base, head_sha = excluded.head_sha,
                    state = excluded.state, checks = excluded.checks,
                    synced_at = excluded.synced_at, runs_etag = excluded.runs_etag,
                    status_etag = excluded.status_etag",
                params![
                    project_id,
                    u.pr_number,
                    u.head,
                    u.base,
                    u.head_sha,
                    checks_state(&checks).as_str(),
                    json,
                    synced_at,
                    runs_etag,
                    status_etag
                ],
            )?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(name: &str, conclusion: Option<&str>) -> PrCheck {
        PrCheck {
            name: name.into(),
            source: CheckSource::CheckRun,
            conclusion: conclusion.map(String::from),
            url: None,
            job_id: None,
        }
    }

    #[test]
    fn situacao_e_resumo() {
        let s = PrChecksSummary::of(&[]);
        assert_eq!(
            (s.state, s.text.as_str()),
            (ChecksState::Pending, "sem checks")
        );
        let s = PrChecksSummary::of(&[
            check("build", Some("success")),
            check("lint", Some("skipped")),
            check("test", Some("failure")),
        ]);
        assert_eq!((s.total, s.failed, s.pending), (3, 1, 0));
        assert_eq!(
            (s.state, s.text.as_str()),
            (ChecksState::Failure, "3 checks, 1 falhou")
        );
        let s = PrChecksSummary::of(&[check("build", Some("success")), check("test", None)]);
        assert_eq!(s.state, ChecksState::Pending);
        assert_eq!(s.text, "2 checks, 1 em andamento");
        let s = PrChecksSummary::of(&[check("build", Some("success"))]);
        assert_eq!(
            (s.state, s.text.as_str()),
            (ChecksState::Success, "1 check, passou")
        );
        // Falha vence o que ainda roda.
        let s = PrChecksSummary::of(&[check("a", Some("timed_out")), check("b", None)]);
        assert_eq!(s.state, ChecksState::Failure);
        assert_eq!(s.text, "2 checks, 1 falhou, 1 em andamento");
    }
}
