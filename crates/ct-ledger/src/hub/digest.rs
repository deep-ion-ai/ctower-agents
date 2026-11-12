//! Resumo da manhã do piloto automático (#185, épico #178) e a situação da
//! branch de integração de cada projeto autônomo.
//!
//! Quem decide quando o resumo sai é o `cthubd` (pela política,
//! `BudgetPolicy::digest_due`); aqui ficam os dados que ele junta do
//! espelho e do ledger do hub e o resumo guardado (migração 37, um por dia
//! local: `local_day` é único, então ele nunca sai duas vezes no mesmo dia).
//!
//! **Sem texto do agente:** o resumo leva só número e título das issues
//! (do GitHub), contagens, a conta, o uso e links. As mensagens das
//! decisões, dos eventos e das sessões nunca entram, e a notificação da Web
//! Push ([`digest_push_body`]) leva só o projeto e as contagens.
//!
//! **Issues à frente da base:** as do projeto em `em_revisao` com um PR
//! mesclado na branch de integração (o espelho `pr_cache`, `base` = a
//! branch). Quando o PR da branch para a base é mesclado, elas vão a
//! `concluido` e saem da conta. O PR `wip/autonoma → base` em si não fica
//! no `pr_cache` (não é de work item): o link vem do relato do ambiente do
//! modo (`HubProject.autonomy_report.pr_url`). Os checks dele, sim, ficam no
//! espelho (`pr_checks`, #129), lidos pelo sync do hub: o `ci` é o resumo.

use super::{Hub, HubProject};
use crate::Result;
use ct_core::{AutonomyState, WorkItemStatus};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

/// Resumos guardados além do mais recente (o painel mostra só os últimos).
const DIGEST_KEEP: i64 = 60;

/// Uma issue citada no resumo: número e título do espelho (nunca texto do
/// agente).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct DigestIssue {
    pub number: i64,
    /// `None` = fora do espelho.
    pub title: Option<String>,
}

/// A branch de integração de um projeto autônomo
/// (`hub.autopilot.wip`, Visão geral do projeto e o resumo da manhã).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct AutonomyWip {
    pub owner: String,
    pub repo: String,
    pub name: String,
    /// Branch de integração (`wip/autonoma` no padrão) e a base.
    pub branch: String,
    pub base: String,
    /// Issues mescladas na branch que ainda não chegaram à base.
    pub ahead: Vec<DigestIssue>,
    /// PR rascunho `branch → base`, do relato do ambiente do modo.
    pub pr_url: Option<String>,
    /// Resumo dos checks do PR `branch → base` no espelho do hub (#129),
    /// como "3 checks, 1 falhou"; `None` = sem PR aberto ou checks ainda não
    /// lidos.
    pub ci: Option<String>,
    /// A branch está em conflito com a base (relato do ambiente).
    pub conflict: bool,
    /// Mensagem do relato `error` da branch (ex.: o push recusado); `None`
    /// sem erro. Com erro, o agendador não inicia issues no projeto.
    #[serde(default)]
    pub error: Option<String>,
}

/// Um projeto autônomo no resumo da manhã.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct DigestProject {
    pub owner: String,
    pub repo: String,
    pub name: String,
    pub branch: String,
    pub base: String,
    /// Issues com PR mesclado na branch de integração desde o resumo
    /// anterior.
    pub merged: Vec<DigestIssue>,
    /// Issues em `bloqueado` agora.
    pub blocked: Vec<DigestIssue>,
    /// Decisões autônomas ainda sem revisão (todas) e as criadas no período.
    pub decisions_pending: u32,
    pub decisions_new: u32,
    /// Issues à frente da base (mescladas na branch, ainda não na base).
    pub ahead: u32,
    pub pr_url: Option<String>,
    /// Resumo dos checks do PR `branch → base` (#129), como em
    /// [`AutonomyWip::ci`].
    pub ci: Option<String>,
    pub conflict: bool,
    /// Mensagem do relato `error` da branch de integração; `None` sem erro.
    #[serde(default)]
    pub error: Option<String>,
}

