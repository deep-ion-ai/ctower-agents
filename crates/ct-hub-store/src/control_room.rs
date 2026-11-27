//! Leitura por projeto para a Sala de controle (#354, épico #353): os
//! PRs/MRs abertos com o resumo dos checks e as execuções de épico ativas com
//! o progresso, de todos os projetos do hub numa chamada
//! (`hub.control_room.projects`).
//!
//! Tudo sai do `hub.db`, sem consulta nova à forja: os PRs do espelho
//! (`pr_cache`, só os abertos), os checks do espelho (`pr_checks`, #129) e as
//! execuções de épico (#298). O título do PR é o da issue ligada: o espelho
//! não guarda o título do PR. O PR da branch de integração do modo autônomo
//! não fica no `pr_cache` e não entra (é do bloco do piloto,
//! `hub.autopilot.wip`).

use super::Hub;
use crate::{EpicNode, EpicRun, HubProject, Result};
use ct_core::{EpicNodeState, EpicRunStatus, ForgeKind, TrackerKind};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::hash::{DefaultHasher, Hash, Hasher};

/// Eventos do motor que dizem que ele não conseguiu agir: o mais recente
/// sendo um destes, a execução está parada por falha.
pub const EPIC_FAILURE_EVENTS: &[&str] =
    &["start_failed", "refused", "continue_failed", "plan_failed"];

/// Resposta de `hub.control_room.projects`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct ControlRoomProjects {
    /// Um por projeto do hub, na ordem dos projetos (`rank`, nome).
    pub projects: Vec<ControlRoomProject>,
    /// Relógio do hub na resposta (ms).
    pub now: i64,
}

/// O que a Sala de controle mostra de um projeto vindo do hub.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct ControlRoomProject {
    pub project_id: i64,
    pub owner: String,
    pub repo: String,
    pub name: String,
    pub tracker: TrackerKind,
    /// `github` = PRs; `gitlab` = MRs.
    pub forge: ForgeKind,
    /// PRs/MRs abertos, do mais novo ao mais antigo.
    pub prs: Vec<ControlRoomPr>,
    /// Execuções de épico ativas (nem concluídas nem canceladas), por id.
    pub epic_runs: Vec<ControlRoomEpicRun>,
}

/// Um PR/MR aberto do espelho.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct ControlRoomPr {
    pub number: i64,
    /// Título da issue ligada (o espelho não guarda o do PR); `None` = a
    /// issue não está no espelho.
    pub title: Option<String>,
    /// Número da issue ligada (o work item do PR).
    pub issue: i64,
    /// Referência da issue ligada (`owner/repo#n` ou a chave do Jira).
    pub issue_ref: String,
    pub url: String,
    pub head: String,
    pub base: String,
    pub updated_at: i64,
    pub checks: ControlRoomChecks,
}

/// Situação dos checks do PR para o card.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum ControlRoomChecksState {
    /// Todos passaram.
    Success,
    /// Algum em andamento, ou nenhum ainda (o CI pode não ter começado).
    Pending,
    /// Algum falhou (vence os em andamento).
    Failure,
    /// Sem leitura no espelho (MR do GitLab, PR ainda não lido).
    None,
}

/// Resumo dos checks do head do PR, do espelho.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct ControlRoomChecks {
    pub state: ControlRoomChecksState,
    pub total: u32,
    pub failed: u32,
    pub pending: u32,
    /// "3 checks, 1 falhou"; `None` sem leitura.
    pub text: Option<String>,
    /// Nomes dos checks que falharam, na ordem guardada.
    pub failed_names: Vec<String>,
    /// SHA a que os checks se referem; `None` sem leitura.
    pub head_sha: Option<String>,
    /// Última leitura que mudou algo (ms); `None` sem leitura.
    pub synced_at: Option<i64>,
}

impl ControlRoomChecks {
    fn none() -> Self {
        ControlRoomChecks {
            state: ControlRoomChecksState::None,
            total: 0,
            failed: 0,
            pending: 0,
            text: None,
            failed_names: vec![],
            head_sha: None,
            synced_at: None,
        }
    }