/// Uso da semanal de uma conta com política no período do resumo.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct DigestUsage {
    /// Chave da conta; na web, `conta-N`.
    pub account: String,
    /// Uso da semanal agora (0 se renovou); `None` = sem leitura.
    pub weekly_used: Option<f64>,
    /// Reset da semanal considerado agora.
    pub resets_at: Option<i64>,
    /// Uso no resumo anterior, se houve.
    pub weekly_before: Option<f64>,
    /// Quanto subiu no período: a diferença, com o mesmo reset; renovada
    /// no meio, o uso atual (desde o reset). No primeiro resumo (sem
    /// anterior) não há base para a diferença: também o uso atual da
    /// semanal. `None` sem leitura.
    pub delta: Option<f64>,
}

/// O resumo da manhã (`hub.autopilot.digest`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct MorningDigest {
    /// Ignorado ao gravar.
    pub id: i64,
    /// Dia local (no fuso da política) a que o resumo pertence.
    pub local_day: i64,
    /// Quando saiu (ms) e o início do período (o resumo anterior ou, no
    /// primeiro, 24 h antes).
    pub at: i64,
    pub since: i64,
    pub projects: Vec<DigestProject>,
    pub usage: Vec<DigestUsage>,
}

impl MorningDigest {
    /// Algo a revisar: issue mesclada, bloqueada ou decisão pendente.
    pub fn has_news(&self) -> bool {
        self.projects.iter().any(|p| {
            !p.merged.is_empty() || !p.blocked.is_empty() || p.decisions_pending > 0 || p.ahead > 0
        })
    }
}

/// Corpo da notificação da Web Push do resumo: **só** o projeto e as
/// contagens (o link vai no `url` da notificação), nunca título de issue,
/// conta nem mensagem do agente.
pub fn digest_push_body(d: &MorningDigest) -> String {
    let parts: Vec<String> = d
        .projects
        .iter()
        .filter_map(|p| {
            let mut c = Vec::new();
            let n = |v: usize, one: &str, many: &str| {
                format!("{v} {}", if v == 1 { one } else { many })
            };
            if !p.merged.is_empty() {
                c.push(n(p.merged.len(), "mesclada na wip", "mescladas na wip"));
            }
            if !p.blocked.is_empty() {
                c.push(n(p.blocked.len(), "bloqueada", "bloqueadas"));
            }
            if p.decisions_pending > 0 {
                c.push(n(
                    p.decisions_pending as usize,
                    "decisão para revisar",
                    "decisões para revisar",
                ));
            }
            if p.ahead > 0 {
                c.push(n(p.ahead as usize, "à frente da base", "à frente da base"));
            }
            (!c.is_empty()).then(|| format!("{}: {}", p.name, c.join(", ")))
        })
        .collect();
    if parts.is_empty() {
        "Nada novo para revisar no piloto automático".into()
    } else {
        parts.join(" · ")
    }
}

/// Mensagem do relato `error` da branch de integração (`None` sem erro).
fn report_error(report: Option<&ct_core::AutonomyReport>) -> Option<String> {
    report.filter(|r| r.state == AutonomyState::Error).map(|r| {
        r.message
            .clone()
            .unwrap_or_else(|| format!("erro em {}", r.branch))
    })
}

/// Branch de integração (normalizada) e base do projeto autônomo.
fn branches(p: &HubProject) -> (String, String) {
    (
        p.autonomy.branch().to_string(),
        p.autonomy.base(&p.base_branch).to_string(),
    )
}