    fn of(c: &crate::PrChecks) -> Self {
        let s = c.summary();
        ControlRoomChecks {
            state: match s.state {
                crate::ChecksState::Success => ControlRoomChecksState::Success,
                crate::ChecksState::Pending => ControlRoomChecksState::Pending,
                crate::ChecksState::Failure => ControlRoomChecksState::Failure,
            },
            total: s.total,
            failed: s.failed,
            pending: s.pending,
            text: Some(s.text),
            failed_names: c.failed().map(|x| x.name.clone()).collect(),
            head_sha: Some(c.head_sha.clone()),
            synced_at: Some(c.synced_at),
        }
    }
}

/// Uma sub-issue da execução no card.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct ControlRoomEpicNode {
    pub number: i64,
    pub item_ref: String,
    /// Título no espelho; `None` = fora do espelho.
    pub title: Option<String>,
    pub state: EpicNodeState,
    /// PR da sub-issue, se aberto.
    pub pr: Option<i64>,
}

/// Por que a execução está parada.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum ControlRoomStopKind {
    /// Pausa manual.
    Pausado,
    /// Janela da conta esgotada: retoma sozinha em `until`.
    Estacionado,
    /// Pedido de decisão pendente.
    AguardandoDecisao,
    /// Plano esperando aprovação.
    AguardandoAprovacao,
    /// O motor não conseguiu agir (o evento mais recente é uma recusa ou falha).
    Falha,
    /// O ambiente da execução está fora do ar: o motor espera.
    AmbienteFora,
}

/// O motivo de a execução estar parada, o mais relevante para agir.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct ControlRoomStop {
    pub kind: ControlRoomStopKind,
    /// Texto para a pessoa (motivo da pausa, texto do pedido, mensagem do
    /// motor); `None` = sem texto.
    pub reason: Option<String>,
    /// Sub-issue do motivo, se houver.
    pub number: Option<i64>,
    /// Estacionada: quando retoma (ms).
    pub until: Option<i64>,
    /// Desde quando (ms), se conhecido.
    pub since: Option<i64>,
}

/// Uma execução de épico ativa no card.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct ControlRoomEpicRun {
    pub run_id: i64,
    pub epic: i64,
    pub epic_ref: String,
    /// Título do épico no espelho; `None` = fora do espelho.
    pub title: Option<String>,
    pub status: EpicRunStatus,
    pub env_id: String,
    /// O canal do ambiente da execução está aberto. O `ct-hub-store` não sabe
    /// (marca `true`); o `cthubd` acerta ([`ControlRoomEpicRun::mark_env`]).
    pub env_online: bool,
    /// Versão do plano aprovado em vigor; `None` = nenhuma aprovada ainda.
    pub plan_version: Option<i64>,
    /// Sub-issues mescladas na `wip` e o total, pelo plano aprovado em vigor
    /// (0 e 0 sem plano aprovado).
    pub done: u32,
    pub total: u32,
    /// Sub-issues em andamento (sessão ou PR vivos), na ordem do plano.
    pub current: Vec<ControlRoomEpicNode>,
    /// A próxima a iniciar: a primeira pronta na ordem do plano, ou a
    /// primeira pendente.
    pub next: Option<ControlRoomEpicNode>,
    /// Pedidos de decisão pendentes.
    pub pending_decisions: u32,
    /// O motivo de estar parada; `None` = andando.
    pub stop: Option<ControlRoomStop>,
    pub updated_at: i64,
}

impl ControlRoomEpicRun {
    /// Acerta `env_online` e, com o ambiente fora e nenhum outro motivo, a
    /// parada `ambiente_fora` (o motor só espera numa execução que ele dirige).
    pub fn mark_env(&mut self, online: bool) {
        self.env_online = online;
        let waits_env = !matches!(
            self.status,
            EpicRunStatus::AguardandoAprovacao | EpicRunStatus::Pausado
        );
        if !online && self.stop.is_none() && waits_env {
            self.stop = Some(ControlRoomStop {
                kind: ControlRoomStopKind::AmbienteFora,
                reason: None,
                number: None,
                until: None,
                since: None,
            });
        }
    }

    /// Os textos livres (motivo da parada) passam por `f`: a web aplica a
    /// máscara de secrets.
    pub fn map_text(&mut self, f: impl Fn(&str) -> String) {
        if let Some(s) = &mut self.stop {
            if let Some(r) = &s.reason {
                s.reason = Some(f(r));
            }
        }
    }
}

impl Hub {
    /// `hub.control_room.projects`: PRs/MRs abertos e execuções de épico
    /// ativas de todos os projetos do hub.
    pub fn control_room_projects(&self, now: i64) -> Result<ControlRoomProjects> {
        let mut runs: BTreeMap<i64, Vec<EpicRun>> = BTreeMap::new();
        for r in self.active_epic_runs()? {
            runs.entry(r.project_id).or_default().push(r);
        }
        let mut projects = Vec::new();
        for p in self.list_projects()? {
            let titles = self.issue_titles(p.id)?;
            let prs = self.control_room_prs(&p, &titles)?;
            let epic_runs = runs
                .remove(&p.id)
                .unwrap_or_default()
                .into_iter()
                .map(|r| self.control_room_run(&p, &titles, r))
                .collect::<Result<_>>()?;
            projects.push(ControlRoomProject {
                project_id: p.id,
                owner: p.gh_owner.clone(),
                repo: p.gh_repo.clone(),
                name: p.name.clone(),
                tracker: p.tracker.kind(),
                forge: p.forge.kind(),
                prs,
                epic_runs,
            });
        }
        Ok(ControlRoomProjects { projects, now })
    }