impl Hub {
    fn digest_issues(&self, sql: &str, args: impl rusqlite::Params) -> Result<Vec<DigestIssue>> {
        let mut stmt = self.conn.prepare(sql)?;
        let rows = stmt.query_map(args, |r| {
            Ok(DigestIssue {
                number: r.get(0)?,
                title: r.get(1)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Issues mescladas na branch de integração e ainda em revisão (à
    /// frente da base).
    fn ahead_of_base(&self, project_id: i64, branch: &str) -> Result<Vec<DigestIssue>> {
        self.digest_issues(
            "SELECT DISTINCT w.gh_number, ic.title FROM pr_cache p
             JOIN work_item w ON w.id = p.work_item_id
             LEFT JOIN issue_cache ic ON ic.project_id = w.project_id
                                     AND ic.gh_number = w.gh_number
             WHERE p.project_id = ?1 AND p.base = ?2 AND p.state = 'merged'
               AND w.status = ?3
             ORDER BY w.gh_number",
            params![project_id, branch, WorkItemStatus::EmRevisao.as_str()],
        )
    }

    /// Resumo dos checks do PR `branch → base` no espelho (#129).
    fn wip_ci(&self, project_id: i64, branch: &str, base: &str) -> Result<Option<String>> {
        Ok(self
            .pr_checks_by_head(project_id, branch, base)?
            .map(|c| c.summary().text))
    }

    /// Projetos com o modo autônomo ligado, na ordem do hub; com `only`,
    /// só aquele (`owner`, `repo`).
    fn autonomous_projects(&self, only: Option<(&str, &str)>) -> Result<Vec<HubProject>> {
        Ok(self
            .list_projects()?
            .into_iter()
            .filter(|p| p.autonomy.enabled)
            .filter(|p| only.is_none_or(|(o, r)| p.gh_owner == o && p.gh_repo == r))
            .collect())
    }

    /// A branch de integração de cada projeto autônomo (`hub.autopilot.wip`).
    pub fn autonomy_wip(&self, only: Option<(&str, &str)>) -> Result<Vec<AutonomyWip>> {
        let mut out = Vec::new();
        for p in self.autonomous_projects(only)? {
            let (branch, base) = branches(&p);
            let report = p.autonomy_report.as_ref();
            out.push(AutonomyWip {
                ahead: self.ahead_of_base(p.id, &branch)?,
                pr_url: report.and_then(|r| r.pr_url.clone()),
                ci: self.wip_ci(p.id, &branch, &base)?,
                conflict: report.is_some_and(|r| r.state == AutonomyState::Conflict),
                error: report_error(report),
                owner: p.gh_owner,
                repo: p.gh_repo,
                name: p.name,
                branch,
                base,
            });
        }
        Ok(out)
    }

    /// Os projetos do resumo da manhã: o período é `(since, now]`.
    pub fn digest_projects(&self, since: i64, now: i64) -> Result<Vec<DigestProject>> {
        let mut out = Vec::new();
        for p in self.autonomous_projects(None)? {
            let (branch, base) = branches(&p);
            // PR da issue mesclado na branch no período (o `updated_at` do
            // PR mesclado é a hora do merge, como em `merged_into`).
            let merged = self.digest_issues(
                "SELECT DISTINCT w.gh_number, ic.title FROM pr_cache pr
                 JOIN work_item w ON w.id = pr.work_item_id
                 LEFT JOIN issue_cache ic ON ic.project_id = w.project_id
                                         AND ic.gh_number = w.gh_number
                 WHERE pr.project_id = ?1 AND pr.base = ?2 AND pr.state = 'merged'
                   AND pr.updated_at > ?3 AND pr.updated_at <= ?4
                 ORDER BY w.gh_number",
                params![p.id, branch, since, now],
            )?;
            let blocked = self.digest_issues(
                "SELECT w.gh_number, ic.title FROM work_item w
                 LEFT JOIN issue_cache ic ON ic.project_id = w.project_id
                                         AND ic.gh_number = w.gh_number
                 WHERE w.project_id = ?1 AND w.status = ?2
                 ORDER BY w.gh_number",
                params![p.id, WorkItemStatus::Bloqueado.as_str()],
            )?;
            let (pending, new): (u32, u32) = self.conn.query_row(
                "SELECT count(*), coalesce(sum(d.created_at > ?2), 0)
                 FROM decision d JOIN work_item w ON w.id = d.work_item_id
                 WHERE w.project_id = ?1 AND d.origin = 'autonoma' AND d.reviewed_at IS NULL",
                params![p.id, since],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            let ahead = self.ahead_of_base(p.id, &branch)?.len() as u32;
            let report = p.autonomy_report.as_ref();
            out.push(DigestProject {
                merged,
                blocked,
                decisions_pending: pending,
                decisions_new: new,
                ahead,
                pr_url: report.and_then(|r| r.pr_url.clone()),
                ci: self.wip_ci(p.id, &branch, &base)?,
                conflict: report.is_some_and(|r| r.state == AutonomyState::Conflict),
                error: report_error(report),
                owner: p.gh_owner,
                repo: p.gh_repo,
                name: p.name,
                branch,
                base,
            });
        }
        Ok(out)
    }

    /// Dia local do último resumo enviado.
    pub fn last_digest_day(&self) -> Result<Option<i64>> {
        Ok(self
            .conn
            .query_row("SELECT max(local_day) FROM autopilot_digest", [], |r| {
                r.get(0)
            })
            .optional()?
            .flatten())
    }

    /// Grava o resumo do dia; `None` se o dia já tinha resumo (nunca sai
    /// duas vezes). Apaga os mais velhos que os [`DIGEST_KEEP`] últimos.
    pub fn save_digest(&self, d: &MorningDigest) -> Result<Option<MorningDigest>> {
        let content = serde_json::to_string(d).expect("MorningDigest serializa");
        let n = self.conn.execute(
            "INSERT OR IGNORE INTO autopilot_digest (local_day, at, since, content)
             VALUES (?1, ?2, ?3, ?4)",
            params![d.local_day, d.at, d.since, content],
        )?;
        if n == 0 {
            return Ok(None);
        }
        let id = self.conn.last_insert_rowid();
        self.conn.execute(
            "DELETE FROM autopilot_digest WHERE id NOT IN
               (SELECT id FROM autopilot_digest ORDER BY local_day DESC LIMIT ?1)",
            [DIGEST_KEEP],
        )?;
        Ok(Some(MorningDigest { id, ..d.clone() }))
    }

    /// Os resumos mais recentes, do mais novo ao mais velho.
    pub fn list_digests(&self, limit: u32) -> Result<Vec<MorningDigest>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, content FROM autopilot_digest ORDER BY local_day DESC, id DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map([limit.clamp(1, DIGEST_KEEP as u32)], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (id, raw) = row?;
            if let Ok(d) = serde_json::from_str::<MorningDigest>(&raw) {
                out.push(MorningDigest { id, ..d });
            }
        }
        Ok(out)
    }

    /// Resumos gravados desde `since` (para a Web Push).
    pub(super) fn digests_since(&self, since: i64) -> Result<Vec<MorningDigest>> {
        Ok(self
            .list_digests(DIGEST_KEEP as u32)?
            .into_iter()
            .filter(|d| d.at >= since)
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        GithubSnapshot, HubProjectPatch, IssueRow, NewHubProject, OutboxEntry, PrRow,
        ProjectChange, SyncChange,
    };
    use ct_core::{AutonomyConfig, AutonomyReport, PrState};

    /// Texto que só o agente escreveria: nunca pode sair no resumo.
    const AGENTE: &str = "MENSAGEM-DO-AGENTE";

    fn issue(n: i64) -> IssueRow {
        IssueRow {
            gh_number: n,
            title: format!("ACME-{n}"),
            state: "open".into(),
            url: String::new(),
            updated_at: 1,
            ..Default::default()
        }
    }

    fn pr(n: i64, wi: i64, base: &str, state: PrState, at: i64) -> PrRow {
        PrRow {
            number: n,
            work_item_id: wi,
            head: format!("feat/{n}"),
            base: base.into(),
            state,
            url: format!("https://github.example/acme/acme-web/pull/{n}"),
            updated_at: at,
            head_sha: None,
        }
    }

    /// `acme/acme-web` no modo autônomo em `env-auto`, com o relato do PR
    /// rascunho, as issues 123 a 126 no espelho e: a 123 mesclada na wip
    /// em `t=100`, a 124 mesclada em `t=10`, a 125 com PR aberto para a wip
    /// e a 126 bloqueada; uma decisão autônoma sem revisão (`t=150`) e uma
    /// revisada.
    fn hub() -> Hub {
        let hub = Hub::open_in_memory().unwrap();
        hub.ingest("env-auto", "env-auto", &[]).unwrap();
        let p = hub
            .add_project(&NewHubProject {
                owner: "acme".into(),
                repo: "acme-web".into(),
                name: "acme-web".into(),
                base_branch: "develop".into(),
                protected_branches: vec!["develop".into(), "main".into()],
            })
            .unwrap()
            .project;
        hub.set_env_autonomous("env-auto", true).unwrap();
        hub.add_assignment("acme", "acme-web", "env-auto", None)
            .unwrap();
        hub.update_project(
            "acme",
            "acme-web",
            &HubProjectPatch {
                autonomy: Some(AutonomyConfig {
                    enabled: true,
                    env_id: Some("env-auto".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
        )
        .unwrap();
        let report = OutboxEntry {
            seq: 1,
            created_at: 1,
            change: SyncChange::Project(ProjectChange {
                owner: "acme".into(),
                repo: "acme-web".into(),
                name: "acme-web".into(),
                repo_path: "/tmp/acme-web".into(),
                base_branch: "develop".into(),
                protected_branches: vec![],
                created_at: 1,
                hlc: 1,
                state: None,
                error: None,
                applied_rev: Some(1),
                tracker: Default::default(),
                forge: Default::default(),
                branch_pattern: None,
                attribution: true,
                autonomy: Some(AutonomyReport {
                    state: AutonomyState::Ok,
                    branch: "wip/autonoma".into(),
                    base: "develop".into(),
                    message: None,
                    conflicts: vec![],
                    pr_url: Some("https://github.example/acme/acme-web/pull/5".into()),
                    pr_pending: false,
                    at: 1,
                }),
            }),
        };
        hub.ingest("env-auto", "env-auto", &[report]).unwrap();
        let wi: Vec<i64> = (123..=126)
            .map(|n| hub.ensure_work_item("acme", "acme-web", n).unwrap())
            .collect();
        hub.apply_github(
            &GithubSnapshot {
                project_id: p.id,
                issues: (123..=126).map(issue).collect(),
                prs: vec![
                    pr(60, wi[0], "wip/autonoma", PrState::Merged, 100),
                    pr(61, wi[1], "wip/autonoma", PrState::Merged, 10),
                    pr(62, wi[2], "wip/autonoma", PrState::Open, 100),
                ],
                ..Default::default()
            },
            "hub-id",
        )
        .unwrap();
        hub.conn
            .execute(
                "UPDATE work_item SET status = 'bloqueado' WHERE id = ?1",
                [wi[3]],
            )
            .unwrap();
        for (uid, reviewed, at) in [("d1", None, 150), ("d2", Some(160), 50)] {
            hub.conn
                .execute(
                    "INSERT INTO decision (uid, env_id, work_item_id, origin, title, context,
                                           decision, reviewed_at, created_at)
                     VALUES (?1, 'env-auto', ?2, 'autonoma', ?3, ?3, ?3, ?4, ?5)",
                    params![uid, wi[0], AGENTE, reviewed, at],
                )
                .unwrap();
        }
        hub
    }

    #[test]
    fn issues_a_frente_da_base_e_o_pr_da_wip() {
        let hub = hub();
        let w = hub.autonomy_wip(None).unwrap();
        assert_eq!(w.len(), 1);
        let w = &w[0];
        assert_eq!(
            (w.branch.as_str(), w.base.as_str()),
            ("wip/autonoma", "develop")
        );
        // A 123 e a 124 foram mescladas na wip; a 125 só tem PR aberto.
        let ahead: Vec<i64> = w.ahead.iter().map(|i| i.number).collect();
        assert_eq!(ahead, vec![123, 124]);
        assert_eq!(w.ahead[0].title.as_deref(), Some("ACME-123"));
        assert_eq!(
            w.pr_url.as_deref(),
            Some("https://github.example/acme/acme-web/pull/5")
        );
        assert_eq!(w.ci, None);
        assert!(!w.conflict);
        assert_eq!(w.error, None);
        assert_eq!(hub.autonomy_wip(Some(("acme", "outro"))).unwrap(), vec![]);
    }

    /// #129: o `ci` da wip e do resumo é o resumo dos checks do PR
    /// `wip/autonoma → develop` no espelho do hub; os checks do PR de uma
    /// issue ficam pelo work item. PR fora da rodada sai do espelho.
    #[test]
    fn ci_da_wip_vem_dos_checks_do_espelho() {
        use crate::{CheckSource, ChecksRead, ChecksState, PrCheck, PrChecksFetch};
        let hub = hub();
        let p = hub.list_projects().unwrap()[0].id;
        let check = |name: &str, conclusion: Option<&str>, job: Option<i64>| PrCheck {
            name: name.into(),
            source: CheckSource::CheckRun,
            conclusion: conclusion.map(String::from),
            url: Some(format!("https://github.example/acme/acme-web/runs/{name}")),
            job_id: job,
        };
        let fetch = |n: i64, head: &str, base: &str, checks: Vec<PrCheck>| PrChecksFetch {
            pr_number: n,
            head: head.into(),
            base: base.into(),
            head_sha: format!("sha-{n}"),
            runs: Some(ChecksRead {
                checks,
                etag: Some("\"e1\"".into()),
            }),
            statuses: Some(ChecksRead::default()),
        };
        let apply = |c: crate::PrChecksSnapshot| {
            hub.apply_github(
                &GithubSnapshot {
                    project_id: p,
                    pr_checks: Some(c),
                    ..Default::default()
                },
                "hub-id",
            )
            .unwrap()
        };
        apply(crate::PrChecksSnapshot {
            open: vec![5, 62],
            updates: vec![
                fetch(
                    5,
                    "wip/autonoma",
                    "develop",
                    vec![
                        check("build", Some("success"), Some(1)),
                        check("e2e", Some("failure"), Some(2)),
                    ],
                ),
                fetch(
                    62,
                    "feat/62",
                    "wip/autonoma",
                    vec![check("build", None, Some(3))],
                ),
            ],
        });
        let w = &hub.autonomy_wip(None).unwrap()[0];
        assert_eq!(w.ci.as_deref(), Some("2 checks, 1 falhou"));
        let d = hub.digest_projects(0, 1_000).unwrap();
        assert_eq!(d[0].ci.as_deref(), Some("2 checks, 1 falhou"));
        let c = hub
            .pr_checks_by_head(p, "wip/autonoma", "develop")
            .unwrap()
            .unwrap();
        assert_eq!((c.pr_number, c.state), (5, ChecksState::Failure));
        assert_eq!(c.failed().next().unwrap().job_id, Some(2));
        // O PR 62 é da issue 125: lido pelo work item.
        let wi = hub.ensure_work_item("acme", "acme-web", 125).unwrap();
        let by_wi = hub.pr_checks_for_work_item(wi).unwrap();
        assert_eq!(by_wi.len(), 1);
        assert_eq!(
            (by_wi[0].pr_number, by_wi[0].state),
            (62, ChecksState::Pending)
        );
        assert_eq!(hub.pr_checks(p, 62).unwrap().unwrap(), by_wi[0]);

        // Rodada sem os PRs abertos: os checks saem.
        apply(crate::PrChecksSnapshot::default());
        assert_eq!(hub.autonomy_wip(None).unwrap()[0].ci, None);
        assert!(hub.list_pr_checks(p).unwrap().is_empty());
    }

    #[test]
    fn erro_da_branch_aparece_na_wip_e_no_resumo() {
        let hub = hub();
        let report = OutboxEntry {
            seq: 2,
            created_at: 2,
            change: SyncChange::Project(ProjectChange {
                owner: "acme".into(),
                repo: "acme-web".into(),
                name: "acme-web".into(),
                repo_path: "/tmp/acme-web".into(),
                base_branch: "develop".into(),
                protected_branches: vec![],
                created_at: 1,
                hlc: 2,
                state: None,
                error: None,
                applied_rev: Some(1),
                tracker: Default::default(),
                forge: Default::default(),
                branch_pattern: None,
                attribution: true,
                autonomy: Some(AutonomyReport {
                    state: AutonomyState::Error,
                    branch: "wip/autonoma".into(),
                    base: "develop".into(),
                    message: Some("push de wip/autonoma recusado (403)".into()),
                    conflicts: vec![],
                    pr_url: None,
                    pr_pending: false,
                    at: 2,
                }),
            }),
        };
        hub.ingest("env-auto", "env-auto", &[report]).unwrap();
        let w = &hub.autonomy_wip(None).unwrap()[0];
        assert!(!w.conflict);
        assert_eq!(
            w.error.as_deref(),
            Some("push de wip/autonoma recusado (403)")
        );
        let d = &hub.digest_projects(20, 200).unwrap()[0];
        assert_eq!(d.error, w.error);
        // Resumo gravado antes do campo continua legível (sem `error`).
        let mut v = serde_json::to_value(d).unwrap();
        v.as_object_mut().unwrap().remove("error");
        let old: DigestProject = serde_json::from_value(v).unwrap();
        assert_eq!(old.error, None);
    }

    #[test]
    fn resumo_do_periodo_sem_texto_do_agente() {
        let hub = hub();
        let ps = hub.digest_projects(20, 200).unwrap();
        let p = &ps[0];
        // Só a 123 entrou na wip no período (a 124, antes dele).
        assert_eq!(
            p.merged,
            vec![DigestIssue {
                number: 123,
                title: Some("ACME-123".into())
            }]
        );
        assert_eq!(
            p.blocked.iter().map(|i| i.number).collect::<Vec<_>>(),
            [126]
        );
        assert_eq!((p.decisions_pending, p.decisions_new), (1, 1));
        assert_eq!(p.ahead, 2);
        let d = MorningDigest {
            local_day: 20_000,
            at: 200,
            since: 20,
            projects: ps,
            ..Default::default()
        };
        assert!(d.has_news());
        let json = serde_json::to_string(&d).unwrap();
        assert!(!json.contains(AGENTE), "{json}");
        let body = digest_push_body(&d);
        assert_eq!(
            body,
            "acme-web: 1 mesclada na wip, 1 bloqueada, 1 decisão para revisar, 2 à frente da base"
        );
        assert!(!body.contains(AGENTE) && !body.contains("ACME-"));
    }

    #[test]
    fn um_resumo_por_dia() {
        let hub = hub();
        assert_eq!(hub.last_digest_day().unwrap(), None);
        let d = MorningDigest {
            local_day: 20_000,
            at: 200,
            since: 20,
            ..Default::default()
        };
        let saved = hub.save_digest(&d).unwrap().unwrap();
        assert!(saved.id > 0);
        assert_eq!(hub.save_digest(&d).unwrap(), None);
        assert_eq!(hub.last_digest_day().unwrap(), Some(20_000));
        hub.save_digest(&MorningDigest {
            local_day: 20_001,
            at: 300,
            ..d.clone()
        })
        .unwrap()
        .unwrap();
        let all = hub.list_digests(10).unwrap();
        assert_eq!(
            all.iter().map(|d| d.local_day).collect::<Vec<_>>(),
            [20_001, 20_000]
        );
        assert_eq!(hub.digests_since(250).unwrap().len(), 1);
        assert_eq!(
            digest_push_body(&d),
            "Nada novo para revisar no piloto automático"
        );
    }
}