    /// Impressão digital dos PRs abertos e dos checks do projeto, para o
    /// push `hub.control_room.changed`: muda quando a Sala de controle tem
    /// algo novo a mostrar.
    pub fn control_room_pr_stamp(&self, project_id: i64) -> Result<u64> {
        let mut h = DefaultHasher::new();
        let mut stmt = self.conn.prepare(
            "SELECT number, work_item_id, head, base, url, updated_at FROM pr_cache
             WHERE project_id = ?1 AND state = 'open' ORDER BY number",
        )?;
        let rows = stmt.query_map([project_id], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, i64>(5)?,
            ))
        })?;
        for row in rows {
            row?.hash(&mut h);
        }
        let mut stmt = self.conn.prepare(
            "SELECT pr_number, head_sha, checks FROM pr_checks
             WHERE project_id = ?1 ORDER BY pr_number",
        )?;
        let rows = stmt.query_map([project_id], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?;
        for row in rows {
            row?.hash(&mut h);
        }
        Ok(h.finish())
    }

    fn issue_titles(&self, project_id: i64) -> Result<HashMap<i64, String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT gh_number, title FROM issue_cache WHERE project_id = ?1")?;
        let rows = stmt.query_map([project_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    fn control_room_prs(
        &self,
        p: &HubProject,
        titles: &HashMap<i64, String>,
    ) -> Result<Vec<ControlRoomPr>> {
        let checks: HashMap<i64, crate::PrChecks> = self
            .list_pr_checks(p.id)?
            .into_iter()
            .map(|c| (c.pr_number, c))
            .collect();
        let mut stmt = self.conn.prepare(
            "SELECT c.number, w.gh_number, c.url, c.head, c.base, c.updated_at
             FROM pr_cache c JOIN work_item w ON w.id = c.work_item_id
             WHERE c.project_id = ?1 AND c.state = 'open'
             ORDER BY c.number DESC",
        )?;
        let rows = stmt.query_map(params![p.id], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, i64>(5)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (number, issue, url, head, base, updated_at) = row?;
            out.push(ControlRoomPr {
                number,
                title: titles.get(&issue).cloned(),
                issue,
                issue_ref: p.item_ref(issue),
                url,
                head,
                base,
                updated_at,
                checks: checks
                    .get(&number)
                    .map(ControlRoomChecks::of)
                    .unwrap_or_else(ControlRoomChecks::none),
            });
        }
        Ok(out)
    }

    fn control_room_run(
        &self,
        p: &HubProject,
        titles: &HashMap<i64, String>,
        run: EpicRun,
    ) -> Result<ControlRoomEpicRun> {
        let nodes: HashMap<i64, EpicNode> = self
            .epic_nodes(run.id)?
            .into_iter()
            .map(|n| (n.number, n))
            .collect();
        // Ordem e total do plano aprovado em vigor.
        let plan = match run.plan_version {
            Some(v) => self.epic_plan(run.id, v)?,
            None => None,
        };
        let order: Vec<i64> = plan
            .map(|pl| pl.nodes.iter().map(|n| n.number).collect())
            .unwrap_or_default();
        let card = |n: &EpicNode| ControlRoomEpicNode {
            number: n.number,
            item_ref: p.item_ref(n.number),
            title: titles.get(&n.number).cloned(),
            state: n.state,
            pr: n.pr,
        };
        let in_plan = || order.iter().filter_map(|n| nodes.get(n));
        let done = in_plan()
            .filter(|n| n.state == EpicNodeState::Mesclada)
            .count() as u32;
        let current = in_plan().filter(|n| n.state.is_live()).map(card).collect();
        let next = in_plan()
            .find(|n| n.state == EpicNodeState::Pronta)
            .or_else(|| in_plan().find(|n| n.state == EpicNodeState::Pendente))
            .map(card);
        let pending: Vec<_> = self.epic_decisions(run.id, true)?;
        let last = self.epic_run_events_tail(run.id, 1)?.pop();
        let stop = match run.status {
            EpicRunStatus::Pausado => Some(ControlRoomStop {
                kind: ControlRoomStopKind::Pausado,
                reason: run.paused_reason.clone(),
                number: None,
                until: None,
                since: Some(run.updated_at),
            }),
            EpicRunStatus::Estacionado => Some(ControlRoomStop {
                kind: ControlRoomStopKind::Estacionado,
                reason: run.paused_reason.clone(),
                number: None,
                until: run.resume_at,
                since: Some(run.updated_at),
            }),
            _ => None,
        }
        .or_else(|| {
            pending.first().map(|d| ControlRoomStop {
                kind: ControlRoomStopKind::AguardandoDecisao,
                reason: Some(d.text.clone()),
                number: d.number,
                until: None,
                since: Some(d.created_at),
            })
        })
        .or_else(|| {
            (run.status == EpicRunStatus::AguardandoAprovacao).then_some(ControlRoomStop {
                kind: ControlRoomStopKind::AguardandoAprovacao,
                reason: None,
                number: None,
                until: None,
                since: Some(run.updated_at),
            })
        })
        .or_else(|| {
            last.filter(|e| EPIC_FAILURE_EVENTS.contains(&e.kind.as_str()))
                .map(|e| ControlRoomStop {
                    kind: ControlRoomStopKind::Falha,
                    reason: Some(e.message),
                    number: e.number,
                    until: None,
                    since: Some(e.at),
                })
        });
        Ok(ControlRoomEpicRun {
            run_id: run.id,
            epic: run.epic,
            epic_ref: p.item_ref(run.epic),
            title: titles.get(&run.epic).cloned(),
            status: run.status,
            env_id: run.env_id,
            env_online: true,
            plan_version: run.plan_version,
            done,
            total: order.len() as u32,
            current,
            next,
            pending_decisions: pending.len() as u32,
            stop,
            updated_at: run.updated_at,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        CheckSource, ChecksRead, EpicApproval, EpicPlanDraft, EpicPlanNodeInput, EpicRunEvent,
        GithubSnapshot, IssueRow, NewEpicRun, NewHubProject, PrCheck, PrChecksFetch,
        PrChecksSnapshot, PrRow,
    };
    use ct_core::{EpicDecisionOrigin, PrState};

    fn project(hub: &Hub, repo: &str) -> i64 {
        hub.add_project(&NewHubProject {
            owner: "acme".into(),
            repo: repo.into(),
            name: repo.into(),
            base_branch: "develop".into(),
            protected_branches: vec![],
        })
        .unwrap()
        .project
        .id
    }

    fn issue(n: i64, parent: Option<i64>) -> IssueRow {
        IssueRow {
            gh_number: n,
            title: format!("ACME-{n}"),
            state: "open".into(),
            parent_number: parent,
            has_sub_issues: parent.is_none() && n == 10,
            url: format!("https://github.example/acme/acme-web/issues/{n}"),
            updated_at: 1,
            ..Default::default()
        }
    }

    fn pr(n: i64, wi: i64, head: &str, base: &str, state: PrState) -> PrRow {
        PrRow {
            number: n,
            work_item_id: wi,
            head: head.into(),
            base: base.into(),
            state,
            url: format!("https://github.example/acme/acme-web/pull/{n}"),
            updated_at: n,
            head_sha: Some(format!("sha-{n}")),
        }
    }

    fn check(name: &str, conclusion: Option<&str>) -> PrCheck {
        PrCheck {
            name: name.into(),
            source: CheckSource::CheckRun,
            conclusion: conclusion.map(String::from),
            url: None,
            job_id: None,
        }
    }

    fn fetch(n: i64, head: &str, base: &str, checks: Vec<PrCheck>) -> PrChecksFetch {
        PrChecksFetch {
            pr_number: n,
            head: head.into(),
            base: base.into(),
            head_sha: format!("sha-{n}"),
            runs: Some(ChecksRead { checks, etag: None }),
            statuses: Some(ChecksRead::default()),
        }
    }

    /// `acme/acme-web` com o épico 10 (sub-issues 11, 12 e 13), PRs abertos
    /// das 11 e 12 e um fechado da 13, e checks: o da 11 falhou, o da 12
    /// passou; o PR 33 não tem leitura. `acme/acme-api` fica vazio.
    fn hub() -> (Hub, i64) {
        let hub = Hub::open_in_memory().unwrap();
        let pid = project(&hub, "acme-web");
        project(&hub, "acme-api");
        let wi: Vec<i64> = [11, 12, 13]
            .iter()
            .map(|n| hub.ensure_work_item("acme", "acme-web", *n).unwrap())
            .collect();
        hub.apply_github(
            &GithubSnapshot {
                project_id: pid,
                issues: vec![
                    issue(10, None),
                    issue(11, Some(10)),
                    issue(12, Some(10)),
                    issue(13, Some(10)),
                ],
                prs: vec![
                    pr(31, wi[0], "feat/11", "develop", PrState::Open),
                    pr(32, wi[1], "feat/12", "develop", PrState::Open),
                    pr(33, wi[2], "feat/13", "develop", PrState::Open),
                    pr(30, wi[2], "feat/13-old", "develop", PrState::Closed),
                ],
                pr_checks: Some(PrChecksSnapshot {
                    open: vec![31, 32],
                    updates: vec![
                        fetch(
                            31,
                            "feat/11",
                            "develop",
                            vec![
                                check("build", Some("success")),
                                check("e2e", Some("failure")),
                            ],
                        ),
                        fetch(
                            32,
                            "feat/12",
                            "develop",
                            vec![check("build", Some("success"))],
                        ),
                    ],
                }),
                synced_at: 5,
                ..Default::default()
            },
            "hub-id",
        )
        .unwrap();
        (hub, pid)
    }

    fn running(hub: &Hub, pid: i64) -> EpicRun {
        let run = hub
            .create_epic_run(
                &NewEpicRun {
                    project_id: pid,
                    epic: 10,
                    env_id: "env-a".into(),
                    started_by: "fulano".into(),
                    max_parallel: None,
                    plan_model: None,
                    plan_effort: None,
                },
                1,
            )
            .unwrap();
        let node = |n: i64| EpicPlanNodeInput {
            number: n,
            model: Some("sonnet".into()),
            ..Default::default()
        };
        hub.save_epic_plan_draft(
            run.id,
            &EpicPlanDraft {
                nodes: vec![node(11), node(12), node(13)],
                summary: "plano".into(),
                ..Default::default()
            },
            2,
        )
        .unwrap();
        hub.approve_epic_plan(
            run.id,
            &EpicApproval {
                version: 1,
                approved_by: "fulano".into(),
                max_parallel: 2,
                code_review: false,
                fingerprints: Default::default(),
            },
            3,
        )
        .unwrap()
    }

    #[test]
    fn prs_abertos_com_os_checks_do_espelho() {
        let (hub, _) = hub();
        let v = hub.control_room_projects(99).unwrap();
        assert_eq!(v.now, 99);
        let names: Vec<_> = v.projects.iter().map(|p| p.repo.as_str()).collect();
        assert_eq!(names, vec!["acme-api", "acme-web"]);
        let api = &v.projects[0];
        assert!(api.prs.is_empty() && api.epic_runs.is_empty());
        let web = &v.projects[1];
        assert_eq!(web.forge, ForgeKind::Github);
        // Só os abertos, do mais novo ao mais antigo.
        let nums: Vec<i64> = web.prs.iter().map(|p| p.number).collect();
        assert_eq!(nums, vec![33, 32, 31]);
        let p31 = &web.prs[2];
        assert_eq!(p31.title.as_deref(), Some("ACME-11"));
        assert_eq!(
            (p31.issue, p31.issue_ref.as_str()),
            (11, "acme/acme-web#11")
        );
        assert_eq!(p31.url, "https://github.example/acme/acme-web/pull/31");
        assert_eq!(p31.checks.state, ControlRoomChecksState::Failure);
        assert_eq!(p31.checks.failed_names, vec!["e2e"]);
        assert_eq!(p31.checks.text.as_deref(), Some("2 checks, 1 falhou"));
        assert_eq!(p31.checks.head_sha.as_deref(), Some("sha-31"));
        assert_eq!(web.prs[1].checks.state, ControlRoomChecksState::Success);
        // Sem leitura no espelho: sem dados.
        assert_eq!(web.prs[0].checks, ControlRoomChecks::none());
    }

    #[test]
    fn checks_sem_nenhum_ou_em_andamento_ficam_pendentes() {
        let (hub, pid) = hub();
        hub.apply_github(
            &GithubSnapshot {
                project_id: pid,
                pr_checks: Some(PrChecksSnapshot {
                    open: vec![31, 32, 33],
                    updates: vec![
                        fetch(32, "feat/12", "develop", vec![check("build", None)]),
                        fetch(33, "feat/13", "develop", vec![]),
                    ],
                }),
                ..Default::default()
            },
            "hub-id",
        )
        .unwrap();
        let v = hub.control_room_projects(1).unwrap();
        let prs = &v.projects[1].prs;
        assert_eq!(prs[0].checks.state, ControlRoomChecksState::Pending);
        assert_eq!(prs[0].checks.text.as_deref(), Some("sem checks"));
        assert_eq!(prs[1].checks.state, ControlRoomChecksState::Pending);
        assert_eq!(prs[1].checks.pending, 1);
        // A leitura da 31 continua (304 nas duas fontes = nada novo).
        assert_eq!(prs[2].checks.state, ControlRoomChecksState::Failure);
    }

    #[test]
    fn execucao_ativa_com_progresso_em_andamento_e_proxima() {
        let (hub, pid) = hub();
        let run = running(&hub, pid);
        for (n, to) in [
            (11, EpicNodeState::Pronta),
            (11, EpicNodeState::EmExecucao),
            (11, EpicNodeState::EmPr),
            (11, EpicNodeState::Mesclada),
            (12, EpicNodeState::Pronta),
            (12, EpicNodeState::EmExecucao),
            (13, EpicNodeState::Pronta),
        ] {
            let _ = hub.set_epic_node_state(run.id, n, to, None, 4);
        }
        hub.update_epic_node(
            run.id,
            12,
            &crate::EpicNodePatch {
                pr: Some(Some(32)),
                ..Default::default()
            },
            4,
        )
        .unwrap();
        let v = hub.control_room_projects(5).unwrap();
        let r = &v.projects[1].epic_runs[0];
        assert_eq!((r.epic, r.epic_ref.as_str()), (10, "acme/acme-web#10"));
        assert_eq!(r.title.as_deref(), Some("ACME-10"));
        assert_eq!(r.status, EpicRunStatus::Executando);
        assert_eq!((r.done, r.total, r.plan_version), (1, 3, Some(1)));
        assert_eq!(r.current.len(), 1);
        assert_eq!(r.current[0].number, 12);
        assert_eq!(r.current[0].title.as_deref(), Some("ACME-12"));
        assert_eq!(r.current[0].pr, Some(32));
        assert_eq!(r.next.as_ref().map(|n| n.number), Some(13));
        assert_eq!(r.stop, None);
        assert!(r.env_online);
    }

    #[test]
    fn parada_com_o_motivo_e_terminada_fora() {
        let (hub, pid) = hub();
        let run = running(&hub, pid);
        let stop = |hub: &Hub| {
            hub.control_room_projects(9).unwrap().projects[1].epic_runs[0]
                .stop
                .clone()
        };
        // Falha do motor: o evento mais recente.
        hub.add_epic_run_event(&EpicRunEvent {
            run_id: run.id,
            at: 6,
            kind: "start_failed".into(),
            number: Some(11),
            message: "ambiente recusou a sessão".into(),
            ..Default::default()
        })
        .unwrap();
        let s = stop(&hub).unwrap();
        assert_eq!(s.kind, ControlRoomStopKind::Falha);
        assert_eq!(
            (s.reason.as_deref(), s.number, s.since),
            (Some("ambiente recusou a sessão"), Some(11), Some(6))
        );
        // Um evento depois da falha: andando de novo.
        hub.add_epic_run_event(&EpicRunEvent {
            run_id: run.id,
            at: 7,
            kind: "started".into(),
            number: Some(11),
            message: "sessão iniciada".into(),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(stop(&hub), None);
        // Pedido de decisão pendente.
        hub.add_epic_decision(
            run.id,
            Some(12),
            EpicDecisionOrigin::CiEsgotado,
            "CI esgotou as tentativas",
            8,
        )
        .unwrap();
        let s = stop(&hub).unwrap();
        assert_eq!(s.kind, ControlRoomStopKind::AguardandoDecisao);
        assert_eq!(s.reason.as_deref(), Some("CI esgotou as tentativas"));
        assert_eq!(s.number, Some(12));
        assert_eq!(
            hub.control_room_projects(9).unwrap().projects[1].epic_runs[0].pending_decisions,
            1
        );
        // Pausa manual vence o pedido.
        hub.set_epic_run_status(
            run.id,
            EpicRunStatus::Pausado,
            Some("pausa do Fulano"),
            None,
            9,
        )
        .unwrap();
        let s = stop(&hub).unwrap();
        assert_eq!(
            (s.kind, s.reason.as_deref()),
            (ControlRoomStopKind::Pausado, Some("pausa do Fulano"))
        );
        // Terminada não aparece.
        hub.set_epic_run_status(run.id, EpicRunStatus::Cancelado, None, None, 10)
            .unwrap();
        assert!(hub.control_room_projects(11).unwrap().projects[1]
            .epic_runs
            .is_empty());
    }

    #[test]
    fn estacionada_aguardando_aprovacao_e_ambiente_fora() {
        let (hub, pid) = hub();
        let run = hub
            .create_epic_run(
                &NewEpicRun {
                    project_id: pid,
                    epic: 10,
                    env_id: "env-a".into(),
                    started_by: "fulano".into(),
                    max_parallel: None,
                    plan_model: None,
                    plan_effort: None,
                },
                1,
            )
            .unwrap();
        let first =
            |hub: &Hub| hub.control_room_projects(9).unwrap().projects[1].epic_runs[0].clone();
        // Planejando: sem plano aprovado, 0 de 0, sem parada.
        let r = first(&hub);
        assert_eq!((r.done, r.total, r.stop.clone()), (0, 0, None));
        let mut off = r.clone();
        off.mark_env(false);
        assert_eq!(off.stop.unwrap().kind, ControlRoomStopKind::AmbienteFora);
        hub.save_epic_plan_draft(
            run.id,
            &EpicPlanDraft {
                nodes: vec![EpicPlanNodeInput {
                    number: 11,
                    model: Some("sonnet".into()),
                    ..Default::default()
                }],
                ..Default::default()
            },
            2,
        )
        .unwrap();
        let mut r = first(&hub);
        assert_eq!(
            r.stop.as_ref().map(|s| s.kind),
            Some(ControlRoomStopKind::AguardandoAprovacao)
        );
        // O plano espera a pessoa, não o ambiente: o motivo continua.
        r.mark_env(false);
        assert_eq!(
            r.stop.as_ref().map(|s| s.kind),
            Some(ControlRoomStopKind::AguardandoAprovacao)
        );
        assert!(!r.env_online);
        hub.approve_epic_plan(
            run.id,
            &EpicApproval {
                version: 1,
                approved_by: "fulano".into(),
                max_parallel: 2,
                code_review: false,
                fingerprints: Default::default(),
            },
            3,
        )
        .unwrap();
        hub.set_epic_run_status(
            run.id,
            EpicRunStatus::Estacionado,
            Some("janela de 5h esgotada"),
            Some(500),
            4,
        )
        .unwrap();
        let mut r = first(&hub);
        let s = r.stop.clone().unwrap();
        assert_eq!(
            (s.kind, s.reason.as_deref(), s.until),
            (
                ControlRoomStopKind::Estacionado,
                Some("janela de 5h esgotada"),
                Some(500)
            )
        );
        assert_eq!((r.done, r.total), (0, 1));
        r.map_text(|t| t.to_uppercase());
        assert_eq!(
            r.stop.unwrap().reason.as_deref(),
            Some("JANELA DE 5H ESGOTADA")
        );
    }

    #[test]
    fn impressao_digital_muda_com_pr_ou_checks() {
        let (hub, pid) = hub();
        let a = hub.control_room_pr_stamp(pid).unwrap();
        assert_eq!(a, hub.control_room_pr_stamp(pid).unwrap());
        // Mesma rodada de novo (nada mudou): a mesma.
        hub.apply_github(
            &GithubSnapshot {
                project_id: pid,
                ..Default::default()
            },
            "hub-id",
        )
        .unwrap();
        assert_eq!(a, hub.control_room_pr_stamp(pid).unwrap());
        // Check novo no PR 32.
        hub.apply_github(
            &GithubSnapshot {
                project_id: pid,
                pr_checks: Some(PrChecksSnapshot {
                    open: vec![31, 32],
                    updates: vec![fetch(
                        32,
                        "feat/12",
                        "develop",
                        vec![check("build", Some("failure"))],
                    )],
                }),
                ..Default::default()
            },
            "hub-id",
        )
        .unwrap();
        let b = hub.control_room_pr_stamp(pid).unwrap();
        assert_ne!(a, b);
        // PR 33 mesclado: sai dos abertos.
        let wi = hub.ensure_work_item("acme", "acme-web", 13).unwrap();
        hub.apply_github(
            &GithubSnapshot {
                project_id: pid,
                prs: vec![pr(33, wi, "feat/13", "develop", PrState::Merged)],
                pr_checks: Some(PrChecksSnapshot {
                    open: vec![31, 32],
                    updates: vec![],
                }),
                ..Default::default()
            },
            "hub-id",
        )
        .unwrap();
        assert_ne!(b, hub.control_room_pr_stamp(pid).unwrap());
    }
}
